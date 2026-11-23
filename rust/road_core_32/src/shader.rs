//! GLSL ES 2.0/1.20 (Java Edition style) to GLSL ES 3.00 transpiler.
//!
//! The 1.17 renderer exposes an OpenGL ES 3.0 context, so Java shaders written
//! against the desktop/ES2 dialect need the mechanical rewrites below. The
//! transpiler is intentionally conservative: anything it cannot rewrite safely
//! is rejected instead of being injected into the game's pipeline.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShaderError {
    Empty,
    Unsupported(&'static str),
    NoMain,
}

/// Transpiles a Java/ES2 shader to GLSL ES 3.00.
pub fn transpile_java_glsl(source: &str, stage: ShaderStage) -> Result<String, ShaderError> {
    if source.trim().is_empty() {
        return Err(ShaderError::Empty);
    }
    let lowered = source.to_ascii_lowercase();
    for forbidden in ["#include", "gl_fragdata", "gl_fragdepth"] {
        if lowered.contains(forbidden) {
            return Err(ShaderError::Unsupported(forbidden));
        }
    }
    if !lowered.contains("void main") {
        return Err(ShaderError::NoMain);
    }

    let mut body = String::with_capacity(source.len() + 128);
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#version") {
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }

    let stage_keyword = match stage {
        ShaderStage::Vertex => "out",
        ShaderStage::Fragment => "in",
    };
    let body = body
        .replace("attribute ", "in ")
        .replace("varying ", &format!("{stage_keyword} "))
        .replace("texture2D(", "texture(")
        .replace("textureCube(", "texture(")
        .replace("texture2DProj(", "textureProj(");

    let mut output = String::with_capacity(body.len() + 128);
    output.push_str("#version 300 es\n");
    match stage {
        ShaderStage::Vertex => {
            output.push_str("precision highp float;\nprecision highp int;\n");
        }
        ShaderStage::Fragment => {
            if !lowered.contains("precision ") {
                output.push_str("precision highp float;\nprecision highp int;\n");
            }
            if body.contains("gl_FragColor") {
                output.push_str("layout(location = 0) out vec4 pc_fragColor;\n");
            }
        }
    }
    output.push_str(&body.replace("gl_FragColor", "pc_fragColor"));
    Ok(output)
}

/// Transpiles a vertex/fragment pair, returning them in that order.
pub fn transpile_pair(vertex: &str, fragment: &str) -> Result<(String, String), ShaderError> {
    Ok((
        transpile_java_glsl(vertex, ShaderStage::Vertex)?,
        transpile_java_glsl(fragment, ShaderStage::Fragment)?,
    ))
}

/// Cheap validation used before handing a shader to the driver.
pub fn looks_like_post_process(fragment: &str) -> bool {
    let lowered = fragment.to_ascii_lowercase();
    lowered.contains("uniform sampler2d") && (lowered.contains("gl_fragcolor") || lowered.contains("pc_fragcolor"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERTEX: &str = "#version 120\nattribute vec3 aPos;\nattribute vec2 aUv;\nvarying vec2 vUv;\nvoid main() {\n  vUv = aUv;\n  gl_Position = vec4(aPos, 1.0);\n}\n";

    const FRAGMENT: &str = "uniform sampler2D uTex;\nvarying vec2 vUv;\nvoid main() {\n  vec4 c = texture2D(uTex, vUv);\n  gl_FragColor = vec4(c.rgb * 1.1, c.a);\n}\n";

    #[test]
    fn vertex_shader_is_rewritten() {
        let output = transpile_java_glsl(VERTEX, ShaderStage::Vertex).expect("vertex");
        assert!(output.starts_with("#version 300 es\n"));
        assert!(output.contains("in vec3 aPos;"));
        assert!(output.contains("out vec2 vUv;"));
        assert!(!output.contains("attribute"));
        assert!(!output.contains("#version 120"));
    }

    #[test]
    fn fragment_shader_gets_es3_output() {
        let output = transpile_java_glsl(FRAGMENT, ShaderStage::Fragment).expect("fragment");
        assert!(output.contains("layout(location = 0) out vec4 pc_fragColor;"));
        assert!(output.contains("texture(uTex, vUv)"));
        assert!(output.contains("pc_fragColor = vec4"));
        assert!(!output.contains("gl_FragColor"));
        assert!(output.contains("precision highp float;"));
    }

    #[test]
    fn existing_precision_is_respected() {
        let source = "precision mediump float;\nvoid main() { gl_FragColor = vec4(1.0); }";
        let output = transpile_java_glsl(source, ShaderStage::Fragment).expect("fragment");
        assert_eq!(output.matches("precision mediump float;").count(), 1);
        assert!(!output.contains("precision highp float;"));
    }

    #[test]
    fn comments_before_version_are_preserved() {
        let source = "// Sildur's Basic\n#version 120\nvoid main() { gl_FragColor = vec4(1.0); }";
        let output = transpile_java_glsl(source, ShaderStage::Fragment).expect("fragment");
        assert!(output.starts_with("#version 300 es\n"));
        assert!(output.contains("// Sildur's Basic"));
    }

    #[test]
    fn include_directives_are_rejected() {
        let source = "#include <common.glsl>\nvoid main() {}";
        assert_eq!(
            transpile_java_glsl(source, ShaderStage::Fragment),
            Err(ShaderError::Unsupported("#include"))
        );
    }

    #[test]
    fn missing_main_is_rejected() {
        assert_eq!(
            transpile_java_glsl("uniform float x;", ShaderStage::Fragment),
            Err(ShaderError::NoMain)
        );
    }

    #[test]
    fn empty_source_is_rejected() {
        assert_eq!(transpile_java_glsl("   \n", ShaderStage::Fragment), Err(ShaderError::Empty));
    }

    #[test]
    fn pair_transpiles_both_stages() {
        let (vertex, fragment) = transpile_pair(VERTEX, FRAGMENT).expect("pair");
        assert!(vertex.contains("in vec3 aPos;"));
        assert!(fragment.contains("pc_fragColor"));
        assert!(looks_like_post_process(FRAGMENT));
    }
}
