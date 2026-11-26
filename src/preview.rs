//! Unwrapped-mandrel preview of the generated G-code, like cyclone's `plot`
//! command but interactive, with an optional 3D view of the tow paths
//! (`tube3d`) and a layer range slider for both.
//!
//! The mandrel surface is cut along its length and laid flat: X is carriage
//! position (mm) and Y is the distance around the mandrel. Unlike cyclone's
//! plot (which uses degrees as pixels), Y is in mm of surface arc, so wind
//! angles on screen are the real angles. Tows are drawn at their real width,
//! each with a darker edge so crossings stay visible, coloured by layer.
//!
//! Angles are where the mandrel really is, so each layer sits where it's
//! wound even though cyclone re-zeroes Y between layers (see [`Run`]).

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use eframe::glow;
use resvg::tiny_skia;

use crate::layer_slider;
use crate::theme::{self, Palette};
use crate::tow_gl::TowGl;
use crate::tow_paths::{self, TowMesh};
use crate::tube3d::{Show, TubeView};

/// Layer colours (Space Bar Red first), cycled for more layers.
const LAYER_COLORS: [Color32; 8] = [
    Color32::from_rgb(0xEF, 0x33, 0x40),
    Color32::from_rgb(0xF2, 0xA9, 0x00),
    Color32::from_rgb(0x2B, 0xB3, 0xA6),
    Color32::from_rgb(0x3B, 0x82, 0xF6),
    Color32::from_rgb(0x8B, 0x5C, 0xF6),
    Color32::from_rgb(0x4C, 0xAF, 0x50),
    Color32::from_rgb(0xEC, 0x48, 0x99),
    Color32::from_rgb(0xF9, 0x73, 0x16),
];

pub fn layer_color(i: usize) -> Color32 {
    LAYER_COLORS[i % LAYER_COLORS.len()]
}

/// Largest side of the rendered preview image, in pixels.
const MAX_IMAGE_SIDE: usize = 3072;

// ---------------------------------------------------------------- toolpath

/// One straight piece of tow on the unwrapped surface, already split at the
/// 0°/360° seam. Coordinates are mm: x along the mandrel, y around it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub layer: u16,
}

/// A straight run of tow, from (x0 mm, a0°) to (x1, a1): where the carriage
/// and mandrel really are, counted from the start of the G-code. A G92 renames
/// the current position without moving anything, so it doesn't change these;
/// cyclone re-zeroes Y between layers, and not always by whole turns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    pub x0: f64,
    pub a0: f64,
    pub x1: f64,
    pub a1: f64,
    pub layer: u16,
}

#[derive(Clone, Debug)]
pub struct Toolpath {
    /// The runs, cut at the 0°/360° seam, for the unwrapped view.
    pub pieces: Vec<Piece>,
    /// The path as straight runs (the planner's ~1 mm moves joined back
    /// together), for the 3D view.
    pub runs: Vec<Run>,
    /// Mandrel length covered by the plot (wind length), mm.
    pub length: f32,
    /// Distance around the mandrel (circumference), mm.
    pub circumference: f32,
    pub tow_width: f32,
    /// Label for each layer, e.g. "helical".
    pub layers: Vec<String>,
    /// Each G92 that renamed the position: (line index, X offset mm, angle
    /// offset °). From the line after it, real position = G-code position +
    /// offset.
    pub offsets: Vec<(usize, f64, f64)>,
}

impl Toolpath {
    /// The real position (mm, degrees) of a G-code position, once lines
    /// `..sent` have been sent (so their G92s apply).
    pub fn real_position(&self, x: f64, y: f64, sent: usize) -> (f64, f64) {
        let applied = self.offsets.partition_point(|(line, ..)| *line < sent);
        match applied.checked_sub(1).map(|i| self.offsets[i]) {
            Some((_, dx, da)) => (x + dx, y + da),
            None => (x, y),
        }
    }
}

