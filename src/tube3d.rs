//! Optional 3D view of the wound tube: the tow paths themselves, every run of
//! tow a ribbon at its true width lying on a steel mandrel, like a slicer's
//! preview. `tow_paths` lays the ribbons out and `tow_gl` draws them.
//!
//! The mandrel and its outline are a few hundred egui triangles made each
//! frame. The ribbons are built once per G-code and kept on the GPU, so a
//! frame costs the same however big the wind is. Nothing 3D exists until the
//! view is switched on, and like the rest of the window it's only redrawn
//! when something changes.

use std::f32::consts::TAU;
use std::sync::Arc;

use eframe::egui::{
    Align2, Color32, Mesh, PointerButton, Pos2, Rect, Sense, Shape, Stroke, TextStyle, Ui, Vec2,
    epaint::{Vertex, WHITE_UV},
};

use crate::preview::{Toolpath, layer_color};
use crate::theme::{self, Palette};
use crate::tow_gl::{Frame3d, TowGl};
use crate::tow_paths::TowMesh;

/// Facets around the tube: enough for a smooth outline. The tow paths are
/// round whatever this is.
const FACETS: usize = 96;
/// Bare mandrel, where no tow covers it.
const STEEL: Color32 = Color32::from_rgb(0xC4, 0xC9, 0xD1);
/// Where the light comes from, in view space: upper right, towards the
/// viewer, so the end the starting pose shows is lit too.
const LIGHT: Vec3 = [0.35, 0.6, 0.72];
/// Radians turned per point dragged.
const TURN_PER_POINT: f32 = 0.01;
/// Width of a thin line on screen, points.
const THIN_LINE: f32 = 1.5;
/// Where across a tow (0 its middle, 1 its edge) its darker edge starts, and
/// how dark that is: as in the unwrapped view.
const TOW_CORE: f32 = 0.72;
const TOW_EDGE: f32 = 0.45;

/// x along the tube, y up, z towards the viewer.
type Vec3 = [f32; 3];
/// Rows of a 3 × 3 rotation.
type Mat3 = [[f32; 3]; 3];

/// What to show, besides the toolpath.
pub struct Show<'a> {
    /// For each layer, whether it's drawn.
    pub layers: &'a [bool],
    /// A thin line along the middle of each pass instead of a tow at its
    /// true width.
    pub thin_lines: bool,
    /// Where a running job is: real position (mm, degrees).
    pub marker: Option<(f64, f64)>,
}

struct Camera {
    /// Turns the tube (model) into view space; changed by dragging.
    rot: Mat3,
    /// 1.0 = the tube fits the view in its starting pose.
    zoom: f32,
    /// The tube's centre, offset from the middle of the view, in points.
    pan: Vec2,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            rot: start_pose(),
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }
}

/// Turned and tipped a little, so it reads as a tube straight away.
fn start_pose() -> Mat3 {
    mul(&rot_x(18f32.to_radians()), &rot_y(-22f32.to_radians()))
}

/// The tow paths on the GPU, and where each layer is in them.
struct Paths {
    /// The toolpath they were built from.
    toolpath: Arc<Toolpath>,
    /// Each layer's (first vertex, count).
    layers: Vec<(u32, u32)>,
    /// X (mm) of a packed 0, and the span to a packed 65535.
    x_min: f32,
    x_span: f32,
}

#[derive(Default)]
pub struct TubeView {
    camera: Camera,
    gl: TowGl,
    paths: Option<Paths>,
}

impl TubeView {
    /// Whether the tow paths of `tp` are in hand.
    pub fn has_paths_for(&self, tp: &Arc<Toolpath>) -> bool {
        self.paths.as_ref().is_some_and(|p| Arc::ptr_eq(&p.toolpath, tp))
    }

    /// Show these tow paths, built from `tp`.
    pub fn set_paths(&mut self, tp: &Arc<Toolpath>, mesh: TowMesh) {
        self.gl.upload(mesh.bytes);
        self.paths = Some(Paths {
            toolpath: tp.clone(),
            layers: mesh.layers,
            x_min: mesh.x_min,
            x_span: mesh.x_span,
        });
    }

