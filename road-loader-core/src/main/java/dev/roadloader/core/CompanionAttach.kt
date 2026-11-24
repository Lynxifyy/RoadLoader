package dev.roadloader.core

/**
 * Attach result codes shared by the companion service and the client.
 *
 * The negative values are the return codes of `road_companion_attach`; the
 * client-side ones are produced before the core is called.
 */
object CompanionAttach {
    const val OK = 0
    const val BAD_ARGUMENT = -1
    const val BAD_ABI = -2
    const val BUSY = -3
    const val TOO_LARGE = -4

    /** True when a size is acceptable for an attach request. */
    fun sizeAllowed(size: Long): Boolean = size in 1..RoadConstants.MAX_REGION_BYTES

    /** A short sentence for the status line. */
    fun describe(code: Int): String = when (code) {
        OK -> "Companion attached to the shared region."
        BAD_ARGUMENT -> "Companion refused the region (bad argument)."
        BAD_ABI -> "Companion refused the region (ABI mismatch)."
        BUSY -> "Companion already has a client attached."
        TOO_LARGE -> "Companion refused the region (larger than 128 MB)."
        else -> "Companion refused the region (code $code)."
    }
}