/// Rebuild the toolpath from G-code: follows G0/G1 moves and G92 resets,
/// and uses cyclone's `; Parameters {...}` header and `; Layer N of M: type`
/// comments for the mandrel size, tow width and layer boundaries.
pub fn parse_toolpath(gcode: &[String]) -> Toolpath {
    let mut diameter: Option<f64> = None;
    let mut wind_length: Option<f64> = None;
    let mut tow_width: Option<f64> = None;
    let mut layers: Vec<String> = Vec::new();
    let mut current_layer: Option<u16> = None;

    // Where the machine is in G-code coordinates, and what to add to that
    // for where it really is.
    let (mut x, mut y) = (0.0_f64, 0.0_f64);
    let (mut dx, mut da) = (0.0_f64, 0.0_f64);
    let mut offsets = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    let mut max_x: f64 = 0.0;

    for (index, line) in gcode.iter().enumerate() {
        let line = line.trim();
        if let Some(comment) = line.strip_prefix(';') {
            let comment = comment.trim();
            if let Some(json) = comment.strip_prefix("Parameters ") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
                    diameter = v["mandrel"]["diameter"].as_f64();
                    wind_length = v["mandrel"]["windLength"].as_f64();
                    tow_width = v["tow"]["width"].as_f64();
                }
            } else if let Some(rest) = comment.strip_prefix("Layer ") {
                // "Layer 2 of 5: helical"
                if let Some((_, kind)) = rest.split_once(": ") {
                    layers.push(kind.trim().to_string());
                    current_layer = Some((layers.len() - 1) as u16);
                }
            }
            continue;
        }
        let code = line.split(';').next().unwrap_or("").trim();
        let mut words = code.split_whitespace();
        let Some(cmd) = words.next() else { continue };
        let (mut nx, mut ny) = (None, None);
        for w in words {
            let (axis, value) = w.split_at(1);
            let Ok(v) = value.parse::<f64>() else { continue };
            match axis {
                "X" | "x" => nx = Some(v),
                "Y" | "y" => ny = Some(v),
                _ => {}
            }
        }
        match cmd {
            "G0" | "G1" | "G00" | "G01" => {
                let (tx, ty) = (nx.unwrap_or(x), ny.unwrap_or(y));
                if let Some(layer) = current_layer
                    && (tx != x || ty != y) {
                        let real = Run { x0: x + dx, a0: y + da, x1: tx + dx, a1: ty + da, layer };
                        push_move(&mut runs, real);
                    }
                x = tx;
                y = ty;
                max_x = max_x.max(x + dx);
            }
            // Rename the current position without moving.
            "G92" => {
                let (tx, ty) = (nx.unwrap_or(x), ny.unwrap_or(y));
                if tx != x || ty != y {
                    dx += x - tx;
                    da += y - ty;
                    offsets.push((index, dx, da));
                }
                x = tx;
                y = ty;
            }
            _ => {}
        }
    }

    let length = wind_length.unwrap_or(max_x).max(1.0);
    // Without a header the mandrel size is unknown: plot degrees as mm.
    let circumference = diameter.map(|d| std::f64::consts::PI * d).unwrap_or(360.0).max(1.0);
    let tow_width = tow_width.unwrap_or(length / 100.0).max(0.01);
    let mm_per_degree = circumference / 360.0;

    let mut pieces = Vec::with_capacity(runs.len() * 2);
    for run in &runs {
        split_at_seam(run.x0, run.a0, run.x1, run.a1, |(ax, ad), (bx, bd)| {
            pieces.push(Piece {
                x0: ax as f32,
                y0: (ad * mm_per_degree) as f32,
                x1: bx as f32,
                y1: (bd * mm_per_degree) as f32,
                layer: run.layer,
            });
        });
    }

    Toolpath {
        pieces,
        runs,
        length: length as f32,
        circumference: circumference as f32,
        tow_width: tow_width as f32,
        layers,
        offsets,
    }
}

/// Add a move to the path, extending the last run when the move carries
/// straight on from it (the planner splits each move into ~1 mm pieces).
fn push_move(runs: &mut Vec<Run>, m: Run) {
    if let Some(last) = runs.last_mut()
        && last.layer == m.layer
        && carries_on(last, &m)
    {
        last.x1 = m.x1;
        last.a1 = m.a1;
    } else {
        runs.push(m);
    }
}

/// Whether `next` heads the same way as `run`, give or take the G-code's
/// rounding. Moves always start where the last one ended.
fn carries_on(run: &Run, next: &Run) -> bool {
    let (ax, aa) = (run.x1 - run.x0, run.a1 - run.a0);
    let (bx, ba) = (next.x1 - next.x0, next.a1 - next.a0);
    let dot = ax * bx + aa * ba;
    let cross = ax * ba - aa * bx;
    dot > 0.0 && cross.abs() <= 1e-5 * ax.hypot(aa) * bx.hypot(ba)
}

/// Split a move from (x0, d0°) to (x1, d1°) where it crosses a multiple of
/// 360°, giving pieces in 0..=360° local coordinates.
fn split_at_seam(
    x0: f64,
    d0: f64,
    x1: f64,
    d1: f64,
    mut emit: impl FnMut((f64, f64), (f64, f64)),
) {
    if d0 == d1 {
        let local = d0.rem_euclid(360.0);
        emit((x0, local), (x1, local));
        return;
    }
    let up = d1 > d0;
    let (mut xa, mut da) = (x0, d0);
    // Safety bound: a move cannot cross more seams than this.
    for _ in 0..100_000 {
        // The 360° band containing `da`, leaning in the direction of travel.
        let base = if up {
            (da / 360.0).floor() * 360.0
        } else {
            (da / 360.0).ceil() * 360.0 - 360.0
        };
        let boundary = if up { base + 360.0 } else { base };
        let ends_here = if up { d1 <= boundary } else { d1 >= boundary };
        let (xe, de) = if ends_here {
            (x1, d1)
        } else {
            (x0 + (boundary - d0) / (d1 - d0) * (x1 - x0), boundary)
        };
        emit((xa, da - base), (xe, de - base));
        if ends_here {
            return;
        }
        xa = xe;
        da = de;
    }
}

