package dev.soab.zazabor.cast

import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.net.URI
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import javax.net.ssl.HttpsURLConnection
import javax.net.ssl.SSLSocket
import javax.net.ssl.SSLSocketFactory

/**
 * A WebSocket (RFC 6455) that looks like a [Socket] to the link and the remote channel, so ZLNK
 * and ZREM run over the internet path unchanged (docs/plans/remote.md "Any network"): bytes
 * written between two flushes go out as one binary message, and the messages that come in are
 * read back as one byte stream. The PC's bridge does the same on its side.
 *
 * Everything inside is already sealed end to end; TLS (with the hostname checked) is only the
 * outer layer Cloudflare needs. Plain JDK, so the JVM tests can drive it against a local server.
 */
class WsSocket private constructor(private val inner: Socket) : Socket() {
    companion object {
        /** A message from the PC: its bridge sends at most 64 KiB, so this is generous. */
        private const val MAX_MESSAGE = 8L shl 20

        /** Dials `wss://host/path` (or `ws://` for tests) and upgrades. Blocking. */
        fun open(url: String, timeoutMs: Int = 10_000): WsSocket {
            val uri = URI(url)
            val secure = when (uri.scheme) {
                "wss" -> true
                "ws" -> false
                else -> throw IOException("Not a WebSocket address: $url")
            }
            val host = uri.host ?: throw IOException("No host in $url")
            val port = if (uri.port > 0) uri.port else if (secure) 443 else 80
            val plain = Socket()
            plain.connect(InetSocketAddress(host, port), timeoutMs)
            plain.tcpNoDelay = true
            plain.keepAlive = true
            val sock: Socket = if (secure) {
                val tls = (SSLSocketFactory.getDefault() as SSLSocketFactory).createSocket(plain, host, port, true) as SSLSocket
                tls.soTimeout = timeoutMs
                tls.startHandshake()
                if (!HttpsURLConnection.getDefaultHostnameVerifier().verify(host, tls.session)) {
                    tls.close()
                    throw IOException("$host's certificate isn't for $host")
                }
                tls
            } else {
                plain
            }
            try {
                sock.soTimeout = timeoutMs
                val ws = WsSocket(sock)
                ws.upgrade(host, if ((secure && port == 443) || (!secure && port == 80)) host else "$host:$port", uri.rawPath.ifEmpty { "/" })
                sock.soTimeout = 0
                return ws
            } catch (e: Exception) {
                sock.close()
                throw e
            }
        }

        fun acceptKey(key: String): String =
            Base64.getEncoder().encodeToString(MessageDigest.getInstance("SHA-1").digest((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").toByteArray()))
    }

    private val rawIn = BufferedInputStream(inner.getInputStream(), 1 shl 16)
    private val rawOut = inner.getOutputStream()
    private val writeLock = Object()
    private val random = SecureRandom()
    @Volatile private var closedFlag = false

    private fun upgrade(host: String, hostHeader: String, path: String) {
        val key = ByteArray(16).also { random.nextBytes(it) }.let { Base64.getEncoder().encodeToString(it) }
        val req = "GET $path HTTP/1.1\r\nHost: $hostHeader\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
            "Sec-WebSocket-Key: $key\r\nSec-WebSocket-Version: 13\r\nUser-Agent: ZazaborLink\r\n\r\n"
        rawOut.write(req.toByteArray(Charsets.US_ASCII))
        rawOut.flush()
        val status = readHeaderLine() ?: throw EOFException("$host closed the connection")
        val headers = mutableMapOf<String, String>()
        while (true) {
            val line = readHeaderLine() ?: throw EOFException("$host closed the connection")
            if (line.isEmpty()) break
            val i = line.indexOf(':')
            if (i > 0) headers[line.substring(0, i).trim().lowercase()] = line.substring(i + 1).trim()
        }
        val code = status.split(' ').getOrNull(1)?.toIntOrNull() ?: 0
        if (code != 101) {
            throw IOException(
                when (code) {
                    404 -> "$host doesn't lead to this PC's link (404)"
                    429 -> "$host refused: too many tries, wait a few minutes"
                    502, 503, 530 -> "The PC isn't reachable through $host right now ($code). Is it on, with Zazabor Home running?"
                    else -> "$host answered $code instead of opening the link"
                },
            )
        }
        if (headers["sec-websocket-accept"] != acceptKey(key)) throw IOException("$host's WebSocket answer didn't match")
    }

    private fun readHeaderLine(): String? {
        val bytes = ByteArrayOutputStream()
        while (true) {
            val b = rawIn.read()
            if (b < 0) return if (bytes.size() == 0) null else bytes.toString("US-ASCII")
            if (b == '\n'.code) break
            if (bytes.size() > 8192) throw IOException("Header line too long")
            bytes.write(b)
        }
        return bytes.toString("US-ASCII").trimEnd('\r')
    }

    /** Writes one masked frame (a client's frames must be masked). */
    private fun writeFrame(op: Int, payload: ByteArray, off: Int, len: Int) {
        val head = ByteArray(14)
        var n = 0
        head[n++] = (0x80 or op).toByte()
        when {
            len < 126 -> head[n++] = (0x80 or len).toByte()
            len <= 0xffff -> {
                head[n++] = (0x80 or 126).toByte()
                head[n++] = (len ushr 8).toByte()
                head[n++] = len.toByte()
            }
            else -> {
                head[n++] = (0x80 or 127).toByte()
                for (i in 7 downTo 0) head[n++] = (len.toLong() ushr (8 * i)).toByte()
            }
        }
        val mask = ByteArray(4).also { random.nextBytes(it) }
        System.arraycopy(mask, 0, head, n, 4)
        n += 4
        val body = ByteArray(len)
        for (i in 0 until len) body[i] = (payload[off + i].toInt() xor mask[i and 3].toInt()).toByte()
        synchronized(writeLock) {
            if (closedFlag) throw IOException("The connection is closed")
            rawOut.write(head, 0, n)
            rawOut.write(body)
            rawOut.flush()
        }
    }

    private val input = object : InputStream() {
        private var buf = ByteArray(0)
        private var pos = 0

        /** Reads frames until one carries data; false at the end of the stream. */
        private fun fill(): Boolean {
            while (pos >= buf.size) {
                val b0 = rawIn.read()
                if (b0 < 0) return false
                val b1 = rawIn.read()
                if (b1 < 0) return false
                val op = b0 and 0x0f
                var len = (b1 and 0x7f).toLong()
                if (len == 126L) len = ((readByte() shl 8) or readByte()).toLong()
                else if (len == 127L) {
                    len = 0
                    repeat(8) { len = (len shl 8) or readByte().toLong() }
                }
                if (len < 0 || len > MAX_MESSAGE) throw IOException("A message from the PC was too big")
                val masked = b1 and 0x80 != 0
                val mask = if (masked) ByteArray(4).also { readFully(it) } else null
                val payload = ByteArray(len.toInt())
                readFully(payload)
                if (mask != null) for (i in payload.indices) payload[i] = (payload[i].toInt() xor mask[i and 3].toInt()).toByte()
                when (op) {
                    0, 1, 2 -> {
                        buf = payload
                        pos = 0
                    }
                    8 -> {
                        try {
                            writeFrame(8, ByteArray(0), 0, 0)
                        } catch (_: IOException) {
                        }
                        return false
                    }
                    9 -> writeFrame(10, payload, 0, payload.size)
                    else -> {}
                }
            }
            return true
        }

        private fun readByte(): Int {
            val b = rawIn.read()
            if (b < 0) throw EOFException("The connection ended")
            return b
        }

        private fun readFully(into: ByteArray) {
            var got = 0
            while (got < into.size) {
                val n = rawIn.read(into, got, into.size - got)
                if (n < 0) throw EOFException("The connection ended")
                got += n
            }
        }

        override fun read(): Int {
            if (!fill()) return -1
            return buf[pos++].toInt() and 0xff
        }

        override fun read(b: ByteArray, off: Int, len: Int): Int {
            if (len == 0) return 0
            if (!fill()) return -1
            val n = minOf(len, buf.size - pos)
            System.arraycopy(buf, pos, b, off, n)
            pos += n
            return n
        }

        override fun available(): Int = buf.size - pos

        override fun close() = this@WsSocket.close()
    }

    /** Collects writes; each flush sends what was collected as one message. */
    private val output = object : OutputStream() {
        private val pending = ByteArrayOutputStream()

        override fun write(b: Int) {
            synchronized(pending) { pending.write(b) }
        }

        override fun write(b: ByteArray, off: Int, len: Int) {
            synchronized(pending) { pending.write(b, off, len) }
        }

        override fun flush() {
            val bytes = synchronized(pending) {
                val out = pending.toByteArray()
                pending.reset()
                out
            }
            if (bytes.isNotEmpty()) writeFrame(2, bytes, 0, bytes.size)
        }

        override fun close() = this@WsSocket.close()
    }

    override fun getInputStream(): InputStream = input

    override fun getOutputStream(): OutputStream = output

    override fun setSoTimeout(timeout: Int) {
        inner.soTimeout = timeout
    }

    override fun getSoTimeout(): Int = inner.soTimeout

    override fun setTcpNoDelay(on: Boolean) {}

    override fun setKeepAlive(on: Boolean) {}

    override fun isConnected(): Boolean = inner.isConnected

    override fun isClosed(): Boolean = closedFlag || inner.isClosed

    /** Closes the connection at once (no close frame: a writer stuck on a full socket would hold it up). */
    override fun close() {
        if (closedFlag) return
        closedFlag = true
        try {
            inner.close()
        } catch (_: IOException) {
        }
    }
}
