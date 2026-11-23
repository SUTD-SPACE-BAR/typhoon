//! Optional 3D view of the wound tube.
//!
//! The 2D preview already renders the whole wind as an image of the mandrel's
//! surface, unrolled flat. This view wraps that image around a shaded
//! cylinder, so it shows exactly the same tows and layer colours. It is about
//! 200 triangles, made only while the view is open and redrawn only while
//! it's being turned or zoomed, so it stays light on old laptops.

use std::f32::consts::TAU;

use eframe::egui::{
    Align2, Color32, Mesh, PointerButton, Pos2, Sense, Shape, Stroke, TextStyle, TextureId, Ui,
    Vec2,
    epaint::{Vertex, WHITE_UV},
};

use crate::preview::Toolpath;
use crate::theme::Palette;

/// Facets around the tube: enough for a smooth outline. The tows come from
/// the preview image, so this doesn't limit their detail.
const FACETS: usize = 96;
/// Bare mandrel, where no tow covers it.
const STEEL: Color32 = Color32::from_rgb(0xC4, 0xC9, 0xD1);
/// Where the light comes from, in view space: upper right, towards the
/// viewer, so the end the starting pose shows is lit too.
const LIGHT: Vec3 = [0.35, 0.6, 0.72];
/// Radians turned per point dragged.
const TURN_PER_POINT: f32 = 0.01;

/// x along the tube, y up, z towards the viewer.
type Vec3 = [f32; 3];
/// Rows of a 3 × 3 rotation.
type Mat3 = [[f32; 3]; 3];

pub struct TubeView {
    /// Turns the tube (model) into view space; changed by dragging.
    rot: Mat3,
    /// 1.0 = the tube fits the view in its starting pose.
    zoom: f32,
    /// The tube's centre, offset from the middle of the view, in points.
    pan: Vec2,
}

impl Default for TubeView {
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

impl TubeView {
    /// Draw the tube in the space left in `ui`. `texture` is the preview's
    /// unrolled image; `marker` is where a running job is (mm, degrees).
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        tp: &Toolpath,
        texture: Option<TextureId>,
        marker: Option<(f64, f64)>,
        p: &Palette,
    ) {
        let size = ui.available_size().max(Vec2::new(100.0, 80.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let length = tp.length.max(1e-3);
        let radius = (tp.circumference / TAU).max(1e-3);

        // Drag turns, right/middle-drag moves, the wheel zooms about the
        // cursor, double-click starts over.
        if response.dragged_by(PointerButton::Primary) {
            let d = response.drag_delta();
            let turn = mul(&rot_x(d.y * TURN_PER_POINT), &rot_y(d.x * TURN_PER_POINT));
            self.rot = orthonormalize(mul(&turn, &self.rot));
        }
        if response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle) {
            self.pan += response.drag_delta();
        }
        if let Some(hover) = response.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let before = self.zoom;
                self.zoom = (self.zoom * (scroll * 0.0025).exp()).clamp(0.3, 40.0);
                let at = hover - rect.center();
                self.pan = at - (at - self.pan) * (self.zoom / before);
            }
        }
        if response.double_clicked() {
            *self = Self::default();
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, p.corner(4), p.well);

        // Scale so the starting pose fills the view, and keep it while turning.
        let (lo, hi) = projected_bounds(&start_pose(), length, radius);
        let fit = 0.9 * (rect.width() / (hi.x - lo.x)).min(rect.height() / (hi.y - lo.y));
        let scale = fit * self.zoom;
        let centre = rect.center() + self.pan;
        let rot = self.rot;
        let project = |v: Vec3| {
            let w = apply(&rot, v);
            Pos2::new(centre.x + w[0] * scale, centre.y - w[1] * scale)
        };
        let light = normalize(LIGHT);
        // Brightness of a surface with normal `n`: some ambient, plus diffuse.
        let shade = |n: Vec3| 0.45 + 0.55 * dot(apply(&rot, n), light).max(0.0);

        let half = length / 2.0;
        let around = |k: usize| k as f32 / FACETS as f32 * TAU;
        let normal = |a: f32| [0.0, a.cos(), a.sin()];
        let point = |x: f32, a: f32| [x, radius * a.cos(), radius * a.sin()];
        let visible = visible_facets(&rot);

        // The bare mandrel, then the preview image over it (transparent where
        // no tow is). Only facets facing the viewer: for a convex tube those
        // never overlap, so no depth sorting is needed.
        let mut bare = Mesh::default();
        let mut wound = texture.map(Mesh::with_texture);
        for k in (0..FACETS).filter(|k| visible[*k]) {
            let (a0, a1) = (around(k), around(k + 1));
            let corners = [(-half, a0, 0.0), (half, a0, 1.0), (half, a1, 1.0), (-half, a1, 0.0)];
            let mut bare_quad = [Vertex::default(); 4];
            let mut wound_quad = [Vertex::default(); 4];
            for (i, (x, a, u)) in corners.into_iter().enumerate() {
                let pos = project(point(x, a));
                let light = shade(normal(a));
                bare_quad[i] = Vertex { pos, uv: WHITE_UV, color: scaled(STEEL, light) };
                let gray = (255.0 * light).round() as u8;
                wound_quad[i] = Vertex { pos, uv: Pos2::new(u, a / TAU), color: Color32::from_gray(gray) };
            }
            push_quad(&mut bare, bare_quad);
            if let Some(mesh) = &mut wound {
                push_quad(mesh, wound_quad);
            }
        }
        painter.add(Shape::mesh(bare));
        if let Some(mesh) = wound {
            painter.add(Shape::mesh(mesh));
        }

        // Ends: a filled disc when it faces the viewer, else just the near
        // half of its rim. Plus the two long edges of the silhouette.
        let outline = Stroke::new(1.0, p.card_stroke);
        for (x, n) in [(-half, [-1.0, 0.0, 0.0]), (half, [1.0, 0.0, 0.0])] {
            let rim: Vec<Pos2> = (0..FACETS).map(|k| project(point(x, around(k)))).collect();
            if apply(&rot, n)[2] > 0.0 {
                painter.add(Shape::convex_polygon(rim, scaled(STEEL, shade(n) * 0.9), outline));
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
        if let Some((mx, mdeg)) = marker {
            let a = (mdeg as f32).to_radians();
            if apply(&rot, normal(a))[2] > 0.0 {
                let at = project(point(mx as f32 - half, a));
                painter.circle_stroke(at, 9.0, Stroke::new(5.0, Color32::from_black_alpha(140)));
                painter.circle_stroke(at, 9.0, Stroke::new(2.5, Color32::WHITE));
                painter.circle_filled(at, 3.0, p.accent);
            }
        }

        painter.text(
            rect.left_bottom() + Vec2::new(8.0, -6.0),
            Align2::LEFT_BOTTOM,
            "Drag to turn · scroll to zoom · right-drag to move · double-click to reset",
            TextStyle::Small.resolve(ui.style()),
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
