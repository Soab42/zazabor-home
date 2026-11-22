package dev.soab.zazabor.cast

import android.app.Activity
import android.content.pm.ActivityInfo
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
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
import android.view.WindowManager
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.TextView
import dev.soab.zazabor.cast.RemoteProto.Input
import dev.soab.zazabor.cast.RemoteProto.Keysym
import org.json.JSONObject
import kotlin.concurrent.thread

/**
 * Controls the PC from the phone (docs/plans/remote.md): Screen (the PC's picture, touch moves
 * the pointer), Touchpad (no picture, lowest battery), Keyboard (the phone's keyboard types on the
 * PC, plus the keys it lacks) and Media. One sealed session to the PC serves all four; the PC asks
 * its user first, and shows a red pill for as long as it runs.
 */
class RemoteActivity : Activity(), RemoteClient.Listener {
    companion object {
        private const val SCREEN = 0
        private const val TOUCHPAD = 1
        private const val KEYBOARD = 2
        private const val MEDIA = 3
        private val TABS = listOf("Screen", "Touchpad", "Keyboard", "Media")

        /** The picture size the phone asks the PC for (the PC never sends more than its monitor has). */
        private const val MAX_W = 1600
        private const val MAX_H = 900

        private class Quality(val label: String, val w: Int, val h: Int, val kbps: Int)

        private val QUALITIES = listOf(Quality("900p", 1600, 900, 4000), Quality("1080p", 1920, 1080, 6000), Quality("720p", 1280, 720, 3000), Quality("540p", 960, 540, 1500))
    }

    private var quality = 0

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
    private val stickyButtons = mutableMapOf<Int, Button>()

