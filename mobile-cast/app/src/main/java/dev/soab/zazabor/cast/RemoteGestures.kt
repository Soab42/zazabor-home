package dev.soab.zazabor.cast

import dev.soab.zazabor.cast.RemoteProto.Input
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sqrt

/** One finger on the screen. */
data class Pt(val id: Int, val x: Float, val y: Float)

/**
 * Where the PC's picture sits inside the view and how it's zoomed. The picture is laid out to fit
 * (letterboxed) and then scaled by `zoom` about its own top-left corner and moved by
 * (`tx`, `ty`), which is what the view's scale and translation do on the SurfaceView.
 */
class Viewport(var viewW: Float, var viewH: Float, var picW: Float, var picH: Float) {
    var zoom = 1f
    var tx = 0f
    var ty = 0f

    /** The fitted picture's rectangle before zoom: left, top, width, height. */
    fun fit(): FloatArray {
        if (picW <= 0f || picH <= 0f || viewW <= 0f || viewH <= 0f) return floatArrayOf(0f, 0f, viewW, viewH)
        val s = min(viewW / picW, viewH / picH)
        val w = picW * s
        val h = picH * s
        return floatArrayOf((viewW - w) / 2, (viewH - h) / 2, w, h)
    }

    /** A touch position as 0..65535 across the PC's monitor (clamped to the picture). */
    fun toWire(px: Float, py: Float): IntArray {
        val (fx, fy, fw, fh) = fit().let { listOf(it[0], it[1], it[2], it[3]) }
        val u = ((px - fx - tx) / (fw * zoom)).coerceIn(0f, 1f)
        val v = ((py - fy - ty) / (fh * zoom)).coerceIn(0f, 1f)
        return intArrayOf((u * 65535f + 0.5f).toInt(), (v * 65535f + 0.5f).toInt())
    }

    /** Zooms by `factor` keeping the point under (`cx`, `cy`) where it is. */
    fun zoomAt(factor: Float, cx: Float, cy: Float) {
        val (fx, fy) = fit().let { it[0] to it[1] }
        val next = (zoom * factor).coerceIn(1f, MAX_ZOOM)
        val f = next / zoom
        tx = cx - fx - (cx - fx - tx) * f
        ty = cy - fy - (cy - fy - ty) * f
        zoom = next
        clamp()
    }

    fun pan(dx: Float, dy: Float) {
        tx += dx
        ty += dy
        clamp()
    }

    fun reset() {
        zoom = 1f
        tx = 0f
        ty = 0f
    }

    /**
     * While zoomed, a finger near an edge of the view slides the picture that way, so what's off
     * screen can be reached with the finger that also moves the pointer. Deeper into the edge band
     * means faster. True if the picture moved.
     */
    fun edgePan(x: Float, y: Float, step: Float): Boolean {
        if (zoom <= 1.01f || viewW <= 0f || viewH <= 0f) return false
        fun push(p: Float, size: Float): Float {
            val band = size * EDGE
            return when {
                p < band -> step * (1f - p / band)
                p > size - band -> -step * (1f - (size - p) / band)
                else -> 0f
            }
        }
        val dx = push(x, viewW)
        val dy = push(y, viewH)
        if (dx == 0f && dy == 0f) return false
        val (ox, oy) = tx to ty
        pan(dx, dy)
        return tx != ox || ty != oy
    }

    /**
     * Keeps the zoomed picture in view: larger than the view on an axis, it moves until its edges
     * meet the view's edges (not just the fitted rectangle's, which left a letterboxed picture
     * unable to scroll up or down); smaller, it stays centred.
     */
    private fun clamp() {
        val f = fit()
        tx = clampAxis(tx, f[0], f[2] * zoom, viewW)
        ty = clampAxis(ty, f[1], f[3] * zoom, viewH)
    }

    private fun clampAxis(t: Float, start: Float, size: Float, view: Float): Float =
        if (size <= view) (view - size) / 2f - start else t.coerceIn(view - size - start, -start)

    companion object {
        const val MAX_ZOOM = 5f
        /** The share of the view at each edge where a finger slides a zoomed picture. */
        const val EDGE = 0.12f
    }
}

/** The pointer's speed: slow movements stay precise, quick ones cover the screen. */
fun accelerate(delta: Float, speed: Float): Float {
    val a = abs(delta)
    val gain = speed * (1f + min(a, 40f) / 20f)
    return delta * gain
}

/**
 * Turns fingers into the PC's mouse. Feed it the fingers that are down after each touch event
 * (empty when all are up). Direct mode puts the pointer where a finger is; trackpad mode moves it
 * by how far the finger moved. Both: a tap clicks, a two-finger tap right-clicks, a three-finger
 * tap middle-clicks, a long press then a move drags, two fingers moving scroll or (spreading or
 * pinching) zoom the picture, and once they've zoomed, the same two fingers move it; three fingers
 * moving pan it too. A finger resting near an edge of a zoomed picture keeps sliding it that way.
 */
