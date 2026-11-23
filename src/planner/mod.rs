//! Filament winding toolpath planner: a faithful Rust port of the planner in
//! cyclone (https://github.com/reilleya/cyclone, GPL-3.0) by Andrew Reilley.
//!
//! Given the same `.wind` input, `plan_wind` produces the same G-code as
//! `npm run cli -- plan`, line for line. The golden tests in `tests.rs` check
//! this against output recorded from cyclone itself.

use std::sync::Arc;

pub mod jsnum;
mod machine;
pub mod types;
pub mod validate;
#[cfg(test)]
mod tests;

use machine::{Target, WinderMachine};
pub use types::*;

use jsnum::to_js_string;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
}

/// A message cyclone would have printed to the console while planning.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayerSummary {
    /// Index into `WindParameters::layers`.
    pub index: usize,
    pub wind_type: &'static str,
    pub time_s: f64,
    pub tow_m: f64,
}

#[derive(Clone, Debug)]
pub struct PlanResult {
    /// Shared, so a running job and the preview can hold it without copying.
    pub gcode: Arc<Vec<String>>,
    pub layers: Vec<LayerSummary>,
    pub total_time_s: f64,
    pub total_tow_m: f64,
    pub log: Vec<LogLine>,
}

impl PlanResult {
    pub fn warnings(&self) -> impl Iterator<Item = &str> {
        self.log
            .iter()
            .filter(|l| l.level == LogLevel::Warn)
            .map(|l| l.text.as_str())
    }

    /// The G-code as cyclone writes it to a file (newline-joined, no trailing newline).
    pub fn gcode_text(&self) -> String {
        self.gcode.join("\n")
    }
}

struct Log(Vec<LogLine>);

impl Log {
    fn info(&mut self, text: impl Into<String>) {
        self.0.push(LogLine {
            level: LogLevel::Info,
            text: text.into(),
        });
    }
    fn warn(&mut self, text: impl Into<String>) {
        self.0.push(LogLine {
            level: LogLevel::Warn,
            text: text.into(),
        });
    }
}

/// Port of cyclone's `planWind`.
pub fn plan_wind(params: &WindParameters, verbose: bool) -> PlanResult {
    let mandrel = &params.mandrel_parameters;
    let tow = &params.tow_parameters;
    let mut m = WinderMachine::new(mandrel.diameter, verbose);
    let mut log = Log(Vec::new());
    let mut layers = Vec::new();

    // Same as JSON.stringify({mandrel, tow}) for a file with cyclone's key order.
    m.insert_comment(&format!(
        r#"Parameters {{"mandrel":{{"diameter":{},"windLength":{}}},"tow":{{"width":{},"thickness":{}}}}}"#,
        to_js_string(mandrel.diameter),
        to_js_string(mandrel.wind_length),
        to_js_string(tow.width),
        to_js_string(tow.thickness),
    ));
    m.add_raw_gcode("G0 X0 Y0 Z0");
    m.set_feed_rate(params.default_feed_rate);

    let mut encountered_terminal = false;
    let mut cumulative_time_s = 0.0;
    let mut cumulative_tow_m = 0.0;
    let count = params.layers.len();

    for (index, layer) in params.layers.iter().enumerate() {
        if encountered_terminal {
            log.warn("WARNING: Attempting to plan a layer after a terminal layer, aborting...");
            break;
        }
        let comment = format!("Layer {} of {}: {}", index + 1, count, layer.wind_type());
        log.info(comment.clone());
        m.insert_comment(&comment);

        match layer {
            Layer::Hoop(hoop) => {
                plan_hoop_layer(&mut m, hoop, mandrel, tow);
                encountered_terminal = encountered_terminal || hoop.terminal;
            }
            Layer::Helical(helical) => plan_helical_layer(&mut m, &mut log, helical, mandrel, tow),
            Layer::Skip(skip) => plan_skip_layer(&mut m, skip),
        }

        let layer_time = m.time_s() - cumulative_time_s;
        let layer_tow = m.tow_length_m() - cumulative_tow_m;
        log.info(format!("Layer time estimate: {} seconds", to_js_string(layer_time)));
        log.info(format!("Layer tow required: {} meters", to_js_string(layer_tow)));
        layers.push(LayerSummary {
            index,
            wind_type: layer.wind_type(),
            time_s: layer_time,
            tow_m: layer_tow,
        });
        cumulative_time_s = m.time_s();
        cumulative_tow_m = m.tow_length_m();
        log.info("-".repeat(80));
    }

    log.info(format!("\nTotal time estimate: {} seconds", to_js_string(cumulative_time_s)));
    log.info(format!("Total tow required: {} meters\n", to_js_string(cumulative_tow_m)));

    PlanResult {
        gcode: Arc::new(m.into_gcode()),
        layers,
        total_time_s: cumulative_time_s,
        total_tow_m: cumulative_tow_m,
        log: log.0,
    }
}

