package dev.roadloader.core

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RoadAssetsTest {
    private fun payload(size: Int): ByteArray =
        ByteArray(size) { index -> (index % 251).toByte() }

    @Test
    fun streamStartsWithBeginAndEndsWithCommit() {
        val payload = payload(500)
        val commands = RoadAssets.commands(7, payload)
        val chunks = commands.filter { it.kind == RoadConstants.CMD_ASSET_CHUNK }
        assertEquals(3, chunks.size)
        assertEquals(RoadConstants.CMD_ASSET_BEGIN, commands.first().kind)
        assertEquals((7L shl 32) or 3L, commands.first().value)
        assertNull(commands.first().payload)
        assertEquals(RoadConstants.CMD_ASSET_COMMIT, commands.last().kind)
        assertEquals((7L shl 32) or 500L, commands.last().value)
        assertEquals(4, commands.last().payload?.size)
        assertEquals(commands.size, chunks.size + 2)
    }

    @Test
    fun chunksCoverThePayloadInOrder() {
        val payload = payload(1000)
        val chunks = RoadAssets.commands(1, payload).filter {
            it.kind == RoadConstants.CMD_ASSET_CHUNK
        }
        assertEquals(6, chunks.size)
        chunks.forEachIndexed { index, command ->
            assertEquals((1L shl 32) or index.toLong(), command.value)
            assertTrue((command.payload?.size ?: 0) <= RoadConstants.ASSET_CHUNK_BYTES)
        }
        val rebuilt = chunks.flatMap { it.payload!!.toList() }.toByteArray()
        assertArrayEquals(payload, rebuilt)
    }

    @Test
    fun emptyPayloadStillCommits() {
        val commands = RoadAssets.commands(2, ByteArray(0))
        assertEquals(2, commands.size)
        assertEquals(RoadConstants.CMD_ASSET_BEGIN, commands[0].kind)
        assertEquals((2L shl 32) or 0L, commands[0].value)
        assertEquals(RoadConstants.CMD_ASSET_COMMIT, commands[1].kind)
        assertArrayEquals(ByteArray(4), commands[1].payload)
    }

    @Test
    fun commitCarriesALittleEndianCrc() {
        val payload = "road loader".toByteArray()
        val expected = java.util.zip.CRC32().apply { update(payload) }.value
        val commit = RoadAssets.commands(3, payload).last()
        val encoded = commit.payload!!
        val decoded = (encoded[0].toLong() and 0xFF) or
            ((encoded[1].toLong() and 0xFF) shl 8) or
            ((encoded[2].toLong() and 0xFF) shl 16) or
            ((encoded[3].toLong() and 0xFF) shl 24)
        assertEquals(expected, decoded)
    }

    @Test
    fun resultReportsEveryCommand() {
        val result = RoadAssets.Result(
            assetId = 1,
            bytes = 512L * 1024,
            chunks = 2731,
            queued = 2733,
            millis = 8,
        )
        assertTrue(result.allQueued())
        assertTrue(result.describe().contains("2733/2733 commands queued"))
        assertTrue(result.megabytesPerSecond() > 0.0)
    }
}
