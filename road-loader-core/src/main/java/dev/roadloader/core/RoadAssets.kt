package dev.roadloader.core

import java.io.ByteArrayOutputStream
import java.io.File
import java.util.zip.ZipFile

/**
 * Streams resource-pack style payloads to the companion over the command ring.
 *
 * The companion stages the chunks, then the commit carries the length and CRC32
 * of the whole payload so the 32-bit side can prove it assembled what was sent.
 * The verdict is published in the header and polled back through [RoadStats].
 */
object RoadAssets {
    /** Default asset id for the demo payload. */
    const val DEMO_ASSET_ID = 1

    data class Result(
        val assetId: Int,
        val bytes: Long,
        val chunks: Int,
        val queued: Int,
        val millis: Long,
    ) {
        fun megabytesPerSecond(): Double =
            if (millis <= 0) 0.0 else (bytes / 1024.0 / 1024.0) / (millis / 1000.0)

        /** True when every command made it into the ring; the verdict comes later. */
        fun allQueued(): Boolean = queued == chunks + 2

        fun describe(): String =
            "sent ${bytes / 1024} KiB in $millis ms (${"%.1f".format(megabytesPerSecond())} MiB/s), " +
                "$queued/${chunks + 2} commands queued"
    }

    /** One command of the stream, as the companion will see it. */
    data class Command(val kind: Int, val value: Long, val payload: ByteArray?)

    /**
     * Builds the full command sequence for [payload]: begin, one chunk per
     * [RoadConstants.ASSET_CHUNK_BYTES], then commit with the whole-asset CRC.
     */
    fun commands(assetId: Int, payload: ByteArray): List<Command> {
        val chunkSize = RoadConstants.ASSET_CHUNK_BYTES
        val chunks = if (payload.isEmpty()) 0 else (payload.size + chunkSize - 1) / chunkSize
        val id = assetId.toLong() shl 32
        val out = ArrayList<Command>(chunks + 2)
        out += Command(RoadConstants.CMD_ASSET_BEGIN, id or chunks.toLong(), null)
        var index = 0
        while (index < chunks) {
            val offset = index * chunkSize
            val end = minOf(offset + chunkSize, payload.size)
            out += Command(
                RoadConstants.CMD_ASSET_CHUNK,
                id or index.toLong(),
                payload.copyOfRange(offset, end),
            )
            index += 1
        }
        out += Command(RoadConstants.CMD_ASSET_COMMIT, id or payload.size.toLong(), crc32(payload))
        return out
    }

    fun preload(session: RoadSession, payload: ByteArray, assetId: Int = DEMO_ASSET_ID): Result {
        val started = System.nanoTime()
        val commands = commands(assetId, payload)
        var queued = 0
        commands.forEachIndexed { index, command ->
            val payload = command.payload
            val pushed = if (payload == null) {
                session.pushCommand(command.kind, command.value)
            } else {
                session.pushCommand(command.kind, command.value, payload)
            }
            if (pushed) {
                queued += 1
            }
            if (index % 32 == 31) {
                Thread.sleep(1)
            }
        }
        val millis = (System.nanoTime() - started) / 1_000_000
        return Result(assetId, payload.size.toLong(), commands.size - 2, queued, millis)
    }

    /**
     * Waits for the companion to finish the stream started by [preload] and
     * returns its verdict. Polling stops as soon as the status leaves
     * "pending", so a healthy stream returns in a few milliseconds.
     */
    fun awaitVerdict(session: RoadSession, timeoutMs: Long = 2000): RoadStats {
        val deadline = System.currentTimeMillis() + timeoutMs
        var stats = session.readStats()
        while (System.currentTimeMillis() < deadline) {
            stats = session.readStats()
            if (stats.assetStatus != RoadConstants.ASSET_STATUS_PENDING &&
                stats.assetStatus != RoadConstants.ASSET_STATUS_NONE
            ) {
                return stats
            }
            Thread.sleep(10)
        }
        return stats
    }

    /** 4-byte little-endian CRC32, the layout `CMD_ASSET_COMMIT` expects. */
    fun crc32(payload: ByteArray): ByteArray {
        val crc = java.util.zip.CRC32()
        crc.update(payload)
        val value = crc.value
        return byteArrayOf(
            (value and 0xFF).toByte(),
            ((value ushr 8) and 0xFF).toByte(),
            ((value ushr 16) and 0xFF).toByte(),
            ((value ushr 24) and 0xFF).toByte(),
        )
    }

    /** Builds a deterministic payload so the throughput test is repeatable. */
    fun samplePayload(bytes: Int): ByteArray {
        val output = ByteArrayOutputStream(bytes)
        var value = 0
        while (output.size() < bytes) {
            output.write(value)
            value = (value + 7) and 0xFF
        }
        return output.toByteArray()
    }

    /** Lists the asset entries of an APK; used by the import screen. */
    fun listAssets(apk: File, limit: Int = 12): List<String> =
        runCatching {
            ZipFile(apk).use { zip ->
                zip.entries().asSequence()
                    .filter { it.name.startsWith("assets/") && !it.isDirectory }
                    .map { it.name }
                    .take(limit)
                    .toList()
            }
        }.getOrDefault(emptyList())
}
