package dev.roadloader.core

import android.content.Context
import android.opengl.GLES20
import android.opengl.GLSurfaceView
import android.view.MotionEvent
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10

/**
 * What the presenter measured over the last second.
 *
 * [fps] counts frames that were actually received from the companion and drawn;
 * it is 0 when nothing arrives. [frameAgeMs] is the age of the newest frame when
 * it was drawn, measured from the companion's publish timestamp to the upload,
 * so it covers capture, the shared slot and the texture upload.
 */
data class PresentStats(val fps: Int, val frameAgeMs: Int, val framesTotal: Long)

/**
 * Draws the companion's RGBA frames on a fullscreen quad and forwards touches
 * into the shared input ring.
 */
class FrameSurfaceView(context: Context) : GLSurfaceView(context) {
    private val renderer = FrameRenderer()

    var session: RoadSession? = null
        set(value) {
            field = value
            renderer.session = value
        }

    var onPresentStats: ((PresentStats) -> Unit)? = null
    var onFirstFrame: (() -> Unit)? = null

    init {
        setEGLContextClientVersion(2)
        setRenderer(renderer)
        renderMode = RENDERMODE_CONTINUOUSLY
        renderer.onPresentStats = { stats -> post { onPresentStats?.invoke(stats) } }
        renderer.onFirstFrame = { post { onFirstFrame?.invoke() } }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        val active = session ?: return super.onTouchEvent(event)
        if (width == 0 || height == 0) return true
        val scaleX = active.width.toFloat() / width.toFloat()
        val scaleY = active.height.toFloat() / height.toFloat()
        val kind = when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> RoadConstants.INPUT_DOWN
            MotionEvent.ACTION_MOVE -> RoadConstants.INPUT_MOVE
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> RoadConstants.INPUT_UP
            MotionEvent.ACTION_CANCEL -> RoadConstants.INPUT_CANCEL
            else -> return true
        }
        val timestamp = System.nanoTime()
        for (index in 0 until event.pointerCount) {
            val pointerId = event.getPointerId(index)
            // A move event carries every pointer; only the other actions carry
            // the pointer that changed.
            if (kind == RoadConstants.INPUT_MOVE ||
                index == event.actionIndex
            ) {
                active.pushInput(
                    kind = kind,
                    pointerId = pointerId,
                    action = event.actionMasked,
                    x = event.getX(index) * scaleX,
                    y = event.getY(index) * scaleY,
                    pressure = event.getPressure(index),
                    size = event.getSize(index),
                    timestampNs = timestamp,
                )
            }
        }
        return true
    }

    private class FrameRenderer : Renderer {
        var session: RoadSession? = null
        var onPresentStats: ((PresentStats) -> Unit)? = null
        var onFirstFrame: (() -> Unit)? = null

        private var program = 0
        private var texture = 0
        private var positionHandle = 0
        private var uvHandle = 0
        private var samplerHandle = 0
        private var quad: FloatBuffer? = null
        private var textureWidth = 0
        private var textureHeight = 0
        private var firstFrameDelivered = false
        private var framesThisSecond = 0
        private var framesTotal = 0L
        private var lastSequence = -1L
        private var newestAgeMs = 0
        private var secondStartedNs = System.nanoTime()
        private var packedBuffer: ByteBuffer? = null
        private var strideWarned = false

        override fun onSurfaceCreated(gl: GL10?, config: EGLConfig?) {
            program = buildProgram(VERTEX_SHADER, FRAGMENT_SHADER)
            texture = createTexture()
            positionHandle = GLES20.glGetAttribLocation(program, "aPosition")
            uvHandle = GLES20.glGetAttribLocation(program, "aTexCoord")
            samplerHandle = GLES20.glGetUniformLocation(program, "uTexture")
            val vertices = floatArrayOf(
                -1f, -1f, 0f, 1f,
                1f, -1f, 1f, 1f,
                -1f, 1f, 0f, 0f,
                1f, 1f, 1f, 0f,
            )
            quad = ByteBuffer.allocateDirect(vertices.size * 4)
                .order(ByteOrder.nativeOrder())
                .asFloatBuffer()
                .apply {
                    put(vertices)
                    position(0)
                }
        }

        override fun onSurfaceChanged(gl: GL10?, width: Int, height: Int) {
            GLES20.glViewport(0, 0, width, height)
        }

        override fun onDrawFrame(gl: GL10?) {
            GLES20.glClearColor(0.02f, 0.03f, 0.05f, 1f)
            GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)
            val active = session
            if (active != null) {
                val frame = active.pollFrame()
                if (frame != null) {
                    val now = System.nanoTime()
                    if (frame.publishedNs > 0 && now > frame.publishedNs) {
                        newestAgeMs = ((now - frame.publishedNs) / 1_000_000L).toInt()
                    }
                    // A slot can be republished with the same pixels; skipping
                    // the upload keeps the GL thread free for the next frame.
                    if (frame.sequence != lastSequence) {
                        lastSequence = frame.sequence
                        upload(frame)
                        framesThisSecond += 1
                        framesTotal += 1
                    }
                    active.releaseFrame(frame.slot)
                    if (!firstFrameDelivered) {
                        firstFrameDelivered = true
                        onFirstFrame?.invoke()
                    }
                }
            }
            if (textureWidth > 0 && textureHeight > 0) {
                draw()
            }
            val now = System.nanoTime()
            if (now - secondStartedNs >= 1_000_000_000L) {
                onPresentStats?.invoke(PresentStats(framesThisSecond, newestAgeMs, framesTotal))
                framesThisSecond = 0
                newestAgeMs = 0
                secondStartedNs = now
            }
        }

        private fun upload(frame: RoadFrame) {
            val pixels = packedPixels(frame)
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, texture)
            pixels.position(0)
            pixels.limit(frame.width * frame.height * 4)
            if (frame.width != textureWidth || frame.height != textureHeight) {
                GLES20.glTexImage2D(
                    GLES20.GL_TEXTURE_2D,
                    0,
                    GLES20.GL_RGBA,
                    frame.width,
                    frame.height,
                    0,
                    GLES20.GL_RGBA,
                    GLES20.GL_UNSIGNED_BYTE,
                    pixels,
                )
                textureWidth = frame.width
                textureHeight = frame.height
            } else {
                GLES20.glTexSubImage2D(
                    GLES20.GL_TEXTURE_2D,
                    0,
                    0,
                    0,
                    frame.width,
                    frame.height,
                    GLES20.GL_RGBA,
                    GLES20.GL_UNSIGNED_BYTE,
                    pixels,
                )
            }
        }

        /**
         * Returns a buffer whose rows are packed at `width * 4` bytes.
         *
         * The companion publishes packed rows, so the common path returns the
         * shared buffer untouched. A padded stride is repacked instead of being
         * uploaded with the wrong row length, which would shear the image.
         */
        private fun packedPixels(frame: RoadFrame): ByteBuffer {
            val packed = frame.width * frame.height * 4
            val shared = frame.buffer
            if (FrameContract.isTightlyPacked(frame.width, frame.stride)) {
                return shared
            }
            if (!strideWarned) {
                strideWarned = true
                Log.w(TAG, "stride ${frame.stride} != ${frame.width * 4}, repacking rows")
            }
            val scratch = packedBuffer?.takeIf { it.capacity() >= packed }
                ?: ByteBuffer.allocateDirect(packed).order(ByteOrder.nativeOrder()).also {
                    packedBuffer = it
                }
            val rowBytes = frame.width * 4
            val source = shared.duplicate()
            source.position(0)
            source.limit(minOf(shared.capacity(), frame.stride * frame.height))
            scratch.position(0)
            for (row in 0 until frame.height) {
                source.position(row * frame.stride)
                source.limit(row * frame.stride + rowBytes)
                scratch.put(source)
                source.limit(minOf(shared.capacity(), frame.stride * frame.height))
            }
            return scratch
        }

        private fun draw() {
            GLES20.glUseProgram(program)
            GLES20.glActiveTexture(GLES20.GL_TEXTURE0)
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, texture)
            GLES20.glUniform1i(samplerHandle, 0)
            val buffer = quad ?: return
            buffer.position(0)
            GLES20.glVertexAttribPointer(positionHandle, 2, GLES20.GL_FLOAT, false, 16, buffer)
            GLES20.glEnableVertexAttribArray(positionHandle)
            buffer.position(2)
            GLES20.glVertexAttribPointer(uvHandle, 2, GLES20.GL_FLOAT, false, 16, buffer)
            GLES20.glEnableVertexAttribArray(uvHandle)
            GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4)
            GLES20.glDisableVertexAttribArray(positionHandle)
            GLES20.glDisableVertexAttribArray(uvHandle)
            GLES20.glUseProgram(0)
        }

        private fun createTexture(): Int {
            val handles = IntArray(1)
            GLES20.glGenTextures(1, handles, 0)
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, handles[0])
            GLES20.glTexParameteri(
                GLES20.GL_TEXTURE_2D,
                GLES20.GL_TEXTURE_MIN_FILTER,
                GLES20.GL_LINEAR,
            )
            GLES20.glTexParameteri(
                GLES20.GL_TEXTURE_2D,
                GLES20.GL_TEXTURE_MAG_FILTER,
                GLES20.GL_LINEAR,
            )
            GLES20.glTexParameteri(
                GLES20.GL_TEXTURE_2D,
                GLES20.GL_TEXTURE_WRAP_S,
                GLES20.GL_CLAMP_TO_EDGE,
            )
            GLES20.glTexParameteri(
                GLES20.GL_TEXTURE_2D,
                GLES20.GL_TEXTURE_WRAP_T,
                GLES20.GL_CLAMP_TO_EDGE,
            )
            return handles[0]
        }

        private fun buildProgram(vertexSource: String, fragmentSource: String): Int {
            val vertex = compile(GLES20.GL_VERTEX_SHADER, vertexSource)
            val fragment = compile(GLES20.GL_FRAGMENT_SHADER, fragmentSource)
            val handle = GLES20.glCreateProgram()
            GLES20.glAttachShader(handle, vertex)
            GLES20.glAttachShader(handle, fragment)
            GLES20.glLinkProgram(handle)
            GLES20.glDeleteShader(vertex)
            GLES20.glDeleteShader(fragment)
            return handle
        }

        private fun compile(type: Int, source: String): Int {
            val shader = GLES20.glCreateShader(type)
            GLES20.glShaderSource(shader, source)
            GLES20.glCompileShader(shader)
            return shader
        }

        companion object {
            private const val TAG = "FrameSurfaceView"

            private const val VERTEX_SHADER = """
                attribute vec2 aPosition;
                attribute vec2 aTexCoord;
                varying vec2 vTexCoord;
                void main() {
                    vTexCoord = aTexCoord;
                    gl_Position = vec4(aPosition, 0.0, 1.0);
                }
            """

            private const val FRAGMENT_SHADER = """
                precision mediump float;
                varying vec2 vTexCoord;
                uniform sampler2D uTexture;
                void main() {
                    gl_FragColor = texture2D(uTexture, vTexCoord);
                }
            """
        }
    }
}
