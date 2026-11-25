package dev.soab.zazabor.cast

import java.io.ByteArrayOutputStream

/**
 * The remote channel's wire format (`ZREM/1`, docs/plans/remote.md): session keys, the input
 * events the phone sends to the PC as compact binary records, and the video frames that come back.
 * Plain Kotlin, so the JVM tests pin the exact bytes the PC's Rust decoder reads.
 */
object RemoteProto {
    const val MAGIC = "ZREM/1"
    const val K_JSON = 1
    const val K_VIDEO = 0x10
    /** The PC's sound: `11 ‖ flags ‖ pts µs (8) ‖ one Opus packet`. */
    const val K_AUDIO = 0x11
    const val K_INPUT = 0x20
    const val MAX_FRAME = 4 shl 20
    const val F_KEY = 1

    /** (phone→PC key, PC→phone key) for one remote session. */
    fun sessionKeys(linkKey: ByteArray, phoneNonce: ByteArray, pcNonce: ByteArray): Pair<ByteArray, ByteArray> {
        val k = ZlnkCrypto.hkdf(linkKey, phoneNonce + pcNonce, "zrem session", 64)
        return k.copyOfRange(0, 32) to k.copyOfRange(32, 64)
    }

    /** What the Media tab asks for. */
    enum class Media(val code: Int) { PLAY_PAUSE(1), NEXT(2), PREV(3), VOL_UP(4), VOL_DOWN(5), MUTE(6), STOP(7), SEEK_BACK(8), SEEK_FWD(9) }

    /** One input event; positions are 0..65535 across the captured monitor. */
    sealed class Input {
        data class MoveAbs(val x: Int, val y: Int) : Input()
        data class MoveRel(val dx: Int, val dy: Int) : Input()
        data class Button(val button: Int, val down: Boolean) : Input()
        data class Wheel(val dx: Int, val dy: Int) : Input()
        data class Key(val keysym: Int, val down: Boolean) : Input()
        data class Text(val text: String) : Input()
        data class MediaKey(val media: Media) : Input()

        /** The frame's plaintext: `20 ‖ kind ‖ fields`, big-endian. */
        fun encode(): ByteArray {
            val out = ByteArrayOutputStream()
            fun u8(v: Int) = out.write(v and 0xff)
            fun u16(v: Int) {
                u8(v ushr 8)
                u8(v)
            }
            fun u32(v: Int) {
                u16(v ushr 16)
                u16(v)
            }
            u8(K_INPUT)
            when (this) {
                is MoveAbs -> {
                    u8(1)
                    u16(x.coerceIn(0, 65535))
                    u16(y.coerceIn(0, 65535))
                }
                is MoveRel -> {
                    u8(2)
                    u16(dx.coerceIn(-32768, 32767))
                    u16(dy.coerceIn(-32768, 32767))
                }
                is Button -> {
                    u8(3)
                    u8(button)
                    u8(if (down) 1 else 0)
                }
                is Wheel -> {
                    u8(4)
                    u16(dx.coerceIn(-32768, 32767))
                    u16(dy.coerceIn(-32768, 32767))
                }
                is Key -> {
                    u8(5)
                    u32(keysym)
                    u8(if (down) 1 else 0)
                }
                is Text -> {
                    var bytes = text.toByteArray(Charsets.UTF_8)
                    if (bytes.size > MAX_TEXT) {
                        var end = MAX_TEXT
                        while (end > 0 && (bytes[end].toInt() and 0xC0) == 0x80) end--
                        bytes = bytes.copyOf(end)
                    }
                    u8(6)
                    u16(bytes.size)
                    out.write(bytes)
                }
                is MediaKey -> {
                    u8(7)
                    u8(media.code)
                }
            }
            return out.toByteArray()
        }
    }

    const val MAX_TEXT = 4096

    /** A decoded video frame: where its Annex-B data starts in the plaintext. */
    class Video(val key: Boolean, val ptsUs: Long, val offset: Int)

    fun parseVideo(plain: ByteArray): Video {
        require(plain.size > 10 && plain[0].toInt() == K_VIDEO) { "Not a video frame" }
        var pts = 0L
        for (i in 2 until 10) pts = (pts shl 8) or (plain[i].toLong() and 0xff)
        return Video((plain[1].toInt() and F_KEY) != 0, pts, 10)
    }

    /** Where an audio frame's Opus packet starts. */
    const val AUDIO_OFFSET = 10

    /**
     * The `OpusHead` (RFC 7845 §5.1) MediaCodec wants as csd-0: version 1, the channel count,
     * the pre-skip, the input rate, no gain, mapping family 0.
     */
    fun opusHead(channels: Int, preSkip: Int, rate: Int): ByteArray {
        val out = ByteArrayOutputStream()
        out.write("OpusHead".toByteArray(Charsets.US_ASCII))
        out.write(1)
        out.write(channels)
        out.write(preSkip and 0xff)
        out.write((preSkip ushr 8) and 0xff)
        for (i in 0 until 4) out.write((rate ushr (8 * i)) and 0xff)
        out.write(0)
        out.write(0)
        out.write(0)
        return out.toByteArray()
    }

    /** A little-endian 64-bit value (csd-1 and csd-2 are nanoseconds this way). */
    fun le64(v: Long): ByteArray = ByteArray(8) { i -> (v ushr (8 * i)).toByte() }

    /** X11 keysyms the keyboard tab sends. */
    object Keysym {
        const val BACKSPACE = 0xff08
        const val TAB = 0xff09
        const val RETURN = 0xff0d
        const val ESCAPE = 0xff1b
        const val DELETE = 0xffff
        const val HOME = 0xff50
        const val LEFT = 0xff51
        const val UP = 0xff52
        const val RIGHT = 0xff53
        const val DOWN = 0xff54
        const val PAGE_UP = 0xff55
        const val PAGE_DOWN = 0xff56
        const val END = 0xff57
        const val INSERT = 0xff63
        const val SHIFT_L = 0xffe1
        const val CONTROL_L = 0xffe3
        const val ALT_L = 0xffe9
        const val SUPER_L = 0xffeb
        const val F1 = 0xffbe

        /** F1..F12. */
        fun function(n: Int): Int = F1 + (n - 1).coerceIn(0, 11)

        /** The keysym a typed character is: Latin-1 as is, the rest a Unicode keysym. */
        fun forChar(cp: Int): Int? = when {
            cp == '\n'.code || cp == '\r'.code -> RETURN
            cp == '\t'.code -> TAB
            cp < 0x20 || cp == 0x7f -> null
            cp <= 0xff -> cp
            else -> 0x01000000 or cp
        }

        /** The keysym of an Android key event that isn't a character (Del, Enter, arrows…). */
        fun forAndroidKey(keyCode: Int): Int? = when (keyCode) {
            67 -> BACKSPACE // KEYCODE_DEL
            112 -> DELETE // KEYCODE_FORWARD_DEL
            66, 160 -> RETURN // ENTER, NUMPAD_ENTER
            61 -> TAB
            111 -> ESCAPE
            19 -> UP
            20 -> DOWN
            21 -> LEFT
            22 -> RIGHT
            122 -> HOME // MOVE_HOME
            123 -> END
            92 -> PAGE_UP
            93 -> PAGE_DOWN
            124 -> INSERT
            else -> null
        }
    }
}