// --------------------------------------------------------------- rendering

/// Draw the toolpath into an RGBA image covering `length` x `circumference`.
pub fn render(tp: &Toolpath, visible: &[bool], max_side: usize) -> ColorImage {
    let aspect = tp.circumference / tp.length;
    let (w, h) = if aspect <= 1.0 {
        (max_side, ((max_side as f32 * aspect).round() as usize).max(8))
    } else {
        (((max_side as f32 / aspect).round() as usize).max(8), max_side)
    };
    let mut pixmap = tiny_skia::Pixmap::new(w as u32, h as u32).expect("non-zero size");
    let scale = w as f32 / tp.length;
    let to_px = tiny_skia::Transform::from_scale(scale, scale);

    let tow_px = tp.tow_width;
    let edge_stroke = tiny_skia::Stroke {
        width: tow_px,
        line_cap: tiny_skia::LineCap::Butt,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    let core_stroke = tiny_skia::Stroke {
        width: tow_px * 0.72,
        ..edge_stroke.clone()
    };

    // Group consecutive, connected pieces of one layer into polylines, so each
    // pass is stroked as one band and later passes visibly cross earlier ones.
    let mut i = 0;
    let pieces = &tp.pieces;
    while i < pieces.len() {
        let layer = pieces[i].layer;
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(pieces[i].x0, pieces[i].y0);
        pb.line_to(pieces[i].x1, pieces[i].y1);
        let (mut min_y, mut max_y) = (pieces[i].y0.min(pieces[i].y1), pieces[i].y0.max(pieces[i].y1));
        let mut j = i + 1;
        while j < pieces.len()
            && pieces[j].layer == layer
            && pieces[j].x0 == pieces[j - 1].x1
            && pieces[j].y0 == pieces[j - 1].y1
        {
            pb.line_to(pieces[j].x1, pieces[j].y1);
            min_y = min_y.min(pieces[j].y0.min(pieces[j].y1));
            max_y = max_y.max(pieces[j].y0.max(pieces[j].y1));
            j += 1;
        }
        if visible.get(layer as usize).copied().unwrap_or(true)
            && let Some(path) = pb.finish() {
                let color = layer_color(layer as usize);
                let mut edge = tiny_skia::Paint::default();
                let e = color.gamma_multiply(0.45);
                edge.set_color_rgba8(e.r(), e.g(), e.b(), 255);
                edge.anti_alias = true;
                let mut core = tiny_skia::Paint::default();
                core.set_color_rgba8(color.r(), color.g(), color.b(), 255);
                core.anti_alias = true;

                // Draw copies one circumference up/down where the band
                // touches the seam, so tows wrap visually.
                let half = tp.tow_width / 2.0;
                let mut offsets = vec![0.0];
                if min_y < half {
                    offsets.push(tp.circumference);
                }
                if max_y > tp.circumference - half {
                    offsets.push(-tp.circumference);
                }
                for dy in offsets {
                    let t = to_px.pre_translate(0.0, dy);
                    pixmap.stroke_path(&path, &edge, &edge_stroke, t, None);
                    pixmap.stroke_path(&path, &core, &core_stroke, t, None);
                }
            }
        i = j;
    }

    ColorImage::from_rgba_premultiplied([w, h], pixmap.data())
}

// ---------------------------------------------------------------------- UI

/// Where a running job is: the target of the last line sent, in G-code
/// coordinates, once lines `..sent` have gone to the board.
#[derive(Clone, Copy, Debug)]
pub struct JobMarker {
    pub x: f64,
    pub y: f64,
    pub sent: usize,
}

/// What the background thread made: always the toolpath, plus whatever the
/// view on screen was missing.
struct Done {
    toolpath: Arc<Toolpath>,
    /// The unwrapped image, and which layers it shows.
    image: Option<(Vec<bool>, ColorImage)>,
    mesh: Option<TowMesh>,
}

struct Work {
    /// The G-code it's working on.
    source: Arc<Vec<String>>,
    rx: Receiver<Done>,
}

pub struct Preview {
    /// The G-code the current toolpath came from.
    source: Option<Arc<Vec<String>>>,
    toolpath: Option<Arc<Toolpath>>,
    texture: Option<TextureHandle>,
    /// Which layers `texture` shows.
    texture_layers: Vec<bool>,
    work: Option<Work>,
    /// The background thread died on this G-code (a bug): don't retry it.
    failed: bool,
    /// The layer chips: false for a layer switched off.
    chips: Vec<bool>,
    /// Lowest and highest layer shown, from the slider (inclusive).
    range: (usize, usize),
    /// 1.0 = whole mandrel fits the view.
    zoom: f32,
    /// World point (mm) at the centre of the view.
    center: Option<Pos2>,
    true_aspect: bool,
    /// 3D: thin centre lines instead of tows at their true width.
    thin_lines: bool,
    /// The 3D tube view, only while it's switched on.
    tube: Option<TubeView>,
    /// What closed 3D views left on the GPU, freed in `sync`.
    retired: Vec<TowGl>,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            source: None,
            toolpath: None,
            texture: None,
            texture_layers: Vec::new(),
            work: None,
            failed: false,
            chips: Vec::new(),
            range: (0, 0),
            zoom: 1.0,
            center: None,
            true_aspect: true,
            thin_lines: false,
            tube: None,
            retired: Vec::new(),
        }
    }
}

