//! Wind designer: edit a cyclone `.wind` (mandrel, tow, layers), check it,
//! and generate G-code on a background thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, DragValue, Frame, Layout, Margin, RichText,
    ScrollArea, Stroke, Ui,
};

use crate::planner::validate::{self, Issue, Severity};
use crate::planner::{
    HelicalLayer, HoopLayer, Layer, MandrelParameters, PlanResult, SkipLayer,
    TowParameters, WindParameters, plan_wind,
};
use crate::theme;

/// Width at which the summary moves beside the editor instead of below it.
const SIDE_BY_SIDE_WIDTH: f32 = 640.0;
const SUMMARY_WIDTH: f32 = 270.0;

pub fn default_wind() -> WindParameters {
    // The example from cyclone's README.
    WindParameters {
        layers: vec![
            Layer::Hoop(HoopLayer { terminal: false }),
            Layer::Helical(default_helical()),
        ],
        mandrel_parameters: MandrelParameters {
            diameter: 69.75,
            wind_length: 940.0,
        },
        tow_parameters: TowParameters {
            width: 7.0,
            thickness: 0.5,
        },
        default_feed_rate: 9000.0,
    }
}

fn default_helical() -> HelicalLayer {
    HelicalLayer {
        wind_angle: 55.0,
        pattern_number: 2.0,
        skip_index: 1.0,
        lock_degrees: 720.0,
        lead_in_mm: 30.0,
        lead_out_degrees: 90.0,
        skip_initial_near_lock: None,
    }
}

fn default_layer(kind: &str) -> Layer {
    match kind {
        "helical" => Layer::Helical(default_helical()),
        "skip" => Layer::Skip(SkipLayer {
            mandrel_rotation: 90.0,
        }),
        _ => Layer::Hoop(HoopLayer { terminal: false }),
    }
}

/// G-code produced from a snapshot of the designer's wind.
pub struct Generated {
    pub result: Arc<PlanResult>,
    pub params: WindParameters,
    pub verbose: bool,
    pub elapsed: Duration,
    /// Suggested file stem (from the .wind file name).
    pub name: String,
}

struct PlanJob {
    rx: Receiver<(PlanResult, Duration)>,
    params: WindParameters,
    verbose: bool,
    name: String,
}

#[derive(Clone, Copy)]
enum Pending {
    New,
    Open,
}

enum LayerAction {
    Up(usize),
    Down(usize),
    Duplicate(usize),
    Delete(usize),
    Add(&'static str),
}

pub struct Designer {
    pub params: WindParameters,
    path: Option<PathBuf>,
    /// Content as last loaded or saved, to tell whether there are unsaved edits.
    saved: Option<WindParameters>,
    /// Message from the last file operation: (text, is_error).
    status: Option<(String, bool)>,
    job: Option<PlanJob>,
    generated: Option<Generated>,
    pending: Option<Pending>,
    /// Per-layer collapsed flags, kept in step with `params.layers`.
    collapsed: Vec<bool>,
}

impl Designer {
    /// Restore the wind from saved settings, or start from cyclone's example.
    pub fn new(saved_json: Option<&str>, path: Option<&str>) -> Self {
        let params = saved_json
            .and_then(|j| WindParameters::from_json(j).ok())
            .unwrap_or_else(default_wind);
        let path = path.map(PathBuf::from);
        // If the file on disk still matches, it isn't "unsaved".
        let saved = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| WindParameters::from_json(&t).ok());
        let collapsed = vec![false; params.layers.len()];
        Self {
            params,
            path,
            saved,
            status: None,
            job: None,
            generated: None,
            pending: None,
            collapsed,
        }
    }

    pub fn path_string(&self) -> Option<String> {
        self.path.as_ref().map(|p| p.display().to_string())
    }

    pub fn to_json(&self) -> String {
        self.params.to_json_pretty()
    }

