//! Top-level app state and the Mainsail-style panel layout.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Button, Color32, DragValue, Frame, Layout, Margin, ProgressBar, RichText,
    ScrollArea, TextEdit, Ui,
};

use crate::background::Background;
use crate::designer::{self, Designer};
use crate::job::{JobEstimate, JobSource};
use crate::preview::Preview;
use crate::serial::{self, Connection, Event, JobEvent, LogKind, Position};
use crate::settings::{self, MainTab, Settings};
use crate::sound::{self, Alert, Preset, Sound};
use crate::theme::{self, ThemeChoice};

/// Console lines kept in memory.
const CONSOLE_LIMIT: usize = 2000;

/// At or above this window width (points) the console gets its own column.
const WIDE_WIDTH: f32 = 1250.0;
/// Below this width the toolhead column starts narrower.
const NARROW_WIDTH: f32 = 1000.0;
const MIN_MAIN_CARD_HEIGHT: f32 = 260.0;
const MIN_CONSOLE_HEIGHT: f32 = 150.0;
/// Below this window height (points) a docked console leaves too little room,
/// so it becomes a tab next to the designer instead.
const SHORT_HEIGHT: f32 = 800.0;

/// Where the console lives, depending on the window size.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsoleDock {
    Right,
    Bottom,
    Tab,
}

#[derive(Clone, PartialEq, Eq)]
enum ConnState {
    Disconnected,
    /// Port open, waiting for the board to answer.
    Connecting,
    Ready,
    /// The connection ended with an error (shown in the console).
    Failed,
    /// Marlin halted (E-STOP or a fault) and must be reset.
    Halted,
}

impl ConnState {
    fn label(&self, busy: bool, p: &theme::Palette) -> (&'static str, Color32) {
        match self {
            ConnState::Disconnected => ("Disconnected", p.text_dim),
            ConnState::Connecting => ("Connecting…", theme::WARN_AMBER),
            ConnState::Ready if busy => ("Busy", theme::WARN_AMBER),
            ConnState::Ready => ("Ready", theme::OK_GREEN),
            ConnState::Failed => ("Error", theme::RED),
            ConnState::Halted => ("Halted", theme::RED),
        }
    }
}

struct PortEntry {
    name: String,
    description: String,
    /// USB serial adapters (like the TinyBee's CH340) are listed first.
    usb: bool,
}

struct ConsoleLine {
    kind: LogKind,
    text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum JobPhase {
    #[default]
    Idle,
    Running,
    /// Waiting for the moves already sent to finish.
    Pausing,
    Paused,
}

/// The job that is running (or ran last), as the Job card shows it.
#[derive(Default)]
struct JobView {
    phase: JobPhase,
    name: String,
    lines: Option<Arc<Vec<String>>>,
    estimate: Option<Arc<JobEstimate>>,
    /// Lines the board has accepted.
    done: usize,
    total: usize,
    /// Where the last move sent is headed.
    target: Option<Position>,
    run_started: Option<Instant>,
    /// Running time before `run_started` (pauses excluded).
    run_time: Duration,
    /// How the last job ended: (message, succeeded).
    outcome: Option<(String, bool)>,
}

impl JobView {
    fn active(&self) -> bool {
        self.phase != JobPhase::Idle
    }

    fn running_time(&self) -> Duration {
        self.run_time + self.run_started.map_or(Duration::ZERO, |s| s.elapsed())
    }

    fn stop_clock(&mut self) {
        if let Some(s) = self.run_started.take() {
            self.run_time += s.elapsed();
        }
    }

