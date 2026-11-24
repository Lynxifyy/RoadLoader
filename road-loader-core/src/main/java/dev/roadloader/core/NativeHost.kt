package dev.roadloader.core

/** JNI surface of the host (64-bit UI) side. */
object NativeHost {
    init {
        NativeLibrary.ensureLoaded()
    }

    external fun coreVersion(): Int
    external fun coreStatus(): String
    external fun abiInfo(): String

    external fun createSession(width: Int, height: Int): Long
    external fun sessionFd(handle: Long): Int
    external fun duplicateFd(handle: Long): Int
    external fun regionSize(handle: Long): Long
    external fun frameBuffer(handle: Long, slot: Int): java.nio.ByteBuffer?
    external fun pollFrame(handle: Long, out: LongArray): Int
    external fun releaseFrame(handle: Long, slot: Int)

    external fun pushInput(
        handle: Long,
        kind: Int,
        pointerId: Int,
        action: Int,
        x: Float,
        y: Float,
        pressure: Float,
        size: Float,
        timestampNs: Long,
        button: Int,
        keyCode: Int,
    ): Int

    external fun pushCommand(handle: Long, kind: Int, value: Long): Int
    external fun pushCommandPayload(handle: Long, kind: Int, value: Long, payload: ByteArray?): Int
    external fun pollEvent(handle: Long): String?
    /**
     * Copies the next audio block into [out] and, when [meta] has room, fills it
     * with `[frames, channels, sampleRate, timestampNs]`.
     */
    external fun popAudio(handle: Long, out: ByteArray, meta: LongArray): Int
    external fun stats(handle: Long, out: LongArray): Int
    external fun selfTest(handle: Long): Int
    external fun destroySession(handle: Long)

    external fun setHeapLimit(bytes: Long)
    external fun allocStats(out: LongArray): Int
    external fun probeElf(path: String): String
    external fun scanElf(path: String, pattern: String): Long
    external fun transpileShader(source: String, stage: Int): String?
    external fun crc32(data: ByteArray): Int
    external fun setVolume(handle: Long, volume: Float): Int
    external fun requestSelfTest(handle: Long): Int
}

/** JNI surface of the companion (32-bit engine) side. */
object NativeCompanion {
    init {
        NativeLibrary.ensureLoaded()
    }

    external fun attach(fd: Int, size: Long, flags: Int, heapLimit: Long): Int
    external fun stop(): Int
    external fun isRunning(): Int
    external fun selfTest(): Int
    external fun abiInfo(): String
}
