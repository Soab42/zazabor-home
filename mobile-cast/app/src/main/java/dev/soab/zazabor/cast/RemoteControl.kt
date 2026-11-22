package dev.soab.zazabor.cast

import org.json.JSONObject

/**
 * What the PC asks this phone to do while it controls it (`ctl` messages, docs/plans/remote.md).
 * Positions are fractions (0..1) of the screen, so the PC's window can be any size and the phone
 * can be turned.
 */
sealed class Ctl {
    data class Tap(val x: Float, val y: Float) : Ctl()
    data class LongPress(val x: Float, val y: Float) : Ctl()

    /** A finger moving through `points` over `ms`. */
    data class Swipe(val points: List<Pair<Float, Float>>, val ms: Long) : Ctl()

    /** A mouse wheel at (x, y): `dy` of the screen's height, positive = further down the page. */
    data class Scroll(val x: Float, val y: Float, val dx: Float, val dy: Float) : Ctl()
    data class Global(val action: String) : Ctl()
    data class Text(val text: String) : Ctl()

    /** "enter", "del", "left"… */
    data class Key(val key: String) : Ctl()
    data class Volume(val direction: String) : Ctl()

    companion object {
        val GLOBALS = setOf("back", "home", "recents", "notifications", "quick", "lock", "screenshot", "power")
        const val MAX_POINTS = 256

        fun parse(msg: JSONObject): Ctl? {
            fun f(key: String) = msg.optDouble(key, Double.NaN).toFloat().takeIf { !it.isNaN() }?.coerceIn(0f, 1f)
            return when (msg.optString("k")) {
                "tap" -> Tap(f("x") ?: return null, f("y") ?: return null)
                "long" -> LongPress(f("x") ?: return null, f("y") ?: return null)
                "swipe" -> {
                    val p = msg.optJSONArray("p") ?: return null
                    if (p.length() < 2 || p.length() > MAX_POINTS) return null
                    val points = (0 until p.length()).map { i ->
                        val pt = p.optJSONArray(i) ?: return null
                        pt.optDouble(0).toFloat().coerceIn(0f, 1f) to pt.optDouble(1).toFloat().coerceIn(0f, 1f)
                    }
                    Swipe(points, msg.optLong("ms", 250).coerceIn(50, 5000))
                }
                "scroll" -> Scroll(
                    f("x") ?: return null, f("y") ?: return null,
                    msg.optDouble("dx", 0.0).toFloat().coerceIn(-1f, 1f), msg.optDouble("dy", 0.0).toFloat().coerceIn(-1f, 1f),
                )
                "global" -> msg.optString("a").takeIf { it in GLOBALS }?.let { Global(it) }
                "text" -> msg.optString("s").takeIf { it.isNotEmpty() }?.take(4096)?.let { Text(it) }
                "key" -> msg.optString("key").takeIf { it in KEYS }?.let { Key(it) }
                "volume" -> msg.optString("d").takeIf { it in setOf("up", "down", "mute") }?.let { Volume(it) }
                else -> null
            }
        }

        val KEYS = setOf("enter", "del", "left", "right", "up", "down")
    }
}

/** The pure maths of turning a [Ctl] into pixels (the service draws them with `Path`). */
object ControlMath {
    /** The swipe a mouse wheel turn makes: from (x, y) to the point `dy`/`dx` of the screen further on. */
    fun scrollSwipe(s: Ctl.Scroll): Ctl.Swipe {
        // Scrolling down means the finger moves up.
        val ex = (s.x - s.dx).coerceIn(0.02f, 0.98f)
        val ey = (s.y - s.dy).coerceIn(0.02f, 0.98f)
        return Ctl.Swipe(listOf(s.x to s.y, ex to ey), 260)
    }

    fun toPixels(points: List<Pair<Float, Float>>, w: Int, h: Int): List<Pair<Float, Float>> =
        points.map { (x, y) -> x * (w - 1) to y * (h - 1) }

    /** The text after typing `typed` at the cursor in `text`; (new text, new cursor). */
    fun insert(text: String, start: Int, end: Int, typed: String): Pair<String, Int> {
        val a = start.coerceIn(0, text.length)
        val b = end.coerceIn(a, text.length)
        return (text.substring(0, a) + typed + text.substring(b)) to (a + typed.length)
    }

    /** The text after Backspace: the selection, or the character before the cursor. */
    fun backspace(text: String, start: Int, end: Int): Pair<String, Int> {
        val a = start.coerceIn(0, text.length)
        val b = end.coerceIn(a, text.length)
        if (b > a) return (text.substring(0, a) + text.substring(b)) to a
        if (a == 0) return text to 0
        // Never leaves half of a surrogate pair behind.
        val from = if (a >= 2 && Character.isLowSurrogate(text[a - 1]) && Character.isHighSurrogate(text[a - 2])) a - 2 else a - 1
        return (text.substring(0, from) + text.substring(a)) to from
    }
}
