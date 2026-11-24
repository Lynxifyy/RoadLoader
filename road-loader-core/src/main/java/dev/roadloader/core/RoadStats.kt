package dev.roadloader.core

import dev.roadloader.core.RoadConstants.STATS_AUDIO_CONSUMED
import dev.roadloader.core.RoadConstants.STATS_AUDIO_DROPPED
import dev.roadloader.core.RoadConstants.STATS_ASSET_BYTES
import dev.roadloader.core.RoadConstants.STATS_ASSET_CRC
import dev.roadloader.core.RoadConstants.STATS_ASSET_STATUS
import dev.roadloader.core.RoadConstants.STATS_AUDIO_PUSHED
import dev.roadloader.core.RoadConstants.STATS_CRASH_ADDR
import dev.roadloader.core.RoadConstants.STATS_CRASH_SIGNAL
import dev.roadloader.core.RoadConstants.STATS_DROPPED
import dev.roadloader.core.RoadConstants.STATS_FLAGS
import dev.roadloader.core.RoadConstants.STATS_FRAME_COUNT
import dev.roadloader.core.RoadConstants.STATS_HEAP_LIMIT
import dev.roadloader.core.RoadConstants.STATS_HEAP_PEAK
import dev.roadloader.core.RoadConstants.STATS_HEAP_USED
import dev.roadloader.core.RoadConstants.STATS_HEARTBEAT_AGE_MS
import dev.roadloader.core.RoadConstants.STATS_HEARTBEAT_SEQ
import dev.roadloader.core.RoadConstants.STATS_INPUT_CONSUMED
import dev.roadloader.core.RoadConstants.STATS_INPUT_DROPPED
import dev.roadloader.core.RoadConstants.STATS_INPUT_PUSHED
import dev.roadloader.core.RoadConstants.STATS_LAST_INPUT_KIND
import dev.roadloader.core.RoadConstants.STATS_LAST_INPUT_SEQ
import dev.roadloader.core.RoadConstants.STATS_PID
import dev.roadloader.core.RoadConstants.STATS_PRESENTED
import dev.roadloader.core.RoadConstants.STATS_RENDER_AVG_US
import dev.roadloader.core.RoadConstants.STATS_RENDER_MAX_US
import dev.roadloader.core.RoadConstants.STATS_SELF_TEST
import dev.roadloader.core.RoadConstants.STATS_STATUS

/** Snapshot of the counters published by the companion. */
class RoadStats(private val values: LongArray) {
    val status: Int get() = values[STATS_STATUS].toInt()
    val flags: Int get() = values[STATS_FLAGS].toInt()
    val pid: Int get() = values[STATS_PID].toInt()
    val heartbeatSeq: Long get() = values[STATS_HEARTBEAT_SEQ]
    val heartbeatAgeMs: Long get() = values[STATS_HEARTBEAT_AGE_MS]
    val frameCount: Long get() = values[STATS_FRAME_COUNT]
    val presented: Long get() = values[STATS_PRESENTED]
    val droppedFrames: Long get() = values[STATS_DROPPED]
    val inputPushed: Long get() = values[STATS_INPUT_PUSHED]
    val inputConsumed: Long get() = values[STATS_INPUT_CONSUMED]
    val inputDropped: Long get() = values[STATS_INPUT_DROPPED]
    val heapUsed: Long get() = values[STATS_HEAP_USED]
    val heapPeak: Long get() = values[STATS_HEAP_PEAK]
    val heapLimit: Long get() = values[STATS_HEAP_LIMIT]
    val renderAvgUs: Long get() = values[STATS_RENDER_AVG_US]
    val renderMaxUs: Long get() = values[STATS_RENDER_MAX_US]
    val selfTest: Long get() = values[STATS_SELF_TEST]
    val crashSignal: Long get() = values[STATS_CRASH_SIGNAL]
    val crashAddress: Long get() = values[STATS_CRASH_ADDR]
    val lastInputSeq: Long get() = values[STATS_LAST_INPUT_SEQ]
    val lastInputKind: Long get() = values[STATS_LAST_INPUT_KIND]
    val audioPushed: Long get() = values[STATS_AUDIO_PUSHED]
    val audioConsumed: Long get() = values[STATS_AUDIO_CONSUMED]
    val audioDropped: Long get() = values[STATS_AUDIO_DROPPED]
    val assetBytes: Long get() = values[STATS_ASSET_BYTES]
    val assetCrc: Long get() = values[STATS_ASSET_CRC]
    val assetStatus: Int get() = values[STATS_ASSET_STATUS].toInt()