impl Preview {
    /// Showing the 3D tube rather than the unwrapped surface.
    pub fn is_3d(&self) -> bool {
        self.tube.is_some()
    }

    /// Keep the preview in step with the G-code to show. `gl` (the OpenGL
    /// context) frees what closed 3D views left on the GPU.
    pub fn sync(
        &mut self,
        ctx: &egui::Context,
        plan: Option<&Arc<Vec<String>>>,
        gl: Option<&glow::Context>,
    ) {
        match gl {
            Some(gl) => self.retired.drain(..).for_each(|closed| closed.destroy(gl)),
            // Not OpenGL: nothing was ever put on a GPU.
            None => self.retired.clear(),
        }
        self.pick_up(ctx);

        let changed = match (plan, &self.source) {
            (Some(p), Some(s)) => !Arc::ptr_eq(p, s),
            (None, None) => false,
            _ => true,
        };
        if changed {
            // New plan: show every layer and fit the view again.
            self.source = plan.cloned();
            self.toolpath = None;
            self.texture = None;
            self.texture_layers.clear();
            self.failed = false;
            self.chips.clear();
            self.range = (0, 0);
            self.zoom = 1.0;
            self.center = None;
            if let Some(tube) = &mut self.tube {
                tube.clear_paths();
            }
        }
        self.start_work(ctx);
    }

    /// Free everything the 3D view has on the GPU (on exit).
    pub fn free_gpu(&mut self, gl: &glow::Context) {
        self.retired.drain(..).for_each(|closed| closed.destroy(gl));
        if let Some(tube) = &self.tube {
            tube.gl().destroy(gl);
        }
    }

