//! Space Bar brand palette and Mainsail-style dark and light themes.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Frame, Margin, Rect, RichText, Shadow, Stroke, TextStyle,
    Theme, Ui, UiBuilder,
};
use serde::{Deserialize, Serialize};

/// Space Bar Grey (#262626).
pub const GREY: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
/// Space Bar Red (#EF3340): accent and E-STOP.
pub const RED: Color32 = Color32::from_rgb(0xEF, 0x33, 0x40);
/// Space Bar White (#F2F2F2).
pub const WHITE: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF2);

pub const OK_GREEN: Color32 = Color32::from_rgb(0x4C, 0xAF, 0x50);
pub const WARN_AMBER: Color32 = Color32::from_rgb(0xE0, 0x9A, 0x00);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeChoice {
    /// Follow the operating system's light/dark setting.
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "🖥 System",
            ThemeChoice::Dark => "🌙 Dark",
            ThemeChoice::Light => "☀ Light",
        }
    }
    fn preference(self) -> egui::ThemePreference {
        match self {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
            ThemeChoice::Light => egui::ThemePreference::Light,
        }
    }
}

/// Colours that differ between the dark and light themes.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub background: Color32,
    pub card: Color32,
    pub card_header: Color32,
    pub card_stroke: Color32,
    pub top_bar: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    /// Console log and other "well" backgrounds.
    pub well: Color32,
    /// Background of a layer row inside the designer.
    pub inset: Color32,
}

const DARK: Palette = Palette {
    dark: true,
    background: Color32::from_rgb(0x1A, 0x1A, 0x1A),
    card: Color32::from_rgba_premultiplied(0x21, 0x21, 0x21, 0xD2),
    card_header: Color32::from_rgb(0x30, 0x30, 0x30),
    card_stroke: Color32::from_rgb(0x34, 0x34, 0x34),
    top_bar: Color32::from_rgba_premultiplied(0x12, 0x12, 0x12, 0xE6),
    text: WHITE,
    text_dim: Color32::from_rgb(0xA0, 0xA0, 0xA0),
    well: Color32::from_rgb(0x14, 0x14, 0x14),
    inset: Color32::from_rgb(0x2C, 0x2C, 0x2C),
};

const LIGHT: Palette = Palette {
    dark: false,
    background: WHITE,
    card: Color32::from_rgba_premultiplied(0xEB, 0xEB, 0xEB, 0xEB),
    card_header: Color32::from_rgb(0xE4, 0xE4, 0xE4),
    card_stroke: Color32::from_rgb(0xD4, 0xD4, 0xD4),
    top_bar: Color32::from_rgba_premultiplied(0xFF, 0xFF, 0xFF, 0xF0),
    text: GREY,
    text_dim: Color32::from_rgb(0x6E, 0x6E, 0x6E),
    well: Color32::from_rgb(0xF7, 0xF7, 0xF7),
    inset: Color32::from_rgb(0xF4, 0xF4, 0xF4),
};

/// The palette for whichever theme is currently showing.
pub fn pal(ctx: &egui::Context) -> Palette {
    match ctx.theme() {
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
    }
}

/// Install both styles once at startup, then pick the theme.
pub fn install(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.style_mut_of(Theme::Dark, |s| style(s, &DARK));
    ctx.style_mut_of(Theme::Light, |s| style(s, &LIGHT));
    set_choice(ctx, choice);
}

pub fn set_choice(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(choice.preference());
}

fn style(style: &mut egui::Style, p: &Palette) {
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Heading, FontId::proportional(18.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;

    let v = &mut style.visuals;
    v.dark_mode = p.dark;
    v.override_text_color = Some(p.text);
    v.panel_fill = p.background;
    v.window_fill = if p.dark { GREY } else { Color32::WHITE };
    v.extreme_bg_color = p.well;
    v.text_edit_bg_color = Some(p.well);
    v.faint_bg_color = p.inset;
    v.code_bg_color = p.well;
    v.hyperlink_color = RED;
    v.warn_fg_color = WARN_AMBER;
    v.error_fg_color = RED;
    v.selection.bg_fill = RED.gamma_multiply(if p.dark { 0.55 } else { 0.8 });
    v.selection.stroke = Stroke::new(1.0, if p.dark { WHITE } else { Color32::WHITE });
    v.window_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, p.card_stroke);
    v.window_shadow = Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }),
    };

    let (idle, hover) = if p.dark {
        (Color32::from_rgb(0x3A, 0x3A, 0x3A), Color32::from_rgb(0x4A, 0x4A, 0x4A))
    } else {
        (Color32::from_rgb(0xDC, 0xDC, 0xDC), Color32::from_rgb(0xCF, 0xCF, 0xCF))
    };
    let radius = CornerRadius::same(5);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_stroke);
    w.noninteractive.corner_radius = radius;
    for wv in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = radius;
    }
    w.inactive.bg_fill = idle;
    w.inactive.weak_bg_fill = idle;
    w.hovered.bg_fill = hover;
    w.hovered.weak_bg_fill = hover;
    w.hovered.bg_stroke = Stroke::new(1.0, RED.gamma_multiply(0.8));
    w.active.bg_fill = RED.gamma_multiply(0.7);
    w.active.weak_bg_fill = RED.gamma_multiply(0.7);
    w.open.bg_fill = hover;
    w.open.weak_bg_fill = hover;
}

