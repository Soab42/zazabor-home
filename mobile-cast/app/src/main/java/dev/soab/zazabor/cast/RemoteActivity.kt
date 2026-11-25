package dev.soab.zazabor.cast

import android.app.Activity
import android.content.pm.ActivityInfo
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.util.TypedValue
import android.view.Gravity
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.TextView
import android.widget.Toast
import dev.soab.zazabor.cast.RemoteProto.Input
import dev.soab.zazabor.cast.RemoteProto.Keysym
import org.json.JSONObject
import kotlin.concurrent.thread

/**
 * Controls the PC from the phone (docs/plans/remote.md): Screen (the PC's picture, touch moves
 * the pointer), Camera (the PC's webcam), Touchpad (no picture, lowest battery), Keyboard (the
 * phone's keyboard types on the PC, plus the keys it lacks) and Media. The sound button plays the
 * PC's own sound or its microphone. One sealed session to the PC serves them all, on the same
 * Wi-Fi or over the internet; the PC asks its user first, and shows a pill for as long as it runs
 * (with the camera and the microphone on it while they're on).
 */
class RemoteActivity : Activity(), RemoteClient.Listener {
    companion object {
        private const val SCREEN = 0
        private const val CAMERA = 1
        private const val TOUCHPAD = 2
        private const val KEYBOARD = 3
        private const val MEDIA = 4
        private val TABS = listOf("Screen", "Camera", "Touchpad", "Keyboard", "Media")
        /** The modifier buttons, by label. */
        private val STICKY = mapOf("Ctrl" to Keysym.CONTROL_L, "Alt" to Keysym.ALT_L, "Shift" to Keysym.SHIFT_L, "Super" to Keysym.SUPER_L)
        private val SOUNDS = listOf("off" to "Sound off", "sound" to "PC sound", "mic" to "PC mic")
        private const val CHROME_MS = 3000L

        private class Quality(val label: String, val w: Int, val h: Int, val kbps: Int)

        private val QUALITIES = listOf(Quality("900p", 1600, 900, 4000), Quality("1080p", 1920, 1080, 6000), Quality("720p", 1280, 720, 3000), Quality("540p", 960, 540, 1500))
        private const val INTERNET_QUALITY = 2
        private const val INTERNET_START_KBPS = 2000
    }

    private var quality = 0

    /** Over the internet: the bitrate follows the ping (it drops when the PC's frames queue up). */
    private var internet = false
    private var targetKbps = 4000
    private var kbps = 4000
    private var minRtt = Long.MAX_VALUE
    private var steadySince = 0L

    /** What the PC shows: "screen" or "camera", the cameras it has, and whether it allows them. */
    private var pcSource = "screen"
    private var cameraCount = 0
    private var camera = 0
    private var media = false
    private var audio: RemoteAudio? = null
    private var sound = "off"
    private lateinit var soundButton: TextView

    private val main = Handler(Looper.getMainLooper())
    private var client: RemoteClient? = null
    private var decoder: RemoteDecoder? = null
    private var surface: SurfaceHolder? = null
    private var mode = SCREEN
    private var pcName = "PC"
    private var picW = 0
    private var picH = 0
    private var monitors = 1
    private var monitor = 0
    private var viewOnly = false
    private var videoOn = true
    private var rtt = -1L
    private var lastFrames = 0L
    private var lastBytes = 0L
    private var stats = ""
    private var ended = false

    private lateinit var tabs: List<TextView>
    /** The top bar and the screen's tools: hidden while the PC's screen fills the phone. */
    private lateinit var bar: View
    private lateinit var tools: View
    private lateinit var reveal: TextView
    private var chromeShown = true
    private val hideChrome = Runnable { if (showsPicture) setChrome(false) }

    /** Screen and Camera show the PC's picture full-screen. */
    private val showsPicture: Boolean get() = mode == SCREEN || mode == CAMERA
    private lateinit var status: TextView
    private lateinit var statsLabel: TextView
    private lateinit var content: FrameLayout
    private lateinit var screenView: FrameLayout
    private lateinit var surfaceView: SurfaceView
    private lateinit var touch: TouchView
    private lateinit var padView: FrameLayout
    private lateinit var keyboardView: LinearLayout
    private lateinit var mediaView: LinearLayout
    private lateinit var typer: EditText
    private lateinit var viewport: Viewport
    private lateinit var screenGestures: GestureEngine
    private lateinit var padGestures: GestureEngine
    private val held = linkedSetOf<Int>()
    /** Each modifier's buttons (the Keyboard tab's and the Screen tab's key strip). */
    private val stickyButtons = mutableMapOf<Int, MutableList<Button>>()

    /** The Screen tab's keyboard: a strip of keys and a box under the picture. */
    private lateinit var screenArea: LinearLayout
    private lateinit var screenKeys: LinearLayout
    private lateinit var screenTyper: EditText
    private var screenKeysOpen = false

    /** The Screen and Camera tabs held upright (remembered), for typing. */
    private var portrait = false

