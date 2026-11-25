package dev.soab.zazabor.cast

import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.Closeable
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import java.io.IOException
import java.net.Socket
import java.security.SecureRandom
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * The phone's end of the remote channel (`ZREM/1`): dials the one-shot port the PC opened for
 * `remote.start`, proves itself with the single-use token and a first sealed `ready` frame, then
 * reads video and messages (on its own thread) and writes input events (in order, on another, so
 * touches never block on the network).
 */
class RemoteClient(
    private val host: String,
    private val port: Int,
    /** Over the internet path: the PC's `wss://…/ws/remote/<port>` (then `host` isn't used). */
    private val wss: String?,
    private val token: ByteArray,
    private val phoneId: String,
    private val linkKey: ByteArray,
) : Closeable {
    interface Listener {
        fun onMessage(msg: JSONObject)
        fun onVideo(plain: ByteArray, frame: RemoteProto.Video)
        fun onAudio(plain: ByteArray) {}
        fun onClosed(reason: String?)
    }

    private var socket: Socket? = null
    private var input: DataInputStream? = null
    private var output: DataOutputStream? = null
    private lateinit var sendKey: ByteArray
    private lateinit var recvKey: ByteArray
    private var sendCounter = 0L
    private var recvCounter = 0L
    private val closed = AtomicBoolean(false)
    private val writer = Executors.newSingleThreadExecutor { r -> Thread(r, "zrem-write").apply { isDaemon = true } }

    val isOpen: Boolean get() = !closed.get()

    /** Dials and handshakes (blocking). */
    fun connect(prepare: (Socket) -> Unit = {}) {
        val s = if (wss != null) WsSocket.open(wss) else Link.open(host, port, 5000, prepare)
        socket = s
        s.soTimeout = 10_000
        val inp = DataInputStream(BufferedInputStream(s.getInputStream(), 1 shl 16))
        val out = DataOutputStream(s.getOutputStream())
        input = inp
        output = out
        val nonce = ByteArray(16).also { SecureRandom().nextBytes(it) }
        out.write("${RemoteProto.MAGIC}\n".toByteArray())
        Link.writeLine(out, JSONObject().put("phoneId", phoneId).put("token", Link.b64(token)).put("nonce", Link.b64(nonce)))
        out.flush()
        val answer = JSONObject(Link.readLine(inp) ?: throw EOFException("The PC closed the connection"))
        if (!answer.optBoolean("ok")) throw IOException("The PC refused the remote connection")
        val pcNonce = Link.unb64(answer.getString("nonce"))
        val (toPc, toPhone) = RemoteProto.sessionKeys(linkKey, nonce, pcNonce)
        sendKey = toPc
        recvKey = toPhone
        writeSealed(jsonPlain(JSONObject().put("t", "ready")))
        s.soTimeout = 0
    }

    private fun jsonPlain(msg: JSONObject): ByteArray {
        val body = msg.toString().toByteArray(Charsets.UTF_8)
        return ByteArray(1 + body.size).also {
            it[0] = RemoteProto.K_JSON.toByte()
            body.copyInto(it, 1)
        }
    }

    @Synchronized
    private fun writeSealed(plain: ByteArray) {
        val sealed = ZlnkCrypto.seal(sendKey, sendCounter++, plain)
        val out = output ?: throw IOException("Not connected")
        out.writeInt(sealed.size)
        out.write(sealed)
        out.flush()
    }

    private fun queue(plain: ByteArray) {
        if (closed.get()) return
        try {
            writer.execute {
                try {
                    writeSealed(plain)
                } catch (e: IOException) {
                    close(e.message)
                }
            }
        } catch (_: Exception) {
        }
    }

    fun send(input: RemoteProto.Input) = queue(input.encode())

    fun send(msg: JSONObject) = queue(jsonPlain(msg))

    /** Reads frames until the channel ends. */
    fun start(listener: Listener) {
        thread(name = "zrem-read", isDaemon = true) {
            var reason: String? = null
            try {
                val inp = input ?: return@thread
                while (!closed.get()) {
                    val length = inp.readInt()
                    if (length < ZlnkCrypto.TAG_BYTES || length > RemoteProto.MAX_FRAME + ZlnkCrypto.TAG_BYTES) throw IOException("Bad frame length $length")
                    val sealed = ByteArray(length)
                    inp.readFully(sealed)
                    val plain = ZlnkCrypto.open(recvKey, recvCounter++, sealed)
                    if (plain.isEmpty()) continue
                    when (plain[0].toInt()) {
                        RemoteProto.K_VIDEO -> listener.onVideo(plain, RemoteProto.parseVideo(plain))
                        RemoteProto.K_AUDIO -> listener.onAudio(plain)
                        RemoteProto.K_JSON -> listener.onMessage(JSONObject(String(plain, 1, plain.size - 1, Charsets.UTF_8)))
                    }
                }
            } catch (e: Exception) {
                if (!closed.get()) reason = e.message
            } finally {
                close(reason)
                listener.onClosed(reason)
            }
        }
    }

    fun close(reason: String? = null) {
        if (!closed.compareAndSet(false, true)) return
        try {
            socket?.close()
        } catch (_: IOException) {
        }
        writer.shutdown()
    }

    override fun close() = close(null)
}