    fn dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.params)
    }

    fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled.wind".into())
    }

    fn file_stem(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "wind".into())
    }

    pub fn generated(&self) -> Option<&Generated> {
        self.generated.as_ref()
    }

    /// The wind has changed since the G-code was generated.
    pub fn is_stale(&self, verbose: bool) -> bool {
        self.generated
            .as_ref()
            .is_some_and(|g| g.params != self.params || g.verbose != verbose)
    }

    pub fn is_generating(&self) -> bool {
        self.job.is_some()
    }

    // ------------------------------------------------------------ planning

    fn start_generate(&mut self, ctx: &egui::Context, verbose: bool) {
        let params = self.params.clone();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let snapshot = params.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = plan_wind(&snapshot, verbose);
            let _ = tx.send((result, started.elapsed()));
            ctx.request_repaint();
        });
        self.job = Some(PlanJob {
            rx,
            params,
            verbose,
            name: self.file_stem(),
        });
    }

    /// Pick up a finished background plan. Call once per frame.
    pub fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        match job.rx.try_recv() {
            Ok((result, elapsed)) => {
                let job = self.job.take().expect("checked above");
                self.generated = Some(Generated {
                    result: Arc::new(result),
                    params: job.params,
                    verbose: job.verbose,
                    elapsed,
                    name: job.name,
                });
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.status = Some(("The planner stopped unexpectedly.".into(), true));
            }
        }
    }

    // --------------------------------------------------------------- files

    fn new_wind(&mut self) {
        self.params = default_wind();
        self.collapsed = vec![false; self.params.layers.len()];
        self.path = None;
        self.saved = None;
        self.status = None;
    }

    fn open_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open wind")
            .add_filter("Cyclone wind", &["wind", "json"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.pick_file() else { return };
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| WindParameters::from_json(&t).map_err(|e| e.to_string()))
        {
            Ok(params) => {
                self.collapsed = vec![false; params.layers.len()];
                self.saved = Some(params.clone());
                self.params = params;
                self.status = Some((format!("Opened {}", path.display()), false));
                self.path = Some(path);
            }
            Err(e) => {
                self.status = Some((format!("Could not open {}: {e}", path.display()), true));
            }
        }
    }

    fn save(&mut self, save_as: bool) {
        let path = match (&self.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save wind")
                    .add_filter("Cyclone wind", &["wind"])
                    .set_file_name(self.file_name());
                if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(dir);
                }
                match dialog.save_file() {
                    Some(p) => p,
                    None => return,
                }
            }
        };
        match std::fs::write(&path, self.to_json()) {
            Ok(()) => {
                self.saved = Some(self.params.clone());
                self.status = Some((format!("Saved {}", path.display()), false));
                self.path = Some(path);
            }
            Err(e) => self.status = Some((format!("Could not save {}: {e}", path.display()), true)),
        }
    }

    /// Ask where to save the generated G-code and write it. Returns a status message.
    pub fn save_gcode_dialog(&self) -> Option<Result<String, String>> {
        let generated = self.generated.as_ref()?;
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save G-code")
            .add_filter("G-code", &["gcode", "gco", "nc"])
            .set_file_name(format!("{}.gcode", generated.name));
        if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let path = dialog.save_file()?;
        Some(
            std::fs::write(&path, generated.result.gcode_text())
                .map(|()| format!("Wrote {} lines to {}", generated.result.gcode.len(), path.display()))
                .map_err(|e| format!("Could not write {}: {e}", path.display())),
        )
    }

    // ------------------------------------------------------------------ UI

    pub fn ui(&mut self, ui: &mut Ui, verbose: &mut bool) {
        let issues = validate::validate(&self.params);
        let has_errors = issues.iter().any(|i| i.severity == Severity::Error);

        self.toolbar(ui, verbose, &issues, has_errors);
        if let Some((text, is_error)) = &self.status {
            let color = if *is_error { theme::RED } else { theme::pal(ui.ctx()).text_dim };
            ui.label(RichText::new(text).small().color(color));
        }
        ui.add_space(2.0);

        let avail = ui.available_size();
        if avail.x >= SIDE_BY_SIDE_WIDTH {
            ui.horizontal_top(|ui| {
                let gap = ui.spacing().item_spacing.x * 2.0 + 1.0;
                let left_w = avail.x - SUMMARY_WIDTH - gap;
                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, avail.y),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ScrollArea::vertical()
                            .id_salt("designer_editor")
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.editor(ui, &issues));
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(SUMMARY_WIDTH, avail.y),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ScrollArea::vertical()
                            .id_salt("designer_summary")
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.summary(ui, *verbose, &issues));
                    },
                );
            });
        } else {
            ScrollArea::vertical()
                .id_salt("designer_stacked")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.editor(ui, &issues);
                    ui.add_space(8.0);
                    ui.separator();
                    self.summary(ui, *verbose, &issues);
                });
        }

        self.confirm_discard(ui.ctx());
    }

    fn toolbar(&mut self, ui: &mut Ui, verbose: &mut bool, issues: &[Issue], has_errors: bool) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("🗋 New").on_hover_text("Start from cyclone's example wind").clicked() {
                if self.dirty() {
                    self.pending = Some(Pending::New);
                } else {
                    self.new_wind();
                }
            }
            if ui.button("Open…").clicked() {
                if self.dirty() {
                    self.pending = Some(Pending::Open);
                } else {
                    self.open_dialog();
                }
            }
            if ui.button("💾 Save").clicked() {
                self.save(false);
            }
            if ui.button("Save as…").clicked() {
                self.save(true);
            }
            let p = theme::pal(ui.ctx());
            let mut name = RichText::new(self.file_name()).color(p.text_dim);
            if self.dirty() {
                name = RichText::new(format!("{} •", self.file_name())).color(p.text);
            }
            ui.label(name).on_hover_text(match (&self.path, self.dirty()) {
                (Some(path), true) => format!("{} (unsaved changes)", path.display()),
                (Some(path), false) => path.display().to_string(),
                (None, _) => "Not saved to a file yet".into(),
            });

            ui.add_space(10.0);
            let generating = self.is_generating();
            let label = if generating { "Generating…" } else { "▶ Generate G-code" };
            let button = Button::new(RichText::new(label).strong().color(Color32::WHITE))
                .fill(theme::RED);
            let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
            let response = ui
                .add_enabled(!generating && !has_errors, button)
                .on_hover_text("Plan the wind into G-code (runs in the background)")
                .on_disabled_hover_text(if generating {
                    "Already generating".to_string()
                } else {
                    format!("Fix the {errors} error(s) listed in the summary first")
                });
            if response.clicked() {
                self.start_generate(ui.ctx(), *verbose);
            }
            ui.checkbox(verbose, "Verbose comments")
                .on_hover_text("Add a comment explaining each move, like cyclone's -v flag");
        });
    }

    fn editor(&mut self, ui: &mut Ui, issues: &[Issue]) {
        section_title(ui, "Mandrel & tow");
        let params = &mut self.params;
        egui::Grid::new("mandrel_grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                num_row(ui, "Mandrel diameter", &mut params.mandrel_parameters.diameter, " mm", 0.1, 0.0..=10_000.0);
                num_row(ui, "Wind length", &mut params.mandrel_parameters.wind_length, " mm", 1.0, 0.0..=100_000.0);
                num_row(ui, "Tow width", &mut params.tow_parameters.width, " mm", 0.05, 0.0..=1_000.0);
                num_row(ui, "Tow thickness", &mut params.tow_parameters.thickness, " mm", 0.01, 0.0..=100.0)
                    .on_hover_text("Saved in the file; cyclone's planner does not use it yet");
                num_row(ui, "Feed rate", &mut params.default_feed_rate, " /min", 10.0, 0.0..=100_000.0)
                    .on_hover_text("Marlin feed rate for every move (G0 F…)");
            });

        ui.add_space(10.0);
        section_title(ui, "Layers");
        self.collapsed.resize(self.params.layers.len(), false);

        let mut action = None;
        let count = self.params.layers.len();
        for i in 0..count {
            let layer_issues: Vec<&Issue> = issues.iter().filter(|x| x.layer == Some(i)).collect();
            if let Some(a) = self.layer_row(ui, i, count, &layer_issues) {
                action = Some(a);
            }
            ui.add_space(6.0);
        }
        if count == 0 {
            ui.label(RichText::new("No layers yet.").color(theme::pal(ui.ctx()).text_dim));
        }

        ui.horizontal_wrapped(|ui| {
            ui.label("Add:");
            for (kind, label) in [("hoop", "➕ Hoop"), ("helical", "➕ Helical"), ("skip", "➕ Skip")] {
                if ui.button(label).clicked() {
                    action = Some(LayerAction::Add(kind));
                }
            }
        });

        if let Some(action) = action {
            self.apply(action);
        }
    }

    fn apply(&mut self, action: LayerAction) {
        let layers = &mut self.params.layers;
        let collapsed = &mut self.collapsed;
        match action {
            LayerAction::Up(i) if i > 0 => {
                layers.swap(i, i - 1);
                collapsed.swap(i, i - 1);
            }
            LayerAction::Down(i) if i + 1 < layers.len() => {
                layers.swap(i, i + 1);
                collapsed.swap(i, i + 1);
            }
            LayerAction::Duplicate(i) => {
                layers.insert(i + 1, layers[i].clone());
                collapsed.insert(i + 1, collapsed[i]);
            }
            LayerAction::Delete(i) => {
                layers.remove(i);
                collapsed.remove(i);
            }
            LayerAction::Add(kind) => {
                layers.push(default_layer(kind));
                collapsed.push(false);
            }
            _ => {}
        }
    }

    fn layer_row(
        &mut self,
        ui: &mut Ui,
        i: usize,
        count: usize,
        issues: &[&Issue],
    ) -> Option<LayerAction> {
        let p = theme::pal(ui.ctx());
        let worst = issues.iter().map(|x| x.severity).max();
        let stroke_color = match worst {
            Some(Severity::Error) => theme::RED,
            Some(Severity::Warning) => theme::WARN_AMBER,
            None => p.card_stroke,
        };
        let mut action = None;
        let mandrel = self.params.mandrel_parameters.clone();
        let tow = self.params.tow_parameters.clone();

        Frame::NONE
            .fill(p.inset)
            .stroke(Stroke::new(1.0, stroke_color))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Header: collapse toggle, number, type, reorder/duplicate/delete.
                ui.horizontal(|ui| {
                    let collapsed = &mut self.collapsed[i];
                    if ui
                        .add(Button::new(if *collapsed { "⏵" } else { "⏷" }).frame(false))
                        .on_hover_text(if *collapsed { "Expand" } else { "Collapse" })
                        .clicked()
                    {
                        *collapsed = !*collapsed;
                    }
                    ui.label(RichText::new(format!("{}", i + 1)).strong().color(theme::RED));

                    let layer = &mut self.params.layers[i];
                    let current = layer.wind_type();
                    egui::ComboBox::from_id_salt(("layer_type", i))
                        .width(92.0)
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for kind in ["hoop", "helical", "skip"] {
                                if ui.selectable_label(current == kind, kind).clicked() && current != kind {
                                    *layer = default_layer(kind);
                                }
                            }
                        });
                    if self.collapsed[i] {
                        ui.label(RichText::new(layer_summary(&self.params.layers[i])).color(p.text_dim));
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let small = |t: &str| Button::new(t).min_size(egui::vec2(26.0, 24.0));
                        if ui.add(small("🗑")).on_hover_text("Delete layer").clicked() {
                            action = Some(LayerAction::Delete(i));
                        }
                        if ui.add(small("📋")).on_hover_text("Duplicate layer").clicked() {
                            action = Some(LayerAction::Duplicate(i));
                        }
                        if ui
                            .add_enabled(i + 1 < count, small("⏷"))
                            .on_hover_text("Move down")
                            .clicked()
                        {
                            action = Some(LayerAction::Down(i));
                        }
                        if ui.add_enabled(i > 0, small("⏶")).on_hover_text("Move up").clicked() {
                            action = Some(LayerAction::Up(i));
                        }
                    });
                });

                if !self.collapsed[i] {
                    ui.add_space(4.0);
                    match &mut self.params.layers[i] {
                        Layer::Hoop(h) => hoop_fields(ui, h, &mandrel, &tow),
                        Layer::Helical(h) => helical_fields(ui, i, h, &mandrel, &tow),
                        Layer::Skip(s) => {
                            egui::Grid::new(("skip_grid", i)).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                                num_row(ui, "Mandrel rotation", &mut s.mandrel_rotation, "°", 1.0, -100_000.0..=100_000.0)
                                    .on_hover_text("Advance the mandrel by this much, then zero it");
                            });
                        }
                    }
                }

                for issue in issues {
                    issue_label(ui, issue);
                }
            });
        action
    }

    fn summary(&mut self, ui: &mut Ui, verbose: bool, issues: &[Issue]) {
        let p = theme::pal(ui.ctx());
        section_title(ui, "Summary");

        let wind_issues: Vec<&Issue> = issues.iter().filter(|i| i.layer.is_none()).collect();
        let layer_issue_count = issues.len() - wind_issues.len();
        for issue in &wind_issues {
            issue_label(ui, issue);
        }
        if layer_issue_count > 0 {
            ui.label(
                RichText::new(format!("{layer_issue_count} layer issue(s), shown on the layers."))
                    .color(theme::WARN_AMBER),
            );
        }
        if issues.is_empty() {
            ui.label(RichText::new("✔ No problems found").color(theme::OK_GREEN));
        }
        ui.add_space(8.0);

        if self.is_generating() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Generating G-code…");
            });
            return;
        }
        let stale = self.is_stale(verbose);
        let Some(g) = &self.generated else {
            ui.label(RichText::new("Press Generate to plan the G-code.").color(p.text_dim));
            return;
        };
        let r = &g.result;

        if stale {
            ui.label(
                RichText::new("⚠ The wind changed since this was generated. Generate again to update it.")
                    .color(theme::WARN_AMBER),
            );
        }
        egui::Grid::new("totals").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label("Estimated time");
            ui.label(RichText::new(format_duration(r.total_time_s)).strong());
            ui.end_row();
            ui.label("Tow required");
            ui.label(RichText::new(format!("{:.2} m", r.total_tow_m)).strong());
            ui.end_row();
            ui.label("G-code lines");
            ui.label(RichText::new(r.gcode.len().to_string()).strong());
            ui.end_row();
        });
        ui.label(
            RichText::new(format!(
                "Planned in {:.0} ms. Time assumes instant acceleration, like cyclone.",
                g.elapsed.as_secs_f64() * 1000.0
            ))
            .small()
            .color(p.text_dim),
        );

        ui.add_space(6.0);
        egui::Grid::new("per_layer")
            .num_columns(4)
            .striped(true)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                for h in ["#", "Layer", "Time", "Tow"] {
                    ui.label(RichText::new(h).small().color(p.text_dim));
                }
                ui.end_row();
                for l in &r.layers {
                    ui.label((l.index + 1).to_string());
                    ui.label(l.wind_type);
                    ui.label(format_duration(l.time_s));
                    ui.label(format!("{:.2} m", l.tow_m));
                    ui.end_row();
                }
            });

        let warnings: Vec<&str> = r.warnings().collect();
        if !warnings.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("Planner warnings").strong());
            for w in warnings {
                ui.label(RichText::new(format!("⚠ {w}")).color(theme::WARN_AMBER));
            }
        }
    }

    fn confirm_discard(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending else { return };
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("discard_changes")).show(ctx, |ui| {
            ui.set_max_width(340.0);
            ui.heading("Discard unsaved changes?");
            ui.label(format!("{} has changes that are not saved to a file.", self.file_name()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(Button::new(RichText::new("Discard").color(Color32::WHITE)).fill(theme::RED))
                    .clicked()
                {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        if modal.should_close() && choice.is_none() {
            choice = Some(false);
        }
        match choice {
            Some(true) => {
                self.pending = None;
                match pending {
                    Pending::New => self.new_wind(),
                    // The wind is only replaced if a file is actually chosen.
                    Pending::Open => self.open_dialog(),
                }
            }
            Some(false) => self.pending = None,
            None => {}
        }
    }
}

fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).strong().size(15.0));
    ui.add_space(2.0);
}

