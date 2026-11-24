package dev.roadloader.core

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.Process
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Plays the float PCM blocks produced by the companion's audio pacer.
 *
 * The pacer emits one block per 10.58 ms whether or not anything is audible, so
 * the track is never asked to play what it was not given. Silence is written
 * only to keep the jitter buffer at its target level, and the result is read
 * back from the device (`AudioTrack.underrunCount`) instead of assumed.
 */
class RoadAudioBridge(private val session: RoadSession) {
    private var track: AudioTrack? = null

    @Volatile
    private var running = false

    private var thread: Thread? = null

    @Volatile
    var error: String? = null
        private set

    @Volatile
    var blocksPlayed = 0L
        private set

    /**
     * Polls that found the ring empty.
     *
     * The consumer only ever writes what the producer made, so it spends part of
     * each block period waiting; this counts the waits, not failures.
     */
    @Volatile
    var emptyPolls = 0L
        private set

    /** The device's own underrun count for the current track. */
    @Volatile
    var underruns = 0
        private set

    /** Age of the last block when it was handed to the track, in milliseconds. */
    @Volatile
    var audioAgeMs = -1L
        private set

    /** Blocks skipped because playing them would have added latency. */
    @Volatile
    var staleSkipped = 0L
        private set

    /** Bytes handed to the track and not yet played; the jitter buffer level. */
    private var writtenBytes = 0L

    /** Buffer the track was built with, in bytes. */
    private var trackBytes = 0

    @Volatile
    var levelMs = 0L
        private set

    @Volatile
    var format = AudioPolicy.formatName(0, 0)
        private set