fn rad_to_deg(radians: f64) -> f64 {
    radians * 180.0 / std::f64::consts::PI
}

fn deg_to_rad(degrees: f64) -> f64 {
    degrees / 180.0 * std::f64::consts::PI
}

/// A there-and-back hoop pass (or just "there" when terminal), ending with
/// the axes re-zeroed. Assumes an overlap factor of 1.0.
fn plan_hoop_layer(
    m: &mut WinderMachine,
    layer: &HoopLayer,
    mandrel: &MandrelParameters,
    tow: &TowParameters,
) {
    let lock_degrees = 180.0;
    // Delivery head angle. libm (fdlibm) matches JavaScript's Math.atan bit for bit.
    let wind_angle = 90.0 - rad_to_deg(libm::atan(mandrel.diameter / tow.width));
    let mandrel_rotations = mandrel.wind_length / tow.width;
    let far_mandrel = lock_degrees + mandrel_rotations * 360.0;
    let far_lock = far_mandrel + lock_degrees;
    let near_mandrel = far_lock + mandrel_rotations * 360.0;
    let near_lock = near_mandrel + lock_degrees;

    // Small near lock
    m.move_to(Target::xyz(0.0, lock_degrees, 0.0));
    // Tilt delivery head
    m.move_to(Target::new().z(-wind_angle));
    // Wind to the far end
    m.move_to(Target::new().x(mandrel.wind_length).y(far_mandrel));
    // Small far lock
    m.move_to(Target::new().y(far_lock).z(0.0));
    // A terminal layer leaves the carriage at the far end.
    if layer.terminal {
        return;
    }
    // Tilt delivery head
    m.move_to(Target::new().z(wind_angle));
    // Wind back to the near end
    m.move_to(Target::new().x(0.0).y(near_mandrel));
    // Small near lock
    m.move_to(Target::new().y(near_lock).z(0.0));
    m.zero_axes(near_lock);
}