    /// Share of the work done, by estimated time (falls back to lines).
    fn fraction(&self) -> f64 {
        match self.estimate.as_deref() {
            Some(e) if e.total_s > 0.0 => (e.elapsed_at(self.done) / e.total_s).clamp(0.0, 1.0),
            _ if self.total > 0 => self.done as f64 / self.total as f64,
            _ => 0.0,
        }
    }
}

/// Tracks a Home X / Home Z request so "homing done" can be announced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Homing {
    #[default]
    Idle,
    /// Sent; waiting for the board to start on it.
    Requested,
    /// The board is working through the homing commands.
    Running,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialog {
    StartJob,
    StopJob,
    Quit,
}

pub struct TyphoonApp {
    settings: Settings,
    background: Background,
    designer: Designer,
    preview: Preview,
    ports: Vec<PortEntry>,
    state: ConnState,
    conn: Option<Connection>,
    /// The board is still working through commands the user sent.
    busy: bool,
    position: Option<Position>,
    console: Vec<ConsoleLine>,
    console_input: String,
    /// Position in the command history while browsing it with ↑/↓ (0 = newest).
    history_pos: Option<usize>,
    /// What was typed before browsing the history, restored by ↓ past the newest.
    history_draft: String,
    /// Height of the Job card last frame, so the main card can leave room for it.
    job_card_height: f32,
    show_prefs: bool,
    console_dock: ConsoleDock,
    /// What Start will wind: the latest generated G-code or a loaded file.
    job_source: Option<JobSource>,
    /// The generated G-code `job_source` was last updated from.
    generated_seen: Option<Arc<Vec<String>>>,
    job: JobView,
    /// Feed-rate override for M220, in percent.
    speed: u32,
    /// Homed with the buttons since connecting (reset by M84, halts, restarts).
    homed_x: bool,
    homed_z: bool,
    homing: Homing,
    sounds: sound::Player,
    dialog: Option<Dialog>,
    /// Where E-STOP sits in the top bar, so it can be drawn above dialogs.
    estop_rect: Option<egui::Rect>,
    /// Bottom edge of the top bar; windows open below it.
    top_bar_bottom: f32,
}

impl TyphoonApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load(cc.storage);
        theme::install(&cc.egui_ctx, settings.theme);
        let designer = Designer::new(
            settings.wind_json.as_deref(),
            settings.last_wind_file.as_deref(),
        );
        let mut app = Self {
            settings,
            background: Background::new(&cc.egui_ctx),
            designer,
            preview: Preview::default(),
            ports: Vec::new(),
            state: ConnState::Disconnected,
            conn: None,
            busy: false,
            position: None,
            console: vec![ConsoleLine {
                kind: LogKind::Info,
                text: "Typhoon ready. Pick a COM port and connect to talk to the winder.".into(),
            }],
            console_input: String::new(),
            history_pos: None,
            history_draft: String::new(),
            job_card_height: 170.0,
            show_prefs: false,
            console_dock: ConsoleDock::Right,
            job_source: None,
            generated_seen: None,
            job: JobView::default(),
            speed: 100,
            homed_x: false,
            homed_z: false,
            homing: Homing::Idle,
            sounds: sound::Player::default(),
            dialog: None,
            estop_rect: None,
            top_bar_bottom: 56.0,
        };
        app.refresh_ports();
        app
    }

    /// Connected and the board has answered.
    fn connected(&self) -> bool {
        self.conn.is_some() && self.state == ConnState::Ready
    }

    fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        self.console.push(ConsoleLine {
            kind,
            text: text.into(),
        });
        if self.console.len() > CONSOLE_LIMIT {
            let extra = self.console.len() - CONSOLE_LIMIT;
            self.console.drain(..extra);
        }
    }

    fn connect(&mut self, ctx: &egui::Context) {
        let Some(port) = self.settings.port.clone() else { return };
        let baud = self.settings.baud;
        if port == serial::SIMULATOR {
            self.log(LogKind::Info, "Connecting to the built-in simulator…");
        } else {
            self.log(LogKind::Info, format!("Connecting to {port} at {baud} baud…"));
        }
        let options = serial::Options {
            hold_dtr_rts_low: self.settings.hold_dtr_rts_low,
        };
        self.conn = Some(Connection::open(port, baud, options, ctx.clone()));
        self.state = ConnState::Connecting;
        self.position = None;
        self.busy = false;
    }

    fn disconnect(&mut self) {
        self.conn = None; // dropping the handle stops the serial thread
        self.state = ConnState::Disconnected;
        self.busy = false;
        self.log(LogKind::Info, "Disconnected.");
    }

    fn remember_command(&mut self, line: &str) {
        let line = line.trim().to_string();
        let history = &mut self.settings.console_history;
        if history.last() != Some(&line) {
            history.push(line);
        }
        let extra = history.len().saturating_sub(settings::HISTORY_LIMIT);
        history.drain(..extra);
        self.history_pos = None;
        self.history_draft.clear();
    }

    /// Step through sent commands: `older` for ↑, otherwise ↓.
    fn browse_history(&mut self, older: bool) {
        let history = &self.settings.console_history;
        if history.is_empty() {
            return;
        }
        let pos = match (self.history_pos, older) {
            (None, true) => {
                self.history_draft = self.console_input.clone();
                Some(0)
            }
            (None, false) => None,
            (Some(p), true) => Some((p + 1).min(history.len() - 1)),
            (Some(0), false) => None,
            (Some(p), false) => Some(p - 1),
        };
        self.history_pos = pos;
        self.console_input = match pos {
            Some(p) => history[history.len() - 1 - p].clone(),
            None => self.history_draft.clone(),
        };
    }

    /// Queue a command for the board (from the console or a button).
    fn send(&mut self, line: &str) {
        if let Some(conn) = &self.conn {
            conn.send(line);
        }
    }

    /// Play an alert sound, if sounds are on.
    fn alert(&mut self, alert: Alert) {
        if self.settings.sound_on {
            let s = &self.settings;
            self.sounds.alert(s.alert_sounds.get(alert), alert, s.sound_volume);
        }
    }

    fn on_job_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Started { total } => {
                self.job.phase = JobPhase::Running;
                self.job.total = total;
                self.job.done = 0;
                self.job.run_time = Duration::ZERO;
                self.job.run_started = Some(Instant::now());
                self.job.outcome = None;
                let name = self.job.name.clone();
                self.log(LogKind::Info, format!("Job started: {name}"));
            }
            JobEvent::Progress { done, position } => {
                self.job.done = done;
                self.job.target = Some(position);
            }
            JobEvent::Pausing => {
                self.job.phase = JobPhase::Pausing;
                self.job.stop_clock();
                self.log(LogKind::Info, "Pausing: letting the moves already sent finish…");
            }
            JobEvent::Paused => {
                self.job.phase = JobPhase::Paused;
                self.log(LogKind::Info, "Paused. The machine has stopped.");
                self.alert(Alert::Attention);
            }
            JobEvent::Resumed => {
                self.job.phase = JobPhase::Running;
                self.job.run_started = Some(Instant::now());
                self.log(LogKind::Info, "Resumed.");
            }
            JobEvent::Finished => {
                self.job.stop_clock();
                let t = designer::format_duration(self.job.running_time().as_secs_f64());
                self.finish_job(format!("Finished in {t}."), true);
                self.alert(Alert::WindDone);
            }
            JobEvent::Stopped => {
                self.job.stop_clock();
                // A quick stop halts without decelerating, so steps may be lost.
                self.homed_x = false;
                self.homed_z = false;
                let t = designer::format_duration(self.job.running_time().as_secs_f64());
                self.finish_job(
                    format!("Stopped after {t}. Home X and Z before the next job."),
                    false,
                );
            }
            JobEvent::Failed(msg) => {
                self.job.stop_clock();
                self.finish_job(format!("Job failed: {msg}"), false);
                self.alert(Alert::Error);
            }
        }
    }

    fn finish_job(&mut self, message: String, ok: bool) {
        self.job.stop_clock();
        self.job.phase = JobPhase::Idle;
        self.log(if ok { LogKind::Info } else { LogKind::Error }, message.clone());
        self.job.outcome = Some((message, ok));
    }

    /// Use freshly generated G-code as the next job.
    fn pick_up_generated(&mut self) {
        let Some(g) = self.designer.generated() else { return };
        if self
            .generated_seen
            .as_ref()
            .is_some_and(|seen| Arc::ptr_eq(seen, &g.result.gcode))
        {
            return;
        }
        let lines = g.result.gcode.clone();
        let name = format!("{}.gcode", g.name);
        self.generated_seen = Some(lines.clone());
        self.job_source = Some(JobSource::new(name, lines, true));
    }

    fn load_gcode_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Load G-code")
            .add_filter("G-code", &["gcode", "gco", "nc", "txt"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = self
            .settings
            .last_gcode_file
            .as_ref()
            .and_then(|f| std::path::Path::new(f).parent())
        {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.pick_file() else { return };
        match JobSource::load(&path) {
            Ok(src) => {
                self.log(
                    LogKind::Info,
                    format!(
                        "Loaded {}: {} lines, est. {}",
                        src.name,
                        src.lines.len(),
                        designer::format_duration(src.estimate.total_s)
                    ),
                );
                self.settings.last_gcode_file = Some(path.display().to_string());
                self.job_source = Some(src);
            }
            Err(e) => self.log(LogKind::Error, e),
        }
    }

    fn start_job(&mut self) {
        let (Some(conn), Some(src)) = (&self.conn, &self.job_source) else { return };
        self.job.name = src.name.clone();
        self.job.lines = Some(src.lines.clone());
        self.job.estimate = Some(src.estimate.clone());
        self.job.target = None;
        self.job.done = 0;
        self.job.total = src.lines.len();
        conn.start_job(src.lines.clone());
    }

    fn handle_serial_events(&mut self) {
        let Some(conn) = &self.conn else { return };
        let events: Vec<Event> = conn.events().collect();
        for event in events {
            match event {
                Event::Opened => self.state = ConnState::Connecting,
                Event::Ready => {
                    // A fresh connection or a board restart: nothing is homed.
                    self.state = ConnState::Ready;
                    self.homed_x = false;
                    self.homed_z = false;
                    self.homing = Homing::Idle;
                }
                Event::Log(kind, text) => self.log(kind, text),
                Event::Position(p) => self.position = Some(p),
                Event::Busy(b) => {
                    self.busy = b;
                    match (self.homing, b) {
                        (Homing::Requested, true) => self.homing = Homing::Running,
                        (Homing::Running, false) => {
                            self.homing = Homing::Idle;
                            self.alert(Alert::Homed);
                        }
                        _ => {}
                    }
                }
                Event::Halted => {
                    self.state = ConnState::Halted;
                    self.busy = false;
                    self.homed_x = false;
                    self.homed_z = false;
                    self.homing = Homing::Idle;
                    self.alert(Alert::Error);
                }
                Event::NoReply => self.alert(Alert::Attention),
                Event::Job(e) => self.on_job_event(e),
                Event::Closed(error) => {
                    self.conn = None;
                    self.busy = false;
                    self.homed_x = false;
                    self.homed_z = false;
                    if self.job.active() {
                        self.finish_job("The connection closed during the job.".into(), false);
                    }
                    self.homing = Homing::Idle;
                    match error {
                        Some(msg) => {
                            if self.console.last().map(|l| &l.text) != Some(&msg) {
                                self.log(LogKind::Error, msg);
                            }
                            self.state = ConnState::Failed;
                            self.alert(Alert::Error);
                        }
                        None => self.state = ConnState::Disconnected,
                    }
                    break;
                }
            }
        }
    }

    fn refresh_ports(&mut self) {
        self.ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let (description, usb) = match p.port_type {
                    serialport::SerialPortType::UsbPort(usb) => (
                        usb.product
                            .or(usb.manufacturer)
                            .unwrap_or_else(|| format!("USB {:04X}:{:04X}", usb.vid, usb.pid)),
                        true,
                    ),
                    serialport::SerialPortType::BluetoothPort => ("Bluetooth".into(), false),
                    serialport::SerialPortType::PciPort => ("PCI".into(), false),
                    serialport::SerialPortType::Unknown => ("not USB".into(), false),
                };
                // Linux USB serial adapters, in case sysfs gave no USB details.
                let usb = usb
                    || p.port_name.starts_with("/dev/ttyUSB")
                    || p.port_name.starts_with("/dev/ttyACM");
                PortEntry {
                    name: p.port_name,
                    description,
                    usb,
                }
            })
            .collect();
        // USB first (stable, so each group keeps the OS order).
        self.ports.sort_by_key(|p| !p.usb);
        self.ports.push(PortEntry {
            name: serial::SIMULATOR.into(),
            description: "built-in test board".into(),
            usb: false,
        });
        // Keep the remembered port if it is still there, else pick the first
        // USB port. Never pick a non-USB port (e.g. Bluetooth) on our own.
        let remembered_present = self
            .settings
            .port
            .as_ref()
            .is_some_and(|p| self.ports.iter().any(|e| &e.name == p));
        if !remembered_present {
            self.settings.port = self.ports.iter().find(|p| p.usb).map(|p| p.name.clone());
        }
    }

    // ---------------------------------------------------------------- top bar

    /// E-STOP and the settings button are pinned to the right; everything
    /// else wraps onto a second line when the window is too narrow.
    fn top_bar(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        let compact = width < 1000.0;
        let has_link = self.conn.is_some();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let estop = Button::new(RichText::new("E-STOP").strong().color(Color32::WHITE))
                .fill(theme::RED)
                .min_size(egui::vec2(if compact { 76.0 } else { 96.0 }, 30.0));
            let estop = ui
                .add_enabled(has_link, estop)
                .on_hover_text("Send M112: Marlin stops everything and halts until reset")
                .on_disabled_hover_text("Not connected");
            self.estop_rect = Some(estop.rect);
            if estop.clicked()
                && let Some(conn) = &self.conn {
                    conn.emergency_stop();
                }
            let gear = Button::new(RichText::new("⚙").size(17.0))
                .selected(self.show_prefs)
                .min_size(egui::vec2(32.0, 30.0));
            if ui.add(gear).on_hover_text("Preferences").clicked() {
                self.show_prefs = !self.show_prefs;
            }
            let (icon, tip) = if self.settings.sound_on {
                ("🔊", "Alert sounds are on. Click to mute.")
            } else {
                ("🔇", "Alert sounds are off. Click to turn them on.")
            };
            let speaker = Button::new(RichText::new(icon).size(17.0)).min_size(egui::vec2(32.0, 30.0));
            if ui.add(speaker).on_hover_text(tip).clicked() {
                self.settings.sound_on = !self.settings.sound_on;
            }
            ui.add_space(6.0);
            ui.with_layout(
                Layout::left_to_right(Align::Center).with_main_wrap(true),
                |ui| self.top_bar_controls(ui, compact),
            );
        });
    }

    fn top_bar_controls(&mut self, ui: &mut Ui, compact: bool) {
        let p = theme::pal(ui.ctx());
        ui.label(
            RichText::new("Typhoon")
                .strong()
                .size(if compact { 18.0 } else { 21.0 })
                .extra_letter_spacing(0.5)
                .color(p.text),
        );
        if !compact {
            ui.label(RichText::new("filament winder").color(p.text_dim));
        }
        ui.add_space(if compact { 6.0 } else { 16.0 });

        let has_link = self.conn.is_some();
        ui.add_enabled_ui(!has_link, |ui| {
            let selected = self
                .settings
                .port
                .clone()
                .unwrap_or_else(|| "Pick a port".into());
            egui::ComboBox::from_id_salt("port")
                .width(if compact { 130.0 } else { 170.0 })
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if !self.ports.iter().any(|p| p.usb) {
                        ui.label(RichText::new("No USB serial ports found").small().weak());
                    }
                    for p in &self.ports {
                        let text = format!("{}  —  {}", p.name, p.description);
                        let text = if p.usb {
                            RichText::new(text)
                        } else {
                            RichText::new(text).weak()
                        };
                        ui.selectable_value(&mut self.settings.port, Some(p.name.clone()), text);
                    }
                });
            if ui.button("🔄").on_hover_text("Rescan COM ports").clicked() {
                self.refresh_ports();
            }
            egui::ComboBox::from_id_salt("baud")
                .width(90.0)
                .selected_text(self.settings.baud.to_string())
                .show_ui(ui, |ui| {
                    for b in settings::BAUD_RATES {
                        ui.selectable_value(&mut self.settings.baud, b, b.to_string());
                    }
                });
        });
        if has_link {
            if ui
                .add_enabled(!self.job.active(), Button::new("Disconnect"))
                .on_disabled_hover_text("Stop the job first")
                .clicked()
            {
                self.disconnect();
            }
        } else if ui
            .add_enabled(self.settings.port.is_some(), Button::new("Connect"))
            .on_disabled_hover_text("Pick a port first")
            .clicked()
        {
            self.connect(ui.ctx());
        }

        ui.add_space(if compact { 2.0 } else { 8.0 });
        let (label, color) = match self.job.phase {
            JobPhase::Running => (
                format!("Winding {:.0}%", self.job.fraction() * 100.0),
                theme::OK_GREEN,
            ),
            JobPhase::Pausing => ("Pausing…".to_string(), theme::WARN_AMBER),
            JobPhase::Paused => ("Paused".to_string(), theme::WARN_AMBER),
            JobPhase::Idle => {
                let (l, c) = self.state.label(self.busy, &p);
                (l.to_string(), c)
            }
        };
        theme::status_chip(ui, &label, color);
    }

    // ------------------------------------------------------------ preferences

    fn preferences_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_prefs;
        // Always below the top bar (never over E-STOP), scrolling if it's tall.
        let screen = ctx.content_rect();
        let top = self.top_bar_bottom + 6.0;
        egui::Window::new("Preferences")
            // A fresh id, so sizes saved by older versions of this window are ignored.
            .id(egui::Id::unique("preferences_v2"))
            .open(&mut open)
            .collapsible(false)
            // Fit the content, no taller than the space below the bar (then scroll).
            .auto_sized()
            .vscroll(true)
            .min_width(340.0)
            .max_width(400.0)
            // Relative to the constrain rect, which already starts below the bar.
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 0.0))
            .max_height((screen.bottom() - top - 12.0).max(120.0))
            .constrain_to(egui::Rect::from_min_max(egui::pos2(screen.left(), top), screen.max))
            .show(ctx, |ui| {
                ui.label(RichText::new("Appearance").strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label("Theme");
                    for choice in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
                        if ui
                            .selectable_value(&mut self.settings.theme, choice, choice.label())
                            .changed()
                        {
                            theme::set_choice(ctx, choice);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("UI scale");
                    let mut zoom = ctx.zoom_factor();
                    let slider = egui::Slider::new(&mut zoom, 0.7..=1.8)
                        .step_by(0.05)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0));
                    // Apply on release, so the slider doesn't move under the cursor while dragging.
                    let r = ui.add(slider);
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        ctx.set_zoom_factor(zoom);
                    }
                    if ui.small_button("Reset").clicked() {
                        ctx.set_zoom_factor(1.0);
                    }
                });
                ui.label(
                    RichText::new("Ctrl + and Ctrl − also zoom.")
                        .small()
                        .color(theme::pal(ctx).text_dim),
                );
                ui.horizontal(|ui| {
                    ui.label("Logo opacity");
                    ui.add(
                        egui::Slider::new(&mut self.settings.logo_opacity, 0.0..=0.3)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    );
                });

                ui.separator();
                ui.label(RichText::new("Sound").strong());
                ui.checkbox(&mut self.settings.sound_on, "Play alert sounds");
                ui.add_enabled_ui(self.settings.sound_on, |ui| {
                    // A preset fills all four; each can then be changed on its own.
                    ui.horizontal(|ui| {
                        ui.label("Preset");
                        let current = self
                            .settings
                            .alert_sounds
                            .preset()
                            .map_or("Custom", |p| p.label());
                        egui::ComboBox::from_id_salt("sound_preset")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for preset in Preset::ALL {
                                    if ui.selectable_label(current == preset.label(), preset.label()).clicked() {
                                        self.settings.alert_sounds = preset.sounds();
                                    }
                                }
                            });
                    });
                    let mut try_sound = None;
                    egui::Grid::new("alert_sounds")
                        .num_columns(3)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            for alert in Alert::ALL {
                                ui.label(alert.label());
                                let choice = self.settings.alert_sounds.get_mut(alert);
                                egui::ComboBox::from_id_salt(("alert_sound", alert.label()))
                                    .width(210.0)
                                    .selected_text(choice.label())
                                    .show_ui(ui, |ui| {
                                        for sound in Sound::ALL {
                                            ui.selectable_value(choice, sound, sound.label());
                                        }
                                    });
                                if ui.small_button("▶").on_hover_text("Play it").clicked() {
                                    try_sound = Some(*choice);
                                }
                                ui.end_row();
                            }
                        });
                    if let Some(s) = try_sound {
                        sound::play(s, self.settings.sound_volume);
                    }
                    ui.horizontal(|ui| {
                        ui.label("Volume");
                        ui.add(
                            egui::Slider::new(&mut self.settings.sound_volume, 0.0..=1.0)
                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                        );
                    });
                });
                ui.label(
                    RichText::new(
                        "Sounds play only for errors, alerts (job paused, board not answering), \
                         homing done and wind done.",
                    )
                    .small()
                    .color(theme::pal(ctx).text_dim),
                );

                ui.separator();
                ui.label(RichText::new("Machine").strong());
                ui.horizontal(|ui| {
                    ui.label("Z switch is at");
                    ui.add(
                        DragValue::new(&mut self.settings.z_switch_deg)
                            .range(-360.0..=360.0)
                            .speed(0.5)
                            .suffix("°"),
                    );
                    ui.label("from flat");
                });
                ui.label(
                    RichText::new(
                        "After Home Z, Typhoon sets Z to this angle (G92) so 0° is the flat, \
                         neutral head position that cyclone's G-code expects. Use −90° if Z \
                         homes toward Z-min, +90° if it homes toward Z-max.",
                    )
                    .small()
                    .color(theme::pal(ctx).text_dim),
                );
                ui.checkbox(
                    &mut self.settings.z_flat_after_home,
                    "Turn the head back to flat (Z 0°) after homing",
                );

                ui.separator();
                ui.label(RichText::new("Connection").strong());
                ui.checkbox(
                    &mut self.settings.hold_dtr_rts_low,
                    "Hold DTR/RTS low when connecting",
                )
                .on_hover_text(
                    "Stops ESP32 boards such as the MKS TinyBee from resetting when the port \
                     opens. Takes effect on the next connect.",
                );

                ui.separator();
                ui.label(RichText::new("About").strong());
                ui.label(format!("Typhoon {}", env!("CARGO_PKG_VERSION")));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label("A project by ");
                    ui.hyperlink_to("rnadomdude", "https://github.com/rnadomdude");
                    ui.label(".");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label("G-code generator ported from ");
                    ui.hyperlink_to("cyclone", "https://github.com/reilleya/cyclone");
                    ui.label(" by ");
                    ui.hyperlink_to("Andrew Reilley", "https://github.com/reilleya");
                    ui.label(".");
                });
                ui.label(
                    RichText::new("Licensed under GPL-3.0.")
                        .small()
                        .color(theme::pal(ctx).text_dim),
                );
            });
        self.show_prefs = open;
    }

    // --------------------------------------------------------------- toolhead

    fn toolhead_card(&mut self, ui: &mut Ui) {
        // Why the machine controls are off, if they are.
        let disabled: Option<&str> = if !self.connected() {
            Some("Not connected")
        } else if self.job.active() {
            Some("Not available while a job runs")
        } else {
            None
        };
        let enabled = disabled.is_none();
        let why = disabled.unwrap_or("");
        let p = theme::pal(ui.ctx());
        // Commands from the buttons, sent once the card is drawn.
        let mut commands: Vec<String> = Vec::new();
        let (mut home_x, mut home_z, mut motors_off) = (false, false, false);
        // During a job there are no position polls: show where moves are headed.
        let (pos, pos_tip) = match (self.job.active(), self.job.target) {
            (true, Some(t)) => (Some(t), "Target of the last move sent"),
            _ => (self.position, "Last position reported by M114"),
        };
        theme::card(ui, "Toolhead", |ui| {
            let axes = [
                ("X", "Carriage", " mm", pos.map(|p| p.x)),
                ("Y", "Mandrel", "°", pos.map(|p| p.y)),
                ("Z", "Head", "°", pos.map(|p| p.z)),
            ];
            ui.columns(3, |cols| {
                for (ui, (axis, name, unit, value)) in cols.iter_mut().zip(axes) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(axis).strong().size(16.0).color(theme::RED));
                        ui.add(
                            egui::Label::new(RichText::new(name).small().color(p.text_dim))
                                .extend(),
                        );
                    });
                    let text = match value {
                        Some(v) => format!("{v:.2}{unit}"),
                        None => format!("—{unit}"),
                    };
                    ui.add(egui::Label::new(RichText::new(text).monospace().size(15.0)).extend())
                        .on_hover_text(pos_tip);
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let w = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                if ui
                    .add_enabled(enabled, Button::new("🏠 Home X").min_size(egui::vec2(w, 30.0)))
                    .on_hover_text("Home the carriage (G28 X)")
                    .on_disabled_hover_text(why)
                    .clicked()
                {
                    home_x = true;
                }
                if ui
                    .add_enabled(enabled, Button::new("🏠 Home Z").min_size(egui::vec2(w, 30.0)))
                    .on_hover_text(
                        "Home the delivery head (G28 Z), then set Z to the switch angle so 0° \
                         is flat (see Preferences > Machine)",
                    )
                    .on_disabled_hover_text(why)
                    .clicked()
                {
                    home_z = true;
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Jog").strong());
            // Step presets as buttons when there is room, else as a dropdown.
            let compact = ui.available_width() < 290.0;
            let feed = self.settings.jog_feed;
            let rows = [
                ("X", "mm", &settings::X_STEPS_MM[..], &mut self.settings.x_step),
                ("Y", "°", &settings::Y_STEPS_DEG[..], &mut self.settings.y_step),
                ("Z", "°", &settings::Z_STEPS_DEG[..], &mut self.settings.z_step),
            ];
            for (axis, unit, steps, selected) in rows {
                if let Some(distance) = jog_row(ui, disabled, compact, axis, unit, steps, selected) {
                    // Relative move, then back to absolute so job G-code is unaffected.
                    commands.push("G91".into());
                    commands.push(format!("G0 {axis}{distance} F{feed:.0}"));
                    commands.push("G90".into());
                }
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Feed");
                ui.add(
                    DragValue::new(&mut self.settings.jog_feed)
                        .range(10.0..=30_000.0)
                        .speed(50.0)
                        .suffix(" /min"),
                )
                .on_hover_text("Jog speed. Y and Z are in degrees per minute.");
            });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(enabled, Button::new("Set Y = 0°"))
                    .on_hover_text(
                        "Call the mandrel's current angle 0° (G92 Y0). \
                         The mandrel has no home switch.",
                    )
                    .on_disabled_hover_text(why)
                    .clicked()
                {
                    commands.push("G92 Y0".into());
                }
                if ui
                    .add_enabled(enabled, Button::new("Motors off"))
                    .on_hover_text("Release all motors (M84). Positions are lost until you home again.")
                    .on_disabled_hover_text(why)
                    .clicked()
                {
                    motors_off = true;
                }
            });
        });
        if home_x || home_z {
            // "Homing done" sounds once the board has worked through these.
            self.homing = if self.busy { Homing::Running } else { Homing::Requested };
        }
        if home_x {
            commands.insert(0, "G28 X".into());
            self.homed_x = true;
        }
        if home_z {
            let feed = self.settings.jog_feed;
            commands.push("G28 Z".into());
            commands.push(format!("G92 Z{}", self.settings.z_switch_deg));
            if self.settings.z_flat_after_home {
                commands.push(format!("G0 Z0 F{feed:.0}"));
            }
            self.homed_z = true;
        }
        if motors_off {
            commands.push("M84".into());
            self.homed_x = false;
            self.homed_z = false;
        }
        for cmd in commands {
            self.send(&cmd);
        }
    }

    // ---------------------------------------------------------------- central

    fn central(&mut self, ui: &mut Ui) {
        let console_tab = self.console_dock == ConsoleDock::Tab;
        if !console_tab && self.settings.tab == MainTab::Console {
            self.settings.tab = MainTab::Designer;
        }
        ui.horizontal(|ui| {
            let mut tabs = vec![(MainTab::Designer, "Wind Designer"), (MainTab::Preview, "Preview")];
            if console_tab {
                tabs.push((MainTab::Console, "Console"));
            }
            for (tab, label) in tabs {
                if tab_button(ui, self.settings.tab == tab, label).clicked() {
                    self.settings.tab = tab;
                }
            }
        });
        ui.add_space(4.0);

        // The main card fills the height but never drops below a usable
        // minimum; on short screens the whole column scrolls instead.
        let gap = 18.0;
        let main_height =
            (ui.available_height() - self.job_card_height - gap).max(MIN_MAIN_CARD_HEIGHT);
        ScrollArea::vertical()
            .id_salt("central_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                match self.settings.tab {
                    MainTab::Designer => {
                        let verbose = &mut self.settings.verbose_gcode;
                        let designer = &mut self.designer;
                        theme::card_fixed(ui, "Wind Designer", main_height, |ui| {
                            designer.ui(ui, verbose);
                        });
                    }
                    MainTab::Preview => {
                        // While winding, the preview shows the running job and where it is.
                        let (stale, marker) = if self.job.active() {
                            (false, self.job.target.map(|t| (t.x, t.y)))
                        } else {
                            let stale = self.job_source.as_ref().is_some_and(|s| s.from_designer)
                                && self.designer.is_stale(self.settings.verbose_gcode);
                            (stale, None)
                        };
                        let preview = &mut self.preview;
                        let title = if preview.is_3d() {
                            "Preview · 3D tube"
                        } else {
                            "Preview · unwrapped mandrel"
                        };
                        theme::card_fixed(ui, title, main_height, |ui| {
                            preview.ui(ui, stale, marker);
                        });
                    }
                    MainTab::Console => self.console_card(ui, main_height),
                }
                ui.add_space(10.0);
                let job = ui.scope(|ui| self.job_card(ui));
                self.job_card_height = job.response.rect.height();
            });
    }

    fn job_card(&mut self, ui: &mut Ui) {
        #[derive(Clone, Copy)]
        enum Action {
            Load,
            Save,
            Start,
            Pause,
            Resume,
            Stop,
            Speed,
        }
        let p = theme::pal(ui.ctx());
        let verbose = self.settings.verbose_gcode;
        let active = self.job.active();
        let connected = self.connected();
        let mut action = None;
        theme::card(ui, "Job", |ui| {
            // What is being (or will be) wound.
            ui.horizontal_wrapped(|ui| {
                if active {
                    ui.label(RichText::new(&self.job.name).strong());
                    ui.label(
                        RichText::new(format!("line {} of {}", self.job.done, self.job.total))
                            .color(p.text_dim),
                    );
                } else if let Some(src) = &self.job_source {
                    ui.label(RichText::new(&src.name).strong());
                    ui.label(
                        RichText::new(format!(
                            "{} lines · est. {}",
                            src.lines.len(),
                            designer::format_duration(src.estimate.total_s)
                        ))
                        .color(p.text_dim),
                    );
                    if src.from_designer && self.designer.is_stale(verbose) {
                        ui.label(RichText::new("⚠ out of date").color(theme::WARN_AMBER))
                            .on_hover_text("The wind changed since this G-code was generated");
                    }
                    if !src.from_designer {
                        ui.label(RichText::new("(loaded file)").color(p.text_dim));
                    }
                } else {
                    ui.label(
                        RichText::new("No G-code yet. Generate it in the Wind Designer, or load a file.")
                            .color(p.text_dim),
                    );
                }
            });

            let (fraction, text) = self.progress_text();
            let fill = if active || fraction > 0.0 { theme::RED } else { Color32::TRANSPARENT };
            ui.add(
                ProgressBar::new(fraction as f32)
                    .desired_height(20.0)
                    // Readable on both the red fill and the empty track.
                    .text(RichText::new(text).color(if p.dark { Color32::WHITE } else { theme::GREY }))
                    .fill(fill),
            );

            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!active, Button::new("Load .gcode"))
                    .on_hover_text("Wind a G-code file instead (e.g. one made by cyclone)")
                    .on_disabled_hover_text("Not while a job runs")
                    .clicked()
                {
                    action = Some(Action::Load);
                }
                if ui
                    .add_enabled(self.designer.generated().is_some(), Button::new("💾 Save .gcode"))
                    .on_hover_text("Write the generated G-code to a file")
                    .on_disabled_hover_text("Generate G-code first")
                    .clicked()
                {
                    action = Some(Action::Save);
                }
                ui.add_space(12.0);
                match self.job.phase {
                    JobPhase::Idle => {
                        let blocked = if self.job_source.is_none() {
                            Some("Generate or load G-code first")
                        } else if !connected {
                            Some("Connect to the winder first")
                        } else if self.busy {
                            Some("Wait for the commands you sent to finish")
                        } else {
                            None
                        };
                        let start = Button::new(RichText::new("▶ Start").strong().color(Color32::WHITE))
                            .fill(theme::RED);
                        if ui
                            .add_enabled(blocked.is_none(), start)
                            .on_hover_text("Check the setup, then stream the G-code to the winder")
                            .on_disabled_hover_text(blocked.unwrap_or(""))
                            .clicked()
                        {
                            action = Some(Action::Start);
                        }
                    }
                    JobPhase::Running => {
                        if ui
                            .button("⏸ Pause")
                            .on_hover_text("Stop sending; the machine finishes the moves already sent, then holds")
                            .clicked()
                        {
                            action = Some(Action::Pause);
                        }
                    }
                    JobPhase::Pausing => {
                        ui.add_enabled(false, Button::new("Pausing…"));
                    }
                    JobPhase::Paused => {
                        if ui.button("▶ Resume").clicked() {
                            action = Some(Action::Resume);
                        }
                    }
                }
                if ui
                    .add_enabled(active, Button::new("■ Stop"))
                    .on_hover_text("End the job and stop motion at once (M410)")
                    .on_disabled_hover_text("No job running")
                    .clicked()
                {
                    action = Some(Action::Stop);
                }
                ui.add_space(12.0);
                ui.label("Speed");
                let r = ui
                    .add_enabled(
                        connected,
                        DragValue::new(&mut self.speed).range(10..=300).speed(1).suffix("%"),
                    )
                    .on_hover_text("Feed-rate override (M220). Applies right away, also mid-job.");
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    action = Some(Action::Speed);
                }
            });

            if !active
                && let Some((msg, ok)) = &self.job.outcome {
                    let color = if *ok { theme::OK_GREEN } else { theme::WARN_AMBER };
                    ui.label(RichText::new(msg).color(color));
                }
        });

        match action {
            Some(Action::Load) => self.load_gcode_dialog(),
            Some(Action::Save) => match self.designer.save_gcode_dialog() {
                Some(Ok(msg)) => self.log(LogKind::Info, msg),
                Some(Err(msg)) => self.log(LogKind::Error, msg),
                None => {}
            },
            Some(Action::Start) => self.dialog = Some(Dialog::StartJob),
            Some(Action::Pause) => {
                if let Some(c) = &self.conn {
                    c.pause_job();
                }
            }
            Some(Action::Resume) => {
                if let Some(c) = &self.conn {
                    c.resume_job();
                }
            }
            Some(Action::Stop) => self.dialog = Some(Dialog::StopJob),
            Some(Action::Speed) => {
                let s = self.speed;
                self.send(&format!("M220 S{s}"));
            }
            None => {}
        }
    }

    /// Progress bar fill and caption.
    fn progress_text(&self) -> (f64, String) {
        let j = &self.job;
        if !j.active() {
            return match &j.outcome {
                Some((_, true)) => (1.0, "Done".into()),
                Some((_, false)) => (j.fraction(), format!("Ended at {:.0}%", j.fraction() * 100.0)),
                None => (0.0, String::new()),
            };
        }
        let fraction = j.fraction();
        let elapsed = j.running_time().as_secs_f64();
        let mut text = match j.phase {
            JobPhase::Pausing => "Pausing · ".to_string(),
            JobPhase::Paused => "Paused · ".to_string(),
            _ => String::new(),
        };
        text += &format!("{:.0}%", fraction * 100.0);
        if let Some(est) = j.estimate.as_deref() {
            if let Some(layer) = est.layer_at(j.done) {
                text += &format!(" · Layer {layer}");
            }
            // Scale the estimate by how fast it has really gone so far.
            let done_est = est.elapsed_at(j.done);
            let ratio = if done_est > 20.0 { (elapsed / done_est).clamp(0.3, 5.0) } else { 1.0 };
            let left = (est.total_s - done_est).max(0.0) * ratio;
            text += &format!(
                " · {} elapsed · about {} left",
                designer::format_duration(elapsed),
                designer::format_duration(left)
            );
        }
        (fraction, text)
    }

    // ---------------------------------------------------------------- dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog else { return };
        let p = theme::pal(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique(("dialog", dialog as u8))).show(ctx, |ui| {
            ui.set_max_width(420.0);
            match dialog {
                Dialog::StartJob => {
                    let Some(src) = &self.job_source else {
                        close = true;
                        return;
                    };
                    ui.heading("Start winding?");
                    ui.label(format!(
                        "{} · {} lines · about {}",
                        src.name,
                        src.lines.len(),
                        designer::format_duration(src.estimate.total_s)
                    ));
                    ui.add_space(6.0);
                    let check = |ui: &mut Ui, ok: bool, good: &str, bad: &str| {
                        let (mark, color, text) = if ok {
                            ("✔", theme::OK_GREEN, good)
                        } else {
                            ("!", theme::WARN_AMBER, bad)
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(mark).strong().color(color));
                            ui.label(text);
                        });
                    };
                    check(ui, self.homed_x, "Carriage homed", "Carriage not homed since connecting (Home X)");
                    check(
                        ui,
                        self.homed_z,
                        "Delivery head homed and set flat",
                        "Delivery head not homed since connecting (Home Z)",
                    );
                    let y = self.position.map(|p| p.y).unwrap_or(0.0);
                    let y_ok = y.abs() < 0.5;
                    ui.horizontal_wrapped(|ui| {
                        if y_ok {
                            ui.label(RichText::new("✔").strong().color(theme::OK_GREEN));
                            ui.label("Mandrel at 0°");
                        } else {
                            ui.label(RichText::new("!").strong().color(theme::WARN_AMBER));
                            ui.label(format!(
                                "The first move (G0 Y0) turns the mandrel {y:.1}° back to 0°."
                            ));
                            if ui.small_button("Set Y = 0° here").clicked()
                                && let Some(c) = &self.conn {
                                    c.send("G92 Y0");
                                }
                        }
                    });
                    if src.from_designer && self.designer.is_stale(self.settings.verbose_gcode) {
                        check(ui, false, "", "The wind changed since this G-code was generated");
                    }
                    if self.speed != 100 {
                        ui.label(format!("Speed override is {}%.", self.speed));
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Typhoon keeps Windows awake while it winds. Keep the laptop plugged \
                             in and the lid open.",
                        )
                        .small()
                        .color(p.text_dim),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let go = Button::new(RichText::new("Start winding").strong().color(Color32::WHITE))
                            .fill(theme::RED);
                        if ui.add(go).clicked() {
                            self.start_job();
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::StopJob => {
                    ui.heading("Stop the job?");
                    ui.label(
                        "Motion stops at once (M410) and the job can't be resumed. Pause instead \
                         if you only need to fix the tow.",
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let stop = Button::new(RichText::new("Stop now").strong().color(Color32::WHITE))
                            .fill(theme::RED);
                        if ui.add(stop).clicked() {
                            if let Some(c) = &self.conn {
                                c.stop_job();
                            }
                            close = true;
                        }
                        if ui.button("Keep winding").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Quit => {
                    ui.heading("A job is running");
                    ui.label("Quitting stops the job: motion stops at once (M410).");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let quit = Button::new(RichText::new("Stop job and quit").strong().color(Color32::WHITE))
                            .fill(theme::RED);
                        if ui.add(quit).clicked() {
                            if let Some(conn) = self.conn.take() {
                                conn.stop_job();
                                conn.shutdown(Duration::from_millis(1500));
                            }
                            self.job.phase = JobPhase::Idle;
                            close = true;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Keep winding").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if close || (modal.should_close() && dialog != Dialog::Quit) {
            self.dialog = None;
        }

        // A dialog's backdrop must never block E-STOP: draw it again on a
        // layer above the dialog, in the same place.
        if let (Some(rect), true) = (self.estop_rect, self.conn.is_some()) {
            let mut pressed = false;
            egui::Area::new(egui::Id::unique("estop_above_dialogs"))
                .order(egui::Order::Tooltip)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let estop = Button::new(RichText::new("E-STOP").strong().color(Color32::WHITE))
                        .fill(theme::RED)
                        .min_size(rect.size());
                    pressed = ui
                        .add_sized(rect.size(), estop)
                        .on_hover_text("Send M112: Marlin stops everything and halts until reset")
                        .clicked();
                });
            if pressed {
                if let Some(conn) = &self.conn {
                    conn.emergency_stop();
                }
                self.dialog = None;
            }
        }
    }

    // ---------------------------------------------------------------- console

    /// Exactly fills the space it is given; the log scrolls inside it.
    fn console_card(&mut self, ui: &mut Ui, height: f32) {
        // Typing is allowed when idle or paused, not while job lines stream.
        let can_type =
            self.connected() && matches!(self.job.phase, JobPhase::Idle | JobPhase::Paused);
        let why_not = if self.connected() { "Pause the job to type commands" } else { "Not connected" };
        let height = height.max(MIN_CONSOLE_HEIGHT);
        let p = theme::pal(ui.ctx());
        theme::card_fixed(ui, "Console", height, |ui| {
            let mut clear = false;
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.settings.console_hide_ok, "Hide ok / busy");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    clear = ui.small_button("Clear").on_hover_text("Clear the console").clicked();
                });
            });
            if clear {
                self.console.clear();
            }
            let spacing = ui.spacing().item_spacing.y;
            let input_row = ui.spacing().interact_size.y + 4.0;
            let log_frame_margin = 12.0;
            let log_height =
                (ui.available_height() - input_row - log_frame_margin - spacing * 2.0).max(24.0);
            Frame::NONE
                .fill(p.well)
                .corner_radius(4)
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    let hide_quiet = self.settings.console_hide_ok;
                    let shown: Vec<&ConsoleLine> = self
                        .console
                        .iter()
                        .filter(|l| !(hide_quiet && l.kind == LogKind::RxQuiet))
                        .collect();
                    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
                    // Only the visible rows are laid out, so long logs stay cheap.
                    ScrollArea::vertical()
                        .id_salt("console_log")
                        .max_height(log_height)
                        .min_scrolled_height(log_height)
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show_rows(ui, row_height, shown.len(), |ui, rows| {
                            for line in &shown[rows] {
                                let (prefix, color) = match line.kind {
                                    LogKind::Tx => ("> ", p.text),
                                    LogKind::Rx => ("", p.text),
                                    LogKind::RxQuiet | LogKind::Info => ("", p.text_dim),
                                    LogKind::Error => ("", theme::RED),
                                };
                                let text = RichText::new(format!("{prefix}{}", line.text))
                                    .monospace()
                                    .color(color);
                                ui.add(egui::Label::new(text).truncate());
                            }
                        });
                });
            let mut to_send = None;
            let input_id = egui::Id::unique("console_input");
            // ↑/↓ browse previously sent commands (taken before the text box sees them).
            if ui.memory(|m| m.has_focus(input_id)) {
                let (up, down) = ui.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    )
                });
                if up || down {
                    self.browse_history(up);
                    let mut state =
                        egui::text_edit::TextEditState::load(ui.ctx(), input_id).unwrap_or_default();
                    let end = egui::text::CCursor::new(self.console_input.chars().count());
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(end)));
                    state.store(ui.ctx(), input_id);
                }
            }
            ui.horizontal(|ui| {
                ui.add_enabled_ui(can_type, |ui| {
                    let send_w = 60.0;
                    let edit = ui.add(
                        TextEdit::singleline(&mut self.console_input)
                            .id(input_id)
                            .hint_text("Send G-code…")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(ui.available_width() - send_w - 8.0),
                    );
                    let edit = edit.on_hover_text(
                        "Enter sends. Up/Down arrows recall earlier commands. \
                         Separate several commands with |",
                    );
                    let enter =
                        edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let clicked = ui.add_sized([send_w, 26.0], Button::new("Send")).clicked();
                    if (enter || clicked) && !self.console_input.trim().is_empty() {
                        to_send = Some(std::mem::take(&mut self.console_input));
                        edit.request_focus();
                    }
                })
                .response
                .on_disabled_hover_text(why_not);
            });
            if let Some(line) = to_send {
                self.remember_command(&line);
                // Several commands can be sent at once, separated by "|".
                for cmd in line.split('|') {
                    self.send(cmd.trim());
                }
            }
        });
    }
}

