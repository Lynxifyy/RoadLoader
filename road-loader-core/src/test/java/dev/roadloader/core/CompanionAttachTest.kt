package dev.roadloader.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CompanionAttachTest {
    @Test
    fun acceptsSizesInsideTheCap() {
        assertTrue(CompanionAttach.sizeAllowed(1))
        assertTrue(CompanionAttach.sizeAllowed(8L * 1024 * 1024))
        assertTrue(CompanionAttach.sizeAllowed(RoadConstants.MAX_REGION_BYTES))
    }

    @Test
    fun refusesZeroNegativeAndOversizeRegions() {
        assertFalse(CompanionAttach.sizeAllowed(0))
        assertFalse(CompanionAttach.sizeAllowed(-1))
        assertFalse(CompanionAttach.sizeAllowed(RoadConstants.MAX_REGION_BYTES + 1))
    }

    @Test
    fun describesEveryCode() {
        assertEquals(
            "Companion attached to the shared region.",
            CompanionAttach.describe(CompanionAttach.OK),
        )
        assertEquals(
            "Companion refused the region (ABI mismatch).",
            CompanionAttach.describe(CompanionAttach.BAD_ABI),
        )
        assertEquals(
            "Companion already has a client attached.",
            CompanionAttach.describe(CompanionAttach.BUSY),
        )
        assertEquals(
            "Companion refused the region (larger than 128 MB).",
            CompanionAttach.describe(CompanionAttach.TOO_LARGE),
        )
        assertEquals(
            "Companion refused the region (code -9).",
            CompanionAttach.describe(-9),
        )
    }

    @Test
    fun theServiceAliasesMatchTheSharedCodes() {
        assertEquals(CompanionAttach.OK, CompanionServiceBase.ATTACH_OK)
        assertEquals(CompanionAttach.BAD_ABI, CompanionServiceBase.ATTACH_BAD_ABI)
        assertEquals(CompanionAttach.BUSY, CompanionServiceBase.ATTACH_BUSY)
        assertEquals(CompanionAttach.TOO_LARGE, CompanionServiceBase.ATTACH_TOO_LARGE)
    }
}
