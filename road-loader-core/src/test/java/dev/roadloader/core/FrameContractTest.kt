package dev.roadloader.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameContractTest {
    @Test
    fun acceptsATightlyPackedFrame() {
        assertNull(FrameContract.check(width = 64, height = 32, stride = 256, length = 8192, capacity = 8192))
        assertTrue(FrameContract.isTightlyPacked(64, 256))
    }

    @Test
    fun acceptsAPaddedStrideThatFitsTheSlot() {
        assertNull(FrameContract.check(width = 64, height = 32, stride = 512, length = 16384, capacity = 16384))
        assertTrue(!FrameContract.isTightlyPacked(64, 512))
    }

    @Test
    fun rejectsAnEmptyImage() {
        assertEquals("size 0x32", FrameContract.check(0, 32, 256, 8192, 8192))
        assertEquals("size 64x0", FrameContract.check(64, 0, 256, 8192, 8192))
        assertEquals("size -64x32", FrameContract.check(-64, 32, 256, 8192, 8192))
    }

    @Test
    fun rejectsRowsNarrowerThanTheImage() {
        assertEquals(
            "stride 200 < 256",
            FrameContract.check(64, 32, 200, 8192, 8192),
        )
    }

    @Test
    fun rejectsAFrameThatDoesNotFitItsSlot() {
        // 64x32 at stride 512 needs 16384 bytes; the slot only has 8192.
        assertEquals(
            "needs 16384 bytes, slot has 8192",
            FrameContract.check(64, 32, 512, 16384, 8192),
        )
    }

    @Test
    fun rejectsADeclaredLengthShorterThanTheRows() {
        assertEquals("length 4096 < 8192", FrameContract.check(64, 32, 256, 4096, 8192))
    }

    @Test
    fun aZeroLengthIsTreatedAsUnknownNotAsShort() {
        // The native side reports the slot size; zero means "not reported".
        assertNull(FrameContract.check(64, 32, 256, 0, 8192))
    }
}
