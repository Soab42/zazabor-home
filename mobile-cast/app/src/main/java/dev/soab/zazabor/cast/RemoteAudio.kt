package dev.soab.zazabor.cast

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat
import java.nio.ByteBuffer
import kotlin.concurrent.thread

/**
 * Plays the PC's sound (or its microphone) as it arrives: Opus packets from the remote channel go
 * into a MediaCodec decoder, and its PCM into a low-latency AudioTrack. When the track runs more
 * than [MAX_LAG_MS] behind (the network delivered a burst after a stall), the late audio is
 * dropped, so it never drifts behind the picture.
 */
class RemoteAudio(private val rate: Int, private val channels: Int, preSkip: Int) {
    companion object {
        private const val MAX_LAG_MS = 250
    }

    private val codec: MediaCodec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
    private val track: AudioTrack
    @Volatile private var running = true
    private var written = 0L

    init {
        val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, rate, channels).apply {
            setByteBuffer("csd-0", ByteBuffer.wrap(RemoteProto.opusHead(channels, preSkip, rate)))
            setByteBuffer("csd-1", ByteBuffer.wrap(RemoteProto.le64(preSkip * 1_000_000_000L / rate)))
            setByteBuffer("csd-2", ByteBuffer.wrap(RemoteProto.le64(80_000_000L)))
        }
        codec.configure(format, null, null, 0)
        codec.start()
        val mask = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
        val min = AudioTrack.getMinBufferSize(rate, mask, AudioFormat.ENCODING_PCM_16BIT)
        track = AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MUSIC).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(rate).setChannelMask(mask).setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
            .setBufferSizeInBytes(maxOf(min, rate * channels * 2 / 5))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
            .build()
        track.play()
        thread(name = "zrem-audio-out", isDaemon = true) { drain() }
    }

    /** One audio frame's plaintext (the packet starts at [RemoteProto.AUDIO_OFFSET]). */
    fun feed(plain: ByteArray) {
        if (!running || plain.size <= RemoteProto.AUDIO_OFFSET) return
        try {
            val index = codec.dequeueInputBuffer(5_000)
            if (index < 0) return
            val buffer = codec.getInputBuffer(index) ?: return
            buffer.clear()
            val n = plain.size - RemoteProto.AUDIO_OFFSET
            buffer.put(plain, RemoteProto.AUDIO_OFFSET, n)
            codec.queueInputBuffer(index, 0, n, System.nanoTime() / 1000, 0)
        } catch (_: IllegalStateException) {
        }
    }

    private fun drain() {
        val info = MediaCodec.BufferInfo()
        val frameBytes = channels * 2
        var pcm = ByteArray(0)
        while (running) {
            try {
                val index = codec.dequeueOutputBuffer(info, 20_000)
                if (index < 0) continue
                val out = codec.getOutputBuffer(index)
                if (out != null && info.size > 0) {
                    if (pcm.size < info.size) pcm = ByteArray(info.size)
                    out.position(info.offset)
                    out.get(pcm, 0, info.size)
                    val queued = written - track.playbackHeadPosition.toLong()
                    // Too far behind: drop this packet instead of adding to the delay.
                    if (queued * 1000 / rate < MAX_LAG_MS) {
                        val n = track.write(pcm, 0, info.size, AudioTrack.WRITE_NON_BLOCKING)
                        if (n > 0) written += n / frameBytes
                    }
                }
                codec.releaseOutputBuffer(index, false)
            } catch (_: IllegalStateException) {
                if (!running) break
            }
        }
    }

    fun release() {
        running = false
        try {
            codec.stop()
            codec.release()
        } catch (_: Exception) {
        }
        try {
            track.stop()
            track.release()
        } catch (_: Exception) {
        }
    }
}