    private fun dp(v: Int): Int = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    private fun send(i: Input) {
        // The camera's picture isn't the PC's screen: touches on it go nowhere.
        if (!viewOnly && mode != CAMERA) client?.send(i)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.statusBarColor = Color.BLACK
        window.navigationBarColor = Color.BLACK
        portrait = getSharedPreferences("remote", MODE_PRIVATE).getBoolean("portrait", false)
        setContentView(build())
        select(SCREEN)
        connect()
    }

    // ---- the screen ----

    private fun build(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.BLACK)
            // The bars (status, navigation) stay out of the tabs and the picture.
            fitsSystemWindows = true
        }
        val bar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(6), dp(2), dp(10), dp(2))
            setBackgroundColor(getColor(R.color.surface))
        }
        bar.addView(
            TextView(this).apply {
                text = "‹"
                setTextColor(getColor(R.color.text))
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 26f)
                setPadding(dp(10), 0, dp(14), dp(3))
                setOnClickListener { finish() }
                contentDescription = "Close"
            },
        )
        // Five tabs, the sound button and the stats don't fit a phone held upright: they scroll.
        val strip = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
        }
        bar.addView(HorizontalScrollView(this).apply {
            isHorizontalScrollBarEnabled = false
            addView(strip)
        }, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
        tabs = TABS.mapIndexed { i, name ->
            TextView(this).apply {
                text = name
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
                gravity = Gravity.CENTER
                setPadding(dp(12), dp(8), dp(12), dp(8))
                setOnClickListener { select(i) }
            }.also { strip.addView(it) }
        }
        soundButton = TextView(this).apply {
            text = SOUNDS[0].second
            setTextColor(getColor(R.color.muted))
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
            setPadding(dp(10), dp(8), dp(10), dp(8))
            contentDescription = "The PC's sound on this phone"
            setOnClickListener {
                val i = SOUNDS.indexOfFirst { it.first == sound }
                val next = SOUNDS[(i + 1) % SOUNDS.size].first
                // The microphone is skipped when the PC doesn't share it.
                askSound(if (next == "mic" && !media) "off" else next)
            }
        }
        strip.addView(soundButton)
        statsLabel = TextView(this).apply {
            setTextColor(getColor(R.color.muted))
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            gravity = Gravity.END
        }
        // Tap the stats line for a live clock over the picture: with the PC's own clock on its
        // screen, one screenshot shows how late the picture is (diagnostics).
        statsLabel.setOnClickListener { toggleClock() }
        strip.addView(statsLabel, LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { marginStart = dp(8) })
        root.addView(bar, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
        this.bar = bar

        content = FrameLayout(this)
        buildScreen()
        buildPad()
        buildKeyboard()
        buildMedia()
        status = TextView(this).apply {
            setTextColor(getColor(R.color.text))
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
            gravity = Gravity.CENTER
            setPadding(dp(32), dp(32), dp(32), dp(32))
            text = "Asking $pcName…"
        }
        content.addView(screenArea)
        content.addView(padView)
        content.addView(keyboardView)
        content.addView(mediaView)
        content.addView(status, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        root.addView(content, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
        return root
    }

    private fun pill(label: String, onClick: (Button) -> Unit): Button = Button(this).apply {
        text = label
        isAllCaps = false
        setTextColor(getColor(R.color.text))
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
        minHeight = 0
        minimumHeight = 0
        setPadding(dp(14), dp(8), dp(14), dp(8))
        background = GradientDrawable().apply {
            setColor(getColor(R.color.surface_high))
            cornerRadius = dp(20).toFloat()
        }
        setOnClickListener { onClick(this) }
    }

    private fun highlight(b: Button, on: Boolean) {
        (b.background as? GradientDrawable)?.setColor(getColor(if (on) R.color.accent_soft else R.color.surface_high))
        b.setTextColor(getColor(if (on) R.color.accent else R.color.text))
    }

    private fun buildScreen() {
        screenView = FrameLayout(this)
        viewport = Viewport(1f, 1f, 16f, 9f)
        surfaceView = SurfaceView(this)
        surfaceView.holder.addCallback(object : SurfaceHolder.Callback {
            override fun surfaceCreated(holder: SurfaceHolder) {
                surface = holder
                startDecoder()
            }

            override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}

            override fun surfaceDestroyed(holder: SurfaceHolder) {
                decoder?.release()
                decoder = null
                surface = null
            }
        })
        touch = TouchView(this)
        screenGestures = GestureEngine(viewport, resources.displayMetrics.density, ::send) { applyViewport() }
        touch.engine = screenGestures
        screenView.addView(surfaceView, FrameLayout.LayoutParams(1, 1))
        screenView.addView(touch, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        screenView.addOnLayoutChangeListener { _, l, t, r, b, _, _, _, _ ->
            viewport.viewW = (r - l).toFloat()
            viewport.viewH = (b - t).toFloat()
            applyViewport()
        }
        val tools = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(dp(8), dp(8), dp(8), dp(8))
        }
        tools.addView(pill("Direct") { b ->
            screenGestures.trackpad = !screenGestures.trackpad
            b.text = if (screenGestures.trackpad) "Trackpad" else "Direct"
        }.also { it.tag = "direct" })
        tools.addView(pill("Camera 1") { b ->
            if (cameraCount > 1) {
                camera = (camera + 1) % cameraCount
                client?.send(JSONObject().put("t", "source").put("src", "camera").put("i", camera))
                b.text = "Camera ${camera + 1}"
            }
        }.also { it.tag = "camera" })
        tools.addView(pill("Monitor 1") { b ->
            if (monitors > 1) {
                monitor = (monitor + 1) % monitors
                client?.send(JSONObject().put("t", "monitor").put("i", monitor))
                b.text = "Monitor ${monitor + 1}"
            }
        }.also { it.tag = "monitor" })
        tools.addView(pill("900p") { b ->
            quality = (quality + 1) % QUALITIES.size
            val q = QUALITIES[quality]
            b.text = q.label
            targetKbps = q.kbps
            // Over the internet a new quality starts lower and climbs while the ping stays low.
            kbps = if (internet) minOf(q.kbps, INTERNET_START_KBPS) else q.kbps
            steadySince = SystemClock.elapsedRealtime()
            client?.send(JSONObject().put("t", "quality").put("maxW", q.w).put("maxH", q.h).put("kbps", kbps))
        }.also { it.tag = "quality" })
        tools.addView(pill("Reset zoom") {
            viewport.reset()
            applyViewport()
        })
        tools.addView(pill("⌨ Keyboard") { setScreenKeys(!screenKeysOpen) }.also { it.tag = "keys" })
        tools.addView(pill(if (portrait) "Portrait" else "Landscape") { b ->
            portrait = !portrait
            b.text = if (portrait) "Portrait" else "Landscape"
            getSharedPreferences("remote", MODE_PRIVATE).edit().putBoolean("portrait", portrait).apply()
            applyOrientation()
        })
        // More buttons than a phone held upright is wide: the row scrolls sideways.
        val toolsScroll = HorizontalScrollView(this).apply {
            isHorizontalScrollBarEnabled = false
            isFillViewport = false
            addView(tools)
        }
        screenView.addView(toolsScroll, FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM or Gravity.END))
        this.tools = toolsScroll
        // While the controls are hidden, this small button brings them back (touches on the
        // picture belong to the PC, so it can't be a tap anywhere).
        reveal = TextView(this).apply {
            text = "⋯"
            setTextColor(Color.WHITE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
            gravity = Gravity.CENTER
            alpha = 0.55f
            background = GradientDrawable().apply {
                setColor(0x99000000.toInt())
                cornerRadius = dp(18).toFloat()
            }
            contentDescription = "Show the controls"
            visibility = View.GONE
            setOnClickListener { setChrome(true) }
        }
        screenView.addView(reveal, FrameLayout.LayoutParams(dp(40), dp(36), Gravity.TOP or Gravity.END).apply { setMargins(0, dp(8), dp(8), 0) })

        // Typing while watching: a strip of the keys a phone keyboard lacks and a box for the phone's
        // keyboard, under the picture (which shrinks to make room, so nothing is covered).
        screenKeys = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(getColor(R.color.surface))
            visibility = View.GONE
        }
        val ctrl = listOf(Keysym.CONTROL_L)
        screenKeys.addView(
            keyRow(
                sticky("Ctrl", Keysym.CONTROL_L), sticky("Alt", Keysym.ALT_L), sticky("Shift", Keysym.SHIFT_L),
                key("Esc", Keysym.ESCAPE), key("Tab", Keysym.TAB), key("Enter", Keysym.RETURN), key("⌫", Keysym.BACKSPACE), key("Del", Keysym.DELETE),
                key("←", Keysym.LEFT), key("↓", Keysym.DOWN), key("↑", Keysym.UP), key("→", Keysym.RIGHT),
                key("Home", Keysym.HOME), key("End", Keysym.END),
                combo("Copy", ctrl, 'c'.code), combo("Paste", ctrl, 'v'.code), combo("Undo", ctrl, 'z'.code), combo("Select all", ctrl, 'a'.code),
                sticky("Super", Keysym.SUPER_L),
            ),
        )
        val line = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(8), 0, dp(8), dp(6))
        }
        screenTyper = newTyper("Type here: it goes to the PC").apply {
            setPadding(dp(12), dp(8), dp(12), dp(8))
            maxLines = 2
        }
        line.addView(screenTyper, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
        line.addView(pill("✕") { setScreenKeys(false) }.apply { contentDescription = "Hide the keyboard" }, LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { marginStart = dp(6) })
        screenKeys.addView(line)
        screenArea = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        screenArea.addView(screenView, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
        screenArea.addView(screenKeys, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
    }

    /** Opens or closes the Screen tab's keyboard (the strip, the box and the phone's keyboard). */
    private fun setScreenKeys(open: Boolean) {
        screenKeysOpen = open && mode == SCREEN
        screenKeys.visibility = if (screenKeysOpen) View.VISIBLE else View.GONE
        screenView.findViewWithTag<Button>("keys")?.let { highlight(it, screenKeysOpen) }
        val imm = getSystemService(InputMethodManager::class.java)
        if (screenKeysOpen) {
            screenTyper.requestFocus()
            screenTyper.post { imm.showSoftInput(screenTyper, InputMethodManager.SHOW_IMPLICIT) }
        } else {
            imm.hideSoftInputFromWindow(screenTyper.windowToken, 0)
            screenTyper.clearFocus()
        }
    }

    private fun applyOrientation() {
        requestedOrientation = when {
            !showsPicture -> ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
            portrait -> ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT
            else -> ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        }
    }

    /** Lays the picture out (fitted) and zoomed. */
    private fun applyViewport() {
        val fit = viewport.fit()
        val lp = surfaceView.layoutParams as FrameLayout.LayoutParams
        val w = fit[2].toInt().coerceAtLeast(1)
        val h = fit[3].toInt().coerceAtLeast(1)
        if (lp.width != w || lp.height != h || lp.leftMargin != fit[0].toInt() || lp.topMargin != fit[1].toInt()) {
            lp.width = w
            lp.height = h
            lp.leftMargin = fit[0].toInt()
            lp.topMargin = fit[1].toInt()
            lp.gravity = Gravity.TOP or Gravity.START
            surfaceView.layoutParams = lp
        }
        surfaceView.pivotX = 0f
        surfaceView.pivotY = 0f
        surfaceView.scaleX = viewport.zoom
        surfaceView.scaleY = viewport.zoom
        surfaceView.translationX = viewport.tx
        surfaceView.translationY = viewport.ty
    }

    /** Turns touches into [GestureEngine] calls (and its long-press clock). */
    private inner class TouchView(context: android.content.Context) : View(context) {
        var engine: GestureEngine? = null

        override fun onTouchEvent(e: MotionEvent): Boolean {
            val engine = engine ?: return false
            val skip = if (e.actionMasked == MotionEvent.ACTION_POINTER_UP) e.actionIndex else if (e.actionMasked == MotionEvent.ACTION_UP || e.actionMasked == MotionEvent.ACTION_CANCEL) -2 else -1
            val pts = (0 until e.pointerCount).filter { it != skip && skip != -2 }.map { Pt(e.getPointerId(it), e.getX(it), e.getY(it)) }
            engine.onTouch(e.eventTime, pts)
            if (pts.size == 1 && e.actionMasked == MotionEvent.ACTION_DOWN) {
                postDelayed({ engine.longPress(SystemClock.uptimeMillis()) }, 450)
                removeCallbacks(edgeTicker)
                postOnAnimation(edgeTicker)
            }
            if (pts.isEmpty()) removeCallbacks(edgeTicker)
            return true
        }

        /** Keeps sliding a zoomed picture while a finger rests near an edge. */
        private val edgeTicker = object : Runnable {
            override fun run() {
                val engine = engine ?: return
                engine.edgeTick()
                postOnAnimation(this)
            }
        }

        override fun onDetachedFromWindow() {
            removeCallbacks(edgeTicker)
            super.onDetachedFromWindow()
        }
    }

    private fun buildPad() {
        padView = FrameLayout(this)
        padGestures = GestureEngine(Viewport(1f, 1f, 1f, 1f), resources.displayMetrics.density, ::send).apply { trackpad = true }
        val pad = TouchView(this).apply {
            engine = padGestures
            background = GradientDrawable().apply {
                setColor(getColor(R.color.surface))
                cornerRadius = dp(18).toFloat()
                setStroke(dp(1), getColor(R.color.outline))
            }
        }
        val column = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        column.addView(pad, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f).apply { setMargins(dp(10), dp(10), dp(10), dp(6)) })
        column.addView(
            TextView(this).apply {
                text = "Move with one finger · tap to click · two fingers: scroll, tap for right-click · hold, then drag"
                setTextColor(getColor(R.color.muted))
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
                gravity = Gravity.CENTER
            },
        )
        val buttons = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(dp(10), dp(6), dp(10), dp(6))
        }
        fun hold(label: String, button: Int) = pill(label) {}.apply {
            setOnTouchListener { _, e ->
                when (e.actionMasked) {
                    MotionEvent.ACTION_DOWN -> send(Input.Button(button, true))
                    MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> send(Input.Button(button, false))
                }
                true
            }
        }
        buttons.addView(hold("Left", 1), LinearLayout.LayoutParams(0, dp(56), 1f).apply { marginEnd = dp(6) })
        buttons.addView(hold("Middle", 2), LinearLayout.LayoutParams(0, dp(56), 0.5f).apply { marginEnd = dp(6) })
        buttons.addView(hold("Right", 3), LinearLayout.LayoutParams(0, dp(56), 1f))
        column.addView(buttons)
        val speed = SeekBar(this).apply {
            max = 100
            progress = 50
            setPadding(dp(24), dp(4), dp(24), dp(10))
            padGestures.speed = speedOf(progress)
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(bar: SeekBar, value: Int, fromUser: Boolean) {
                    padGestures.speed = speedOf(value)
                }

                override fun onStartTrackingTouch(bar: SeekBar) {}
                override fun onStopTrackingTouch(bar: SeekBar) {}
            })
        }
        column.addView(TextView(this).apply {
            text = "Pointer speed"
            setTextColor(getColor(R.color.muted))
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            gravity = Gravity.CENTER
        })
        column.addView(speed)
        padView.addView(column, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
    }

    // ---- keyboard ----

    private fun tapKey(sym: Int) {
        val mods = held.toList()
        for (m in mods) send(Input.Key(m, true))
        send(Input.Key(sym, true))
        send(Input.Key(sym, false))
        for (m in mods.reversed()) send(Input.Key(m, false))
        held.clear()
        stickyButtons.values.flatten().forEach { highlight(it, false) }
    }

    /** Text from the phone's keyboard; with a modifier held (Ctrl+C) it goes as keys instead. */
    private fun typeText(s: CharSequence) {
        if (held.isEmpty()) {
            send(Input.Text(s.toString()))
            return
        }
        s.codePoints().forEach { cp -> Keysym.forChar(cp)?.let { tapKey(it) } }
    }

    /** A box the phone's keyboard types into: what's typed goes to the PC as it's typed. */
    private fun newTyper(hintText: String): EditText = EditText(this).apply {
        hint = hintText
        setHintTextColor(getColor(R.color.muted))
        setTextColor(getColor(R.color.text))
        inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or InputType.TYPE_TEXT_FLAG_MULTI_LINE or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
        setPadding(dp(16), dp(14), dp(16), dp(14))
        background = GradientDrawable().apply {
            setColor(getColor(R.color.surface_high))
            cornerRadius = dp(16).toFloat()
            setStroke(dp(1), getColor(R.color.outline))
        }
        addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence, start: Int, count: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence, start: Int, before: Int, count: Int) {
                if (before > 0 && count == 0) repeat(before) { tapBackspace() }
                if (count > 0) typeText(s.subSequence(start, start + count))
            }

            override fun afterTextChanged(s: Editable) {
                // The box only carries what was just typed.
                if (s.length > 200) s.clear()
            }
        })
        setOnKeyListener { _, code, e ->
            if (e.action != KeyEvent.ACTION_DOWN) return@setOnKeyListener false
            if (code == KeyEvent.KEYCODE_DEL && text.isNotEmpty()) return@setOnKeyListener false
            val sym = Keysym.forAndroidKey(code) ?: return@setOnKeyListener false
            tapKey(sym)
            true
        }
    }

    /** A scrolling row of key buttons. */
    private fun keyRow(vararg items: Pair<String, (Button) -> Unit>): HorizontalScrollView {
        val line = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(dp(8), dp(4), dp(8), dp(4))
        }
        for ((label, action) in items) {
            val b = pill(label) { action(it) }
            STICKY[label]?.let { sym -> stickyButtons.getOrPut(sym) { mutableListOf() }.add(b) }
            line.addView(b, LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { marginEnd = dp(6) })
        }
        return HorizontalScrollView(this).apply {
            isHorizontalScrollBarEnabled = false
            addView(line)
        }
    }

    /** Ctrl, Alt, Shift, Super: tap to hold for the next key (every copy of the button lights up). */
    private fun sticky(label: String, sym: Int): Pair<String, (Button) -> Unit> = label to {
        if (!held.remove(sym)) held.add(sym)
        stickyButtons[sym]?.forEach { highlight(it, sym in held) }
    }

    private fun key(label: String, sym: Int): Pair<String, (Button) -> Unit> = label to { tapKey(sym) }

    private fun combo(label: String, mods: List<Int>, sym: Int): Pair<String, (Button) -> Unit> = label to {
        for (m in mods) send(Input.Key(m, true))
        send(Input.Key(sym, true))
        send(Input.Key(sym, false))
        for (m in mods.reversed()) send(Input.Key(m, false))
    }

    private fun buildKeyboard() {
        keyboardView = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        typer = newTyper("Type here to type on the PC")
        keyboardView.addView(typer, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { setMargins(dp(10), dp(10), dp(10), dp(6)) })
        keyboardView.addView(keyRow(sticky("Ctrl", Keysym.CONTROL_L), sticky("Alt", Keysym.ALT_L), sticky("Shift", Keysym.SHIFT_L), sticky("Super", Keysym.SUPER_L)))
        keyboardView.addView(
            keyRow(
                key("Esc", Keysym.ESCAPE), key("Tab", Keysym.TAB), key("←", Keysym.LEFT), key("↓", Keysym.DOWN), key("↑", Keysym.UP), key("→", Keysym.RIGHT),
                key("Del", Keysym.DELETE), key("Home", Keysym.HOME), key("End", Keysym.END), key("PgUp", Keysym.PAGE_UP), key("PgDn", Keysym.PAGE_DOWN), key("Ins", Keysym.INSERT),
            ),
        )
        keyboardView.addView(keyRow(*(1..12).map { key("F$it", Keysym.function(it)) }.toTypedArray()))
        val ctrl = listOf(Keysym.CONTROL_L)
        keyboardView.addView(
            keyRow(
                combo("Copy", ctrl, 'c'.code), combo("Paste", ctrl, 'v'.code), combo("Cut", ctrl, 'x'.code), combo("Undo", ctrl, 'z'.code),
                combo("Select all", ctrl, 'a'.code), combo("Alt+Tab", listOf(Keysym.ALT_L), Keysym.TAB), key("Super", Keysym.SUPER_L), combo("Lock", listOf(Keysym.SUPER_L), 'l'.code),
            ),
        )
        keyboardView.addView(
            TextView(this).apply {
                text = "Tap Ctrl, Alt, Shift or Super, then a key on the keyboard above, for a shortcut."
                setTextColor(getColor(R.color.muted))
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
                setPadding(dp(16), dp(10), dp(16), 0)
            },
        )
    }

    private fun tapBackspace() {
        send(Input.Key(Keysym.BACKSPACE, true))
        send(Input.Key(Keysym.BACKSPACE, false))
    }

    // ---- media ----

    private fun buildMedia() {
        mediaView = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(dp(16), dp(16), dp(16), dp(16))
        }
        fun line(vararg items: Pair<String, RemoteProto.Media>) {
            val l = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
            for ((label, media) in items) {
                l.addView(
                    pill(label) { send(Input.MediaKey(media)) }.apply { setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f) },
                    LinearLayout.LayoutParams(0, dp(72), 1f).apply { setMargins(dp(5), dp(5), dp(5), dp(5)) },
                )
            }
            mediaView.addView(l, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
        }
        mediaView.addView(
            TextView(this).apply {
                text = "Plays on the PC: Zazabor's player or any media player that's open"
                setTextColor(getColor(R.color.muted))
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
                gravity = Gravity.CENTER
                setPadding(0, 0, 0, dp(14))
            },
        )
        line("Previous" to RemoteProto.Media.PREV, "Play / pause" to RemoteProto.Media.PLAY_PAUSE, "Next" to RemoteProto.Media.NEXT)
        line("−10 s" to RemoteProto.Media.SEEK_BACK, "Stop" to RemoteProto.Media.STOP, "+10 s" to RemoteProto.Media.SEEK_FWD)
        line("Volume −" to RemoteProto.Media.VOL_DOWN, "Mute" to RemoteProto.Media.MUTE, "Volume +" to RemoteProto.Media.VOL_UP)
    }

    // ---- the controls around the picture ----

    /**
     * Shows or hides the top bar, the screen's tools and the phone's own status and navigation bars.
     * On the Screen tab they hide 3 s after the last touch, so the PC's screen fills the phone.
     */
    private fun setChrome(shown: Boolean) {
        main.removeCallbacks(hideChrome)
        chromeShown = shown
        bar.visibility = if (shown) View.VISIBLE else View.GONE
        tools.visibility = if (shown) View.VISIBLE else View.GONE
        reveal.visibility = if (shown || !showsPicture) View.GONE else View.VISIBLE
        val full = !shown && showsPicture
        if (Build.VERSION.SDK_INT >= 30) {
            window.insetsController?.let { c ->
                if (full) {
                    c.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                    c.hide(WindowInsets.Type.systemBars())
                } else {
                    c.show(WindowInsets.Type.systemBars())
                }
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = if (full) {
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or View.SYSTEM_UI_FLAG_FULLSCREEN or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
            } else {
                View.SYSTEM_UI_FLAG_VISIBLE
            }
        }
        if (shown && showsPicture) main.postDelayed(hideChrome, CHROME_MS)
    }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        // Using the controls keeps them up a little longer.
        if (chromeShown && showsPicture && ev.actionMasked == MotionEvent.ACTION_DOWN) {
            main.removeCallbacks(hideChrome)
            main.postDelayed(hideChrome, CHROME_MS)
        }
        return super.dispatchTouchEvent(ev)
    }

    // ---- modes ----

    private fun select(next: Int) {
        if (next == CAMERA && client != null && !media) {
            toast("$pcName doesn't share its camera. Turn it on in Zazabor Home: Settings → Phone Link → Remote control.")
            return
        }
        if (next == CAMERA && client != null && cameraCount == 0) {
            toast("$pcName has no camera")
            return
        }
        mode = next
        tabs.forEachIndexed { i, t ->
            t.setTextColor(getColor(if (i == next) R.color.accent else R.color.muted))
            t.background = if (i == next) GradientDrawable().apply {
                setColor(getColor(R.color.accent_soft))
                cornerRadius = dp(18).toFloat()
            } else null
        }
        val picture = next == SCREEN || next == CAMERA
        screenArea.visibility = if (picture) View.VISIBLE else View.GONE
        screenView.findViewWithTag<View>("keys")?.visibility = if (next == SCREEN) View.VISIBLE else View.GONE
        if (next != SCREEN && screenKeysOpen) setScreenKeys(false)
        screenView.findViewWithTag<View>("direct")?.visibility = if (next == SCREEN) View.VISIBLE else View.GONE
        screenView.findViewWithTag<View>("monitor")?.visibility = if (next == SCREEN && monitors > 1) View.VISIBLE else View.GONE
        screenView.findViewWithTag<Button>("camera")?.apply {
            visibility = if (next == CAMERA && cameraCount > 1) View.VISIBLE else View.GONE
            text = "Camera ${camera + 1}"
        }
        if (picture) {
            val want = if (next == CAMERA) "camera" else "screen"
            if (want != pcSource) {
                pcSource = want
                client?.send(JSONObject().put("t", "source").put("src", want).put("i", camera))
            }
        }
        padView.visibility = if (next == TOUCHPAD) View.VISIBLE else View.GONE
        keyboardView.visibility = if (next == KEYBOARD) View.VISIBLE else View.GONE
        mediaView.visibility = if (next == MEDIA) View.VISIBLE else View.GONE
        setChrome(true)
        applyOrientation()
        val imm = getSystemService(InputMethodManager::class.java)
        if (next == KEYBOARD) {
            typer.requestFocus()
            typer.post { imm.showSoftInput(typer, InputMethodManager.SHOW_IMPLICIT) }
        } else {
            imm.hideSoftInputFromWindow(typer.windowToken, 0)
        }
        setVideo(picture)
    }

    private fun toast(text: String) = Toast.makeText(this, text, Toast.LENGTH_LONG).show()

    /** Asks the PC for its sound, its microphone, or nothing; the PC's `audio` answer sets the button. */
    private fun askSound(src: String) {
        val c = client ?: return
        c.send(JSONObject().put("t", "audio").put("src", src))
        soundButton.text = "…"
    }

    private fun onAudioInfo(msg: JSONObject) {
        sound = msg.optString("src", "off")
        audio?.release()
        audio = null
        if (sound != "off") {
            audio = try {
                RemoteAudio(msg.optInt("rate", 48_000), msg.optInt("channels", 2), msg.optInt("preSkip", 312))
            } catch (e: Exception) {
                toast("This phone can't play the PC's sound: ${e.message}")
                client?.send(JSONObject().put("t", "audio").put("src", "off"))
                null
            }
        }
        soundButton.text = SOUNDS.firstOrNull { it.first == sound }?.second ?: SOUNDS[0].second
        soundButton.setTextColor(getColor(if (sound == "off") R.color.muted else R.color.accent))
    }

    /** The PC only sends the picture while it's on screen. */
    private fun setVideo(on: Boolean) {
        if (on == videoOn) return
        videoOn = on
        client?.send(JSONObject().put("t", "video").put("on", on))
        if (!on) {
            decoder?.release()
            decoder = null
        } else {
            startDecoder()
        }
    }

    // ---- the session ----

    private fun connect() {
        val tryHost = LinkService.endpoint(this)
        if (tryHost == null) {
            showStatus("Not linked to a PC. Open Zazabor Link and pair first.")
            return
        }
        pcName = tryHost.pcName
        internet = tryHost.internet
        if (internet) {
            quality = INTERNET_QUALITY
            screenView.findViewWithTag<Button>("quality")?.text = QUALITIES[quality].label
        }
        val q = QUALITIES[quality]
        targetKbps = q.kbps
        kbps = if (internet) INTERNET_START_KBPS else q.kbps
        steadySince = SystemClock.elapsedRealtime()
        showStatus(if (internet) "Asking $pcName over the internet…" else "Asking $pcName…")
        thread(name = "zrem-start", isDaemon = true) {
            try {
                val ask = JSONObject().put("t", "remote.start").put("maxW", q.w).put("maxH", q.h).put("fps", 30).put("kbps", kbps).put("video", showsPicture)
                val re = LinkService.request(ask, 75_000)
                val wss = if (tryHost.internet) {
                    re.optString("wss").takeIf { it.startsWith("wss://") } ?: throw java.io.IOException("$pcName didn't say where to connect over the internet. Update Zazabor Home.")
                } else {
                    null
                }
                val c = RemoteClient(tryHost.host, re.getInt("port"), wss, Link.unb64(re.getString("token")), Identity.phoneId(this), tryHost.linkKey)
                c.connect()
                client = c
                videoOn = showsPicture
                main.post { applyInfo(re) }
                c.start(this)
                main.post {
                    status.visibility = View.GONE
                    ticker.begin()
                }
            } catch (e: Exception) {
                main.post { showStatus(e.message ?: "Couldn't connect to the PC") }
            }
        }
    }

    private var clock: TextView? = null
    private val clockTick = object : android.view.Choreographer.FrameCallback {
        override fun doFrame(frameTimeNanos: Long) {
            val c = clock ?: return
            c.text = "%08d".format(System.currentTimeMillis() % 100_000_000)
            android.view.Choreographer.getInstance().postFrameCallback(this)
        }
    }

    private fun toggleClock() {
        val existing = clock
        if (existing != null) {
            screenView.removeView(existing)
            clock = null
            return
        }
        clock = TextView(this).apply {
            setTextColor(Color.YELLOW)
            setBackgroundColor(0xAA000000.toInt())
            typeface = android.graphics.Typeface.MONOSPACE
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 22f)
            setPadding(dp(8), dp(2), dp(8), dp(2))
        }
        screenView.addView(clock, FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM or Gravity.START))
        android.view.Choreographer.getInstance().postFrameCallback(clockTick)
    }

    private fun showStatus(text: String) {
        status.text = text
        status.visibility = View.VISIBLE
    }

    private fun applyInfo(info: JSONObject) {
        picW = info.optInt("w", picW)
        picH = info.optInt("h", picH)
        monitors = info.optJSONArray("monitors")?.length() ?: monitors
        monitor = info.optInt("monitor", monitor)
        viewOnly = info.optBoolean("viewOnly", viewOnly)
        media = info.optBoolean("media", media)
        cameraCount = info.optJSONArray("cameras")?.length() ?: cameraCount
        if (info.has("camera") && !info.isNull("camera")) camera = info.optInt("camera", camera)
        val source = info.optString("source", pcSource)
        pcSource = source
        // The PC went back to the screen (its camera didn't open): follow it.
        if (source == "screen" && mode == CAMERA) select(SCREEN)
        viewport.picW = picW.toFloat()
        viewport.picH = picH.toFloat()
        applyViewport()
        screenView.findViewWithTag<Button>("monitor")?.apply {
            text = "Monitor ${monitor + 1}"
            visibility = if (monitors > 1 && mode == SCREEN) View.VISIBLE else View.GONE
        }
        screenView.findViewWithTag<Button>("camera")?.apply {
            text = "Camera ${camera + 1}"
            visibility = if (cameraCount > 1 && mode == CAMERA) View.VISIBLE else View.GONE
        }
        if (viewOnly) statsLabel.text = "view only"
        startDecoder(restart = true)
    }

    private fun startDecoder(restart: Boolean = false) {
        val holder = surface ?: return
        if (!videoOn || picW == 0) return
        if (decoder != null && !restart) return
        decoder?.release()
        decoder = try {
            RemoteDecoder(holder.surface, picW, picH) { client?.send(JSONObject().put("t", "keyframe")) }
        } catch (e: Exception) {
            main.post { showStatus("This phone can't decode the picture: ${e.message}") }
            null
        }
    }

    override fun onMessage(msg: JSONObject) {
        when (msg.optString("t")) {
            "info" -> main.post { applyInfo(msg) }
            "pong" -> {
                rtt = (System.nanoTime() / 1_000_000) - msg.optLong("ts")
                main.post { adapt() }
            }
            "audio" -> main.post { onAudioInfo(msg) }
            "notice" -> main.post { toast(msg.optString("text")) }
            "error" -> main.post { showStatus(msg.optString("error", "The PC stopped the session")) }
            "end" -> {}
        }
    }

    override fun onVideo(plain: ByteArray, frame: RemoteProto.Video) {
        decoder?.feed(plain, frame)
    }

    override fun onAudio(plain: ByteArray) {
        audio?.feed(plain)
    }

    /**
     * Over the internet: a ping well above the best one seen means the PC's frames are queuing
     * somewhere on the way, so the bitrate drops by 30 %; five calm seconds raise it by 15 %, up
     * to what the quality chip asks for.
     */
    private fun adapt() {
        if (!internet || rtt < 0) return
        minRtt = minOf(minRtt, rtt)
        val now = SystemClock.elapsedRealtime()
        val next = when {
            rtt > minRtt + 250 -> maxOf(400, kbps * 7 / 10)
            rtt < minRtt + 80 && now - steadySince > 5000 -> minOf(targetKbps, kbps * 115 / 100)
            else -> kbps
        }
        if (rtt >= minRtt + 80) steadySince = now
        if (next != kbps) {
            kbps = next
            steadySince = now
            client?.send(JSONObject().put("t", "bitrate").put("kbps", kbps))
        }
    }

    override fun onClosed(reason: String?) {
        if (ended) return
        main.post {
            if (!isFinishing) {
                ticker.stop()
                showStatus(reason?.let { "Disconnected: $it" } ?: "The session ended")
            }
        }
    }

    /** Pings the PC (it hangs up on a silent phone) and refreshes the stats line once a second. */
    private val ticker = object : Runnable {
        private var running = false
        private var n = 0

        fun begin() {
            if (running) return
            running = true
            main.postDelayed(this, 1000)
        }

        fun stop() {
            running = false
            main.removeCallbacks(this)
        }

        override fun run() {
            if (!running) return
            val d = decoder
            if (d != null) {
                val fps = d.frames - lastFrames
                val mbps = (d.bytes - lastBytes) * 8 / 1_000_000.0
                lastFrames = d.frames
                lastBytes = d.bytes
                stats = "%d fps · %.1f Mbps · decode %s ms".format(fps, mbps, if (d.latencyMs != Long.MIN_VALUE) d.latencyMs.toString() else "–")
            } else {
                stats = ""
            }
            if (!viewOnly) statsLabel.text = listOf(stats, if (rtt >= 0) "ping $rtt ms" else "", if (internet) "internet" else "").filter { it.isNotEmpty() }.joinToString(" · ")
            // Over the internet the ping drives the bitrate, so it goes every second.
            if (internet || n++ % 3 == 0) client?.send(JSONObject().put("t", "ping").put("ts", System.nanoTime() / 1_000_000))
            main.postDelayed(this, 1000)
        }
    }

    override fun onStop() {
        super.onStop()
        // The session ends when the screen leaves: the PC shows a red pill for as long as it runs.
        if (!isChangingConfigurations) finish()
    }

    override fun onDestroy() {
        ended = true
        main.removeCallbacks(hideChrome)
        ticker.stop()
        client?.send(JSONObject().put("t", "stop"))
        decoder?.release()
        audio?.release()
        audio = null
        thread(isDaemon = true) {
            Thread.sleep(250)
            client?.close()
        }
        super.onDestroy()
    }
}
