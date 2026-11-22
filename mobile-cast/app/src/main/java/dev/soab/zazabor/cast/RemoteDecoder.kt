package dev.soab.zazabor.cast

import android.media.MediaCodec
import android.media.MediaFormat
import android.view.Surface
import kotlin.concurrent.thread

/**
 * The PC's H.264 on the phone's screen: a MediaCodec decoder that renders each picture the moment
 * it is decoded (low-latency mode), into the Remote screen's SurfaceView. Pictures that arrive
 * while the decoder is busy are dropped and a keyframe is asked for, so the picture catches up
 * instead of running late.
 */
class RemoteDecoder(private val surface: Surface, private val width: Int, private val height: Int, private val onNeedKey: () -> Unit) {
    private val codec: MediaCodec = MediaCodec.createDecoderByType("video/avc")
    @Volatile private var running = true
    @Volatile private var waitingForKey = true
    private var configured = false

    /**
     * Pictures shown, bytes received, and how long the newest picture spent in the decoder (ms,
     * from `feed` to the screen; Long.MIN_VALUE until one is known), for the stats line. It is
     * measured on this phone's own clock, so it needs no agreement with the PC's.
     */
    @Volatile var frames = 0L
        private set
    @Volatile var bytes = 0L
        private set
    @Volatile var latencyMs = Long.MIN_VALUE
        private set

    // When each picture went into the decoder, by its presentation time.
    private val stampKeys = LongArray(64) { -1L }

    init {
        val format = MediaFormat.createVideoFormat("video/avc", width, height).apply {
            setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 1 shl 21)
        }
        codec.configure(format, surface, null, 0)
        codec.start()
        configured = true
        thread(name = "zrem-decode-out", isDaemon = true) { drain() }
    }

    /** Feeds one access unit (Annex-B starting at `offset` in `plain`). */
    fun feed(plain: ByteArray, frame: RemoteProto.Video) {
        if (!running) return
        bytes += plain.size - frame.offset
        if (waitingForKey && !frame.key) return
        try {
            val index = codec.dequeueInputBuffer(8_000)
            if (index < 0) {
                // Busy: skip this picture and the ones that depend on it.
                waitingForKey = true
                onNeedKey()
                return
            }
            if (frame.key) waitingForKey = false
            val buffer = codec.getInputBuffer(index) ?: return
            buffer.clear()
            buffer.put(plain, frame.offset, plain.size - frame.offset)
            val us = System.nanoTime() / 1000
            stampKeys[(us % stampKeys.size).toInt()] = us
            val flags = if (frame.key) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0
            codec.queueInputBuffer(index, 0, plain.size - frame.offset, us, flags)
        } catch (_: IllegalStateException) {
            // The decoder was stopped under us (the picture was switched off or the surface went).
        }
    }

    private fun drain() {
        val info = MediaCodec.BufferInfo()
        while (running) {
            try {
                val index = codec.dequeueOutputBuffer(info, 20_000)
                if (index >= 0) {
                    val slot = (info.presentationTimeUs % stampKeys.size).toInt()
                    if (stampKeys[slot] == info.presentationTimeUs) {
                        latencyMs = (System.nanoTime() / 1000 - info.presentationTimeUs) / 1000
                    }
                    codec.releaseOutputBuffer(index, true)
                    frames++
                }
            } catch (_: IllegalStateException) {
                if (!running) break
            }
        }
    }

    fun release() {
        running = false
        try {
            if (configured) {
                codec.stop()
                codec.release()
            }
        } catch (_: Exception) {
        }
    }
}
