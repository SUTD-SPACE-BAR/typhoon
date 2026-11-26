//! Draws the tow paths (`tow_paths::TowMesh`) with OpenGL, inside egui's own
//! frame through an egui_glow paint callback.
//!
//! The vertices go to the GPU once per G-code. After that a frame only sends
//! the camera and makes one draw call per layer shown, so it costs the same
//! however big the wind is. The vertex shader wraps the unrolled ribbons round
//! the tube; the fragment shader drops what's on the far side (the tube is
//! convex, so that's exactly what faces away) and shades the rest, with a
//! darker edge on each tow as in the unwrapped view. Tows are drawn in G-code
//! order, so later ones lie on top, as wound: no depth buffer is needed.
//!
//! eframe may get an OpenGL 3.3 core, OpenGL ES 2.0 or older compatibility
//! context, so the shaders are written for all of them, as egui_glow's are.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use eframe::egui::{self, Rect};
use eframe::egui_glow;
use eframe::glow::{self, HasContext as _};

use crate::tow_paths::VERTEX_BYTES;

/// What one frame draws.
#[derive(Clone, Debug)]
pub struct Frame3d {
    /// Turns the tube (model) into view space; rows.
    pub rot: [[f32; 3]; 3],
    /// Clip-space units per mm (x, y).
    pub scale: [f32; 2],
    /// Clip-space position of the tube's middle.
    pub offset: [f32; 2],
    /// Where the light comes from, in view space (unit length).
    pub light: [f32; 3],
    /// Model X (mm from the tube's middle) of a packed 0, and the span to a
    /// packed 65535.
    pub x_min: f32,
    pub x_span: f32,
    pub radius: f32,
    /// Half a ribbon's width, mm.
    pub half_width: f32,
    /// Where across a tow (0 its middle, 1 its edge) the darker edge starts;
    /// over 1 for none.
    pub core: f32,
    /// Half the width of the blend into the edge, in the same units.
    pub soft: f32,
    /// Brightness of the edge (1: as bright as the middle).
    pub edge: f32,
    /// (first vertex, count, colour 0..=1) of each layer to draw, bottom
    /// layer first.
    pub layers: Vec<(u32, u32, [f32; 3])>,
}

#[derive(Default)]
struct Slot {
    /// Vertices to send to the GPU before the next draw.
    pending: Option<Vec<u8>>,
    gpu: Option<Gpu>,
    /// Why drawing failed, e.g. a shader the driver wouldn't compile.
    error: Option<String>,
}

/// The 3D view's GPU side, shared with its paint callbacks. The OpenGL
/// objects are made on the first draw, where the context is at hand.
#[derive(Clone, Default)]
pub struct TowGl(Arc<Mutex<Slot>>);

impl TowGl {
    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Send these vertices (`VERTEX_BYTES` each) to the GPU at the next draw.
    pub fn upload(&self, bytes: Vec<u8>) {
        self.slot().pending = Some(bytes);
    }

    /// Why the tow paths can't be drawn, if they can't.
    pub fn error(&self) -> Option<String> {
        self.slot().error.clone()
    }

    /// A paint callback that draws `frame` into `rect`.
    pub fn callback(&self, ctx: &egui::Context, rect: Rect, frame: Frame3d) -> egui::PaintCallback {
        let shared = self.clone();
        let ctx = ctx.clone();
        let draw = move |_info: egui::PaintCallbackInfo, painter: &egui_glow::Painter| {
            let gl: &glow::Context = painter.gl();
            let mut slot = shared.slot();
            if slot.error.is_some() {
                return;
            }
            if slot.gpu.is_none() {
                match unsafe { Gpu::new(gl) } {
                    Ok(gpu) => slot.gpu = Some(gpu),
                    Err(error) => {
                        slot.error = Some(error);
                        // Show the message on the next frame.
                        ctx.request_repaint();
                        return;
                    }
                }
            }
            let pending = slot.pending.take();
            if let Some(gpu) = &mut slot.gpu {
                unsafe {
                    if let Some(bytes) = pending {
                        gpu.upload(gl, &bytes);
                    }
                    gpu.draw(gl, &frame);
                }
            }
        };
        egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(draw)),
        }
    }

    /// Free the OpenGL objects. The context must be current, as it is on
    /// the UI thread.
    pub fn destroy(&self, gl: &glow::Context) {
        let mut slot = self.slot();
        slot.pending = None;
        if let Some(gpu) = slot.gpu.take() {
            unsafe { gpu.destroy(gl) };
        }
    }
}

struct Uniforms {
    rot: Option<glow::UniformLocation>,
    scale: Option<glow::UniformLocation>,
    offset: Option<glow::UniformLocation>,
    light: Option<glow::UniformLocation>,
    x_min: Option<glow::UniformLocation>,
    x_span: Option<glow::UniformLocation>,
    radius: Option<glow::UniformLocation>,
    half_width: Option<glow::UniformLocation>,
    color: Option<glow::UniformLocation>,
    core: Option<glow::UniformLocation>,
    soft: Option<glow::UniformLocation>,
    edge: Option<glow::UniformLocation>,
}

