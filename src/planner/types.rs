//! `.wind` file format, identical to cyclone's (JSON, metric units).
//!
//! Numbers are kept as `f64` everywhere, as in cyclone's JavaScript, so the
//! planner's arithmetic and output match it exactly.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MandrelParameters {
    pub diameter: f64,
    pub wind_length: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TowParameters {
    pub width: f64,
    /// Currently unused by the planner (as in cyclone).
    pub thickness: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoopLayer {
    /// One-way pass that ends at the far end; no layers may follow it.
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelicalLayer {
    /// Angle between the tow and the mandrel axis's normal, in degrees.
    pub wind_angle: f64,
    /// Number of evenly spaced start positions around the mandrel.
    pub pattern_number: f64,
    /// Increment applied to the start position index after each circuit.
    /// Read from the file but not used by cyclone's planner.
    pub skip_index: f64,
    /// Mandrel rotation at each end of a circuit, in degrees.
    pub lock_degrees: f64,
    /// Carriage travel at each end during which the head rotates into place, in mm.
    #[serde(rename = "leadInMM")]
    pub lead_in_mm: f64,
    /// Portion of each lock during which the head rotates back to level, in degrees.
    pub lead_out_degrees: f64,
    /// For consecutive helical layers: skip the extra near lock at the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_initial_near_lock: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkipLayer {
    /// Degrees to advance the mandrel.
    pub mandrel_rotation: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "windType", rename_all = "lowercase")]
pub enum Layer {
    Hoop(HoopLayer),
    Helical(HelicalLayer),
    Skip(SkipLayer),
}

impl Layer {
    /// The `windType` string, as used in cyclone's comments and logs.
    pub fn wind_type(&self) -> &'static str {
        match self {
            Layer::Hoop(_) => "hoop",
            Layer::Helical(_) => "helical",
            Layer::Skip(_) => "skip",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindParameters {
    pub layers: Vec<Layer>,
    pub mandrel_parameters: MandrelParameters,
    pub tow_parameters: TowParameters,
    /// Feed rate for all moves, in Marlin units per minute.
    pub default_feed_rate: f64,
}

impl WindParameters {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("wind parameters always serialise")
    }
}
