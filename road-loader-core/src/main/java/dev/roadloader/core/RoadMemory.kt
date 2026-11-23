package dev.roadloader.core

/** Memory accounting and the low-memory policy (Phase 15). */
object RoadMemory {
    data class Usage(val live: Long, val peak: Long, val limit: Long, val failures: Long)

    /** Above this the UI starts shedding chunks instead of risking an OOM. */
    const val WARNING_BYTES = 1_200L * 1024 * 1024

    /** Hard ceiling for the 32-bit heap. */
    const val DEFAULT_LIMIT_BYTES = 1_536L * 1024 * 1024

    fun usage(): Usage {
        val values = LongArray(4)
        if (NativeHost.allocStats(values) != 0) {
            return Usage(0, 0, DEFAULT_LIMIT_BYTES, 0)
        }
        return Usage(values[0], values[1], values[2], values[3])
    }

    fun setLimit(bytes: Long) {
        NativeHost.setHeapLimit(if (bytes <= 0) DEFAULT_LIMIT_BYTES else bytes)
    }

    /**
     * Applies the low-memory policy and returns a user-facing message when it
     * had to act, or null when the heap is healthy.
     */
    fun applyPressurePolicy(session: RoadSession): String? {
        val stats = session.readStats()
        if (stats.heapUsed < WARNING_BYTES) return null
        session.setRenderDistance(4)
        return "Low memory (${stats.heapUsedMb()} MB): render distance reduced to 4 chunks"
    }

    fun mb(bytes: Long): String = "${bytes / (1024 * 1024)} MB"
}