struct Gpu {
    program: glow::Program,
    buffer: glow::Buffer,
    /// Where vertex arrays are core: core profiles can't draw without one.
    vao: Option<glow::VertexArray>,
    a_pos: u32,
    a_shape: u32,
    u: Uniforms,
    /// Vertices in `buffer`.
    vertices: u32,
}

impl Gpu {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let version = egui_glow::ShaderVersion::get(gl);
        let header = format!(
            "{}\n#define NEW_SHADER_INTERFACE {}\n",
            version.version_declaration(),
            version.is_new_shader_interface() as i32
        );
        unsafe {
            let program = link(
                gl,
                &format!("{header}{VERTEX_SHADER}"),
                &format!("{header}{FRAGMENT_SHADER}"),
            )?;
            let (Some(a_pos), Some(a_shape)) = (
                gl.get_attrib_location(program, "a_pos"),
                gl.get_attrib_location(program, "a_shape"),
            ) else {
                gl.delete_program(program);
                return Err("the shader lost its vertex inputs".into());
            };
            let u = |name: &str| gl.get_uniform_location(program, name);
            let u = Uniforms {
                rot: u("u_rot"),
                scale: u("u_scale"),
                offset: u("u_offset"),
                light: u("u_light"),
                x_min: u("u_x_min"),
                x_span: u("u_x_span"),
                radius: u("u_radius"),
                half_width: u("u_half_width"),
                color: u("u_color"),
                core: u("u_core"),
                soft: u("u_soft"),
                edge: u("u_edge"),
            };
            let buffer = match gl.create_buffer() {
                Ok(buffer) => buffer,
                Err(error) => {
                    gl.delete_program(program);
                    return Err(error);
                }
            };
            let vao = if has_vertex_arrays(gl) {
                match gl.create_vertex_array() {
                    Ok(vao) => Some(vao),
                    Err(error) => {
                        gl.delete_buffer(buffer);
                        gl.delete_program(program);
                        return Err(error);
                    }
                }
            } else {
                None
            };
            Ok(Self { program, buffer, vao, a_pos, a_shape, u, vertices: 0 })
        }
    }

    unsafe fn upload(&mut self, gl: &glow::Context, bytes: &[u8]) {
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        self.vertices = (bytes.len() / VERTEX_BYTES) as u32;
    }

    unsafe fn draw(&self, gl: &glow::Context, f: &Frame3d) {
        if self.vertices == 0 || f.layers.is_empty() {
            return;
        }
        let r = f.rot;
        // Column by column, as GLSL reads it (GL ES can't transpose).
        let columns = [
            r[0][0], r[1][0], r[2][0], r[0][1], r[1][1], r[2][1], r[0][2], r[1][2], r[2][2],
        ];
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_3_f32_slice(self.u.rot.as_ref(), false, &columns);
            gl.uniform_2_f32(self.u.scale.as_ref(), f.scale[0], f.scale[1]);
            gl.uniform_2_f32(self.u.offset.as_ref(), f.offset[0], f.offset[1]);
            gl.uniform_3_f32(self.u.light.as_ref(), f.light[0], f.light[1], f.light[2]);
            gl.uniform_1_f32(self.u.x_min.as_ref(), f.x_min);
            gl.uniform_1_f32(self.u.x_span.as_ref(), f.x_span);
            gl.uniform_1_f32(self.u.radius.as_ref(), f.radius);
            gl.uniform_1_f32(self.u.half_width.as_ref(), f.half_width);
            gl.uniform_1_f32(self.u.core.as_ref(), f.core);
            gl.uniform_1_f32(self.u.soft.as_ref(), f.soft);
            gl.uniform_1_f32(self.u.edge.as_ref(), f.edge);

            if let Some(vao) = self.vao {
                gl.bind_vertex_array(Some(vao));
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            let stride = VERTEX_BYTES as i32;
            gl.vertex_attrib_pointer_f32(self.a_pos, 2, glow::UNSIGNED_SHORT, true, stride, 0);
            gl.vertex_attrib_pointer_f32(self.a_shape, 4, glow::BYTE, true, stride, 4);
            gl.enable_vertex_attrib_array(self.a_pos);
            gl.enable_vertex_attrib_array(self.a_shape);
            for &(first, count, [red, green, blue]) in &f.layers {
                // Never read past what's uploaded.
                let count = count.min(self.vertices.saturating_sub(first));
                if count >= 3 {
                    gl.uniform_3_f32(self.u.color.as_ref(), red, green, blue);
                    gl.draw_arrays(glow::TRIANGLE_STRIP, first as i32, count as i32);
                }
            }
            // Leave the vertex inputs as egui expects them.
            gl.disable_vertex_attrib_array(self.a_pos);
            gl.disable_vertex_attrib_array(self.a_shape);
            if self.vao.is_some() {
                gl.bind_vertex_array(None);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
    }

    unsafe fn destroy(self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_buffer(self.buffer);
            if let Some(vao) = self.vao {
                gl.delete_vertex_array(vao);
            }
        }
    }
}

