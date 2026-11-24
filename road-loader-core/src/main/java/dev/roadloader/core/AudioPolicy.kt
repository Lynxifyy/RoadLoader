package dev.roadloader.core

import kotlin.math.abs

/**
 * Decisions the audio consumer makes, kept free of Android types.
 *
 * The bridge itself needs an `AudioTrack`; the policy that keeps that track fed
 * does not, so it can be unit tested on the host.
 */
object AudioPolicy {
    /** Bytes in one companion audio block (`AUDIO_BLOCK_SIZE` in the Rust layout). */
    const val BLOCK_BYTES = 4096

    /** Blocks written before `play()` so the track starts with a buffer. */
    const val PRIME_BLOCKS = 3

    /**
     * Blocks the track buffer must be able to hold, whatever the device reports.
     *
     * Six, so the five-block target level always fits with a block of headroom.
     */
    const val MIN_BUFFER_BLOCKS = 6

    /** Volume steps the UI cycles through. */
    val VOLUME_STEPS = floatArrayOf(1.0f, 0.5f, 0.0f)

    /**
     * Track buffer size in bytes.
     *
     * `AudioTrack.getMinBufferSize` is a floor, not a target: on several devices
     * it is smaller than one block, and a track that cannot hold a whole block
     * underruns on the first slow poll. Rounding up to whole blocks also stops a
     * block from straddling the buffer wrap.
     */
    fun bufferBytes(deviceMinimum: Int, blockBytes: Int = BLOCK_BYTES): Int {
        require(blockBytes > 0) { "block size must be positive" }
        val floor = blockBytes.toLong() * MIN_BUFFER_BLOCKS
        val wanted = maxOf(deviceMinimum.toLong() * 2, floor)
        val rounded = ((wanted + blockBytes - 1) / blockBytes) * blockBytes
        return rounded.coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
    }

    /** Next volume step after [current], wrapping back to full volume. */
    fun nextVolume(current: Float): Float {
        val index = VOLUME_STEPS.indexOfFirst { abs(it - current) < 0.01f }
        return VOLUME_STEPS[(if (index < 0) 0 else index + 1) % VOLUME_STEPS.size]
    }

    /** The format sentence the overlay shows, e.g. `48k/2ch`. */
    fun formatName(sampleRate: Long, channels: Long): String {
        if (sampleRate <= 0 || channels <= 0) return "no blocks yet"
        return "${sampleRate / 1000}k/${channels}ch"
    }

    /**
     * One-line health summary.
     *
     * `underruns` is the device's own count, so it is the number that says
     * whether playback actually broke; `empty`, `stale` and `age` describe the
     * bridge that feeds it.
     */
    fun health(
        underruns: Int,
        emptyPolls: Long,
        stale: Long,
        played: Long,
        ageMs: Long,
        levelMs: Long,
        format: String,
    ): String {
        val age = if (ageMs < 0) "unknown" else "${ageMs}ms"
        return "underruns $underruns | empty $emptyPolls | stale $stale | played $played" +
            " | age $age | buf ${levelMs}ms | $format"
    }

    /** Bytes one stereo f32 frame occupies: the unit `AudioTrack` counts in. */
    const val BYTES_PER_FRAME = 8L

    /**
     * Bytes of audio the track must hold before the consumer stops topping it up.
     *
     * Five blocks is 53 ms: enough slack that a scheduler hiccup on a loaded
     * device does not empty the track, small enough that the delay stays in the
     * range game audio already lives in. The track buffer is at least six
     * blocks (`MIN_BUFFER_BLOCKS`), and eight on the test device, so the target
     * always fits with headroom.
     */
    const val TARGET_LEVEL_BYTES = 5L * BLOCK_BYTES

    /** Milliseconds of audio buffered for [levelBytes]. */
    fun levelMs(levelBytes: Long, sampleRate: Int = 48_000, channels: Int = 2): Long {
        val bytesPerMs = sampleRate.toLong() * channels * 4 / 1000
        if (bytesPerMs <= 0) return 0
        return levelBytes / bytesPerMs
    }

    /**
     * True when a block is too old to play.
     *
     * A backlog deeper than [MAX_AGE_MS] means the consumer fell behind and then
     * caught up by playing stale audio: the user hears the sound that belongs to
     * the frame they already saw. Skipping to a fresh block bounds that latency.
     */
    fun isStale(ageMs: Long): Boolean = ageMs > MAX_AGE_MS

    /** Deepest backlog the consumer accepts before skipping ahead. */
    const val MAX_AGE_MS = 60L

    /** True when the device reported an underrun: the only proof playback broke. */
    fun needsWarning(underruns: Int): Boolean = underruns > 0
}