fn plan_helical_layer(
    m: &mut WinderMachine,
    log: &mut Log,
    layer: &HelicalLayer,
    mandrel: &MandrelParameters,
    tow: &TowParameters,
) {
    let delivery_head_pass_start_angle = -10.0;

    let lead_out_degrees = layer.lead_out_degrees;
    let wind_lead_in_mm = layer.lead_in_mm;
    let lock_degrees = layer.lock_degrees;
    // Head angle during a "there" pass.
    let delivery_head_angle_degrees = -(90.0 - layer.wind_angle);
    let mandrel_circumference = std::f64::consts::PI * mandrel.diameter;
    // Width one pass of tow occupies on the mandrel at this angle.
    let tow_arc_length = tow.width / libm::cos(deg_to_rad(layer.wind_angle));
    // Circuits needed to cover the surface (each circuit is there + back).
    let num_circuits = (mandrel_circumference / tow_arc_length).ceil();
    let pattern_step_degrees = 360.0 * (1.0 / num_circuits);
    // Mandrel surface travel per pass for this length and angle.
    let pass_rotation_mm = mandrel.wind_length * libm::tan(deg_to_rad(layer.wind_angle));
    let pass_rotation_degrees = 360.0 * (pass_rotation_mm / mandrel_circumference);
    let pass_degrees_per_mm = pass_rotation_degrees / mandrel.wind_length;
    let pattern_number = layer.pattern_number;
    let number_of_patterns = num_circuits / layer.pattern_number;
    let lead_in_degrees = pass_degrees_per_mm * wind_lead_in_mm;
    let main_pass_degrees = pass_degrees_per_mm * (mandrel.wind_length - wind_lead_in_mm);

    struct Pass {
        delivery_head_sign: f64,
        lead_in_end_mm: f64,
        full_pass_end_mm: f64,
    }
    let passes = [
        // There
        Pass {
            delivery_head_sign: 1.0,
            lead_in_end_mm: wind_lead_in_mm,
            full_pass_end_mm: mandrel.wind_length,
        },
        // Back
        Pass {
            delivery_head_sign: -1.0,
            lead_in_end_mm: mandrel.wind_length - wind_lead_in_mm,
            full_pass_end_mm: 0.0,
        },
    ];

    log.info(format!("Doing helical wind, {} circuits", to_js_string(num_circuits)));
    if num_circuits % layer.pattern_number != 0.0 {
        log.warn(format!(
            "Circuit number of {} not divisible by pattern number of {}",
            to_js_string(num_circuits),
            to_js_string(layer.pattern_number)
        ));
        return;
    }

    if !layer.skip_initial_near_lock.unwrap_or(false) {
        m.move_to(Target::xyz(0.0, lock_degrees, 0.0));
        m.set_position(Target::new().y(0.0));
    }

    let mut mandrel_position = 0.0;
    let mut pattern_index = 0.0;
    while pattern_index < number_of_patterns {
        let mut in_pattern_index = 0.0;
        while in_pattern_index < pattern_number {
            m.insert_comment(&format!(
                "\tPattern: {}/{} Circuit: {}/{}",
                to_js_string(pattern_index + 1.0),
                to_js_string(number_of_patterns),
                to_js_string(in_pattern_index + 1.0),
                to_js_string(pattern_number)
            ));

            for pass in &passes {
                // Wind to this pass's start while levelling the head from the last pass.
                m.move_to(Target::new().y(mandrel_position).z(0.0));
                // Tilt the head to its start position.
                m.move_to(Target::new().z(pass.delivery_head_sign * delivery_head_pass_start_angle));
                // Lead in, tilting the head into its final position.
                mandrel_position += lead_in_degrees;
                m.move_to(
                    Target::new()
                        .x(pass.lead_in_end_mm)
                        .y(mandrel_position)
                        .z(pass.delivery_head_sign * delivery_head_angle_degrees),
                );
                // Wind to the end of the pass.
                mandrel_position += main_pass_degrees;
                m.move_to(Target::new().x(pass.full_pass_end_mm).y(mandrel_position));
                // Lead out.
                mandrel_position += lead_out_degrees;
                m.move_to(
                    Target::new()
                        .y(mandrel_position)
                        .z(pass.delivery_head_sign * delivery_head_pass_start_angle),
                );
                mandrel_position += lock_degrees - lead_out_degrees - (pass_rotation_degrees % 360.0);
            }

            // Next start position in this pattern.
            mandrel_position += pattern_step_degrees * num_circuits / layer.pattern_number;
            in_pattern_index += 1.0;
        }
        // Next pattern start position.
        mandrel_position += pattern_step_degrees;
        pattern_index += 1.0;
    }

    mandrel_position += lock_degrees;
    m.move_to(Target::new().y(mandrel_position).z(0.0));
    m.zero_axes(mandrel_position);
}

fn plan_skip_layer(m: &mut WinderMachine, layer: &SkipLayer) {
    m.move_to(Target::xyz(0.0, layer.mandrel_rotation, 0.0));
    m.set_position(Target::new().y(0.0));
}
