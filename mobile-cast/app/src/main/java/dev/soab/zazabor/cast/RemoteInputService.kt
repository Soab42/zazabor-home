package dev.soab.zazabor.cast

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.ComponentName
import android.content.Context
import android.graphics.Color
import android.graphics.Path
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.media.AudioManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.DisplayMetrics
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo

/**
 * Lets the PC drive this phone while the user has said yes for a session (docs/plans/remote.md):
 * taps, swipes and scrolls become gestures, Back/Home/Recents and friends are global actions,
 * typed text goes into the focused field. It does nothing until [Control] says a session is on,
 * reads nothing from the screen except the focused text field it types into, and draws a red
 * border around the screen for as long as it is being used.
 */
class RemoteInputService : AccessibilityService() {
    companion object {
        @Volatile
        var instance: RemoteInputService? = null
            private set

        private const val SERVICE = "dev.soab.zazabor.cast/dev.soab.zazabor.cast.RemoteInputService"

        /** Whether the user turned the service on in Android's Accessibility settings. */
        fun enabled(context: Context): Boolean {
            if (instance != null) return true
            val list = Settings.Secure.getString(context.contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES) ?: return false
            val me = ComponentName(context, RemoteInputService::class.java)
            return list.split(':').any { ComponentName.unflattenFromString(it) == me || it == SERVICE }
        }
    }

    private var border: View? = null

    override fun onServiceConnected() {
        instance = this
        LinkService.refresh(this)
    }

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        instance = null
        Control.end(this, "The accessibility service was turned off", tellPc = true)
        LinkService.refresh(this)
        return super.onUnbind(intent)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

    override fun onInterrupt() {}

    /** Shows or hides the red border around the whole screen (any thread; the window is made on the main one). */
    fun showBorder(on: Boolean) {
        Handler(Looper.getMainLooper()).post {
            val wm = getSystemService(WindowManager::class.java)
            if (!on) {
                border?.let {
                    try {
                        wm.removeView(it)
                    } catch (_: Exception) {
                    }
                }
                border = null
                return@post
            }
            if (border != null) return@post
            val view = View(this).apply {
                background = GradientDrawable().apply {
                    setColor(Color.TRANSPARENT)
                    setStroke((6 * resources.displayMetrics.density).toInt(), Color.rgb(229, 72, 77))
                }
            }
            val params = WindowManager.LayoutParams(
                WindowManager.LayoutParams.MATCH_PARENT, WindowManager.LayoutParams.MATCH_PARENT,
                WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
                PixelFormat.TRANSLUCENT,
            )
            try {
                wm.addView(view, params)
                border = view
            } catch (e: Exception) {
                android.util.Log.w("ZazaborLink", "couldn't draw the control border", e)
            }
        }
    }

    private fun size(): Pair<Int, Int> {
        val m = DisplayMetrics()
        @Suppress("DEPRECATION")
        getSystemService(WindowManager::class.java).defaultDisplay.getRealMetrics(m)
        return m.widthPixels to m.heightPixels
    }

    private fun path(points: List<Pair<Float, Float>>): Path {
        val (w, h) = size()
        val px = ControlMath.toPixels(points, w, h)
        return Path().apply {
            moveTo(px[0].first, px[0].second)
            if (px.size == 1) lineTo(px[0].first + 1f, px[0].second) else for (i in 1 until px.size) lineTo(px[i].first, px[i].second)
        }
    }

    private fun gesture(points: List<Pair<Float, Float>>, ms: Long) {
        val stroke = GestureDescription.StrokeDescription(path(points), 0, ms.coerceAtLeast(1))
        dispatchGesture(GestureDescription.Builder().addStroke(stroke).build(), null, null)
    }

