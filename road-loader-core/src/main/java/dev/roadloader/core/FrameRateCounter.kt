package dev.roadloader.core

/**
 * Frames per second from a monotonically increasing frame counter.
 *
 * The counter is read every poll; the rate is the number of frames produced
 * between two samples divided by the time between them. The first sample has no
 * rate yet, and a sample taken too soon after the previous one is ignored so a
 * burst of polls cannot produce a wild number.
 */
class FrameRateCounter(private val minimumWindowNs: Long = 100_000_000L) {
    private var lastFrames = -1L
    private var lastSampleNs = 0L
    private var lastRate = 0.0

    /** Current rate, 0.0 until a second sample has been taken. */
    val rate: Double get() = lastRate

    /** Feeds a counter value. Returns the new rate, or null when not updated. */
    fun sample(frames: Long, nowNs: Long): Double? {
        if (frames < 0) {
            reset()
            return null
        }
        if (lastFrames < 0) {
            lastFrames = frames
            lastSampleNs = nowNs
            return null
        }
        val elapsed = nowNs - lastSampleNs
        if (elapsed < minimumWindowNs) return null
        if (elapsed <= 0 || frames < lastFrames) {
            reset()
            lastFrames = frames
            lastSampleNs = nowNs
            return null
        }
        lastRate = (frames - lastFrames) * 1_000_000_000.0 / elapsed.toDouble()
        lastFrames = frames
        lastSampleNs = nowNs
        return lastRate
    }

    fun reset() {
        lastFrames = -1L
        lastSampleNs = 0L
        lastRate = 0.0
    }
}
