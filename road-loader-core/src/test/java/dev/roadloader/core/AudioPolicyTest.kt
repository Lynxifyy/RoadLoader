package dev.roadloader.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioPolicyTest {
    @Test
    fun bufferAlwaysHoldsWholeBlocks() {
        // A device that reports a tiny minimum still gets a six-block buffer.
        assertEquals(6 * AudioPolicy.BLOCK_BYTES, AudioPolicy.bufferBytes(1_000))
        // A generous minimum is honoured, then rounded up to a block boundary.
        assertEquals(40_960, AudioPolicy.bufferBytes(20_000))
        // Rounding is up, never down: 5,000 * 2 = 10,000 is below the floor.
        assertEquals(6 * AudioPolicy.BLOCK_BYTES, AudioPolicy.bufferBytes(5_000))
    }

    @Test
    fun bufferIsAMultipleOfTheBlock() {
        for (minimum in intArrayOf(0, 512, 3_840, 9_600, 44_100)) {
            val bytes = AudioPolicy.bufferBytes(minimum)
            assertEquals(0, bytes % AudioPolicy.BLOCK_BYTES)
            assertTrue(bytes >= 6 * AudioPolicy.BLOCK_BYTES)
        }
    }

    @Test
    fun bufferNeverGoesNegative() {
        assertEquals(6 * AudioPolicy.BLOCK_BYTES, AudioPolicy.bufferBytes(-1))
    }

    @Test
    fun volumeStepsWrapAround() {
        assertEquals(0.5f, AudioPolicy.nextVolume(1.0f), 0.0f)
        assertEquals(0.0f, AudioPolicy.nextVolume(0.5f), 0.0f)
        assertEquals(1.0f, AudioPolicy.nextVolume(0.0f), 0.0f)
        // An unknown value falls back to full volume: never silently muted.
        assertEquals(1.0f, AudioPolicy.nextVolume(0.3f), 0.0f)
    }

    @Test
    fun formatNameDescribesTheStream() {
        assertEquals("48k/2ch", AudioPolicy.formatName(48_000, 2))
        assertEquals("44k/1ch", AudioPolicy.formatName(44_100, 1))
        assertEquals("no blocks yet", AudioPolicy.formatName(0, 0))
    }

    @Test
    fun healthReportsTheNumbersThatMatter() {
        val line = AudioPolicy.health(0, 0, 0, 412, 18, 53, "48k/2ch")
        assertTrue(line.contains("underruns 0"))
        assertTrue(line.contains("played 412"))
        assertTrue(line.contains("age 18ms"))
        assertTrue(line.contains("48k/2ch"))
    }

    @Test
    fun healthSaysWhenTheAgeIsUnknown() {
        val line = AudioPolicy.health(3, 7, 2, 0, -1, 0, "no blocks yet")
        assertTrue(line.contains("underruns 3"))
        assertTrue(line.contains("age unknown"))
    }

    @Test
    fun healthShowsTheBufferLevel() {
        assertTrue(AudioPolicy.health(0, 0, 0, 1, 5, 53, "48k/2ch").contains("buf 53ms"))
    }

    @Test
    fun levelConvertsBytesToMilliseconds() {
        // 48 kHz stereo f32 is 384 bytes per millisecond.
        assertEquals(0L, AudioPolicy.levelMs(0))
        assertEquals(10L, AudioPolicy.levelMs(3_840))
        assertEquals(53L, AudioPolicy.levelMs(5L * AudioPolicy.BLOCK_BYTES))
        // The target must fit inside the smallest buffer the policy allows.
        assertTrue(
            AudioPolicy.TARGET_LEVEL_BYTES <=
                AudioPolicy.bufferBytes(0).toLong(),
        )
    }

    @Test
    fun stalenessIsBoundedByTheAgeCap() {
        assertFalse(AudioPolicy.isStale(AudioPolicy.MAX_AGE_MS))
        assertTrue(AudioPolicy.isStale(AudioPolicy.MAX_AGE_MS + 1))
        assertFalse(AudioPolicy.isStale(0))
    }

    @Test
    fun aCleanRunNeedsNoWarning() {
        assertFalse(AudioPolicy.needsWarning(0))
        assertTrue(AudioPolicy.needsWarning(1))
    }
}
