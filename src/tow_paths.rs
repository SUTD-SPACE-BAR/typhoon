//! Tow paths for the 3D view: every run of tow as a ribbon at its true width,
//! packed for the GPU (`tow_gl` draws them).
//!
//! Ribbons are laid out on the unrolled mandrel (mm along it and mm round
//! it), where every run of tow is a straight line, and are only wrapped round
//! the tube by the vertex shader. Each layer is one triangle strip, so showing
//! a range of layers is just drawing a range of vertices: nothing is rebuilt.

use crate::preview::{Run, Toolpath};

/// Largest turn of the mandrel between two points along a ribbon. A hoop
/// move turns it 72° for each mm of carriage travel, which would look
/// polygonal on the tube.
pub const STEP_DEGREES: f64 = 7.5;
/// The step grows to keep a wind under this many vertices (12 MB).
pub const MAX_VERTICES: usize = 1_500_000;
/// Bytes per vertex: X and the angle as 16-bit fractions, then the offset
/// from the tow's middle and the side as signed bytes (`a_pos` and `a_shape`
/// in `tow_gl`'s vertex shader).
pub const VERTEX_BYTES: usize = 8;
/// Corners turning more than 120° (cosine below this) are drawn as two
/// butt-ended ribbons instead of a mitre, which keeps a mitre at most two
/// half widths long.
const MITRE_MIN_COS: f64 = -0.5;

/// A wind's tow paths, ready for the GPU.
pub struct TowMesh {
    /// `VERTEX_BYTES` per vertex.
    pub bytes: Vec<u8>,
    /// Each layer's triangle strip: (first vertex, vertex count).
    pub layers: Vec<(u32, u32)>,
    /// The X (mm) that packs to 0, and the span that packs to 65535.
    pub x_min: f32,
    pub x_span: f32,
}

#[cfg(test)]
impl TowMesh {
    fn vertex_count(&self) -> usize {
        self.bytes.len() / VERTEX_BYTES
    }
}

/// Lay every run of `tp` out as a ribbon one tow wide.
pub fn build(tp: &Toolpath) -> TowMesh {
    let runs = &tp.runs;
    let mm_per_degree = tp.circumference as f64 / 360.0;
    let (x_min, x_max) = runs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), r| {
        (lo.min(r.x0).min(r.x1), hi.max(r.x0).max(r.x1))
    });
    let (x_min, x_span) = if runs.is_empty() {
        (0.0, 1.0)
    } else {
        (x_min, (x_max - x_min).max(1e-3))
    };
    let step = step_degrees(runs);

    let layer_count = tp.layers.len().max(runs.last().map_or(0, |r| r.layer as usize + 1));
    let mut layers = vec![(0, 0); layer_count];
    let mut packer = Packer { bytes: Vec::new(), x_min, x_span };
    // Runs come in G-code order, so each layer's are together.
    for group in runs.chunk_by(|a, b| a.layer == b.layer) {
        let first = packer.vertices();
        strip(&mut packer, group, step, mm_per_degree);
        layers[group[0].layer as usize] = (first as u32, (packer.vertices() - first) as u32);
    }
    TowMesh {
        bytes: packer.bytes,
        layers,
        x_min: x_min as f32,
        x_span: x_span as f32,
    }
}

/// `STEP_DEGREES`, or coarser where that would make more than `MAX_VERTICES`.
fn step_degrees(runs: &[Run]) -> f64 {
    let degrees: f64 = runs.iter().map(|r| (r.a1 - r.a0).abs()).sum();
    // Two vertices a step, and each run can add up to ten more: a step
    // rounded up, a break, a fresh start.
    let room = MAX_VERTICES as f64 / 2.0 - 5.0 * runs.len() as f64;
    if room <= 0.0 {
        f64::INFINITY
    } else {
        STEP_DEGREES.max(degrees / room)
    }
}