    private fun dp(v: Int): Int = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    private fun send(i: Input) {
        if (!viewOnly) client?.send(i)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.statusBarColor = Color.BLACK
        window.navigationBarColor = Color.BLACK
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
        tabs = TABS.mapIndexed { i, name ->
            TextView(this).apply {
                text = name
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
                gravity = Gravity.CENTER
                setPadding(dp(12), dp(8), dp(12), dp(8))
                setOnClickListener { select(i) }
            }.also { bar.addView(it) }
        }
        statsLabel = TextView(this).apply {
            setTextColor(getColor(R.color.muted))
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            gravity = Gravity.END
        }
        // Tap the stats line for a live clock over the picture: with the PC's own clock on its
        // screen, one screenshot shows how late the picture is (diagnostics).
        statsLabel.setOnClickListener { toggleClock() }
        bar.addView(statsLabel, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
        root.addView(bar, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))

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
        content.addView(screenView)
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
        })
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
            client?.send(JSONObject().put("t", "quality").put("maxW", q.w).put("maxH", q.h).put("kbps", q.kbps))
        }.also { it.tag = "quality" })
        tools.addView(pill("Reset zoom") {
            viewport.reset()
            applyViewport()
        })
        screenView.addView(tools, FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM or Gravity.END))
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
            }
            return true
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
        stickyButtons.values.forEach { highlight(it, false) }
    }

    /** Text from the phone's keyboard; with a modifier held (Ctrl+C) it goes as keys instead. */
    private fun typeText(s: CharSequence) {
        if (held.isEmpty()) {
            send(Input.Text(s.toString()))
            return
        }
        s.codePoints().forEach { cp -> Keysym.forChar(cp)?.let { tapKey(it) } }
    }

    private fun buildKeyboard() {
        keyboardView = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        typer = EditText(this).apply {
            hint = "Type here to type on the PC"
            setHintTextColor(getColor(R.color.muted))
            setTextColor(getColor(R.color.text))
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or InputType.TYPE_TEXT_FLAG_MULTI_LINE or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
            setPadding(dp(16), dp(14), dp(16), dp(14))
            background = GradientDrawable().apply {
                setColor(getColor(R.color.surface))
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
        keyboardView.addView(typer, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { setMargins(dp(10), dp(10), dp(10), dp(6)) })
        fun row(vararg items: Pair<String, () -> Unit>): HorizontalScrollView {
            val line = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                setPadding(dp(8), dp(4), dp(8), dp(4))
            }
            for ((label, action) in items) {
                line.addView(pill(label) { action() }, LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { marginEnd = dp(6) })
            }
            return HorizontalScrollView(this).apply {
                isHorizontalScrollBarEnabled = false
                addView(line)
            }
        }
        fun sticky(label: String, sym: Int): Pair<String, () -> Unit> = label to {
            if (!held.remove(sym)) held.add(sym)
            stickyButtons[sym]?.let { highlight(it, sym in held) }
        }
        fun key(label: String, sym: Int): Pair<String, () -> Unit> = label to { tapKey(sym) }
        fun combo(label: String, mods: List<Int>, sym: Int): Pair<String, () -> Unit> = label to {
            for (m in mods) send(Input.Key(m, true))
            send(Input.Key(sym, true))
            send(Input.Key(sym, false))
            for (m in mods.reversed()) send(Input.Key(m, false))
        }
        val modifiers = row(sticky("Ctrl", Keysym.CONTROL_L), sticky("Alt", Keysym.ALT_L), sticky("Shift", Keysym.SHIFT_L), sticky("Super", Keysym.SUPER_L))
        val modLine = (modifiers.getChildAt(0) as LinearLayout)
        listOf(Keysym.CONTROL_L, Keysym.ALT_L, Keysym.SHIFT_L, Keysym.SUPER_L).forEachIndexed { i, sym -> stickyButtons[sym] = modLine.getChildAt(i) as Button }
        keyboardView.addView(modifiers)
        keyboardView.addView(
            row(
                key("Esc", Keysym.ESCAPE), key("Tab", Keysym.TAB), key("←", Keysym.LEFT), key("↓", Keysym.DOWN), key("↑", Keysym.UP), key("→", Keysym.RIGHT),
                key("Del", Keysym.DELETE), key("Home", Keysym.HOME), key("End", Keysym.END), key("PgUp", Keysym.PAGE_UP), key("PgDn", Keysym.PAGE_DOWN), key("Ins", Keysym.INSERT),
            ),
        )
        keyboardView.addView(row(*(1..12).map { key("F$it", Keysym.function(it)) }.toTypedArray()))
        val ctrl = listOf(Keysym.CONTROL_L)
        keyboardView.addView(
            row(
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

    // ---- modes ----

    private fun select(next: Int) {
        mode = next
        tabs.forEachIndexed { i, t ->
            t.setTextColor(getColor(if (i == next) R.color.accent else R.color.muted))
            t.background = if (i == next) GradientDrawable().apply {
                setColor(getColor(R.color.accent_soft))
                cornerRadius = dp(18).toFloat()
            } else null
        }
        screenView.visibility = if (next == SCREEN) View.VISIBLE else View.GONE
        padView.visibility = if (next == TOUCHPAD) View.VISIBLE else View.GONE
        keyboardView.visibility = if (next == KEYBOARD) View.VISIBLE else View.GONE
        mediaView.visibility = if (next == MEDIA) View.VISIBLE else View.GONE
        requestedOrientation = if (next == SCREEN) ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE else ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
        val imm = getSystemService(InputMethodManager::class.java)
        if (next == KEYBOARD) {
            typer.requestFocus()
            typer.post { imm.showSoftInput(typer, InputMethodManager.SHOW_IMPLICIT) }
        } else {
            imm.hideSoftInputFromWindow(typer.windowToken, 0)
        }
        setVideo(next == SCREEN)
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
        showStatus("Asking $pcName…")
        thread(name = "zrem-start", isDaemon = true) {
            try {
                val ask = JSONObject().put("t", "remote.start").put("maxW", MAX_W).put("maxH", MAX_H).put("fps", 30).put("kbps", 4000).put("video", mode == SCREEN)
                val re = LinkService.request(ask, 75_000)
                val c = RemoteClient(tryHost.host, re.getInt("port"), Link.unb64(re.getString("token")), Identity.phoneId(this), tryHost.linkKey)
                c.connect()
                client = c
                videoOn = mode == SCREEN
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
        viewport.picW = picW.toFloat()
        viewport.picH = picH.toFloat()
        applyViewport()
        screenView.findViewWithTag<Button>("monitor")?.apply {
            text = "Monitor ${monitor + 1}"
            visibility = if (monitors > 1) View.VISIBLE else View.GONE
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
            "pong" -> rtt = (System.nanoTime() / 1_000_000) - msg.optLong("ts")
            "error" -> main.post { showStatus(msg.optString("error", "The PC stopped the session")) }
            "end" -> {}
        }
    }

    override fun onVideo(plain: ByteArray, frame: RemoteProto.Video) {
        decoder?.feed(plain, frame)
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
            if (!viewOnly) statsLabel.text = listOf(stats, if (rtt >= 0) "ping $rtt ms" else "").filter { it.isNotEmpty() }.joinToString(" · ")
            if (n++ % 3 == 0) client?.send(JSONObject().put("t", "ping").put("ts", System.nanoTime() / 1_000_000))
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
        ticker.stop()
        client?.send(JSONObject().put("t", "stop"))
        decoder?.release()
        thread(isDaemon = true) {
            Thread.sleep(250)
            client?.close()
        }
        super.onDestroy()
    }
}