    /// Take in what the background thread finished, if it's still wanted.
    fn pick_up(&mut self, ctx: &egui::Context) {
        let Some(work) = &self.work else { return };
        let current = self.source.as_ref().is_some_and(|s| Arc::ptr_eq(s, &work.source));
        match work.rx.try_recv() {
            Ok(done) => {
                self.work = None;
                if current {
                    self.accept(ctx, done);
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.work = None;
                if current {
                    self.failed = true;
                }
            }
        }
    }

    fn accept(&mut self, ctx: &egui::Context, done: Done) {
        let layers = done.toolpath.layers.len();
        if self.chips.len() != layers {
            self.chips = vec![true; layers];
            self.range = (0, layers.saturating_sub(1));
        }
        if let Some((shown, image)) = done.image {
            self.texture = Some(ctx.load_texture("wind-preview", image, TextureOptions::LINEAR));
            self.texture_layers = shown;
        }
        if let (Some(mesh), Some(tube)) = (done.mesh, &mut self.tube) {
            tube.set_paths(&done.toolpath, mesh);
        }
        self.toolpath = Some(done.toolpath);
    }

    /// Which layers to draw: chip on, and inside the slider's range.
    fn shown(&self, layers: usize) -> Vec<bool> {
        let (lo, hi) = self.range;
        (0..layers)
            .map(|i| self.chips.get(i).copied().unwrap_or(true) && lo <= i && i <= hi)
            .collect()
    }

    /// Start the background thread on whatever is missing: the toolpath, the
    /// unwrapped image (2D) or the tow paths (3D). One job at a time; this
    /// runs every frame, so anything changed meanwhile is caught up on next.
    fn start_work(&mut self, ctx: &egui::Context) {
        if self.work.is_some() || self.failed {
            return;
        }
        let Some(source) = self.source.clone() else { return };
        let toolpath = self.toolpath.clone();
        // Until the toolpath is known, the image shows every layer.
        let layers = toolpath.as_ref().map(|tp| self.shown(tp.layers.len()));
        let want_image = self.tube.is_none()
            && (self.texture.is_none() || layers.as_ref().is_none_or(|l| *l != self.texture_layers));
        let want_mesh = self
            .tube
            .as_ref()
            .is_some_and(|tube| !toolpath.as_ref().is_some_and(|tp| tube.has_paths_for(tp)));
        if toolpath.is_some() && !want_image && !want_mesh {
            return;
        }

        let max_side = MAX_IMAGE_SIDE.min(ctx.input(|i| i.max_texture_side));
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let lines = source.clone();
        std::thread::spawn(move || {
            let toolpath = toolpath.unwrap_or_else(|| Arc::new(parse_toolpath(&lines)));
            let image = want_image.then(|| {
                let shown: Vec<bool> = (0..toolpath.layers.len())
                    .map(|i| layers.as_ref().and_then(|l| l.get(i).copied()).unwrap_or(true))
                    .collect();
                let image = render(&toolpath, &shown, max_side);
                (shown, image)
            });
            let mesh = want_mesh.then(|| tow_paths::build(&toolpath));
            let _ = tx.send(Done { toolpath, image, mesh });
            ctx.request_repaint();
        });
        self.work = Some(Work { source, rx });
    }

    /// `marker`: where a running job is.
    pub fn ui(&mut self, ui: &mut Ui, stale: bool, marker: Option<JobMarker>) {
        let p = theme::pal(ui.ctx());
        let Some(tp) = self.toolpath.clone() else {
            ui.centered_and_justified(|ui| {
                if self.failed {
                    ui.label(RichText::new("Couldn't draw a preview of this G-code.").color(p.text_dim));
                } else if self.source.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Drawing the preview…");
                    });
                } else {
                    ui.label(
                        RichText::new(
                            "Generate G-code in the Wind Designer (or load a file) to see the preview.",
                        )
                        .color(p.text_dim),
                    );
                }
            });
            return;
        };
        let layers = tp.layers.len();

        self.controls(ui, &tp, &p);
        if stale {
            ui.label(
                RichText::new("⚠ The wind changed since this G-code was generated.")
                    .color(theme::WARN_AMBER),
            );
        }

        // The view, with the layer slider down its right-hand side.
        let (area, _) =
            ui.allocate_exact_size(ui.available_size().max(Vec2::new(100.0, 80.0)), Sense::hover());
        let slider_width = if layers > 1 { layer_slider::WIDTH } else { 0.0 };
        let view = Rect::from_min_max(area.min, Pos2::new(area.max.x - slider_width, area.max.y));
        if layers > 1 {
            self.arrow_keys(ui, area, layers);
        }
        let shown = self.shown(layers);
        let marker = marker.map(|m| tp.real_position(m.x, m.y, m.sent));
        if let Some(tube) = &mut self.tube {
            let show = Show { layers: &shown, thin_lines: self.thin_lines, marker };
            tube.ui(ui, view, &tp, show, &p);
        } else {
            self.plot(ui, view, &tp, marker, &p);
        }
        if layers > 1 {
            let slider = Rect::from_min_max(Pos2::new(view.max.x, area.min.y), area.max);
            if layer_slider::show(ui, slider, &mut self.range, &tp.layers, &self.chips, &p) {
                ui.ctx().request_repaint();
            }
        }
        self.start_work(ui.ctx());
    }

    /// Layer chips, the 3D switch and the view's own buttons.
    fn controls(&mut self, ui: &mut Ui, tp: &Toolpath, p: &Palette) {
        let layers = tp.layers.len();
        // Past a handful of layers the chips keep to their numbers.
        let compact = layers > 6;
        let (lo, hi) = self.range;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Layers").strong());
            for (i, name) in tp.layers.iter().enumerate() {
                let color = layer_color(i);
                let on = self.chips.get(i).copied().unwrap_or(true);
                // Off by its chip: struck through. Off by the slider: faded.
                let in_range = (lo..=hi).contains(&i);
                let label = if compact {
                    format!("■ {}", i + 1)
                } else {
                    format!("■ {} {name}", i + 1)
                };
                let text = match (on, in_range) {
                    (true, true) => RichText::new(label).color(color),
                    (true, false) => RichText::new(label).color(color.gamma_multiply(0.45)),
                    (false, _) => RichText::new(label).color(p.text_dim).strikethrough(),
                };
                let chip = egui::Button::new(text)
                    .fill(if on && in_range { p.inset } else { Color32::TRANSPARENT })
                    .stroke(Stroke::new(1.0, match (on, in_range) {
                        (true, true) => color.gamma_multiply(0.7),
                        (true, false) => color.gamma_multiply(0.3),
                        (false, _) => p.card_stroke,
                    }));
                let mut hover = if compact {
                    format!("Layer {}: {name}. Show or hide it", i + 1)
                } else {
                    "Show or hide this layer".to_owned()
                };
                if on && !in_range {
                    hover.push_str(". The layer slider is hiding it for now.");
                }
                if ui.add(chip).on_hover_text(hover).clicked()
                    && let Some(v) = self.chips.get_mut(i) {
                        *v = !*v;
                    }
            }
            if ui.small_button("All").on_hover_text("Show every layer").clicked() {
                self.chips.iter_mut().for_each(|v| *v = true);
                self.range = (0, layers.saturating_sub(1));
            }
            ui.separator();
            let mut in_3d = self.tube.is_some();
            if ui
                .toggle_value(&mut in_3d, "3D view")
                .on_hover_text(
                    "Draw the tow paths on a 3D tube. Nothing 3D is made until this is on.",
                )
                .changed()
            {
                if in_3d {
                    self.tube = Some(TubeView::default());
                } else if let Some(tube) = self.tube.take() {
                    self.retired.push(tube.into_gl());
                }
            }
            if let Some(tube) = &mut self.tube {
                ui.toggle_value(&mut self.thin_lines, "Thin lines").on_hover_text(
                    "Draw each pass as a thin line along its middle instead of a tow at its \
                     true width, so the layers underneath show through",
                );
                if ui.small_button("Reset view").on_hover_text("Or double-click the tube").clicked() {
                    tube.reset_camera();
                }
            } else {
                ui.checkbox(&mut self.true_aspect, "True proportions")
                    .on_hover_text("Off: stretch to fill the view (angles look distorted)");
                if ui.small_button("Fit").on_hover_text("Reset zoom (or double-click the plot)").clicked() {
                    self.zoom = 1.0;
                    self.center = None;
                }
            }
            if self.work.is_some() {
                ui.spinner();
            }
        });
    }

    /// ↑/↓ move the slider's top handle while the pointer is over the view.
    fn arrow_keys(&mut self, ui: &mut Ui, area: Rect, layers: usize) {
        if !ui.rect_contains_pointer(area) || ui.memory(|m| m.focused().is_some()) {
            return;
        }
        let (up, down) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
            )
        });
        let (lo, hi) = &mut self.range;
        if up {
            *hi = (*hi + 1).min(layers - 1);
        }
        if down {
            *hi = hi.saturating_sub(1).max(*lo);
        }
    }

    /// The unwrapped mandrel in `outer`. `marker`: where a running job is
    /// (mm, degrees).
    fn plot(&mut self, ui: &mut Ui, outer: Rect, tp: &Toolpath, marker: Option<(f64, f64)>, p: &Palette) {
        // Plot area, leaving room for axis labels.
        let response = ui.interact(outer, ui.make_persistent_id("unwrapped_plot"), Sense::click_and_drag());
        let left_margin = 44.0;
        let bottom_margin = 34.0;
        let plot = Rect::from_min_max(
            outer.min + Vec2::new(left_margin, 6.0),
            outer.max - Vec2::new(8.0, bottom_margin),
        );
        if plot.width() < 20.0 || plot.height() < 20.0 {
            return;
        }

        let world = Vec2::new(tp.length, tp.circumference);
        let fit = Vec2::new(plot.width() / world.x, plot.height() / world.y);
        let base_scale = if self.true_aspect {
            Vec2::splat(fit.min_elem())
        } else {
            fit
        };
        let scale = base_scale * self.zoom;
        let mut center = self.center.unwrap_or(Pos2::new(world.x / 2.0, world.y / 2.0));

        // Interaction: wheel zooms about the cursor, drag pans, double-click fits.
        if let Some(hover) = response.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 && plot.contains(hover) {
                let before = to_world(hover, plot, center, scale);
                self.zoom = (self.zoom * (scroll * 0.0025).exp()).clamp(1.0, 60.0);
                let scale2 = base_scale * self.zoom;
                let after = to_world(hover, plot, center, scale2);
                center += before - after;
            }
        }
        if response.dragged() {
            let d = response.drag_delta();
            center -= Vec2::new(d.x / scale.x, d.y / scale.y);
        }
        if response.double_clicked() {
            self.zoom = 1.0;
            center = Pos2::new(world.x / 2.0, world.y / 2.0);
        }
        let scale = base_scale * self.zoom;
        // Keep at least part of the mandrel in view.
        center.x = center.x.clamp(0.0, world.x);
        center.y = center.y.clamp(0.0, world.y);
        self.center = Some(center);

        let painter = ui.painter_at(outer);
        let mandrel = Rect::from_min_max(
            to_screen(Pos2::ZERO, plot, center, scale),
            to_screen(Pos2::new(world.x, world.y), plot, center, scale),
        );
        let plot_painter = painter.with_clip_rect(plot);
        plot_painter.rect_filled(mandrel, CornerRadius::ZERO, p.well);
        if let Some(texture) = &self.texture {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            plot_painter.image(texture.id(), mandrel, uv, Color32::WHITE);
        }
        plot_painter.rect_stroke(
            mandrel,
            CornerRadius::ZERO,
            Stroke::new(1.0, p.card_stroke),
            egui::StrokeKind::Outside,
        );

        // Where the running job is.
        if let Some((mx, mdeg)) = marker {
            let wy = (mdeg.rem_euclid(360.0) / 360.0) as f32 * tp.circumference;
            let at = to_screen(Pos2::new(mx as f32, wy), plot, center, scale);
            if plot.expand(6.0).contains(at) {
                let ring = Stroke::new(2.5, Color32::WHITE);
                plot_painter.circle_stroke(at, 9.0, Stroke::new(5.0, Color32::from_black_alpha(140)));
                plot_painter.circle_stroke(at, 9.0, ring);
                plot_painter.circle_filled(at, 3.0, p.accent);
            }
        }

        // Axes: X in mm along the bottom, Y in degrees around the mandrel.
        let axis_color = p.text_dim;
        let font = egui::FontId::proportional(11.0);
        let visible_world = Rect::from_min_max(
            to_world(plot.min, plot, center, scale),
            to_world(plot.max, plot, center, scale),
        );
        let x_step = nice_step(visible_world.width(), (plot.width() / 80.0).max(2.0));
        let mut xv = (visible_world.min.x.max(0.0) / x_step).ceil() * x_step;
        while xv <= visible_world.max.x.min(world.x) + 1e-3 {
            let sx = to_screen(Pos2::new(xv, 0.0), plot, center, scale).x;
            painter.line_segment(
                [Pos2::new(sx, plot.max.y), Pos2::new(sx, plot.max.y + 4.0)],
                Stroke::new(1.0, axis_color),
            );
            painter.text(
                Pos2::new(sx, plot.max.y + 6.0),
                egui::Align2::CENTER_TOP,
                format_tick(xv),
                font.clone(),
                axis_color,
            );
            xv += x_step;
        }
        painter.text(
            Pos2::new(plot.center().x, outer.max.y),
            egui::Align2::CENTER_BOTTOM,
            "carriage X (mm)",
            font.clone(),
            axis_color,
        );
        let deg_per_mm = 360.0 / tp.circumference;
        let visible_deg = visible_world.height() * deg_per_mm;
        let y_step_deg = [15.0, 30.0, 45.0, 90.0]
            .into_iter()
            .find(|s| visible_deg / s <= (plot.height() / 40.0).max(2.0))
            .unwrap_or(90.0);
        let mut deg = ((visible_world.min.y.max(0.0) * deg_per_mm) / y_step_deg).ceil() * y_step_deg;
        while deg <= (visible_world.max.y.min(world.y) * deg_per_mm) + 1e-3 {
            let sy = to_screen(Pos2::new(0.0, deg / deg_per_mm), plot, center, scale).y;
            painter.line_segment(
                [Pos2::new(plot.min.x - 4.0, sy), Pos2::new(plot.min.x, sy)],
                Stroke::new(1.0, axis_color),
            );
            painter.text(
                Pos2::new(plot.min.x - 6.0, sy),
                egui::Align2::RIGHT_CENTER,
                format!("{deg:.0}°"),
                font.clone(),
                axis_color,
            );
            deg += y_step_deg;
        }

        // Cursor readout.
        if let Some(hover) = response.hover_pos().filter(|h| plot.contains(*h)) {
            let w = to_world(hover, plot, center, scale);
            if w.x >= 0.0 && w.x <= world.x && w.y >= 0.0 && w.y <= world.y {
                let text = format!("X {:.1} mm · {:.1}°", w.x, w.y * deg_per_mm);
                let galley = painter.layout_no_wrap(text, font.clone(), p.text);
                let pos = plot.right_top() + Vec2::new(-galley.size().x - 8.0, 6.0);
                let bg = Rect::from_min_size(pos - Vec2::splat(4.0), galley.size() + Vec2::splat(8.0));
                painter.rect_filled(bg, p.corner(4), p.card);
                painter.galley(pos, galley, p.text);
            }
        }
    }
}