/// One layer's runs as a single triangle strip: a left and a right edge
/// vertex for each point along the tow's middle, mitred at corners. Where the
/// path breaks off, or turns too sharply to mitre, the strip carries on
/// through zero-area triangles.
fn strip(packer: &mut Packer, runs: &[Run], step: f64, mm_per_degree: f64) {
    let mut writer = StripWriter { packer, last: None, join: false };
    // The middle of the tow as points (mm, degrees), one unbroken line at a time.
    let mut line: Vec<(f64, f64)> = Vec::new();
    for r in runs {
        let start = (r.x0, r.a0);
        if line.last().is_some_and(|&p| !same_point(p, start)) {
            ribbon(&mut writer, &line, mm_per_degree);
            line.clear();
        }
        if line.is_empty() {
            line.push(start);
        }
        let n = ((r.a1 - r.a0).abs() / step).ceil().max(1.0) as usize;
        for k in 1..=n {
            let t = k as f64 / n as f64;
            line.push((r.x0 + (r.x1 - r.x0) * t, r.a0 + (r.a1 - r.a0) * t));
        }
    }
    ribbon(&mut writer, &line, mm_per_degree);
}

/// Whether two points along the tow are the same, give or take rounding.
fn same_point(p: (f64, f64), q: (f64, f64)) -> bool {
    (p.0 - q.0).abs() <= 1e-6 * (1.0 + p.0.abs()) && (p.1 - q.1).abs() <= 1e-6 * (1.0 + p.1.abs())
}

/// The ribbon along one unbroken line of points (mm, degrees).
fn ribbon(w: &mut StripWriter, line: &[(f64, f64)], mm_per_degree: f64) {
    if line.len() < 2 {
        return;
    }
    // Which way each segment heads on the unrolled surface.
    let dirs: Vec<(f64, f64)> = line
        .windows(2)
        .map(|s| unit(s[1].0 - s[0].0, (s[1].1 - s[0].1) * mm_per_degree))
        .collect();
    w.begin();
    w.point(line[0], left(dirs[0]));
    for i in 1..line.len() - 1 {
        let (din, dout) = (dirs[i - 1], dirs[i]);
        if dot(din, dout) < MITRE_MIN_COS {
            // Too sharp to mitre: end here and start again the other way.
            w.point(line[i], left(din));
            w.begin();
            w.point(line[i], left(dout));
        } else {
            w.point(line[i], mitre(din, dout));
        }
    }
    w.point(line[line.len() - 1], left(dirs[dirs.len() - 1]));
}

/// The left edge's offset (in half widths) at a corner from heading `a` to
/// heading `b`: along the bisector, long enough to keep both edges a half
/// width out.
fn mitre(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (na, nb) = (left(a), left(b));
    let m = unit(na.0 + nb.0, na.1 + nb.1);
    let len = 1.0 / dot(m, na).max(0.5);
    (m.0 * len, m.1 * len)
}

/// A quarter turn anticlockwise: from a heading to its left edge.
fn left((x, s): (f64, f64)) -> (f64, f64) {
    (-s, x)
}

fn unit(x: f64, s: f64) -> (f64, f64) {
    let len = x.hypot(s);
    (x / len, s / len)
}

fn dot(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

/// Writes one layer's triangle strip, joining its pieces.
struct StripWriter<'a> {
    packer: &'a mut Packer,
    /// The last vertex written, repeated to join on the next piece.
    last: Option<[u8; VERTEX_BYTES]>,
    /// The next point starts a new piece.
    join: bool,
}

impl StripWriter<'_> {
    /// Start a new piece of ribbon, joined to the last by zero-area triangles.
    fn begin(&mut self) {
        self.join = self.last.is_some();
    }

    /// The ribbon's two edges across the point `at`; `offset` is the left
    /// edge's, in half widths.
    fn point(&mut self, (x, a): (f64, f64), offset: (f64, f64)) {
        let left = self.packer.pack(x, a, offset, 127);
        let right = self.packer.pack(x, a, (-offset.0, -offset.1), -127);
        if self.join {
            if let Some(last) = self.last {
                self.packer.push(last);
            }
            self.packer.push(left);
            self.join = false;
        }
        self.packer.push(left);
        self.packer.push(right);
        self.last = Some(right);
    }
}

