package dev.roadloader.core

/**
 * Validates a frame descriptor before its buffer is uploaded.
 *
 * The shared slot is a raw RGBA buffer: `stride` bytes per row, `height` rows.
 * A descriptor that does not fit its slot, or whose rows are narrower than the
 * image, would make the texture read past the mapping or shear the picture, so
 * it is dropped instead of drawn.
 */
object FrameContract {
    const val BYTES_PER_PIXEL = 4

    /** Returns null when the descriptor is usable, or the reason it is not. */
    fun check(
        width: Int,
        height: Int,
        stride: Int,
        length: Int,
        capacity: Int,
    ): String? {
        if (width <= 0 || height <= 0) return "size ${width}x$height"
        val minimumStride = width * BYTES_PER_PIXEL
        if (stride < minimumStride) return "stride $stride < $minimumStride"
        val needed = stride.toLong() * height
        if (length in 1 until needed) return "length $length < $needed"
        if (needed > capacity) return "needs $needed bytes, slot has $capacity"
        return null
    }

    /** True when the rows are packed without padding. */
    fun isTightlyPacked(width: Int, stride: Int): Boolean =
        stride == width * BYTES_PER_PIXEL
}
