//! Checks a wind before planning and derives per-layer figures for the
//! designer, using the same formulas as the planner.
//!
//! Cyclone itself silently skips a helical layer whose circuit count is not
//! divisible by its pattern number, and stops at the first layer after a
//! terminal hoop. Here those are reported up front instead.

use super::types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// Index into `WindParameters::layers`, or `None` for the whole wind.
    pub layer: Option<usize>,
    pub severity: Severity,
    pub message: String,
}

impl Issue {
    fn error(layer: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            layer,
            severity: Severity::Error,
            message: message.into(),
        }
    }
    fn warning(layer: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            layer,
            severity: Severity::Warning,
            message: message.into(),
        }
    }
}

/// Derived figures for a helical layer (same formulas as `plan_helical_layer`).
#[derive(Clone, Debug, PartialEq)]
pub struct HelicalInfo {
    /// Circuits (there + back) needed to cover the mandrel.
    pub num_circuits: f64,
    /// Width one pass of tow covers on the mandrel at this angle, in mm.
    pub tow_arc_length_mm: f64,
    /// Mandrel rotation during one pass, in degrees.
    pub pass_rotation_deg: f64,
    /// Pattern numbers that divide the circuit count evenly.
    pub valid_pattern_numbers: Vec<u32>,
}

pub fn helical_info(
    layer: &HelicalLayer,
    mandrel: &MandrelParameters,
    tow: &TowParameters,
) -> Option<HelicalInfo> {
    let usable = mandrel.diameter > 0.0
        && mandrel.wind_length > 0.0
        && tow.width > 0.0
        && layer.wind_angle > 0.0
        && layer.wind_angle < 90.0;
    if !usable {
        return None;
    }
    let angle = layer.wind_angle / 180.0 * std::f64::consts::PI;
    let circumference = std::f64::consts::PI * mandrel.diameter;
    let tow_arc_length_mm = tow.width / libm::cos(angle);
    let num_circuits = (circumference / tow_arc_length_mm).ceil();
    let pass_rotation_mm = mandrel.wind_length * libm::tan(angle);
    let pass_rotation_deg = 360.0 * (pass_rotation_mm / circumference);
    let valid_pattern_numbers = if num_circuits.is_finite() && num_circuits <= 100_000.0 {
        let n = num_circuits as u32;
        (1..=n).filter(|d| n.is_multiple_of(*d)).collect()
    } else {
        Vec::new()
    };
    Some(HelicalInfo {
        num_circuits,
        tow_arc_length_mm,
        pass_rotation_deg,
        valid_pattern_numbers,
    })
}

/// Derived figures for a hoop layer (same formulas as `plan_hoop_layer`).
#[derive(Clone, Debug, PartialEq)]
pub struct HoopInfo {
    /// Delivery head tilt during the pass, in degrees.
    pub head_angle_deg: f64,
    /// Mandrel turns per pass (one tow width per turn).
    pub turns_per_pass: f64,
}

pub fn hoop_info(mandrel: &MandrelParameters, tow: &TowParameters) -> Option<HoopInfo> {
    if !(mandrel.diameter > 0.0 && tow.width > 0.0 && mandrel.wind_length > 0.0) {
        return None;
    }
    Some(HoopInfo {
        head_angle_deg: 90.0 - libm::atan(mandrel.diameter / tow.width) * 180.0 / std::f64::consts::PI,
        turns_per_pass: mandrel.wind_length / tow.width,
    })
}

/// Everything that would make the wind fail, be skipped, or look suspicious.
/// Any `Error` means the G-code would not be what the designer shows.
// `!(x > 0.0)` is deliberate: unlike `x <= 0.0` it also rejects NaN.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn validate(params: &WindParameters) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mandrel = &params.mandrel_parameters;
    let tow = &params.tow_parameters;

    if !(mandrel.diameter > 0.0) {
        issues.push(Issue::error(None, "Mandrel diameter must be greater than 0."));
    }
    if !(mandrel.wind_length > 0.0) {
        issues.push(Issue::error(None, "Wind length must be greater than 0."));
    }
    if !(tow.width > 0.0) {
        issues.push(Issue::error(None, "Tow width must be greater than 0."));
    }
    if !(params.default_feed_rate > 0.0) {
        issues.push(Issue::error(None, "Feed rate must be greater than 0."));
    }
    if params.layers.is_empty() {
        issues.push(Issue::warning(None, "No layers yet: the G-code will only home to zero."));
    }

    let mut terminal_at: Option<usize> = None;
    for (i, layer) in params.layers.iter().enumerate() {
        if let Some(t) = terminal_at {
            issues.push(Issue::error(
                Some(i),
                format!(
                    "Comes after the terminal hoop in layer {}; cyclone stops planning there, so this and later layers are dropped.",
                    t + 1
                ),
            ));
            // One message is enough; later layers are dropped for the same reason.
            break;
        }
        match layer {
            Layer::Hoop(h) => {
                if h.terminal {
                    terminal_at = Some(i);
                }
            }
            Layer::Skip(_) => {}
            Layer::Helical(h) => validate_helical(&mut issues, i, h, mandrel, tow),
        }
    }
    issues
}