struct Packer {
    bytes: Vec<u8>,
    x_min: f64,
    x_span: f64,
}

impl Packer {
    fn vertices(&self) -> usize {
        self.bytes.len() / VERTEX_BYTES
    }

    fn push(&mut self, vertex: [u8; VERTEX_BYTES]) {
        self.bytes.extend_from_slice(&vertex);
    }

    /// A vertex at the tow's middle (`x` mm, `a` degrees) plus `offset` half
    /// widths (at most 2 long); `side` is 127 on the left edge, -127 on the
    /// right.
    fn pack(&self, x: f64, a: f64, offset: (f64, f64), side: i8) -> [u8; VERTEX_BYTES] {
        let x = ((x - self.x_min) / self.x_span * 65535.0).round().clamp(0.0, 65535.0) as u16;
        let a = ((a / 360.0).rem_euclid(1.0) * 65535.0).round() as u16;
        // Rounded away from zero, so a tow is never packed narrower than it
        // is: tows that touch would otherwise show hairline cracks.
        let q = |v: f64| (v * 63.5).abs().ceil().min(127.0).copysign(v) as i8 as u8;
        let ([x0, x1], [a0, a1]) = (x.to_ne_bytes(), a.to_ne_bytes());
        [x0, x1, a0, a1, q(offset.0), q(offset.1), side as u8, 0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::parse_toolpath;

    /// A vertex unpacked: the tow's middle (mm, degrees), the offset from it
    /// in half widths, and the side.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Vertex {
        x: f64,
        a: f64,
        offset: (f64, f64),
        side: i8,
    }

    fn vertices(mesh: &TowMesh, (first, count): (u32, u32)) -> Vec<Vertex> {
        (first as usize..(first + count) as usize)
            .map(|i| {
                let b = &mesh.bytes[i * VERTEX_BYTES..(i + 1) * VERTEX_BYTES];
                let fraction = |lo: u8, hi: u8| u16::from_ne_bytes([lo, hi]) as f64 / 65535.0;
                let half_widths = |v: u8| v as i8 as f64 / 63.5;
                Vertex {
                    x: mesh.x_min as f64 + fraction(b[0], b[1]) * mesh.x_span as f64,
                    a: fraction(b[2], b[3]) * 360.0,
                    offset: (half_widths(b[4]), half_widths(b[5])),
                    side: b[6] as i8,
                }
            })
            .collect()
    }

    fn toolpath(gcode: &str) -> Toolpath {
        parse_toolpath(&gcode.replace("\r\n", "\n").lines().map(str::to_owned).collect::<Vec<_>>())
    }

    /// These runs (x0, a0, x1, a1) as one layer, on a tube 360 mm round (so
    /// a degree is a mm) with a 6 mm tow.
    fn by_hand(runs: &[(f64, f64, f64, f64)]) -> Toolpath {
        Toolpath {
            pieces: Vec::new(),
            runs: runs.iter().map(|&(x0, a0, x1, a1)| Run { x0, a0, x1, a1, layer: 0 }).collect(),
            length: 100.0,
            circumference: 360.0,
            tow_width: 6.0,
            layers: vec!["test".into()],
            offsets: Vec::new(),
        }
    }

    /// Degrees from `a` to `b` the short way round.
    fn apart(a: f64, b: f64) -> f64 {
        let d = (b - a).rem_euclid(360.0);
        d.min(360.0 - d)
    }

    #[test]
    fn layers_are_strips_in_order() {
        let mesh = build(&toolpath(include_str!("../tests/fixtures/mixed.gcode")));
        assert_eq!(mesh.layers.len(), 5);
        let mut next = 0;
        for &(first, count) in &mesh.layers {
            assert_eq!(first, next);
            assert!(count >= 4 && count % 2 == 0, "{count}");
            next = first + count;
        }
        assert_eq!(next as usize, mesh.vertex_count());
    }

    /// Points along a ribbon are at most a step apart, sit on the wind, and
    /// mitres stay short.
    #[test]
    fn ribbons_follow_the_runs() {
        let tp = toolpath(include_str!("../tests/fixtures/mixed.gcode"));
        let mesh = build(&tp);
        for &range in &mesh.layers {
            let all = vertices(&mesh, range);
            let left: Vec<&Vertex> = all.iter().filter(|v| v.side > 0).collect();
            for w in left.windows(2) {
                assert!(apart(w[0].a, w[1].a) <= STEP_DEGREES + 0.01, "{w:?}");
            }
            for v in &all {
                assert!((-0.01..=tp.length as f64 + 0.01).contains(&v.x), "{v:?}");
                assert!(v.offset.0.hypot(v.offset.1) <= 2.0 + 0.02, "{v:?}");
            }
        }
    }

    /// Across a straight run the edges are a tow width apart, square to it.
    #[test]
    fn ribbons_are_a_tow_wide() {
        // Along the tube, then a 45° helix (a mm round for each mm along).
        let mesh = build(&by_hand(&[(0.0, 0.0, 50.0, 0.0), (50.0, 0.0, 100.0, 50.0)]));
        let all = vertices(&mesh, mesh.layers[0]);
        let (l, r) = (all[0], all[1]);
        assert_eq!((l.side, r.side), (127, -127));
        assert!(l.offset.0.abs() < 0.02 && (l.offset.1 - 1.0).abs() < 0.02, "{l:?}");
        assert!(r.offset.0.abs() < 0.02 && (r.offset.1 + 1.0).abs() < 0.02, "{r:?}");

        let l = all[all.len() - 2];
        let (ox, oy) = (l.offset.0 * 3.0, l.offset.1 * 3.0);
        assert!((ox.hypot(oy) - 3.0).abs() < 0.05, "{l:?}");
        assert!((ox + oy).abs() < 0.05, "{l:?}");
    }

    /// Whichever way a tow heads, packing never pulls its edges in, so tows
    /// that touch don't show cracks between them.
    #[test]
    fn edges_are_never_packed_inwards() {
        for k in 0..72 {
            let heading = (k as f64 * 5.0 + 0.3).to_radians();
            let (dx, ds) = (40.0 * heading.cos(), 40.0 * heading.sin());
            let mesh = build(&by_hand(&[(50.0, 100.0, 50.0 + dx, 100.0 + ds)]));
            for v in vertices(&mesh, mesh.layers[0]) {
                let len = v.offset.0.hypot(v.offset.1);
                assert!(len >= 1.0, "{len} half widths heading {}°: {v:?}", k * 5);
            }
        }
    }

    /// A path that doubles back can't be mitred: the strip breaks there,
    /// through zero-area triangles.
    #[test]
    fn sharp_turns_break_the_strip() {
        let mesh = build(&by_hand(&[(10.0, 0.0, 10.0, 90.0), (10.0, 90.0, 10.0, 30.0)]));
        let all = vertices(&mesh, mesh.layers[0]);
        let repeats = all.windows(2).filter(|w| w[0] == w[1]).count();
        assert_eq!(repeats, 2, "{all:?}");
        for v in &all {
            assert!(v.offset.0.hypot(v.offset.1) <= 1.02, "{v:?}");
        }
    }

    /// However long the wind, the step grows to stay under the budget.
    #[test]
    fn stays_under_the_vertex_budget() {
        let mesh = build(&by_hand(&[(0.0, 0.0, 100.0, 1e9)]));
        assert!(mesh.vertex_count() <= MAX_VERTICES, "{}", mesh.vertex_count());
        assert!(mesh.vertex_count() > MAX_VERTICES / 2, "{}", mesh.vertex_count());
    }

    /// About two vertices for every 7.5° the mandrel turns.
    #[test]
    fn two_vertices_a_step() {
        let tp = toolpath(include_str!("../tests/fixtures/hoop.gcode"));
        let degrees: f64 = tp.runs.iter().map(|r| (r.a1 - r.a0).abs()).sum();
        let ideal = 2.0 * degrees / STEP_DEGREES;
        let got = build(&tp).vertex_count() as f64;
        assert!(got >= ideal && got < ideal * 1.1 + 40.0, "{got} vertices for {ideal}");
    }
}
