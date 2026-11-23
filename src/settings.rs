//! User settings that survive restarts (stored by eframe's persistence).

use serde::{Deserialize, Serialize};

pub const STORAGE_KEY: &str = "typhoon_settings";

/// Console commands remembered for ↑/↓ (kept between runs).
pub const HISTORY_LIMIT: usize = 50;

pub const BAUD_RATES: [u32; 5] = [115_200, 250_000, 230_400, 57_600, 9_600];

/// Jog step presets. X is carriage millimetres; Y (mandrel) and Z (delivery head) are degrees.
pub const X_STEPS_MM: [f32; 4] = [0.1, 1.0, 10.0, 100.0];
pub const Y_STEPS_DEG: [f32; 4] = [1.0, 10.0, 90.0, 360.0];
pub const Z_STEPS_DEG: [f32; 4] = [1.0, 5.0, 15.0, 45.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MainTab {
    Designer,
    Preview,
    /// Only offered on short windows, where the console has no room of its own.
    Console,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub port: Option<String>,
    pub baud: u32,
    pub x_step: usize,
    pub y_step: usize,
    pub z_step: usize,
    /// Jog feed rate in Marlin units per minute.
    pub jog_feed: f32,
    pub last_wind_file: Option<String>,
    pub last_gcode_file: Option<String>,
    pub tab: MainTab,
    /// Hide `ok` and `echo:busy` lines in the console.
    pub console_hide_ok: bool,
    pub console_history: Vec<String>,
    pub theme: crate::theme::ThemeChoice,
    /// Background logo opacity, 0..=1.
    pub logo_opacity: f32,
    /// Add cyclone's verbose move comments to generated G-code.
    pub verbose_gcode: bool,
    /// The wind being edited (cyclone `.wind` JSON), so it survives restarts.
    pub wind_json: Option<String>,
    /// Keep DTR/RTS low when opening the port (stops ESP32 boards resetting).
    pub hold_dtr_rts_low: bool,
    /// Angle of the Z (delivery head) limit switch from the flat neutral
    /// position. Home Z sets Z to this (G92) so that Z 0° is flat.
    pub z_switch_deg: f32,
    /// Turn the head back to flat (Z 0°) after homing it.
    pub z_flat_after_home: bool,
    /// Play alert sounds (errors, alerts, homing done, wind done).
    pub sound_on: bool,
    /// Which sound each alert plays.
    pub alert_sounds: crate::sound::AlertSounds,
    /// 0..=1
    pub sound_volume: f32,
    /// Overload theme: pop on every button, tab and checkbox click.
    pub click_sounds: bool,
    /// The alert sounds picked before switching to Overload (which brings its
    /// own), put back when switching away again.
    pub sounds_before_overload: Option<crate::sound::AlertSounds>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: None,
            baud: 115_200,
            x_step: 2,
            y_step: 1,
            z_step: 1,
            jog_feed: 3000.0,
            last_wind_file: None,
            last_gcode_file: None,
            tab: MainTab::Designer,
            console_hide_ok: true,
            console_history: Vec::new(),
            theme: crate::theme::ThemeChoice::System,
            logo_opacity: crate::background::DEFAULT_OPACITY,
            verbose_gcode: false,
            wind_json: None,
            hold_dtr_rts_low: true,
            z_switch_deg: -90.0,
            z_flat_after_home: true,
            sound_on: true,
            alert_sounds: crate::sound::AlertSounds::default(),
            sound_volume: 0.6,
            click_sounds: true,
            sounds_before_overload: None,
        }
    }
}

impl Settings {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let mut s: Self = storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        // Guard against stale indices if the preset lists ever change.
        s.x_step = s.x_step.min(X_STEPS_MM.len() - 1);
        s.y_step = s.y_step.min(Y_STEPS_DEG.len() - 1);
        s.z_step = s.z_step.min(Z_STEPS_DEG.len() - 1);
        s.logo_opacity = s.logo_opacity.clamp(0.0, 0.3);
        s.sound_volume = s.sound_volume.clamp(0.0, 1.0);
        s
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, self);
    }
}