unsafe fn link(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    unsafe {
        let vertex = compile(gl, glow::VERTEX_SHADER, vertex).map_err(|e| format!("vertex shader: {e}"))?;
        let fragment = match compile(gl, glow::FRAGMENT_SHADER, fragment) {
            Ok(shader) => shader,
            Err(error) => {
                gl.delete_shader(vertex);
                return Err(format!("fragment shader: {error}"));
            }
        };
        let program = match gl.create_program() {
            Ok(program) => program,
            Err(error) => {
                gl.delete_shader(vertex);
                gl.delete_shader(fragment);
                return Err(error);
            }
        };
        gl.attach_shader(program, vertex);
        gl.attach_shader(program, fragment);
        gl.link_program(program);
        gl.detach_shader(program, vertex);
        gl.detach_shader(program, fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        if gl.get_program_link_status(program) {
            Ok(program)
        } else {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            Err(format!("linking the shaders: {log}"))
        }
    }
}

unsafe fn compile(gl: &glow::Context, kind: u32, source: &str) -> Result<glow::Shader, String> {
    unsafe {
        let shader = gl.create_shader(kind)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if gl.get_shader_compile_status(shader) {
            Ok(shader)
        } else {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            Err(log)
        }
    }
}

/// Whether vertex array objects are core in this context: OpenGL 3 and up,
/// or GL ES 3 and up. Older contexts draw from the vertex inputs set just
/// before, which is how egui_glow works there too.
fn has_vertex_arrays(gl: &glow::Context) -> bool {
    // E.g. "4.6.0 NVIDIA 560.94", "3.1.0 - Build 9.17.10.4459",
    // "OpenGL ES 2.0 Mesa 23.2.1".
    let version = unsafe { gl.get_parameter_string(glow::VERSION) };
    let number = version.trim_start_matches(|c: char| !c.is_ascii_digit());
    !(number.starts_with('1') || number.starts_with('2'))
}

/// Wraps the unrolled ribbons round the tube and projects them.
const VERTEX_SHADER: &str = r#"
#if NEW_SHADER_INTERFACE
    #define I in
    #define O out
#else
    #define I attribute
    #define O varying
#endif
#ifdef GL_ES
    precision highp float;
#endif

uniform mat3 u_rot;
uniform vec2 u_scale;
uniform vec2 u_offset;
uniform vec3 u_light;
uniform float u_x_min;
uniform float u_x_span;
uniform float u_radius;
uniform float u_half_width;

// The tow's middle: along the tube and round it, as fractions.
I vec2 a_pos;
// The offset from the middle (along, round) in units of two half widths,
// then the side: -1 or 1.
I vec4 a_shape;

O float v_side;
O float v_facing;
O float v_light;

void main() {
    vec2 offset = a_shape.xy * (2.0 * u_half_width);
    float x = u_x_min + a_pos.x * u_x_span + offset.x;
    float angle = a_pos.y * 6.2831853 + offset.y / u_radius;
    vec3 normal = vec3(0.0, cos(angle), sin(angle));
    vec3 view_normal = u_rot * normal;
    vec3 position = u_rot * vec3(x, u_radius * normal.y, u_radius * normal.z);
    v_side = a_shape.z;
    v_facing = view_normal.z;
    v_light = 0.45 + 0.55 * max(dot(view_normal, u_light), 0.0);
    gl_Position = vec4(position.xy * u_scale + u_offset, 0.0, 1.0);
}
"#;

/// Hides the far side; shades each tow, darker at its edges.
const FRAGMENT_SHADER: &str = r#"
#ifdef GL_ES
    precision mediump float;
#endif

uniform vec3 u_color;
uniform float u_core;
uniform float u_soft;
uniform float u_edge;

#if NEW_SHADER_INTERFACE
    in float v_side;
    in float v_facing;
    in float v_light;
    out vec4 f_color;
    #define FRAG_COLOR f_color
#else
    varying float v_side;
    varying float v_facing;
    varying float v_light;
    #define FRAG_COLOR gl_FragColor
#endif

void main() {
    // The far side of the tube, hidden behind the near side.
    if (v_facing < 0.0) {
        discard;
    }
    float edge = smoothstep(u_core - u_soft, u_core + u_soft, abs(v_side));
    FRAG_COLOR = vec4(u_color * mix(1.0, u_edge, edge) * v_light, 1.0);
}
"#;