    val hasShaders: Boolean get() = flags and RoadConstants.FLAG_SHADERS != 0
    val isSafeMode: Boolean get() = flags and RoadConstants.FLAG_SAFE_MODE != 0

    /** True while the companion is running and its heartbeat is fresh. */
    fun isAlive(): Boolean =
        status == RoadConstants.STATUS_RUNNING && heartbeatAgeMs < HEARTBEAT_TIMEOUT_MS

    fun heapUsedMb(): Long = heapUsed / (1024 * 1024)

    fun heapLimitMb(): Long = heapLimit / (1024 * 1024)

    fun describe(): String {
        val heap = "${heapUsedMb()}/${heapLimitMb()} MB"
        val heartbeat = if (heartbeatAgeMs == Long.MAX_VALUE) "never" else "${heartbeatAgeMs}ms"
        return "pid ${pid} | ${statusName(status)} | heap $heap | hb $heartbeat\n" +
            "frames $frameCount produced / $presented presented / $droppedFrames dropped\n" +
            "in ${inputConsumed}/${inputPushed} | audio ${audioConsumed}/${audioPushed}" +
            " | render ${renderAvgUs}us avg / ${renderMaxUs}us max | self-test 0x${selfTest.toString(16)}" +
            "\nassets ${assetBytes / 1024} KiB ${assetStatusName(assetStatus)}"
    }

    companion object {
        const val HEARTBEAT_TIMEOUT_MS = 2000L

        fun assetStatusName(status: Int): String = when (status) {
            RoadConstants.ASSET_STATUS_NONE -> "none"
            RoadConstants.ASSET_STATUS_PENDING -> "streaming"
            RoadConstants.ASSET_STATUS_OK -> "ok"
            RoadConstants.ASSET_STATUS_EMPTY -> "empty chunk"
            RoadConstants.ASSET_STATUS_RANGE -> "chunk out of range"
            RoadConstants.ASSET_STATUS_CHECKSUM -> "chunk checksum"
            RoadConstants.ASSET_STATUS_TOTAL -> "inconsistent total"
            RoadConstants.ASSET_STATUS_DUPLICATE -> "duplicate chunk"
            RoadConstants.ASSET_STATUS_TOO_LARGE -> "too large"
            RoadConstants.ASSET_STATUS_CRC_MISMATCH -> "crc mismatch"
            RoadConstants.ASSET_STATUS_LENGTH_MISMATCH -> "length mismatch"
            RoadConstants.ASSET_STATUS_NO_BEGIN -> "no begin"
            RoadConstants.ASSET_STATUS_INCOMPLETE -> "incomplete"
            RoadConstants.ASSET_STATUS_BAD_TOTAL -> "bad chunk count"
            else -> "status $status"
        }

        fun statusName(status: Int): String = when (status) {
            RoadConstants.STATUS_EMPTY -> "empty"
            RoadConstants.STATUS_INITIALIZING -> "initializing"
            RoadConstants.STATUS_RUNNING -> "running"
            RoadConstants.STATUS_STOPPED -> "stopped"
            RoadConstants.STATUS_CRASHED -> "crashed"
            RoadConstants.STATUS_SAFE_MODE -> "safe-mode"
            else -> "unknown($status)"
        }
    }
}
