package dev.roadloader.core

import java.io.Closeable
import java.nio.ByteBuffer

/**
 * A frame published by the companion. The buffer stays valid until released.
 *
 * [stride] is bytes per row and may exceed `width * 4`; [length] is the size the
 * companion reports for the slot, and [publishedNs] is on the companion's
 * monotonic clock, which is the same clock `System.nanoTime` reads.
 */
data class RoadFrame(
    val slot: Int,
    val sequence: Long,
    val width: Int,
    val height: Int,
    val stride: Int,
    val length: Int,
    val publishedNs: Long,
    val buffer: ByteBuffer,
)

/**
 * Host-side handle of one shared region. The companion maps the same region
 * through the duplicated descriptor.
 */
class RoadSession private constructor(
    val handle: Long,
    val width: Int,
    val height: Int,
) : Closeable {
    private val frameBuffers = arrayOfNulls<ByteBuffer>(RoadConstants.FRAME_SLOT_COUNT)
    private val frameInfo = LongArray(RoadConstants.FRAME_INFO_FIELDS)

    @Volatile
    private var contractWarned = false
    private val statsValues = LongArray(RoadConstants.STATS_LENGTH)

    @Volatile
    private var closed = false

    val regionSize: Long = NativeHost.regionSize(handle)

    fun frameBuffer(slot: Int): ByteBuffer? {
        if (slot !in 0 until RoadConstants.FRAME_SLOT_COUNT) return null
        frameBuffers[slot]?.let { return it }
        val buffer = NativeHost.frameBuffer(handle, slot) ?: return null
        frameBuffers[slot] = buffer
        return buffer
    }

    /**
     * Returns the newest published frame, or null when nothing changed.
     *
     * The slot is claimed by the poll, so every path that does not return a
     * frame releases it again; a slot leaked here would stall the pipeline
     * after `FRAME_SLOT_COUNT` bad frames.
     */
    fun pollFrame(): RoadFrame? {
        if (closed) return null
        if (NativeHost.pollFrame(handle, frameInfo) != 1) return null
        val slot = frameInfo[RoadConstants.FRAME_INFO_SLOT].toInt()
        val buffer = frameBuffer(slot)
        if (buffer == null) {
            NativeHost.releaseFrame(handle, slot)
            return null
        }
        val width = frameInfo[RoadConstants.FRAME_INFO_WIDTH].toInt()
        val height = frameInfo[RoadConstants.FRAME_INFO_HEIGHT].toInt()
        val stride = frameInfo[RoadConstants.FRAME_INFO_STRIDE].toInt()
        val length = frameInfo[RoadConstants.FRAME_INFO_LENGTH].toInt()
        val problem = FrameContract.check(width, height, stride, length, buffer.capacity())
        if (problem != null) {
            if (!contractWarned) {
                contractWarned = true
                android.util.Log.w(TAG, "dropping frame slot $slot: $problem")
            }
            NativeHost.releaseFrame(handle, slot)
            return null
        }
        return RoadFrame(
            slot = slot,
            sequence = frameInfo[RoadConstants.FRAME_INFO_SEQ],
            width = width,
            height = height,
            stride = stride,
            length = length,
            publishedNs = frameInfo[RoadConstants.FRAME_INFO_PUBLISHED_NS],
            buffer = buffer,
        )
    }

    fun releaseFrame(slot: Int) {
        if (!closed) NativeHost.releaseFrame(handle, slot)
    }

    fun pushInput(
        kind: Int,
        pointerId: Int,
        action: Int,
        x: Float,
        y: Float,
        pressure: Float = 1.0f,
        size: Float = 0.0f,
        timestampNs: Long = System.nanoTime(),
        button: Int = 0,
        keyCode: Int = 0,
    ): Boolean = !closed && NativeHost.pushInput(
        handle, kind, pointerId, action, x, y, pressure, size, timestampNs, button, keyCode,
    ) == 0

    fun pushCommand(kind: Int, value: Long): Boolean =
        !closed && NativeHost.pushCommand(handle, kind, value) == 0

    fun pushCommand(kind: Int, value: Long, payload: ByteArray?): Boolean =
        !closed && NativeHost.pushCommandPayload(handle, kind, value, payload) == 0

    fun pollEvent(): String? = if (closed) null else NativeHost.pollEvent(handle)

    /** Pops one audio block into [buffer]; returns the byte count. */
    fun popAudio(buffer: ByteArray): Int = if (closed) 0 else NativeHost.popAudio(handle, buffer)

    fun readStats(): RoadStats {
        if (!closed) NativeHost.stats(handle, statsValues)
        return RoadStats(statsValues.copyOf())
    }

    fun requestSelfTest(): Boolean = pushCommand(RoadConstants.CMD_REQUEST_SELF_TEST, 0)

    fun setVolume(volume: Float) {
        if (!closed) NativeHost.setVolume(handle, volume.coerceIn(0f, 1f))
    }

    fun setShaders(enabled: Boolean) {
        pushCommand(RoadConstants.CMD_SET_SHADERS, if (enabled) 1 else 0)
    }

    fun setSafeMode(enabled: Boolean) {
        pushCommand(RoadConstants.CMD_SET_SAFE_MODE, if (enabled) 1 else 0)
    }

    fun setRenderDistance(chunks: Int) {
        pushCommand(RoadConstants.CMD_SET_RENDER_DISTANCE, chunks.toLong())
    }

    fun duplicateFd(): Int = if (closed) -1 else NativeHost.duplicateFd(handle)

    override fun close() {
        if (closed) return
        closed = true
        frameBuffers.fill(null)
        NativeHost.destroySession(handle)
    }

    companion object {
        private const val TAG = "RoadSession"

        fun create(width: Int, height: Int): RoadSession? {
            if (!NativeLibrary.ensureLoaded()) return null
            val handle = NativeHost.createSession(width, height)
            if (handle == 0L) return null
            return RoadSession(handle, width, height)
        }
    }
}