/// One label + DragValue row in a two-column grid.
fn num_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f64,
    suffix: &str,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> egui::Response {
    ui.label(label);
    let r = ui.add(
        DragValue::new(value)
            .speed(speed)
            .range(range)
            .max_decimals(4)
            .suffix(suffix),
    );
    ui.end_row();
    r
}

fn hoop_fields(ui: &mut Ui, h: &mut HoopLayer, mandrel: &MandrelParameters, tow: &TowParameters) {
    ui.checkbox(&mut h.terminal, "Terminal (one way only, ends at the far end)")
        .on_hover_text("Useful for heat-shrink tape. No layers may follow a terminal layer.");
    if let Some(info) = validate::hoop_info(mandrel, tow) {
        info_line(
            ui,
            &format!(
                "Head tilt {:.2}° · {:.1} mandrel turns per pass",
                info.head_angle_deg, info.turns_per_pass
            ),
        );
    }
}

fn helical_fields(
    ui: &mut Ui,
    i: usize,
    h: &mut HelicalLayer,
    mandrel: &MandrelParameters,
    tow: &TowParameters,
) {
    egui::Grid::new(("helical_grid", i))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            num_row(ui, "Wind angle", &mut h.wind_angle, "°", 0.1, 0.0..=90.0)
                .on_hover_text("Angle of the tow, measured from the mandrel's circumference direction");
            ui.label("Pattern number");
            ui.add(DragValue::new(&mut h.pattern_number).speed(0.05).range(1.0..=10_000.0).fixed_decimals(0))
                .on_hover_text("How many evenly spaced start positions around the mandrel");
            ui.end_row();
            ui.label("Skip index");
            ui.add(DragValue::new(&mut h.skip_index).speed(0.05).range(0.0..=10_000.0).fixed_decimals(0))
                .on_hover_text("Saved in the file; cyclone's planner does not use it yet");
            ui.end_row();
            num_row(ui, "Lock", &mut h.lock_degrees, "°", 1.0, 0.0..=100_000.0)
                .on_hover_text("Mandrel rotation at each end of a pass");
            num_row(ui, "Lead-in", &mut h.lead_in_mm, " mm", 0.5, 0.0..=100_000.0)
                .on_hover_text("Carriage travel at each end while the head tilts into place");
            num_row(ui, "Lead-out", &mut h.lead_out_degrees, "°", 1.0, 0.0..=100_000.0)
                .on_hover_text("Part of the lock during which the head returns to level");
        });
    let mut skip = h.skip_initial_near_lock.unwrap_or(false);
    if ui
        .checkbox(&mut skip, "Skip initial near lock")
        .on_hover_text("For consecutive helical layers: skip the extra lock at the start")
        .changed()
    {
        h.skip_initial_near_lock = Some(skip);
    }

    if let Some(info) = validate::helical_info(h, mandrel, tow) {
        info_line(
            ui,
            &format!(
                "{} circuits · tow covers {:.2} mm · {:.1}° mandrel per pass",
                info.num_circuits, info.tow_arc_length_mm, info.pass_rotation_deg
            ),
        );
        if !info.valid_pattern_numbers.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Valid pattern numbers:").small());
                for n in &info.valid_pattern_numbers {
                    let selected = h.pattern_number == *n as f64;
                    if ui
                        .selectable_label(selected, n.to_string())
                        .on_hover_text("Use this pattern number")
                        .clicked()
                    {
                        h.pattern_number = *n as f64;
                    }
                }
            });
        }
    }
}

fn info_line(ui: &mut Ui, text: &str) {
    let dim = theme::pal(ui.ctx()).text_dim;
    ui.label(RichText::new(text).small().color(dim));
}

fn issue_label(ui: &mut Ui, issue: &Issue) {
    let (icon, color) = match issue.severity {
        Severity::Error => ("❌", theme::RED),
        Severity::Warning => ("⚠", theme::WARN_AMBER),
    };
    ui.label(RichText::new(format!("{icon} {}", issue.message)).color(color));
}

/// Short one-line description shown when a layer is collapsed.
fn layer_summary(layer: &Layer) -> String {
    match layer {
        Layer::Hoop(h) => if h.terminal { "terminal".into() } else { String::new() },
        Layer::Helical(h) => format!("{}° · pattern {}", h.wind_angle, h.pattern_number),
        Layer::Skip(s) => format!("{}°", s.mandrel_rotation),
    }
}

pub fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".into();
    }
    let s = seconds.round() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_wind_is_valid() {
        let issues = validate::validate(&default_wind());
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(0.4), "0s");
        assert_eq!(format_duration(65.0), "1m 05s");
        assert_eq!(format_duration(3725.0), "1h 02m 05s");
    }
}