    fun start(): Boolean {
        if (running) return true
        val created = createTrack()
        if (created == null) {
            Log.w(TAG, "no audio sink: $error; draining the bridge instead")
        } else {
            track = created
        }
        running = true
        thread = Thread({
            // Java thread priority is only a hint; this is the API that moves
            // the thread into the audio band of the scheduler.
            runCatching { Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO) }
                .onFailure { Log.w(TAG, "could not raise the audio thread priority", it) }
            val bytes = ByteArray(AudioPolicy.BLOCK_BYTES)
            val meta = LongArray(4)
            var lastStaleLogAt = 0L
            // Float PCM has to go through a ByteBuffer; write(ByteArray) only
            // accepts 8/16-bit PCM.
            val direct = ByteBuffer.allocateDirect(AudioPolicy.BLOCK_BYTES)
                .order(ByteOrder.nativeOrder())
            val silence = ByteBuffer.allocateDirect(AudioPolicy.BLOCK_BYTES)
                .order(ByteOrder.nativeOrder())
            try {
                if (created != null) {
                    // The bridge starts before the companion has attached, so
                    // priming waits for the first blocks instead of starting an
                    // empty track: a track that plays silence it never received
                    // is an underrun waiting to be logged.
                    val primed = prime(created, bytes, meta, direct)
                    if (primed == 0) {
                        error = "no audio blocks within ${PRIME_TIMEOUT_MS}ms"
                        Log.w(TAG, "priming produced no blocks")
                    }
                    // Prime alone leaves the track one block deep, because the
                    // consumer can only ever write what the producer just made.
                    // Padding to the target level is what gives the track slack
                    // to survive a stall instead of underrunning on the first one.
                    val padded = topUp(created, silence)
                    Log.i(
                        TAG,
                        "primed $primed blocks + ${padded / AudioPolicy.BLOCK_BYTES} silence," +
                            " level ${AudioPolicy.levelMs(levelBytes(created))}ms" +
                            " of $trackBytes bytes",
                    )
                    created.play()
                }
                while (running) {
                    var count = session.popAudio(bytes, meta)
                    if (count <= 0) {
                        emptyPolls += 1
                        // A level below the target means the track is running on
                        // fumes (start-up, or recovery after a stall): refill it
                        // with silence rather than let it underrun.
                        if (created != null && topUp(created, silence) > 0L) {
                            continue
                        }
                        Thread.sleep(2)
                        continue
                    }
                    if (!acceptFormat(meta)) {
                        // Writing samples the track cannot interpret would be
                        // noise, so the bridge stops instead of guessing.
                        running = false
                        break
                    }
                    // Catch up after a stall by skipping to fresh audio instead
                    // of playing the backlog: a deep queue turns into latency
                    // the user hears as sound that trails the picture.
                    var skipped = 0
                    while (AudioPolicy.isStale(blockAgeMs(meta)) && skipped < MAX_STALE_SKIPS) {
                        val next = session.popAudio(bytes, meta)
                        if (next <= 0) break
                        count = next
                        skipped += 1
                    }
                    if (skipped > 0) {
                        staleSkipped += skipped
                        if (System.currentTimeMillis() - lastStaleLogAt > 5_000) {
                            lastStaleLogAt = System.currentTimeMillis()
                            Log.i(TAG, "skipped $skipped stale blocks, age now ${blockAgeMs(meta)}ms")
                        }
                    }
                    audioAgeMs = blockAgeMs(meta)
                    if (created != null) {
                        writtenBytes += writeBlock(created, bytes, count, direct)
                        blocksPlayed += 1
                        levelMs = AudioPolicy.levelMs(levelBytes(created))
                        val reported = created.underrunCount
                        if (reported > underruns) {
                            Log.w(TAG, "underrun: $reported (age ${audioAgeMs}ms, played $blocksPlayed)")
                        }
                        underruns = reported
                    }
                }
            } catch (throwable: Throwable) {
                error = throwable.message ?: throwable.javaClass.simpleName
                Log.w(TAG, "audio playback stopped", throwable)
            } finally {
                running = false
            }
        }, "road-audio").apply {
            start()
        }
        return created != null
    }

    private fun prime(
        audio: AudioTrack,
        bytes: ByteArray,
        meta: LongArray,
        direct: ByteBuffer,
    ): Int {
        var written = 0
        val deadline = System.nanoTime() + PRIME_TIMEOUT_MS * 1_000_000
        while (written < AudioPolicy.PRIME_BLOCKS && System.nanoTime() < deadline) {
            val count = session.popAudio(bytes, meta)
            if (count <= 0) {
                Thread.sleep(2)
                continue
            }
            if (!acceptFormat(meta)) return written
            audioAgeMs = blockAgeMs(meta)
            writtenBytes += writeBlock(audio, bytes, count, direct)
            written += 1
        }
        return written
    }

    private fun blockAgeMs(meta: LongArray): Long =
        ((System.nanoTime() - meta[3]) / 1_000_000).coerceAtLeast(0)

    private fun writeBlock(
        audio: AudioTrack,
        bytes: ByteArray,
        count: Int,
        direct: ByteBuffer,
    ): Int {
        direct.clear()
        direct.put(bytes, 0, count)
        direct.flip()
        return audio.write(direct, count, AudioTrack.WRITE_BLOCKING).coerceAtLeast(0)
    }

    /** Bytes written to the track minus bytes it has played. */
    private fun levelBytes(audio: AudioTrack): Long =
        (writtenBytes - audio.playbackHeadPosition.toLong() * AudioPolicy.BYTES_PER_FRAME)
            .coerceAtLeast(0)

    /**
     * Writes silence until the track holds [AudioPolicy.TARGET_LEVEL_BYTES].
     * Returns the bytes added, so the caller can tell a refill from an idle poll.
     */
    private fun topUp(audio: AudioTrack, silence: ByteBuffer): Long {
        var added = 0L
        while (levelBytes(audio) < AudioPolicy.TARGET_LEVEL_BYTES) {
            silence.clear()
            val written = audio.write(silence, AudioPolicy.BLOCK_BYTES, AudioTrack.WRITE_BLOCKING)
            if (written <= 0) break
            writtenBytes += written
            added += written
        }
        levelMs = AudioPolicy.levelMs(levelBytes(audio))
        return added
    }

    /**
     * Accepts the block only if the track can interpret it. The companion is
     * fixed at 48 kHz stereo today, so a mismatch means one side changed
     * without the other, and playing it would be noise.
     */
    private fun acceptFormat(meta: LongArray): Boolean {
        val channels = meta[1]
        val rate = meta[2]
        format = AudioPolicy.formatName(rate, channels)
        if (rate == SAMPLE_RATE.toLong() && channels == CHANNELS.toLong()) return true
        error = "companion sends $format, track expects 48k/${CHANNELS}ch"
        Log.w(TAG, error!!)
        return false
    }

    private fun createTrack(): AudioTrack? {
        val minimum = AudioTrack.getMinBufferSize(
            SAMPLE_RATE,
            AudioFormat.CHANNEL_OUT_STEREO,
            AudioFormat.ENCODING_PCM_FLOAT,
        )
        if (minimum <= 0) {
            error = "device has no float PCM output"
            return null
        }
        val bufferBytes = AudioPolicy.bufferBytes(minimum)
        trackBytes = bufferBytes
        Log.i(TAG, "track buffer $bufferBytes bytes (device minimum $minimum)")
        return try {
            AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_GAME)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build(),
                )
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                        .setSampleRate(SAMPLE_RATE)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                        .build(),
                )
                .setBufferSizeInBytes(bufferBytes)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .build()
        } catch (throwable: Throwable) {
            error = throwable.message ?: throwable.javaClass.simpleName
            null
        }
    }

    /** The line the developer overlay shows for this bridge. */
    fun health(): String = AudioPolicy.health(
        underruns,
        emptyPolls,
        staleSkipped,
        blocksPlayed,
        audioAgeMs,
        levelMs,
        format,
    )

    fun stop() {
        running = false
        thread?.join(500)
        thread = null
        track?.let { audio ->
            runCatching {
                audio.pause()
                audio.flush()
                audio.release()
            }
        }
        track = null
    }

    companion object {
        private const val TAG = "RoadAudio"
        private const val SAMPLE_RATE = 48_000
        private const val CHANNELS = 2
        private const val PRIME_TIMEOUT_MS = 3_000L
        /** Most blocks one catch-up pass may drop before it plays what it has. */
        private const val MAX_STALE_SKIPS = 8
    }
}
