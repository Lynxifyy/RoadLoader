package dev.roadloader.core

/** Wraps the Rust GLSL ES 3.0 transpiler (Phase 6). */
object ShaderLab {
    const val STAGE_VERTEX = 0
    const val STAGE_FRAGMENT = 1

    /** Returns the ES 3.0 source, or null when the input is unsupported. */
    fun transpile(source: String, stage: Int): String? =
        NativeHost.transpileShader(source, stage)

    /** A Java-style shader used by the demo button. */
    const val SAMPLE_FRAGMENT: String = """
        uniform sampler2D texture;
        varying vec2 texCoord;
        void main() {
            vec4 color = texture2D(texture, texCoord);
            gl_FragColor = vec4(color.rgb * 1.1, color.a);
        }
    """
}