    /// Forget the tow paths (the G-code changed). The GPU buffer is reused
    /// for the next ones.
    pub fn clear_paths(&mut self) {
        self.paths = None;
    }

    pub fn reset_camera(&mut self) {
        self.camera = Camera::default();
    }

    pub fn gl(&self) -> &TowGl {
        &self.gl
    }

    /// Give up the GPU side, to be freed where the OpenGL context is at hand.
    pub fn into_gl(self) -> TowGl {
        self.gl
    }

    /// Draw the tube in `rect`.
    pub fn ui(&mut self, ui: &mut Ui, rect: Rect, tp: &Toolpath, show: Show, p: &Palette) {
        let response = ui.interact(rect, ui.make_persistent_id("tube3d"), Sense::click_and_drag());
        let length = tp.length.max(1e-3);
        let radius = (tp.circumference / TAU).max(1e-3);
        // Tows centred on the ends hang half off the wind: give them steel
        // to lie on.
        let tube_length = length + 2.0 * tp.tow_width;

        // Drag turns, right/middle-drag moves, the wheel zooms about the
        // cursor, double-click starts over.
        let camera = &mut self.camera;
        if response.dragged_by(PointerButton::Primary) {
            let d = response.drag_delta();
            let turn = mul(&rot_x(d.y * TURN_PER_POINT), &rot_y(d.x * TURN_PER_POINT));
            camera.rot = orthonormalize(mul(&turn, &camera.rot));
        }
        if response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle) {
            camera.pan += response.drag_delta();
        }
        if let Some(hover) = response.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let before = camera.zoom;
                camera.zoom = (camera.zoom * (scroll * 0.0025).exp()).clamp(0.3, 40.0);
                let at = hover - rect.center();
                camera.pan = at - (at - camera.pan) * (camera.zoom / before);
            }
        }
        if response.double_clicked() {
            *camera = Camera::default();
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, p.corner(4), p.well);

        // Scale so the starting pose fills the view, and keep it while turning.
        let (lo, hi) = projected_bounds(&start_pose(), tube_length, radius);
        let fit = 0.9 * (rect.width() / (hi.x - lo.x)).min(rect.height() / (hi.y - lo.y));
        let scale = fit * camera.zoom;
        let pan = camera.pan;
        let centre = rect.center() + pan;
        let rot = camera.rot;
        let project = |v: Vec3| {
            let w = apply(&rot, v);
            Pos2::new(centre.x + w[0] * scale, centre.y - w[1] * scale)
        };
        let light = normalize(LIGHT);
        // Brightness of a surface with normal `n`: some ambient, plus diffuse.
        let shade = |n: Vec3| 0.45 + 0.55 * dot(apply(&rot, n), light).max(0.0);

        let half = tube_length / 2.0;
        let around = |k: usize| k as f32 / FACETS as f32 * TAU;
        let normal = |a: f32| [0.0, a.cos(), a.sin()];
        let point = |x: f32, a: f32| [x, radius * a.cos(), radius * a.sin()];
        let rim = |x: f32| -> Vec<Pos2> { (0..FACETS).map(|k| project(point(x, around(k)))).collect() };
        let visible = visible_facets(&rot);
        let ends: [(f32, Vec3); 2] = [(-half, [-1.0, 0.0, 0.0]), (half, [1.0, 0.0, 0.0])];

        // The bare mandrel: only facets facing the viewer. For a convex tube
        // those never overlap, so no depth sorting is needed.
        let mut steel = Mesh::default();
        for k in (0..FACETS).filter(|k| visible[*k]) {
            let (a0, a1) = (around(k), around(k + 1));
            let corners = [(-half, a0), (half, a0), (half, a1), (-half, a1)];
            push_quad(
                &mut steel,
                corners.map(|(x, a)| Vertex {
                    pos: project(point(x, a)),
                    uv: WHITE_UV,
                    color: scaled(STEEL, shade(normal(a))),
                }),
            );
        }
        painter.add(Shape::mesh(steel));
        // An end facing the viewer is a filled disc.
        for (x, n) in ends {
            if apply(&rot, n)[2] > 0.0 {
                painter.add(Shape::convex_polygon(rim(x), scaled(STEEL, shade(n) * 0.9), Stroke::NONE));
            }
        }

        // The tow paths, from the GPU.
        if let Some(paths) = &self.paths {
            let tow_px = tp.tow_width * scale * ui.ctx().pixels_per_point();
            let (half_width, core, edge) = if show.thin_lines {
                ((THIN_LINE / 2.0 / scale).min(tp.tow_width / 2.0), 2.0, 1.0)
            } else {
                // Tows under a couple of pixels wide lose their darker edge,
                // which would only shimmer.
                let edge_shown = ((tow_px - 1.5) / 2.5).clamp(0.0, 1.0);
                // A hair wider than the tow (1%, and a third of a pixel), so
                // tows that touch also touch on screen: no cracks where
                // packing or older drivers round an edge inwards.
                let half_width = tp.tow_width / 2.0 * 1.01 + 0.35 / (scale * ui.ctx().pixels_per_point());
                (half_width, TOW_CORE, 1.0 - edge_shown * (1.0 - TOW_EDGE))
            };
            let to_clip = Vec2::new(2.0 / rect.width(), 2.0 / rect.height());
            let frame = Frame3d {
                rot,
                scale: [scale * to_clip.x, scale * to_clip.y],
                offset: [pan.x * to_clip.x, -pan.y * to_clip.y],
                light,
                x_min: paths.x_min - length / 2.0,
                x_span: paths.x_span,
                radius,
                half_width,
                core,
                // About half a pixel either side of where the edge starts.
                soft: (1.0 / tow_px.max(1.0)).clamp(0.01, 0.3),
                edge,
                layers: paths
                    .layers
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| show.layers.get(*i).copied().unwrap_or(true))
                    .map(|(i, &(first, count))| {
                        let c = layer_color(i);
                        (first, count, [c.r(), c.g(), c.b()].map(|v| v as f32 / 255.0))
                    })
                    .collect(),
            };
            painter.add(self.gl.callback(ui.ctx(), rect, frame));
        }

        // The outline over it all, which also smooths the tows' silhouette:
        // the rim of an end facing the viewer, else the near half of it, and
        // the two long edges.
        let outline = Stroke::new(1.0, p.card_stroke);
        for (x, n) in ends {
            let rim = rim(x);
            if apply(&rot, n)[2] > 0.0 {
                painter.add(Shape::closed_line(rim, outline));
            } else {
                for k in (0..FACETS).filter(|k| visible[*k]) {
                    painter.line_segment([rim[k], rim[(k + 1) % FACETS]], outline);
                }
            }
        }
        for k in 0..FACETS {
            if visible[k] != visible[(k + FACETS - 1) % FACETS] {
                let a = around(k);
                painter.line_segment([project(point(-half, a)), project(point(half, a))], outline);
            }
        }

        // Where the running job is, when that side faces the viewer.
        if let Some((mx, mdeg)) = show.marker {
            let a = (mdeg.rem_euclid(360.0) as f32).to_radians();
            if apply(&rot, normal(a))[2] > 0.0 {
                let at = project(point(mx as f32 - length / 2.0, a));
                painter.circle_stroke(at, 9.0, Stroke::new(5.0, Color32::from_black_alpha(140)));
                painter.circle_stroke(at, 9.0, Stroke::new(2.5, Color32::WHITE));
                painter.circle_filled(at, 3.0, p.accent);
            }
        }

        let small = TextStyle::Small.resolve(ui.style());
        if let Some(error) = self.gl.error() {
            let text = format!("The tow paths can't be drawn on this computer (OpenGL): {error}");
            let galley = painter.layout(text, small.clone(), theme::WARN_AMBER, rect.width() - 16.0);
            painter.galley(rect.left_top() + Vec2::splat(8.0), galley, theme::WARN_AMBER);
        } else if self.paths.is_none() {
            painter.text(
                rect.center_top() + Vec2::new(0.0, 10.0),
                Align2::CENTER_TOP,
                "Laying out the tow paths…",
                small.clone(),
                p.text_dim,
            );
        }
        painter.text(
            rect.left_bottom() + Vec2::new(8.0, -6.0),
            Align2::LEFT_BOTTOM,
            "Drag to turn · scroll to zoom · right-drag to move · double-click to reset",
            small,
            p.text_dim,
        );
    }
}