    /** Does one thing the PC asked for. Call only while a session is on. */
    fun perform(c: Ctl) {
        when (c) {
            is Ctl.Tap -> gesture(listOf(c.x to c.y), 50)
            is Ctl.LongPress -> gesture(listOf(c.x to c.y), 700)
            is Ctl.Swipe -> gesture(c.points, c.ms)
            is Ctl.Scroll -> ControlMath.scrollSwipe(c).let { gesture(it.points, it.ms) }
            is Ctl.Global -> performGlobalAction(
                when (c.action) {
                    "back" -> GLOBAL_ACTION_BACK
                    "home" -> GLOBAL_ACTION_HOME
                    "recents" -> GLOBAL_ACTION_RECENTS
                    "notifications" -> GLOBAL_ACTION_NOTIFICATIONS
                    "quick" -> GLOBAL_ACTION_QUICK_SETTINGS
                    "lock" -> GLOBAL_ACTION_LOCK_SCREEN
                    "screenshot" -> GLOBAL_ACTION_TAKE_SCREENSHOT
                    else -> GLOBAL_ACTION_POWER_DIALOG
                },
            )
            is Ctl.Text -> edit { text, a, b -> ControlMath.insert(text, a, b, c.text) }
            is Ctl.Key -> key(c.key)
            is Ctl.Volume -> {
                val audio = getSystemService(AudioManager::class.java)
                val dir = when (c.direction) {
                    "up" -> AudioManager.ADJUST_RAISE
                    "down" -> AudioManager.ADJUST_LOWER
                    else -> AudioManager.ADJUST_TOGGLE_MUTE
                }
                audio.adjustStreamVolume(AudioManager.STREAM_MUSIC, dir, AudioManager.FLAG_SHOW_UI)
            }
        }
    }

    private fun key(name: String) {
        when (name) {
            "del" -> edit { text, a, b -> ControlMath.backspace(text, a, b) }
            "enter" -> {
                val node = rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return
                if (Build.VERSION.SDK_INT >= 30) {
                    node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
                } else {
                    edit { text, a, b -> ControlMath.insert(text, a, b, "\n") }
                }
            }
            "left", "right" -> {
                val node = rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return
                val at = (if (name == "left") node.textSelectionStart - 1 else node.textSelectionEnd + 1).coerceIn(0, node.text?.length ?: 0)
                select(node, at, at)
            }
            else -> {}
        }
    }

    private fun select(node: AccessibilityNodeInfo, a: Int, b: Int) {
        node.performAction(
            AccessibilityNodeInfo.ACTION_SET_SELECTION,
            Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, a)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, b)
            },
        )
    }

    /** Edits the focused text field: `change(text, selectionStart, selectionEnd)` gives (new text, cursor). */
    private fun edit(change: (String, Int, Int) -> Pair<String, Int>) {
        val node = rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return
        if (!node.isEditable) return
        // A field showing its hint has no text of its own.
        val current = if (node.isShowingHintText) "" else node.text?.toString() ?: ""
        val start = node.textSelectionStart.let { if (it < 0) current.length else it }
        val end = node.textSelectionEnd.let { if (it < 0) current.length else it }
        val (next, cursor) = change(current, start, end)
        node.performAction(
            AccessibilityNodeInfo.ACTION_SET_TEXT,
            Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, next) },
        )
        select(node, cursor, cursor)
    }
}

/** Whether the PC controls this phone right now, and for whom. */
object Control {
    @Volatile
    var pc: String? = null
        private set

    val active: Boolean get() = pc != null

    /** Starts a session: the border on. False when the accessibility service isn't running. */
    fun begin(name: String): Boolean {
        val service = RemoteInputService.instance ?: return false
        pc = name
        service.showBorder(true)
        return true
    }

    /** Ends it (the border off). `tellPc` sends `control.state` so the PC's toolbar goes away. */
    fun end(context: Context, reason: String?, tellPc: Boolean) {
        if (pc == null) return
        pc = null
        RemoteInputService.instance?.showBorder(false)
        LinkService.controlEnded(context)
        if (tellPc) {
            LinkService.emit(org.json.JSONObject().put("t", "control.state").put("on", false).put("reason", reason ?: ""))
        }
    }

    fun handle(msg: org.json.JSONObject) {
        if (!active) return
        val c = Ctl.parse(msg) ?: return
        RemoteInputService.instance?.perform(c)
    }
}
