package dev.roadloader.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FrameRateCounterTest {
    private val second = 1_000_000_000L

    @Test
    fun theFirstSampleHasNoRate() {
        val counter = FrameRateCounter()
        assertNull(counter.sample(100, 0))
        assertEquals(0.0, counter.rate, 0.0)
    }

    @Test
    fun countsFramesProducedBetweenSamples() {
        val counter = FrameRateCounter()
        counter.sample(0, 0)
        assertEquals(60.0, counter.sample(60, second)!!, 0.001)  // 60 frames in 1 s
        assertEquals(120.0, counter.sample(120, second + second / 2)!!, 0.001)  // 60 in 0.5 s
    }

    @Test
    fun ignoresSamplesTakenTooSoon() {
        val counter = FrameRateCounter(minimumWindowNs = 100_000_000L)
        counter.sample(0, 0)
        assertNull(counter.sample(3, 50_000_000L))
        assertEquals(60.0, counter.sample(6, 100_000_000L)!!, 0.001)
    }

    @Test
    fun reportsZeroWhenTheCounterStops() {
        val counter = FrameRateCounter()
        counter.sample(0, 0)
        counter.sample(60, second)
        assertEquals(0.0, counter.sample(60, second + second / 2)!!, 0.0)
    }

    @Test
    fun aCounterGoingBackwardsResetsInsteadOfReportingNonsense() {
        val counter = FrameRateCounter()
        counter.sample(500, 0)
        assertNull(counter.sample(10, second))
        assertEquals(0.0, counter.rate, 0.0)
        // The reset sample becomes the new baseline.
        assertEquals(5.0, counter.sample(15, second + second)!!, 0.001)
    }

    @Test
    fun aNegativeCounterResets() {
        val counter = FrameRateCounter()
        counter.sample(0, 0)
        counter.sample(60, second)
        assertNull(counter.sample(-1, second + second))
        assertEquals(0.0, counter.rate, 0.0)
    }
}