/// Which facets around the tube face the viewer.
fn visible_facets(rot: &Mat3) -> [bool; FACETS] {
    std::array::from_fn(|k| {
        let mid = (k as f32 + 0.5) / FACETS as f32 * TAU;
        apply(rot, [0.0, mid.cos(), mid.sin()])[2] > 0.0
    })
}

/// Screen-space (y down) bounding box of the tube in a pose, before scaling.
fn projected_bounds(rot: &Mat3, length: f32, radius: f32) -> (Pos2, Pos2) {
    let (mut lo, mut hi) = (Pos2::new(f32::MAX, f32::MAX), Pos2::new(f32::MIN, f32::MIN));
    for x in [-length / 2.0, length / 2.0] {
        for k in 0..FACETS {
            let a = k as f32 / FACETS as f32 * TAU;
            let w = apply(rot, [x, radius * a.cos(), radius * a.sin()]);
            let s = Pos2::new(w[0], -w[1]);
            lo = lo.min(s);
            hi = hi.max(s);
        }
    }
    (lo, hi)
}

fn push_quad(mesh: &mut Mesh, quad: [Vertex; 4]) {
    let i = mesh.vertices.len() as u32;
    mesh.vertices.extend(quad);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}

/// `c` with its colour channels scaled by `k` (0..=1).
fn scaled(c: Color32, k: f32) -> Color32 {
    let f = |v: u8| (v as f32 * k.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

// -------------------------------------------------------------- 3D maths

fn rot_x(a: f32) -> Mat3 {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rot_y(a: f32) -> Mat3 {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn apply(m: &Mat3, v: Vec3) -> Vec3 {
    std::array::from_fn(|i| dot(m[i], v))
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: Vec3) -> Vec3 {
    let len = dot(v, v).sqrt().max(1e-9);
    v.map(|x| x / len)
}

/// Undo the rounding drift of many small turns: make the rows unit length
/// and at right angles again (Gram-Schmidt).
fn orthonormalize(m: Mat3) -> Mat3 {
    let x = normalize(m[0]);
    let y0 = m[1];
    let y = normalize(std::array::from_fn(|i| y0[i] - dot(y0, x) * x[i]));
    let z = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    [x, y, z]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_rotation(m: &Mat3) -> bool {
        (0..3).all(|i| (0..3).all(|j| {
            let want = if i == j { 1.0 } else { 0.0 };
            (dot(m[i], m[j]) - want).abs() < 1e-4
        }))
    }

    /// Thousands of small drags still leave a clean rotation.
    #[test]
    fn turning_stays_a_rotation() {
        let mut rot = start_pose();
        for i in 0..5000 {
            let d = (i as f32 * 0.37).sin() * 3.0;
            rot = orthonormalize(mul(&mul(&rot_x(d * TURN_PER_POINT), &rot_y(-d * TURN_PER_POINT)), &rot));
        }
        assert!(is_rotation(&rot));
        assert!(is_rotation(&start_pose()));
    }

    /// About half the tube faces the viewer, as one unbroken run of facets.
    #[test]
    fn half_the_tube_faces_the_viewer() {
        let visible = visible_facets(&start_pose());
        let count = visible.iter().filter(|v| **v).count();
        assert!((FACETS / 2 - 2..=FACETS / 2 + 2).contains(&count), "{count} visible");
        let edges = (0..FACETS).filter(|k| visible[*k] != visible[(k + 1) % FACETS]).count();
        assert_eq!(edges, 2, "visible facets should form one run");
    }

    /// The starting pose is wider than tall for a long tube, so it fills
    /// a wide view.
    #[test]
    fn long_tube_projects_wide() {
        let (lo, hi) = projected_bounds(&start_pose(), 940.0, 35.0);
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        assert!(w > 3.0 * h, "{w} x {h}");
        assert!(w <= 940.0 + 70.0);
    }
}