/// A tab: always drawn as an outlined box so it reads as clickable; the
/// selected one is filled red.
fn tab_button(ui: &mut Ui, selected: bool, label: &str) -> egui::Response {
    let p = theme::pal(ui.ctx());
    let (text, fill, stroke) = if selected {
        (RichText::new(label).strong().color(Color32::WHITE), theme::RED, theme::RED)
    } else {
        (RichText::new(label), p.inset, p.text_dim.gamma_multiply(0.6))
    };
    ui.add(
        Button::new(text)
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(6)
            .min_size(egui::vec2(96.0, 30.0)),
    )
}

/// One axis: −/+ buttons and the step size. Returns the signed distance
/// to jog if a button was clicked.
fn jog_row(
    ui: &mut Ui,
    disabled: Option<&str>,
    compact: bool,
    axis: &str,
    unit: &str,
    steps: &[f32],
    selected: &mut usize,
) -> Option<f32> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(axis).strong().color(theme::RED));
        let step = steps[*selected];
        for (sign, factor) in [("−", -1.0), ("+", 1.0)] {
            if ui
                .add_enabled(
                    disabled.is_none(),
                    Button::new(format!("{axis}{sign}")).min_size(egui::vec2(38.0, 26.0)),
                )
                .on_hover_text(format!("Move {axis} by {}{step} {unit}", if factor < 0.0 { "−" } else { "+" }))
                .on_disabled_hover_text(disabled.unwrap_or(""))
                .clicked()
            {
                clicked = Some(step * factor);
            }
        }
        ui.add_space(4.0);
        if compact {
            egui::ComboBox::from_id_salt(("jog_step", axis))
                .width(70.0)
                .selected_text(format!("{} {unit}", steps[*selected]))
                .show_ui(ui, |ui| {
                    for (i, step) in steps.iter().enumerate() {
                        ui.selectable_value(selected, i, format!("{step} {unit}"));
                    }
                })
                .response
                .on_hover_text("Distance per click");
        } else {
            for (i, step) in steps.iter().enumerate() {
                ui.selectable_value(selected, i, format!("{}", step))
                    .on_hover_text(format!("{step} {unit} per click"));
            }
        }
    });
    clicked
}