class GestureEngine(
    private val viewport: Viewport,
    private val density: Float,
    private val send: (Input) -> Unit,
    private val onViewport: () -> Unit = {},
) {
    var trackpad = false
    var speed = 1.4f

    private val slop = 10f * density
    private val tapMs = 280L
    private val longMs = 420L
    /** Pixels of two-finger travel for one wheel notch. */
    private val notchPx = 36f * density

    private var count = 0
    private var downAt = 0L
    private var start: List<Pt> = emptyList()
    /** Where the fingers were when the most came down: a multi-finger tap clicks there. */
    private var peakPts: List<Pt> = emptyList()
    private var last: List<Pt> = emptyList()
    private var moved = false
    private var dragging = false
    private var pinched = false
    private var peak = 0
    private var wheelRemX = 0f
    private var wheelRemY = 0f
    private var longPending = false

    /** True while a long press is waiting for the caller to call [longPress]. */
    fun wantsLongPress(): Boolean = longPending

    fun onTouch(t: Long, pts: List<Pt>) {
        val n = pts.size
        if (n != count) transition(t, pts)
        else if (n > 0) move(t, pts)
        count = n
        last = pts
    }

    private fun centroid(p: List<Pt>) = floatArrayOf(p.map { it.x }.average().toFloat(), p.map { it.y }.average().toFloat())

    private fun spread(p: List<Pt>): Float = if (p.size < 2) 0f else hypot(p[0].x - p[1].x, p[0].y - p[1].y)

    private fun transition(t: Long, pts: List<Pt>) {
        val n = pts.size
        if (n > count) {
            // A finger came down.
            if (dragging) endDrag()
            if (count == 0) {
                downAt = t
                moved = false
                pinched = false
                peak = 0
                wheelRemX = 0f
                wheelRemY = 0f
            }
            peak = max(peak, n)
            start = pts
            peakPts = pts
            longPending = n == 1
        } else {
            // A finger lifted.
            longPending = false
            if (dragging) {
                endDrag()
            } else if (n == 0 && !moved && !pinched && t - downAt < tapMs) {
                click(peak, peakPts)
            }
            start = pts
        }
    }

    private fun click(fingers: Int, at: List<Pt>) {
        val button = when (fingers) {
            1 -> 1
            2 -> 3
            else -> 2
        }
        if (!trackpad) {
            val c = centroid(at)
            val w = viewport.toWire(c[0], c[1])
            send(Input.MoveAbs(w[0], w[1]))
        }
        send(Input.Button(button, true))
        send(Input.Button(button, false))
    }

    /** The caller's long-press timer fired with the same single finger still down. */
    fun longPress(t: Long) {
        if (count != 1 || moved || !longPending || t - downAt < longMs) return
        longPending = false
        dragging = true
        if (!trackpad) {
            val w = viewport.toWire(last[0].x, last[0].y)
            send(Input.MoveAbs(w[0], w[1]))
        }
        send(Input.Button(1, true))
    }

    /**
     * Called about 60 times a second while one finger is down: a finger resting in an edge band of
     * a zoomed picture keeps sliding it (and the pointer under the finger follows). True if it moved.
     */
    fun edgeTick(): Boolean {
        if (trackpad || count != 1 || peak > 1 || (!moved && !dragging) || last.isEmpty()) return false
        val p = last[0]
        if (!viewport.edgePan(p.x, p.y, 10f * density)) return false
        val w = viewport.toWire(p.x, p.y)
        send(Input.MoveAbs(w[0], w[1]))
        onViewport()
        return true
    }

    private fun endDrag() {
        dragging = false
        send(Input.Button(1, false))
    }

    private fun move(t: Long, pts: List<Pt>) {
        val n = pts.size
        val c = centroid(pts)
        val prev = centroid(last)
        val dx = c[0] - prev[0]
        val dy = c[1] - prev[1]
        if (!moved && hypot(c[0] - centroid(start)[0], c[1] - centroid(start)[1]) > slop) moved = true
        when (n) {
            1 -> {
                // The finger left over after a two-finger gesture does nothing until it lifts.
                if ((!moved && !dragging) || peak > 1) return
                longPending = false
                if (trackpad) {
                    val rx = accelerate(dx, speed).toInt()
                    val ry = accelerate(dy, speed).toInt()
                    if (rx != 0 || ry != 0) send(Input.MoveRel(rx, ry))
                } else {
                    if (viewport.edgePan(pts[0].x, pts[0].y, 22f * density)) onViewport()
                    val w = viewport.toWire(pts[0].x, pts[0].y)
                    send(Input.MoveAbs(w[0], w[1]))
                }
            }
            2 -> {
                val before = spread(last)
                val now = spread(pts)
                // Spreading or pinching zooms; the rest scrolls.
                if (before > 0f && abs(now - before) > abs(hypot(dx, dy)) * 0.8f && abs(now - before) > 1f) {
                    pinched = true
                    viewport.zoomAt(now / before, c[0], c[1])
                    viewport.pan(dx, dy)
                    onViewport()
                } else if (pinched) {
                    // After zooming, the same two fingers move the picture (like a photo).
                    viewport.pan(dx, dy)
                    onViewport()
                } else if (moved) {
                    wheelRemX += dx
                    wheelRemY += dy
                    val ex = (wheelRemX / notchPx * 8f).toInt()
                    val ey = (wheelRemY / notchPx * 8f).toInt()
                    if (ex != 0 || ey != 0) {
                        wheelRemX -= ex * notchPx / 8f
                        wheelRemY -= ey * notchPx / 8f
                        // Fingers moving up scroll the page down, like the phone's own lists.
                        send(Input.Wheel(-ex, -ey))
                    }
                }
            }
            else -> {
                if (moved) {
                    viewport.pan(dx, dy)
                    onViewport()
                }
            }
        }
    }
}

/** How far the speed slider (0..100) turns the pointer: 0.5x to 3x. */
fun speedOf(progress: Int): Float = 0.5f + 2.5f * sqrt(progress.coerceIn(0, 100) / 100f)
