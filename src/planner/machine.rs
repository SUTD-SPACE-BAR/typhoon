//! Port of cyclone's `WinderMachine`: emits G-code, splits moves into ~1 mm
//! carriage segments (Marlin can only pause between commands) and keeps a
//! running time and tow-length estimate.

use super::jsnum::{gcode_number, to_js_string};

/// Machine axes: X carriage (mm), Y mandrel (degrees), Z delivery head (degrees).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coord {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A move target where unspecified axes keep their current value.
/// Axes are always emitted in X, Y, Z order, which is the key order every
/// call site in cyclone uses.
#[derive(Clone, Copy, Debug, Default)]
pub struct Target {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
}

impl Target {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn x(mut self, v: f64) -> Self {
        self.x = Some(v);
        self
    }
    pub fn y(mut self, v: f64) -> Self {
        self.y = Some(v);
        self
    }
    pub fn z(mut self, v: f64) -> Self {
        self.z = Some(v);
        self
    }
    pub fn xyz(x: f64, y: f64, z: f64) -> Self {
        Self::new().x(x).y(y).z(z)
    }

    fn axes(&self) -> impl Iterator<Item = (Axis, f64)> {
        [(Axis::X, self.x), (Axis::Y, self.y), (Axis::Z, self.z)]
            .into_iter()
            .filter_map(|(a, v)| v.map(|v| (a, v)))
    }
}

impl From<Coord> for Target {
    fn from(c: Coord) -> Self {
        Self::xyz(c.x, c.y, c.z)
    }
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn letter(self) -> char {
        match self {
            Axis::X => 'X',
            Axis::Y => 'Y',
            Axis::Z => 'Z',
        }
    }
    fn get(self, c: &Coord) -> f64 {
        match self {
            Axis::X => c.x,
            Axis::Y => c.y,
            Axis::Z => c.z,
        }
    }
    fn set(self, c: &mut Coord, v: f64) {
        match self {
            Axis::X => c.x = v,
            Axis::Y => c.y = v,
            Axis::Z => c.z = v,
        }
    }
}

pub struct WinderMachine {
    verbose: bool,
    gcode: Vec<String>,
    feed_rate_mm_per_min: f64,
    total_time_s: f64,
    total_tow_length_mm: f64,
    last: Coord,
    mandrel_diameter: f64,
}

impl WinderMachine {
    pub fn new(mandrel_diameter: f64, verbose: bool) -> Self {
        Self {
            verbose,
            gcode: Vec::new(),
            feed_rate_mm_per_min: 0.0,
            total_time_s: 0.0,
            total_tow_length_mm: 0.0,
            last: Coord::default(),
            mandrel_diameter,
        }
    }

    pub fn into_gcode(self) -> Vec<String> {
        self.gcode
    }

    pub fn add_raw_gcode(&mut self, command: impl Into<String>) {
        self.gcode.push(command.into());
    }

    pub fn set_feed_rate(&mut self, feed_rate_mm_per_min: f64) {
        self.feed_rate_mm_per_min = feed_rate_mm_per_min;
        self.gcode.push(format!("G0 F{}", gcode_number(feed_rate_mm_per_min)));
    }

    pub fn insert_comment(&mut self, text: &str) {
        self.gcode.push(format!("; {text}"));
    }

    pub fn time_s(&self) -> f64 {
        self.total_time_s
    }

    pub fn tow_length_m(&self) -> f64 {
        self.total_tow_length_mm / 1000.0
    }

    pub fn move_to(&mut self, target: Target) {
        let end = Coord {
            x: target.x.unwrap_or(self.last.x),
            y: target.y.unwrap_or(self.last.y),
            z: target.z.unwrap_or(self.last.z),
        };
        if self.last.x == end.x {
            if self.verbose {
                let text = format!(
                    "Move from {} to {} as a simple move",
                    serialize_coord(&self.last),
                    serialize_coord(&end)
                );
                self.insert_comment(&text);
            }
            return self.move_segment(target);
        }
        // Split so each piece has ~1 mm of carriage travel.
        let segments = (self.last.x - end.x).abs().round() + 1.0;
        if self.verbose {
            let text = format!(
                "Move from {} to {} in {} segments",
                serialize_coord(&self.last),
                serialize_coord(&end),
                to_js_string(segments)
            );
            self.insert_comment(&text);
        }
        for point in interpolate(self.last, end, segments as usize) {
            self.move_segment(point.into());
        }
    }

    pub fn set_position(&mut self, target: Target) {
        let mut command = String::from("G92");
        for (axis, v) in target.axes() {
            command.push_str(&format!(" {}{}", axis.letter(), gcode_number(v)));
            axis.set(&mut self.last, v);
        }
        self.gcode.push(command);
    }

    /// Advance the mandrel to its next 0° position and zero all axes.
    pub fn zero_axes(&mut self, current_angle_degrees: f64) {
        self.set_position(Target::xyz(0.0, current_angle_degrees % 360.0, 0.0));
        self.move_to(Target::new().y(360.0));
        self.set_position(Target::new().y(0.0));
    }

    fn move_segment(&mut self, target: Target) {
        // Marlin treats degrees as mm when timing a move, so this does too.
        let mut distance_marlin_units_sq = 0.0;
        // Real tow length: mandrel arc plus carriage travel, ignoring the head.
        let mut tow_length_mm_sq = 0.0;
        let mut command = String::from("G0");
        for (axis, v) in target.axes() {
            command.push_str(&format!(" {}{}", axis.letter(), gcode_number(v)));

            let component = v - axis.get(&self.last);
            distance_marlin_units_sq += component * component;
            match axis {
                Axis::Y => {
                    let arc_mm =
                        component / 360.0 * self.mandrel_diameter * std::f64::consts::PI;
                    tow_length_mm_sq += arc_mm * arc_mm;
                }
                Axis::X => tow_length_mm_sq += component * component,
                Axis::Z => {}
            }
            axis.set(&mut self.last, v);
        }
        // Assumes instantaneous acceleration, like cyclone.
        self.total_time_s +=
            libm::pow(distance_marlin_units_sq, 0.5) / self.feed_rate_mm_per_min * 60.0;
        self.total_tow_length_mm += libm::pow(tow_length_mm_sq, 0.5);
        self.gcode.push(command);
    }
}

/// `steps` evenly spaced points from `start` to `end`, both included
/// (just `[end]` when `steps == 1`), using cyclone's exact arithmetic.
fn interpolate(start: Coord, end: Coord, steps: usize) -> Vec<Coord> {
    if steps <= 1 {
        return vec![end];
    }
    let n = (steps - 1) as f64;
    let dx = (end.x - start.x) / n;
    let dy = (end.y - start.y) / n;
    let dz = (end.z - start.z) / n;
    (0..steps)
        .map(|i| {
            let i = i as f64;
            Coord {
                x: start.x + i * dx,
                y: start.y + i * dy,
                z: start.z + i * dz,
            }
        })
        .collect()
}

fn serialize_coord(c: &Coord) -> String {
    format!(
        "{{{} {} {}}}",
        to_js_string(c.x),
        to_js_string(c.y),
        to_js_string(c.z)
    )
}