#[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN must count as invalid too
fn validate_helical(
    issues: &mut Vec<Issue>,
    i: usize,
    h: &HelicalLayer,
    mandrel: &MandrelParameters,
    tow: &TowParameters,
) {
    let at = Some(i);
    if !(h.wind_angle > 0.0 && h.wind_angle < 90.0) {
        issues.push(Issue::error(at, "Wind angle must be between 0° and 90°."));
    }
    if !(h.pattern_number >= 1.0) || h.pattern_number.fract() != 0.0 {
        issues.push(Issue::error(at, "Pattern number must be a whole number of 1 or more."));
    } else if let Some(info) = helical_info(h, mandrel, tow)
        && info.num_circuits % h.pattern_number != 0.0 {
            let options: Vec<String> = info
                .valid_pattern_numbers
                .iter()
                .map(|n| n.to_string())
                .collect();
            issues.push(Issue::error(
                at,
                format!(
                    "{} circuits is not divisible by pattern number {}; cyclone would skip this layer. Valid: {}.",
                    info.num_circuits,
                    h.pattern_number,
                    options.join(", ")
                ),
            ));
        }
    if h.lead_in_mm < 0.0 || h.lead_in_mm > mandrel.wind_length {
        issues.push(Issue::error(at, "Lead-in must be between 0 and the wind length."));
    } else if h.lead_in_mm * 2.0 > mandrel.wind_length {
        issues.push(Issue::warning(
            at,
            "Lead-in is more than half the wind length; the lead-ins at each end overlap.",
        ));
    }
    if h.lock_degrees < 0.0 {
        issues.push(Issue::error(at, "Lock degrees cannot be negative."));
    }
    if h.lead_out_degrees < 0.0 {
        issues.push(Issue::error(at, "Lead-out degrees cannot be negative."));
    } else if h.lead_out_degrees > h.lock_degrees {
        issues.push(Issue::warning(
            at,
            "Lead-out is larger than the lock, so the mandrel winds back between passes.",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str) -> WindParameters {
        let path = format!("{}/tests/fixtures/{name}.wind", env!("CARGO_MANIFEST_DIR"));
        WindParameters::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn clean_winds_have_no_errors() {
        for name in ["hoop", "helical"] {
            let issues = validate(&load(name));
            assert!(
                issues.iter().all(|i| i.severity != Severity::Error),
                "{name}: {issues:?}"
            );
        }
    }

    #[test]
    fn flags_indivisible_pattern() {
        let issues = validate(&load("bad_pattern"));
        let e = issues.iter().find(|i| i.severity == Severity::Error).unwrap();
        assert_eq!(e.layer, Some(0));
        assert!(e.message.contains("18 circuits"), "{}", e.message);
        assert!(e.message.contains("1, 2, 3, 6, 9, 18"), "{}", e.message);
    }

    #[test]
    fn flags_layer_after_terminal() {
        let issues = validate(&load("mixed"));
        let errors: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 1, "{issues:?}");
        assert_eq!(errors[0].layer, Some(5));
    }

    #[test]
    fn helical_info_matches_planner_log() {
        // The planner logged "Doing helical wind, 23 circuits" and "11 circuits" for mixed.wind.
        let w = load("mixed");
        let counts: Vec<f64> = w
            .layers
            .iter()
            .filter_map(|l| match l {
                Layer::Helical(h) => helical_info(h, &w.mandrel_parameters, &w.tow_parameters),
                _ => None,
            })
            .map(|i| i.num_circuits)
            .collect();
        assert_eq!(counts, vec![23.0, 11.0]);
    }
}