impl eframe::App for TyphoonApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_serial_events();
        self.designer.poll();
        self.pick_up_generated();
        // The preview follows the running job, else whatever Start would wind.
        let lines = if self.job.active() {
            self.job.lines.clone()
        } else {
            self.job_source.as_ref().map(|s| s.lines.clone())
        };
        self.preview.sync(ctx, lines.as_ref());
        if self.job.active() {
            // Keep the elapsed/ETA clock ticking.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let p = theme::pal(ui.ctx());
        self.background.paint(
            ui.painter(),
            ui.max_rect(),
            p.background,
            p.dark,
            self.settings.logo_opacity,
        );

        let width = ui.available_width();
        let height = ui.available_height();
        let bar = Frame::NONE
            .fill(p.top_bar)
            .inner_margin(Margin::symmetric(14, 8));
        let top = egui::Panel::top("top_bar")
            .frame(bar)
            .resizable(false)
            .show(ui, |ui| self.top_bar(ui));
        self.top_bar_bottom = top.response.rect.bottom();

        let side = Frame::NONE.inner_margin(Margin::same(10));
        let narrow = width < NARROW_WIDTH;
        egui::Panel::left("toolhead")
            .frame(side)
            .default_size(if narrow { 290.0 } else { 330.0 })
            .size_range(250.0..=(width * 0.32).clamp(260.0, 460.0))
            .show_separator_line(false)
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt("toolhead_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.toolhead_card(ui));
            });

        self.console_dock = if width >= WIDE_WIDTH {
            ConsoleDock::Right
        } else if height >= SHORT_HEIGHT {
            ConsoleDock::Bottom
        } else {
            ConsoleDock::Tab
        };
        if self.console_dock == ConsoleDock::Right {
            // Wide screens: console in its own column on the right, never
            // more than a third of the window.
            egui::Panel::right("console")
                .frame(side)
                .default_size(360.0)
                .size_range(260.0..=(width * 0.33).max(280.0))
                .show_separator_line(false)
                .show(ui, |ui| self.console_card(ui, ui.available_height()));
        } else if self.console_dock == ConsoleDock::Bottom {
            // Narrower / squarer screens: console docks under the designer,
            // never more than 40% of the window height.
            let max_h = (height * 0.4).max(MIN_CONSOLE_HEIGHT);
            egui::Panel::bottom("console_bottom")
                .frame(Frame::NONE.inner_margin(Margin {
                    left: 4,
                    right: 10,
                    top: 0,
                    bottom: 10,
                }))
                .default_size((height * 0.28).clamp(MIN_CONSOLE_HEIGHT, max_h))
                .size_range(MIN_CONSOLE_HEIGHT..=max_h)
                .show_separator_line(false)
                .show(ui, |ui| self.console_card(ui, ui.available_height()));
        }

        egui::CentralPanel::no_frame()
            .frame(Frame::NONE.inner_margin(Margin::symmetric(4, 10)))
            .show(ui, |ui| self.central(ui));

        let ctx = ui.ctx().clone();
        // Closing the window mid-wind needs a confirmation.
        if ctx.input(|i| i.viewport().close_requested()) && self.job.active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::Quit);
        }
        self.preferences_window(&ctx);
        self.dialogs(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.wind_json = Some(self.designer.to_json());
        self.settings.last_wind_file = self.designer.path_string();
        self.settings.save(storage);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(visuals.panel_fill).to_array()
    }
}
