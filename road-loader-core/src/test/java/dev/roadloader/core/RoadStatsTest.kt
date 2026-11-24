package dev.roadloader.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RoadStatsTest {
    private fun stats(
        status: Int = RoadConstants.STATUS_RUNNING,
        flags: Int = 0,
        heartbeatAgeMs: Long = 12,
        heapUsed: Long = 64L * 1024 * 1024,
        heapLimit: Long = 1536L * 1024 * 1024,
    ): RoadStats {
        val values = LongArray(RoadConstants.STATS_LENGTH)
        values[RoadConstants.STATS_STATUS] = status.toLong()
        values[RoadConstants.STATS_FLAGS] = flags.toLong()
        values[RoadConstants.STATS_PID] = 4242
        values[RoadConstants.STATS_HEARTBEAT_AGE_MS] = heartbeatAgeMs
        values[RoadConstants.STATS_FRAME_COUNT] = 90
        values[RoadConstants.STATS_INPUT_PUSHED] = 4
        values[RoadConstants.STATS_INPUT_CONSUMED] = 4
        values[RoadConstants.STATS_HEAP_USED] = heapUsed
        values[RoadConstants.STATS_HEAP_LIMIT] = heapLimit
        values[RoadConstants.STATS_RENDER_AVG_US] = 250
        values[RoadConstants.STATS_RENDER_MAX_US] = 900
        values[RoadConstants.STATS_SELF_TEST] = 0x8000001fL
        return RoadStats(values)
    }

    @Test
    fun statusNamesCoverEveryLifecycleState() {
        assertEquals("empty", RoadStats.statusName(RoadConstants.STATUS_EMPTY))
        assertEquals("initializing", RoadStats.statusName(RoadConstants.STATUS_INITIALIZING))
        assertEquals("running", RoadStats.statusName(RoadConstants.STATUS_RUNNING))
        assertEquals("stopped", RoadStats.statusName(RoadConstants.STATUS_STOPPED))
        assertEquals("crashed", RoadStats.statusName(RoadConstants.STATUS_CRASHED))
        assertEquals("safe-mode", RoadStats.statusName(RoadConstants.STATUS_SAFE_MODE))
        assertEquals("unknown(99)", RoadStats.statusName(99))
    }

    @Test
    fun aliveRequiresRunningStatusAndFreshHeartbeat() {
        assertTrue(stats().isAlive())
        assertFalse(stats(status = RoadConstants.STATUS_STOPPED).isAlive())
        assertFalse(stats(heartbeatAgeMs = RoadStats.HEARTBEAT_TIMEOUT_MS).isAlive())
    }

    @Test
    fun flagsAreDecoded() {
        val flags = RoadConstants.FLAG_SHADERS or RoadConstants.FLAG_SAFE_MODE
        val decoded = stats(flags = flags)
        assertTrue(decoded.hasShaders)
        assertTrue(decoded.isSafeMode)
        assertFalse(stats().hasShaders)
    }

    @Test
    fun describeReportsCountersInMegabytes() {
        val text = stats().describe()
        assertTrue(text.contains("pid 4242"))
        assertTrue(text.contains("heap 64/1536 MB"))
        assertTrue(text.contains("hb 12ms"))
        assertTrue(text.contains("in 4/4"))
        assertTrue(text.contains("self-test 0x8000001f"))
    }

    @Test
    fun describeHandlesAMissingHeartbeat() {
        assertTrue(stats(heartbeatAgeMs = Long.MAX_VALUE).describe().contains("hb never"))
    }

    @Test
    fun assetVerdictIsDecoded() {
        val values = LongArray(RoadConstants.STATS_LENGTH)
        values[RoadConstants.STATS_ASSET_BYTES] = 512 * 1024
        values[RoadConstants.STATS_ASSET_CRC] = 0xdeadbeefL
        values[RoadConstants.STATS_ASSET_STATUS] = RoadConstants.ASSET_STATUS_OK.toLong()
        val decoded = RoadStats(values)
        assertEquals(512 * 1024L, decoded.assetBytes)
        assertEquals(0xdeadbeefL, decoded.assetCrc)
        assertEquals(RoadConstants.ASSET_STATUS_OK, decoded.assetStatus)
        assertTrue(decoded.describe().contains("assets 512 KiB ok"))
    }

    @Test
    fun assetStatusNamesCoverEveryCode() {
        val codes = listOf(
            RoadConstants.ASSET_STATUS_NONE,
            RoadConstants.ASSET_STATUS_PENDING,
            RoadConstants.ASSET_STATUS_OK,
            RoadConstants.ASSET_STATUS_EMPTY,
            RoadConstants.ASSET_STATUS_RANGE,
            RoadConstants.ASSET_STATUS_CHECKSUM,
            RoadConstants.ASSET_STATUS_TOTAL,
            RoadConstants.ASSET_STATUS_DUPLICATE,
            RoadConstants.ASSET_STATUS_TOO_LARGE,
            RoadConstants.ASSET_STATUS_CRC_MISMATCH,
            RoadConstants.ASSET_STATUS_LENGTH_MISMATCH,
            RoadConstants.ASSET_STATUS_NO_BEGIN,
            RoadConstants.ASSET_STATUS_INCOMPLETE,
            RoadConstants.ASSET_STATUS_BAD_TOTAL,
        )
        assertEquals(codes.size, codes.toSet().size)
        codes.forEach { code ->
            assertFalse(
                "code $code has no name",
                RoadStats.assetStatusName(code).startsWith("status "),
            )
        }
        assertEquals("status 99", RoadStats.assetStatusName(99))
    }

    @Test
    fun heapHelpersRoundDownToMegabytes() {
        val almostTwoMb = 2L * 1024 * 1024 - 1
        assertEquals(1, stats(heapUsed = almostTwoMb).heapUsedMb())
        assertEquals(1536, stats().heapLimitMb())
    }
}