/// A Mainsail-style card: title strip on top, content below. Sized to its content.
pub fn card<R>(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal(ui.ctx());
    Frame::NONE
        .fill(p.card)
        .corner_radius(CornerRadius::same(8))
        .stroke(Stroke::new(1.0, p.card_stroke))
        .shadow(Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 60 } else { 18 }),
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            Frame::NONE
                .fill(p.card_header)
                .corner_radius(CornerRadius {
                    nw: 8,
                    ne: 8,
                    sw: 0,
                    se: 0,
                })
                .inner_margin(Margin::symmetric(12, 7))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(title.to_uppercase())
                            .strong()
                            .size(12.5)
                            .extra_letter_spacing(1.0),
                    );
                });
            Frame::NONE
                .inner_margin(Margin::same(12))
                .show(ui, add_contents)
                .inner
        })
        .inner
}

/// A card that takes exactly `height` points (full width) and never grows
/// past it. Content gets a fixed-height area, so it must scroll by itself.
pub fn card_fixed<R>(
    ui: &mut Ui,
    title: &str,
    height: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let rect = Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), height));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(*ui.layout()));
    let inner = card(&mut child, title, |ui| {
        let h = ui.available_height().max(0.0);
        ui.set_min_height(h);
        ui.set_max_height(h);
        add_contents(ui)
    });
    ui.advance_cursor_after_rect(rect);
    inner
}

/// Small rounded "status chip" like Mainsail's printer state badge.
pub fn status_chip(ui: &mut Ui, text: &str, color: Color32) {
    Frame::NONE
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.7)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(RichText::new(text).size(12.5).color(color));
            });
        });
}

#[cfg(test)]
mod tests {
    use eframe::egui::epaint::text::{Fonts, TextOptions};
    use eframe::egui::{FontDefinitions, FontId};
    use std::collections::BTreeSet;

    /// Every non-ASCII character in the UI's string literals must exist in
    /// the bundled fonts, otherwise it shows up as an empty box.
    #[test]
    fn every_symbol_in_ui_text_has_a_glyph() {
        let mut fonts = Fonts::new(TextOptions::default(), FontDefinitions::default());
        let sources = [
            include_str!("app.rs"),
            include_str!("designer.rs"),
            include_str!("preview.rs"),
            include_str!("sound.rs"),
            include_str!("theme.rs"),
            include_str!("tube3d.rs"),
            include_str!("serial/mod.rs"),
            include_str!("serial/worker.rs"),
        ];
        let mut missing = BTreeSet::new();
        for source in sources {
            for c in string_literal_chars(source) {
                if !c.is_ascii() && !fonts.has_glyph(&FontId::proportional(14.0), c) {
                    missing.insert(c);
                }
            }
        }
        assert!(missing.is_empty(), "no glyph for: {missing:?}");
    }

    /// The scanner must not be fooled, and the check must be able to fail.
    #[test]
    fn glyph_check_can_fail() {
        let mut fonts = Fonts::new(TextOptions::default(), FontDefinitions::default());
        // Known to render as empty boxes with egui's bundled fonts.
        for c in ['\u{25A1}', '\u{25BC}', '\u{2191}', '\u{2192}'] {
            assert!(!fonts.has_glyph(&FontId::proportional(14.0), c), "{c} unexpectedly present");
        }
        let src = "let a = '\"'; // \"\u{25A1}\"\nlet b = \"x\\\" \u{2699}\ny \u{1F3E0}\";";
        let found: String = string_literal_chars(src).filter(|c| !c.is_ascii()).collect();
        assert_eq!(found, "\u{2699}\u{1F3E0}");
    }

    /// Characters inside "..." string literals, following them across lines
    /// and skipping comments and char literals.
    fn string_literal_chars(src: &str) -> impl Iterator<Item = char> + '_ {
        let chars: Vec<char> = src.chars().collect();
        let mut out = Vec::new();
        let (mut i, mut in_string) = (0, false);
        while i < chars.len() {
            let c = chars[i];
            if in_string {
                match c {
                    '\\' => i += 1, // skip the escaped character
                    '"' => in_string = false,
                    _ => out.push(c),
                }
            } else {
                match c {
                    '"' => in_string = true,
                    '/' if chars.get(i + 1) == Some(&'/') => {
                        while i < chars.len() && chars[i] != '\n' {
                            i += 1;
                        }
                    }
                    // Char literal like '"' or '\'' (not a lifetime).
                    '\'' if chars.get(i + 2) == Some(&'\'') => i += 2,
                    '\'' if chars.get(i + 1) == Some(&'\\') => {
                        while i + 1 < chars.len() && chars[i + 1] != '\'' {
                            i += 1;
                        }
                        i += 1;
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        out.into_iter()
    }
}
