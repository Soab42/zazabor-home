package dev.soab.zazabor.tv

import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONArray
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

    /** The Play Store page of VLC (the web page when there is no store app). */
    @JavascriptInterface
    fun getVlc() {
      if (!start(Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=$VLC")))) {
        start(Intent(Intent.ACTION_VIEW, Uri.parse("https://play.google.com/store/apps/details?id=$VLC")))
      }
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
