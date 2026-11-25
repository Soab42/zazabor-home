package dev.soab.zazabor.cast

import dev.soab.zazabor.cast.RemoteProto.Input
import dev.soab.zazabor.cast.RemoteProto.Media
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.ServerSocket
import kotlin.concurrent.thread

class RemoteProtoTest {
    private fun bytes(vararg v: Int) = ByteArray(v.size) { v[it].toByte() }

    // The same bytes the PC's Rust tests pin (src-tauri/src/remote/tests.rs).
    @Test
    fun inputEventsHaveTheFixedLayoutThePcReads() {
        assertArrayEquals(bytes(0x20, 1, 0x12, 0x34, 0xff, 0xfe), Input.MoveAbs(0x1234, 0xfffe).encode())
        assertArrayEquals(bytes(0x20, 2, 0xff, 0xfe, 0, 3), Input.MoveRel(-2, 3).encode())
        assertArrayEquals(bytes(0x20, 3, 3, 1), Input.Button(3, true).encode())
        assertArrayEquals(bytes(0x20, 4, 0, 0, 0xff, 0xf8), Input.Wheel(0, -8).encode())
        assertArrayEquals(bytes(0x20, 5, 0, 0, 0xff, 0x0d, 1), Input.Key(0xff0d, true).encode())
        assertArrayEquals(bytes(0x20, 6, 0, 2, 'a'.code, 'b'.code), Input.Text("ab").encode())
        assertArrayEquals(bytes(0x20, 7, 6), Input.MediaKey(Media.MUTE).encode())
    }

    @Test
    fun valuesAreClampedToTheirFields() {
        assertArrayEquals(bytes(0x20, 1, 0xff, 0xff, 0, 0), Input.MoveAbs(70000, -5).encode())
        assertArrayEquals(bytes(0x20, 2, 0x7f, 0xff, 0x80, 0), Input.MoveRel(99999, -99999).encode())
    }

    @Test
    fun longTextIsCutAtACharacterNotInsideOne() {
        val long = "é".repeat(RemoteProto.MAX_TEXT)
        val e = Input.Text(long).encode()
        val n = ((e[2].toInt() and 0xff) shl 8) or (e[3].toInt() and 0xff)
        assertTrue(n <= RemoteProto.MAX_TEXT)
        assertEquals(n, e.size - 4)
        assertEquals("é".repeat(n / 2), String(e, 4, n, Charsets.UTF_8))
        // Unicode beyond Latin-1 and emoji survive as UTF-8.
        val t = Input.Text("বাংলা 😀").encode()
        assertEquals("বাংলা 😀", String(t, 4, t.size - 4, Charsets.UTF_8))
    }

    @Test
    fun keysymsForCharactersAndAndroidKeys() {
        assertEquals('a'.code, RemoteProto.Keysym.forChar('a'.code))
        assertEquals(0xe9, RemoteProto.Keysym.forChar('é'.code))
        assertEquals(RemoteProto.Keysym.RETURN, RemoteProto.Keysym.forChar('\n'.code))
        assertEquals(RemoteProto.Keysym.TAB, RemoteProto.Keysym.forChar('\t'.code))
        assertNull(RemoteProto.Keysym.forChar(7))
        assertEquals(0x01000995, RemoteProto.Keysym.forChar(0x0995))
        assertEquals(0x0101f600, RemoteProto.Keysym.forChar(0x1f600))
        assertEquals(RemoteProto.Keysym.BACKSPACE, RemoteProto.Keysym.forAndroidKey(67))
        assertEquals(RemoteProto.Keysym.LEFT, RemoteProto.Keysym.forAndroidKey(21))
        assertNull(RemoteProto.Keysym.forAndroidKey(29)) // KEYCODE_A is a character, typed as text
        assertEquals(0xffbe, RemoteProto.Keysym.function(1))
        assertEquals(0xffc9, RemoteProto.Keysym.function(12))
    }

    @Test
    fun videoFramesDecode() {
        val plain = bytes(0x10, 1, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 1, 0x65)
        val v = RemoteProto.parseVideo(plain)
        assertTrue(v.key)
        assertEquals(0x0102030405060708L, v.ptsUs)
        assertEquals(10, v.offset)
        assertFalse(RemoteProto.parseVideo(bytes(0x10, 0, 0, 0, 0, 0, 0, 0, 0, 1, 9)).key)
    }

    @Test
    fun sessionKeysAreTheirOwn() {
        val key = ByteArray(32) { 7 }
        val (a, b) = RemoteProto.sessionKeys(key, ByteArray(16) { 1 }, ByteArray(16) { 2 })
        assertNotEquals(a.toList(), b.toList())
        val (linkUp, _) = ZlnkCrypto.sessionKeys(key, ByteArray(16) { 1 }, ByteArray(16) { 2 })
        assertNotEquals(a.toList(), linkUp.toList())
    }

    /** A PC that opens the one-shot port the way the Rust side does, to see the phone's end work. */
    @Test
    fun theClientDialsProvesItselfAndTalksSealed() {
        val linkKey = ByteArray(32) { (it * 3).toByte() }
        val token = ByteArray(16) { (0xa0 + it).toByte() }
        val pcNonce = ByteArray(16) { (0xc0 + it).toByte() }
        val server = ServerSocket(0)
        val seen = arrayOfNulls<Any>(3)
        val pc = thread {
            server.accept().use { s ->
                val input = DataInputStream(s.getInputStream())
                val out = DataOutputStream(s.getOutputStream())
                assertEquals("ZREM/1", Link.readLine(input))
                val hello = JSONObject(Link.readLine(input)!!)
                assertEquals("ab".repeat(16), hello.getString("phoneId"))
                assertArrayEquals(token, Link.unb64(hello.getString("token")))
                val phoneNonce = Link.unb64(hello.getString("nonce"))
                Link.writeLine(out, JSONObject().put("ok", true).put("nonce", Link.b64(pcNonce)))
                out.flush()
                val (up, down) = RemoteProto.sessionKeys(linkKey, phoneNonce, pcNonce)
                // The first sealed frame is `ready`, then the input event.
                var counter = 0L
                fun read(): ByteArray {
                    val n = input.readInt()
                    val sealed = ByteArray(n)
                    input.readFully(sealed)
                    return ZlnkCrypto.open(up, counter++, sealed)
                }
                val ready = read()
                seen[0] = String(ready, 1, ready.size - 1)
                seen[1] = read().toList()
                val reply = RemoteProto.K_JSON.toByte().let { byteArrayOf(it) + """{"t":"info","w":1280}""".toByteArray() }
                val sealed = ZlnkCrypto.seal(down, 0, reply)
                out.writeInt(sealed.size)
                out.write(sealed)
                out.flush()
                Thread.sleep(300)
            }
        }
        val got = java.util.concurrent.LinkedBlockingQueue<JSONObject>()
        val client = RemoteClient("127.0.0.1", server.localPort, null, token, "ab".repeat(16), linkKey)
        client.connect()
        client.start(object : RemoteClient.Listener {
            override fun onMessage(msg: JSONObject) {
                got.put(msg)
            }

            override fun onVideo(plain: ByteArray, frame: RemoteProto.Video) {}
            override fun onClosed(reason: String?) {}
        })
        client.send(Input.Button(1, true))
        val info = got.poll(5, java.util.concurrent.TimeUnit.SECONDS)
        pc.join(5000)
        client.close()
        server.close()
        assertEquals("""{"t":"ready"}""", seen[0])
        assertEquals(Input.Button(1, true).encode().toList(), seen[1])
        assertEquals(1280, info?.optInt("w"))
    }
}
