// SPDX-License-Identifier: MIT

//! A minimal directional-diffuse shader for the 3D board.
//!
//! raylib's built-in shader is unlit, and every mesh raylib generates carries
//! normals but **no** vertex colours (verified: `colors().len() == 0` for
//! `gen_mesh_cube`/`_cylinder`/`_sphere`), so there is nothing available to bake
//! shading into. These two short programs use the normals raylib already
//! provides to add a single-light diffuse term, which is what makes the board
//! read as a solid object rather than a silhouette.
//!
//! raylib supplies `matNormal` as the inverse-transpose of the model matrix, so
//! the heavily non-uniform scaling used for the PCB shades correctly.

use raylib::prelude::*;

/// GLSL 330 matches raylib 5.x/6.x desktop builds, whose default shaders are
/// also 330. The attribute and uniform names are raylib's documented defaults.
const LIT_VS: &str = r#"#version 330
in vec3 vertexPosition;
in vec3 vertexNormal;
in vec4 vertexColor;
uniform mat4 mvp;
uniform mat4 matNormal;
out vec3 fragNormal;
out vec4 fragColor;
void main() {
    fragNormal = normalize(mat3(matNormal) * vertexNormal);
    fragColor = vertexColor;
    gl_Position = mvp * vec4(vertexPosition, 1.0);
}
"#;

const LIT_FS: &str = r#"#version 330
in vec3 fragNormal;
in vec4 fragColor;
uniform vec4 colDiffuse;
out vec4 finalColor;
void main() {
    // Fixed key light; the ambient floor keeps unlit faces legible.
    vec3 keyLight = normalize(vec3(0.45, 1.0, 0.35));
    float diffuse = 0.34 + 0.66 * max(dot(normalize(fragNormal), keyLight), 0.0);
    finalColor = vec4(fragColor.rgb * colDiffuse.rgb * diffuse, fragColor.a * colDiffuse.a);
}
"#;

/// Build the diffuse shader used by every lit board part.
pub fn lit(rl: &mut RaylibHandle, thread: &RaylibThread) -> Shader {
    rl.load_shader_from_memory(thread, Some(LIT_VS), Some(LIT_FS))
}

/// Route every material of `model` through `shader`.
///
/// The caller must keep `shader` alive for as long as `model` is drawn, so
/// [`crate::board3d::Board3D`] owns it alongside the models.
pub fn apply(model: &mut Model, shader: &Shader) {
    for material in model.materials_mut() {
        material.set_shader(shader);
    }
}
