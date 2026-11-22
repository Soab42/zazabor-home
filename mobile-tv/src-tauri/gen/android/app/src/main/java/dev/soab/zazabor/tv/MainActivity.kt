package dev.soab.zazabor.tv

import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.Environment
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onResume() {
    super.onResume()
    Updater.maybeCheck(this, "tv")
  }

  // Settings' Check for Updates (src/mobile-tv/main.ts) calls window.ZazaborUpdates.check().
  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    webView.addJavascriptInterface(UpdatesBridge(), "ZazaborUpdates")
    webView.addJavascriptInterface(SystemBridge(), "ZazaborSystem")
    webView.addJavascriptInterface(PlayerBridge(), "ZazaborPlayer")
    webView.addJavascriptInterface(DownloadsBridge(), "ZazaborDownloads")
  }

  // The page draws under the system bars (edge to edge). The WebView reports the status bar as a
  // safe area but not the gesture or button bar, so src/mobile-tv/main.ts asks for it here.
  private inner class SystemBridge {
    @JavascriptInterface
    fun bottomInset(): Float {
      val insets = ViewCompat.getRootWindowInsets(window.decorView) ?: return 0f
      return insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom / resources.displayMetrics.density
    }
  }

  private inner class UpdatesBridge {
    @JavascriptInterface
    fun check() = runOnUiThread { Updater.check(this@MainActivity, "tv", manual = true) }

    @JavascriptInterface
    fun version(): String = packageManager.getPackageInfo(packageName, 0).versionName ?: ""
  }

  /**
   * Playing is VLC's job: a video link opens in VLC with its title; several (episodes, from the
   * chosen one on) go as an M3U playlist in the app's cache so VLC steps on by itself. Without VLC
   * the system offers any video player. Answers "vlc", "chooser" (VLC isn't installed) or "none"
   * (no player at all).
   */
  private inner class PlayerBridge {
    @JavascriptInterface
    fun open(url: String, title: String, listJson: String): String {
      val vlc = vlcInstalled()
      val list = runCatching { JSONArray(listJson) }.getOrNull()
      if (vlc && list != null && list.length() > 1) {
        val played = runCatching { playlist(list) }.getOrNull()
        if (played != null && start(Intent(Intent.ACTION_VIEW).setDataAndType(played, "audio/x-mpegurl").setPackage(VLC)
              .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))) return "vlc"
      }
      val view = Intent(Intent.ACTION_VIEW).setDataAndType(Uri.parse(url), "video/*").putExtra("title", title)
      if (vlc) {
        if (start(Intent(view).setPackage(VLC))) return "vlc"
      } else if (start(Intent.createChooser(view, "Play with"))) {
        return "chooser"
      }
      return "none"
    }

    @JavascriptInterface
    fun hasVlc(): Boolean = vlcInstalled()

    /** The system share sheet with a title and its link. */
    @JavascriptInterface
    fun share(text: String) {
      val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
      start(Intent.createChooser(send, "Share"))
    }

    /** The Play Store page of VLC (the web page when there is no store app). */
    @JavascriptInterface
    fun getVlc() {
      if (!start(Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=$VLC")))) {
        start(Intent(Intent.ACTION_VIEW, Uri.parse("https://play.google.com/store/apps/details?id=$VLC")))
      }
    }
  }

  /**
   * Downloads go through the system's DownloadManager: it keeps going with the app closed, shows its
   * own notification with progress and resumes after a lost connection. Each file lands in the app's
   * Movies folder (`Android/data/<app>/files/Movies/Zazabor TV/<Title (Year)>/`, no storage permission
   * needed). The manager knows only the address and the file, so what the page needs about a download
   * (the title's key, name, episode) is kept in preferences under the manager's id.
   */
  private inner class DownloadsBridge {
    private val prefs get() = getSharedPreferences("tv-downloads", Context.MODE_PRIVATE)
    private val manager get() = getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager

    private fun meta(): JSONObject = runCatching { JSONObject(prefs.getString("meta", "{}") ?: "{}") }.getOrDefault(JSONObject())

    private fun safe(text: String): String {
      val cleaned = text.map { c -> if (c.isISOControl() || "/\\:*?\"<>|".indexOf(c) >= 0) '_' else c }.joinToString("").trim().trim('.').trim()
      return cleaned.take(120).ifEmpty { "Untitled" }
    }

    /** `{wifiOnly, items: [{key, itemId, title, year, label, url, size}]}` -> "ok" or "error: …". */
    @JavascriptInterface
    fun add(json: String): String = try {
      val root = JSONObject(json)
      val wifiOnly = root.optBoolean("wifiOnly")
      val items = root.getJSONArray("items")
      val all = meta()
      for (i in 0 until items.length()) {
        val item = items.getJSONObject(i)
        val url = item.getString("url")
        if (all.keys().asSequence().any { all.getJSONObject(it).optString("url") == url }) continue
        val title = item.getString("title")
        val year = if (item.isNull("year")) 0 else item.optInt("year")
        val file = safe(Uri.decode(url.substringBefore('?').substringAfterLast('/')))
        val folder = safe(if (year > 0) "$title ($year)" else title)
        val label = item.optString("label")
        val request = DownloadManager.Request(Uri.parse(url))
          .setTitle(if (label.isEmpty()) title else "$title - ${label.substringBeforeLast('.')}")
          .setDescription("Zazabor TV")
          .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
          .setDestinationInExternalFilesDir(this@MainActivity, Environment.DIRECTORY_MOVIES, "Zazabor TV/$folder/$file")
        if (wifiOnly) request.setAllowedNetworkTypes(DownloadManager.Request.NETWORK_WIFI)
        val id = manager.enqueue(request)
        all.put(id.toString(), JSONObject().put("key", item.optString("key")).put("itemId", item.optString("itemId")).put("title", title)
          .put("year", if (year > 0) year else JSONObject.NULL).put("label", label).put("url", url).put("added", System.currentTimeMillis()))
      }
      prefs.edit().putString("meta", all.toString()).apply()
      "ok"
    } catch (e: Exception) {
      "error: ${e.message ?: "the download couldn't start"}"
    }

    private fun failure(reason: Int): String = when (reason) {
      DownloadManager.ERROR_INSUFFICIENT_SPACE -> "Not enough free space on this phone."
      DownloadManager.ERROR_DEVICE_NOT_FOUND -> "The storage isn't available."
      DownloadManager.ERROR_CANNOT_RESUME -> "The download couldn't carry on."
      DownloadManager.ERROR_FILE_ALREADY_EXISTS -> "That file already exists."
      DownloadManager.ERROR_HTTP_DATA_ERROR, DownloadManager.ERROR_TOO_MANY_REDIRECTS, DownloadManager.ERROR_UNHANDLED_HTTP_CODE -> "The server stopped sending the file."
      in 400..599 -> "The server answered $reason."
      else -> "The download failed (code $reason)."
    }

    /** Every download as `[{id, key, itemId, title, year, label, url, path, size, got, state, error, added, finished, exists}]`. */
    @JavascriptInterface
    fun list(): String {
      val all = meta()
      val out = JSONArray()
      val ids = all.keys().asSequence().mapNotNull { it.toLongOrNull() }.toList().toLongArray()
      if (ids.isEmpty()) return "[]"
      val seen = HashSet<Long>()
      manager.query(DownloadManager.Query().setFilterById(*ids))?.use { c ->
        while (c.moveToNext()) {
          val id = c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_ID))
          seen.add(id)
          val info = all.optJSONObject(id.toString()) ?: continue
          val status = c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS))
          val reason = c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_REASON))
          val local = c.getString(c.getColumnIndexOrThrow(DownloadManager.COLUMN_LOCAL_URI))?.let { Uri.parse(it).path } ?: ""
          val state = when (status) {
            DownloadManager.STATUS_SUCCESSFUL -> "done"
            DownloadManager.STATUS_FAILED -> "failed"
            DownloadManager.STATUS_RUNNING -> "running"
            DownloadManager.STATUS_PAUSED -> "paused"
            else -> "queued"
          }
          val size = c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_TOTAL_SIZE_BYTES)).coerceAtLeast(0)
          out.put(JSONObject(info.toString())
            .put("id", id.toString()).put("path", local).put("size", size)
            .put("got", c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR)).coerceAtLeast(0))
            .put("state", state).put("error", if (state == "failed") failure(reason) else if (state == "paused") "Waiting for a connection" else "")
            .put("finished", if (state == "done") c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_LAST_MODIFIED_TIMESTAMP)) else 0)
            .put("exists", state == "done" && local.isNotEmpty() && File(local).exists()))
        }
      }
      // ones the user cleared from the system's Downloads app are gone from the list too
      val gone = ids.filter { it !in seen }
      if (gone.isNotEmpty()) {
        gone.forEach { all.remove(it.toString()) }
        prefs.edit().putString("meta", all.toString()).apply()
      }
      return out.toString()
    }

    /** Cancels a running download or deletes a finished one (the file goes with it). */
    @JavascriptInterface
    fun remove(id: String) {
      val number = id.toLongOrNull() ?: return
      val folder = manager.query(DownloadManager.Query().setFilterById(number))?.use { c ->
        if (c.moveToFirst()) c.getString(c.getColumnIndexOrThrow(DownloadManager.COLUMN_LOCAL_URI))?.let { Uri.parse(it).path }?.let { File(it).parentFile } else null
      }
      manager.remove(number)
      // the title's folder goes when nothing else is in it
      if (folder != null && folder.isDirectory && folder.list()?.isEmpty() == true) folder.delete()
      val all = meta()
      all.remove(id)
      prefs.edit().putString("meta", all.toString()).apply()
    }

    /** Plays the finished file in VLC (any player when VLC isn't there). Answers like `ZazaborPlayer.open`. */
    @JavascriptInterface
    fun play(id: String): String {
      val number = id.toLongOrNull() ?: return "none"
      val path = manager.query(DownloadManager.Query().setFilterById(number))?.use { c ->
        if (c.moveToFirst()) c.getString(c.getColumnIndexOrThrow(DownloadManager.COLUMN_LOCAL_URI))?.let { Uri.parse(it).path } else null
      } ?: return "none"
      val file = File(path)
      if (!file.exists()) return "none"
      val uri = FileProvider.getUriForFile(this@MainActivity, "$packageName.fileprovider", file)
      val view = Intent(Intent.ACTION_VIEW).setDataAndType(uri, "video/*").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        .putExtra("title", meta().optJSONObject(id)?.optString("title") ?: file.name)
      return if (vlcInstalled() && start(Intent(view).setPackage(VLC))) "vlc"
      else if (start(Intent.createChooser(view, "Play with"))) "chooser" else "none"
    }
  }

  private fun vlcInstalled(): Boolean = try {
    packageManager.getPackageInfo(VLC, 0)
    true
  } catch (_: PackageManager.NameNotFoundException) {
    false
  }

  private fun start(intent: Intent): Boolean = try {
    startActivity(intent)
    true
  } catch (_: ActivityNotFoundException) {
    false
  }

  /** `[{url, title}, …]` as an M3U in the cache, shared through the FileProvider. */
  private fun playlist(list: JSONArray): Uri {
    val text = StringBuilder("#EXTM3U\n")
    for (i in 0 until list.length()) {
      val entry = list.getJSONObject(i)
      text.append("#EXTINF:-1,").append(entry.optString("title").replace('\n', ' ')).append('\n')
      text.append(entry.getString("url")).append('\n')
    }
    val file = File(cacheDir, "playlist.m3u")
    file.writeText(text.toString())
    return FileProvider.getUriForFile(this, "$packageName.fileprovider", file)
  }

  private companion object {
    const val VLC = "org.videolan.vlc"
  }
}
