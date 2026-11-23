package dev.roadloader.core

import java.io.ByteArrayOutputStream
import java.io.File
import java.util.zip.ZipFile

/**
 * Streams resource-pack style payloads to the companion over the command ring.
 * The real game path writes into a mapped asset file; the demo measures the
 * same ring so throughput and drop behaviour are observable.
 */
object RoadAssets {
    data class Result(
        val bytes: Long,
        val chunks: Int,
        val accepted: Int,
        val millis: Long,
    ) {
        fun megabytesPerSecond(): Double =
            if (millis <= 0) 0.0 else (bytes / 1024.0 / 1024.0) / (millis / 1000.0)

        fun describe(): String =
            "streamed ${bytes / 1024} KiB in $millis ms (${"%.1f".format(megabytesPerSecond())} MiB/s), " +
                "$accepted/$chunks chunks accepted"
    }

    private const val CHUNK = 232

    fun preload(session: RoadSession, payload: ByteArray): Result {
        val started = System.nanoTime()
        var offset = 0
        var chunks = 0
        var accepted = 0
        while (offset < payload.size) {
            val size = minOf(CHUNK, payload.size - offset)
            val chunk = payload.copyOfRange(offset, offset + size)
            if (session.pushCommand(RoadConstants.CMD_ASSET_CHUNK, offset.toLong(), chunk)) {
                accepted += 1
            }
            chunks += 1
            offset += size
            if (chunks % 32 == 0) {
                Thread.sleep(1)
            }
        }
        val millis = (System.nanoTime() - started) / 1_000_000
        return Result(payload.size.toLong(), chunks, accepted, millis)
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