fn to_screen(w: Pos2, plot: Rect, center: Pos2, scale: Vec2) -> Pos2 {
    plot.center() + Vec2::new((w.x - center.x) * scale.x, (w.y - center.y) * scale.y)
}

fn to_world(s: Pos2, plot: Rect, center: Pos2, scale: Vec2) -> Pos2 {
    center + Vec2::new((s.x - plot.center().x) / scale.x, (s.y - plot.center().y) / scale.y)
}

/// A "nice" tick spacing (1, 2 or 5 × 10ⁿ) giving about `target` ticks over `span`.
fn nice_step(span: f32, target: f32) -> f32 {
    let raw = (span / target).max(1e-6);
    let mag = 10f32.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

fn format_tick(v: f32) -> String {
    if (v - v.round()).abs() < 1e-3 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.replace("\r\n", "\n").lines().map(str::to_owned).collect()
    }

    #[test]
    fn seam_splitting() {
        let mut out = Vec::new();
        split_at_seam(0.0, 350.0, 20.0, 370.0, |a, b| out.push((a, b)));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], ((0.0, 350.0), (10.0, 360.0)));
        assert_eq!(out[1], ((10.0, 0.0), (20.0, 10.0)));

        out.clear();
        split_at_seam(0.0, 10.0, 0.0, -20.0, |a, b| out.push((a, b)));
        assert_eq!(out, vec![((0.0, 10.0), (0.0, 0.0)), ((0.0, 360.0), (0.0, 340.0))]);

        out.clear();
        // Exactly on a seam, moving up: starts at the bottom of the next band.
        split_at_seam(0.0, 360.0, 1.0, 370.0, |a, b| out.push((a, b)));
        assert_eq!(out, vec![((0.0, 0.0), (1.0, 10.0))]);
    }

    #[test]
    fn parses_cyclone_output() {
        let tp = parse_toolpath(&lines(include_str!("../tests/fixtures/mixed.gcode")));
        assert_eq!(tp.layers, vec!["hoop", "helical", "helical", "skip", "hoop"]);
        assert!((tp.length - 100.0).abs() < 1e-6);
        assert!((tp.circumference - std::f32::consts::PI * 50.8).abs() < 1e-3);
        assert_eq!(tp.tow_width, 5.0);
        assert!(!tp.pieces.is_empty());
        for p in &tp.pieces {
            for (x, y) in [(p.x0, p.y0), (p.x1, p.y1)] {
                assert!((-1e-3..=100.0 + 1e-3).contains(&x), "x {x}");
                assert!((-1e-3..=tp.circumference + 1e-3).contains(&y), "y {y}");
            }
        }
    }

    #[test]
    fn renders_something() {
        let tp = parse_toolpath(&lines(include_str!("../tests/fixtures/helical.gcode")));
        let image = render(&tp, &[true], 512);
        assert_eq!(image.size[0].max(image.size[1]), 512);
        let painted = image.pixels.iter().filter(|c| c.a() > 0).count();
        // A full helical layer covers most of the surface.
        assert!(painted > image.pixels.len() / 2, "{painted} of {}", image.pixels.len());

        let hidden = render(&tp, &[false], 512);
        assert!(hidden.pixels.iter().all(|c| c.a() == 0));
    }

    /// The same angle, give or take whole turns.
    fn same_angle(a: f64, b: f64) -> bool {
        let d = (a - b).rem_euclid(360.0);
        d < 1e-6 || d > 360.0 - 1e-6
    }

    fn first_run(tp: &Toolpath, layer: u16) -> (usize, Run) {
        let i = tp.runs.iter().position(|r| r.layer == layer).expect("layer has runs");
        (i, tp.runs[i])
    }

    /// In mixed.wind, layer 2 starts after a 540° near lock and a G92 Y0, so
    /// it really starts half a turn round; the 90° skip layer turns the next
    /// layer a further quarter. G-code coordinates would put both at 0°.
    #[test]
    fn layers_sit_where_they_are_wound() {
        let tp = parse_toolpath(&lines(include_str!("../tests/fixtures/mixed.gcode")));

        // One unbroken path: G92 renames positions, it doesn't move the tow.
        for w in tp.runs.windows(2) {
            assert!((w[0].x1 - w[1].x0).abs() < 1e-6 && (w[0].a1 - w[1].a0).abs() < 1e-6, "{w:?}");
        }

        let (i, lock) = first_run(&tp, 1);
        assert!(same_angle(lock.a0, 0.0), "{lock:?}");
        assert!((lock.a1 - lock.a0 - 540.0).abs() < 1e-6 && lock.x0 == lock.x1, "{lock:?}");
        assert!(same_angle(tp.runs[i + 1].a0, 180.0), "{:?}", tp.runs[i + 1]);

        let (_, next_helical) = first_run(&tp, 2);
        assert!(same_angle(next_helical.a0, 180.0), "{next_helical:?}");

        let (_, skip) = first_run(&tp, 3);
        assert!(same_angle(skip.a0, 180.0) && (skip.a1 - skip.a0 - 90.0).abs() < 1e-6, "{skip:?}");
        let (_, after_skip) = first_run(&tp, 4);
        assert!(same_angle(after_skip.a0, 270.0), "{after_skip:?}");
    }

    /// A running job reports G-code coordinates; the G92s sent so far turn
    /// them into real ones.
    #[test]
    fn job_positions_become_real() {
        let gcode = lines(include_str!("../tests/fixtures/mixed.gcode"));
        let tp = parse_toolpath(&gcode);
        assert_eq!(tp.real_position(5.0, 10.0, 0), (5.0, 10.0));

        let layer5 = gcode.iter().position(|l| l.starts_with("; Layer 5 of")).unwrap();
        let (x, a) = tp.real_position(0.0, 10.0, layer5);
        assert!(x.abs() < 1e-9 && same_angle(a, 280.0), "{x} {a}");
        // The skip layer's own G92 Y0 hasn't been sent yet one line earlier.
        let (_, a) = tp.real_position(0.0, 10.0, layer5 - 1);
        assert!(same_angle(a, 190.0), "{a}");
    }

    /// The planner's ~1 mm moves join back into straight runs.
    #[test]
    fn moves_join_into_runs() {
        let gcode = lines(include_str!("../tests/fixtures/helical.gcode"));
        let tp = parse_toolpath(&gcode);
        let moves = gcode.iter().filter(|l| l.starts_with("G0 X")).count();
        assert!(tp.runs.len() * 20 < moves, "{} runs from {moves} moves", tp.runs.len());
    }
}
