package dev.soab.zazabor.cast

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.InputStream
import java.net.ServerSocket
import kotlin.concurrent.thread

class WsSocketTest {
    // RFC 6455 §1.3's example.
    @Test
    fun theAcceptKeyIsTheRfcs() {
        assertEquals("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=", WsSocket.acceptKey("dGhlIHNhbXBsZSBub25jZQ=="))
    }

    @Test
    fun theOpusHeadIsRfc7845s() {
        val head = RemoteProto.opusHead(2, 312, 48_000)
        assertEquals(19, head.size)
        assertEquals("OpusHead", String(head, 0, 8, Charsets.US_ASCII))
        assertEquals(listOf(1, 2, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0), head.drop(8).map { it.toInt() and 0xff })
        assertArrayEquals(byteArrayOf(1, 0, 0, 0, 0, 0, 0, 0), RemoteProto.le64(1))
    }

    private fun line(input: InputStream): String {
        val sb = StringBuilder()
        while (true) {
            val b = input.read()
            if (b < 0 || b == '\n'.code) break
            sb.append(b.toChar())
        }
        return sb.toString().trimEnd('\r')
    }

    /**
     * A stand-in for the PC's bridge: checks the upgrade, unmasks what the phone sends, and answers
     * with unmasked messages split at odd places, which the phone must read as one byte stream.
     */
    @Test
    fun bytesGoThroughAsOneStreamBothWays() {
        val server = ServerSocket(0)
        val got = arrayOfNulls<Any>(3)
        val pc = thread {
            server.accept().use { s ->
                val input = DataInputStream(s.getInputStream())
                val out = DataOutputStream(s.getOutputStream())
                got[0] = line(input)
                var key = ""
                while (true) {
                    val l = line(input)
                    if (l.isEmpty()) break
                    if (l.lowercase().startsWith("sec-websocket-key:")) key = l.substringAfter(':').trim()
                }
                out.write("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${WsSocket.acceptKey(key)}\r\n\r\n".toByteArray())
                out.flush()
                // One masked message from the phone.
                val b0 = input.readUnsignedByte()
                val b1 = input.readUnsignedByte()
                got[1] = (b0 to (b1 and 0x80))
                val len = b1 and 0x7f
                val mask = ByteArray(4).also { input.readFully(it) }
                val body = ByteArray(len).also { input.readFully(it) }
                got[2] = ByteArray(len) { (body[it].toInt() xor mask[it % 4].toInt()).toByte() }
                // "ZLNK/1\n" in two messages, a ping in between, then a 300-byte one (16-bit length).
                fun frame(op: Int, payload: ByteArray) {
                    out.write(0x80 or op)
                    if (payload.size < 126) out.write(payload.size) else {
                        out.write(126)
                        out.writeShort(payload.size)
                    }
                    out.write(payload)
                }
                frame(2, "ZLN".toByteArray())
                frame(9, "hi".toByteArray())
                frame(2, "K/1\n".toByteArray())
                frame(2, ByteArray(300) { it.toByte() })
                out.flush()
                // The phone answers the ping with a pong.
                val p0 = input.readUnsignedByte()
                val p1 = input.readUnsignedByte() and 0x7f
                val pm = ByteArray(4).also { input.readFully(it) }
                val pb = ByteArray(p1).also { input.readFully(it) }
                assertEquals(0x8a, p0)
                assertEquals("hi", String(ByteArray(p1) { (pb[it].toInt() xor pm[it % 4].toInt()).toByte() }))
                Thread.sleep(200)
            }
        }
        val ws = WsSocket.open("ws://127.0.0.1:${server.localPort}/ws/link")
        val out = DataOutputStream(ws.getOutputStream())
        out.writeInt(5)
        out.write("hello".toByteArray())
        out.flush()
        val input = DataInputStream(ws.getInputStream())
        assertEquals("ZLNK/1", Link.readLine(input))
        val rest = ByteArray(300).also { input.readFully(it) }
        assertArrayEquals(ByteArray(300) { it.toByte() }, rest)
        pc.join(5000)
        ws.close()
        server.close()
        assertEquals("GET /ws/link HTTP/1.1", got[0])
        // Final binary frame, masked, and the writes before the flush went out as one message.
        assertEquals(0x82 to 0x80, got[1])
        assertArrayEquals(byteArrayOf(0, 0, 0, 5) + "hello".toByteArray(), got[2] as ByteArray)
        assertTrue(ws.isClosed)
    }
}
