package dev.roadloader.core

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Plays the float PCM blocks produced by the companion's audio bridge. */
class RoadAudioBridge(private val session: RoadSession) {
    private var track: AudioTrack? = null

    @Volatile
    private var running = false

    private var thread: Thread? = null

    @Volatile
    var error: String? = null
        private set

    @Volatile
    var blocksPlayed = 0L
        private set

    fun start(): Boolean {
        if (running) return true
        val created = createTrack()
        if (created == null) {
            Log.w(TAG, "no audio sink: $error; draining the bridge instead")
        } else {
            track = created
            created.play()
        }
        running = true
        thread = Thread({
            val bytes = ByteArray(AUDIO_BUFFER_BYTES)
            // Float PCM has to go through a ByteBuffer; write(ByteArray) only
            // accepts 8/16-bit PCM.
            val direct = ByteBuffer.allocateDirect(AUDIO_BUFFER_BYTES)
                .order(ByteOrder.nativeOrder())
            try {
                while (running) {
                    val count = session.popAudio(bytes)
                    if (count > 0) {
                        if (created != null) {
                            direct.clear()
                            direct.put(bytes, 0, count)
                            direct.flip()
                            created.write(direct, count, AudioTrack.WRITE_BLOCKING)
                            blocksPlayed += 1
                        }
                    } else {
                        Thread.sleep(4)
                    }
                }
            } catch (throwable: Throwable) {
                error = throwable.message ?: throwable.javaClass.simpleName
                Log.w(TAG, "audio playback stopped", throwable)
            }
        }, "road-audio").apply {
            priority = Thread.MAX_PRIORITY
            start()
        }
        return created != null
    }

    private fun createTrack(): AudioTrack? {
        val minimum = AudioTrack.getMinBufferSize(
            SAMPLE_RATE,
            AudioFormat.CHANNEL_OUT_STEREO,
            AudioFormat.ENCODING_PCM_FLOAT,
        )
        if (minimum <= 0) {
            error = "device has no float PCM output"
            return null
        }
        return try {
            AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_GAME)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build(),
                )
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                        .setSampleRate(SAMPLE_RATE)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                        .build(),
                )
                .setBufferSizeInBytes(minimum * 2)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .build()
        } catch (throwable: Throwable) {
            error = throwable.message ?: throwable.javaClass.simpleName
            null
        }
    }

    fun stop() {
        running = false
        thread?.join(500)
        thread = null
        track?.let { audio ->
            runCatching {
                audio.pause()
                audio.flush()
                audio.release()
            }
        }
        track = null
    }

    companion object {
        private const val TAG = "RoadAudio"
        private const val SAMPLE_RATE = 48_000
        private const val AUDIO_BUFFER_BYTES = 4096
    }
}
