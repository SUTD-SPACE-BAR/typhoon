//! Space Bar brand palette, Mainsail-style dark and light themes, and
//! Overload: a pastel Windows 98 / vaporwave theme with a pixel font.
//!
//! egui keeps one style for dark mode and one for light. Overload borrows the
//! light one: choosing it loads the Overload style into that slot and makes
//! [`pal`] return the Overload palette. Choosing any other theme puts egui's
//! own light style back first, so nothing from Overload lingers.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak,
    Frame, Id, Margin, Mesh, Painter, Pos2, Rect, RichText, Sense, Shadow, Shape, Stroke,
    StrokeKind, TextStyle, Theme, Ui, UiBuilder,
};
use serde::{Deserialize, Serialize};

/// Space Bar Grey (#262626).
pub const GREY: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
/// Space Bar Red (#EF3340): the accent, and E-STOP and errors in every theme.
pub const RED: Color32 = Color32::from_rgb(0xEF, 0x33, 0x40);
/// Space Bar White (#F2F2F2).
pub const WHITE: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF2);

pub const OK_GREEN: Color32 = Color32::from_rgb(0x4C, 0xAF, 0x50);
pub const WARN_AMBER: Color32 = Color32::from_rgb(0xE0, 0x9A, 0x00);

/// Overload's accent: dusty rose for text and outlines, pastel pink for fills.
const ROSE: Color32 = Color32::from_rgb(0xC8, 0x5A, 0x9E);
const PASTEL_PINK: Color32 = Color32::from_rgb(0xF4, 0xA9, 0xD3);
/// Overload's text: a deep plum.
const PLUM: Color32 = Color32::from_rgb(0x3B, 0x23, 0x66);
/// Overload button faces: idle, hovered, pressed.
const FACE: Color32 = Color32::from_rgb(0xF3, 0xE8, 0xFF);
const FACE_HOVER: Color32 = Color32::from_rgb(0xFF, 0xDD, 0xF1);
const FACE_PRESSED: Color32 = Color32::from_rgb(0xF7, 0xB5, 0xDB);
/// Overload's Windows 98 bevel: a light top-left edge and a shaded bottom-right one.
const BEVEL_LIGHT: Color32 = Color32::WHITE;
const BEVEL_SHADE: Color32 = Color32::from_rgb(0xB4, 0x9A, 0xD8);
/// Overload title bars: a soft fade from pink through lavender to the
/// background tile's baby blue, so they sit quietly on the tiles.
const TITLE_GRADIENT: [Color32; 3] = [
    Color32::from_rgb(0xF5, 0xC8, 0xEC),
    Color32::from_rgb(0xE2, 0xD3, 0xFB),
    Color32::from_rgb(0xCC, 0xEC, 0xFC),
];
const TITLE_BAR_HEIGHT: f32 = 22.0;

/// Overload's pixel font: DotGothic16, under the SIL Open Font License
/// (see `assets/fonts/OFL.txt`).
const PIXEL_FONT: &[u8] = include_bytes!("../assets/fonts/DotGothic16-Regular.ttf");
const PIXEL_FONT_NAME: &str = "DotGothic16";
/// DotGothic16 is drawn on 16 dots per em. Scaled so 14 pt body text renders
/// at 16 pt, its dots are whole screen pixels (one at 100 % display scaling,
/// two at 200 %), which keeps them crisp.
const PIXEL_FONT_SCALE: f32 = 16.0 / 14.0;
/// One of the pixel font's dots, in points, at the 14 pt size titles use.
const TITLE_DOT: f32 = 14.0 * PIXEL_FONT_SCALE / 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeChoice {
    /// Follow the operating system's light/dark setting.
    #[default]
    System,
    Dark,
    Light,
    /// Pastel Windows 98 / vaporwave, with a pixel font and its own sounds.
    Overload,
}

impl ThemeChoice {
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "🖥 System",
            ThemeChoice::Dark => "🌙 Dark",
            ThemeChoice::Light => "☀ Light",
            ThemeChoice::Overload => "💗 Overload",
        }
    }
    fn preference(self) -> egui::ThemePreference {
        match self {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
            // Overload lives in egui's light slot.
            ThemeChoice::Light | ThemeChoice::Overload => egui::ThemePreference::Light,
        }
    }
}

/// Colours that differ between the themes.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// Windows 98 look: square corners, bevelled edges, gradient title bars.
    pub retro: bool,
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
    /// Highlights drawn as text or lines: axis letters, layer numbers, links,
    /// hover outlines, the preview's position dot.
    pub accent: Color32,
    /// Filled highlights: Start buttons, progress, the selected tab,
    /// selections. E-STOP, stop buttons and errors stay [`RED`] in every theme.
    pub accent_fill: Color32,
    /// Text on `accent_fill`.
    pub on_accent: Color32,
}

impl Palette {
    /// Rounded corners of radius `r`, or square ones in Overload.
    pub fn corner(&self, r: u8) -> CornerRadius {
        CornerRadius::same(if self.retro { 0 } else { r })
    }
}

const DARK: Palette = Palette {
    dark: true,
    retro: false,
    background: Color32::from_rgb(0x1A, 0x1A, 0x1A),
    card: Color32::from_rgba_premultiplied(0x21, 0x21, 0x21, 0xD2),
    card_header: Color32::from_rgb(0x30, 0x30, 0x30),
    card_stroke: Color32::from_rgb(0x34, 0x34, 0x34),
    top_bar: Color32::from_rgba_premultiplied(0x12, 0x12, 0x12, 0xE6),
    text: WHITE,
    text_dim: Color32::from_rgb(0xA0, 0xA0, 0xA0),
    well: Color32::from_rgb(0x14, 0x14, 0x14),
    inset: Color32::from_rgb(0x2C, 0x2C, 0x2C),
    accent: RED,
    accent_fill: RED,
    on_accent: Color32::WHITE,
};

const LIGHT: Palette = Palette {
    dark: false,
    retro: false,
    background: WHITE,
    card: Color32::from_rgba_premultiplied(0xEB, 0xEB, 0xEB, 0xEB),
    card_header: Color32::from_rgb(0xE4, 0xE4, 0xE4),
    card_stroke: Color32::from_rgb(0xD4, 0xD4, 0xD4),
    top_bar: Color32::from_rgba_premultiplied(0xFF, 0xFF, 0xFF, 0xF0),
    text: GREY,
    text_dim: Color32::from_rgb(0x6E, 0x6E, 0x6E),
    well: Color32::from_rgb(0xF7, 0xF7, 0xF7),
    inset: Color32::from_rgb(0xF4, 0xF4, 0xF4),
    accent: RED,
    accent_fill: RED,
    on_accent: Color32::WHITE,
};

const OVERLOAD: Palette = Palette {
    dark: false,
    retro: true,
    // The pink of the background tile (assets/overload-tile.svg).
    background: Color32::from_rgb(0xF9, 0xE2, 0xFD),
    card: Color32::from_rgb(0xFF, 0xF7, 0xFD),
    card_header: Color32::from_rgb(0xF1, 0xE4, 0xFF),
    card_stroke: Color32::from_rgb(0x6B, 0x4F, 0xA0),
    top_bar: Color32::from_rgb(0xEA, 0xDC, 0xFF),
    text: PLUM,
    text_dim: Color32::from_rgb(0x7A, 0x68, 0xA0),
    well: Color32::WHITE,
    inset: Color32::from_rgb(0xF6, 0xEC, 0xFF),
    accent: ROSE,
    accent_fill: PASTEL_PINK,
    on_accent: PLUM,
};

/// The palette for whichever theme is currently showing.
pub fn pal(ctx: &egui::Context) -> Palette {
    if overload_on(ctx) {
        return OVERLOAD;
    }
    match ctx.theme() {
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
    }
}

fn overload_id() -> Id {
    Id::unique("typhoon_overload_theme")
}

/// Whether Overload is the chosen theme (noted in egui's memory by [`set_choice`]).
fn overload_on(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(overload_id())).unwrap_or(false)
}

/// Install the fonts and styles once at startup, then pick the theme.
pub fn install(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_fonts(fonts());
    ctx.style_mut_of(Theme::Dark, |s| style(s, &DARK));
    set_choice(ctx, choice);
}

pub fn set_choice(ctx: &egui::Context, choice: ThemeChoice) {
    let overload = choice == ThemeChoice::Overload;
    ctx.data_mut(|d| d.insert_temp(overload_id(), overload));
    // Start from egui's own light style, so nothing Overload changed lingers.
    ctx.style_mut_of(Theme::Light, |s| {
        *s = Theme::Light.default_style();
        style(s, if overload { &OVERLOAD } else { &LIGHT });
    });
    ctx.set_theme(choice.preference());
}

/// egui's fonts plus Overload's pixel font family.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        PIXEL_FONT_NAME.to_owned(),
        Arc::new(FontData::from_static(PIXEL_FONT).tweak(FontTweak {
            scale: PIXEL_FONT_SCALE,
            // Its tall Japanese line box leaves Latin text sitting low; lift
            // it so capitals are centred.
            y_offset_factor: -0.05,
            ..Default::default()
        })),
    );
    // egui's own fonts come after it, for the icons it lacks (⚙ 🔊 ✔ …).
    let mut family = vec![PIXEL_FONT_NAME.to_owned()];
    family.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(pixel_family(), family);
    fonts
}

fn pixel_family() -> FontFamily {
    FontFamily::Name("pixel".into())
}

fn style(style: &mut egui::Style, p: &Palette) {
    let family = if p.retro { pixel_family() } else { FontFamily::Proportional };
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.0, family.clone())),
        (TextStyle::Body, FontId::new(14.0, family.clone())),
        (TextStyle::Button, FontId::new(14.0, family.clone())),
        (TextStyle::Heading, FontId::new(18.0, family)),
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
    v.hyperlink_color = p.accent;
    v.warn_fg_color = WARN_AMBER;
    v.error_fg_color = RED;
    v.selection.bg_fill = p.accent_fill.gamma_multiply(if p.dark { 0.55 } else { 0.8 });
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
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.8));
    w.active.bg_fill = p.accent_fill.gamma_multiply(0.7);
    w.active.weak_bg_fill = p.accent_fill.gamma_multiply(0.7);
    w.open.bg_fill = hover;
    w.open.weak_bg_fill = hover;

    if p.retro {
        retro_style(style, p);
    }
}

/// Windows 98 touches over the base style: square corners, outlined buttons
/// that sink when pressed, hard drop shadows and solid scroll bars.
fn retro_style(style: &mut egui::Style, p: &Palette) {
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 12.0,
        handle_min_length: 20.0,
        dormant_background_opacity: 1.0,
        active_background_opacity: 1.0,
        interact_background_opacity: 1.0,
        dormant_handle_opacity: 1.0,
        active_handle_opacity: 1.0,
        interact_handle_opacity: 1.0,
        ..egui::style::ScrollStyle::solid()
    };
    // The pixel font's lines are taller; size controls to fit them.
    style.spacing.interact_size.y = 32.0;

    let v = &mut style.visuals;
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.card_stroke);
    v.window_shadow = hard_shadow(p, 4);
    v.popup_shadow = hard_shadow(p, 3);
    v.selection.bg_fill = p.accent_fill;
    // Also the outline of a focused text box, so it has to show on white.
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = CornerRadius::ZERO;
        wv.expansion = 0.0;
        wv.fg_stroke.color = p.text;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, BEVEL_SHADE);
    let outline = Stroke::new(1.0, p.card_stroke);
    for (wv, fill, stroke) in [
        (&mut w.inactive, FACE, outline),
        (&mut w.hovered, FACE_HOVER, Stroke::new(1.0, p.accent)),
        (&mut w.active, FACE_PRESSED, Stroke::new(1.0, p.text)),
        // Also the title strip of the focused window (Preferences).
        (&mut w.open, FACE_PRESSED, outline),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.bg_stroke = stroke;
    }
    // Pressed buttons sink in by a pixel.
    w.active.expansion = -1.0;
}

fn hard_shadow(p: &Palette, offset: i8) -> Shadow {
    Shadow {
        offset: [offset, offset],
        blur: 0,
        spread: 0,
        color: p.card_stroke.gamma_multiply(0.45),
    }
}

/// A Mainsail-style card: title strip on top, content below. Sized to its
/// content. In Overload it's a Windows 98 window instead.
pub fn card<R>(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal(ui.ctx());
    if p.retro {
        return retro_card(ui, title, &p, add_contents);
    }
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

/// A Windows 98 window: bevelled border, gradient title bar with (decorative)
/// minimise, maximise and close boxes, and a hard drop shadow.
fn retro_card<R>(
    ui: &mut Ui,
    title: &str,
    p: &Palette,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let shown = Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_stroke))
        .shadow(hard_shadow(p, 4))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (bar, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), TITLE_BAR_HEIGHT),
                Sense::hover(),
            );
            title_bar(ui.painter(), bar, title);
            Frame::NONE
                .inner_margin(Margin {
                    left: 9,
                    right: 9,
                    top: 3,
                    bottom: 8,
                })
                .show(ui, add_contents)
                .inner
        });
    bevel(ui.painter(), shown.response.rect.shrink(1.0), true);
    shown.inner
}

/// The gradient strip: title on the left, caption boxes on the right.
fn title_bar(painter: &Painter, bar: Rect, title: &str) {
    gradient(painter, bar, &TITLE_GRADIENT, true);
    let boxes = caption_boxes(painter, bar);
    let text = painter.with_clip_rect(bar.with_max_x(boxes.left() - 4.0).intersect(painter.clip_rect()));
    let font = FontId::new(14.0, pixel_family());
    let at = bar.left_center() + egui::vec2(6.0, 0.0);
    // A faint white highlight, then the title in the theme's purple; each
    // drawn twice, one font dot apart, to look bold.
    for (offset, color) in [(TITLE_DOT, Color32::from_white_alpha(170)), (0.0, OVERLOAD.text)] {
        for bold in [0.0, TITLE_DOT] {
            let pos = at + egui::vec2(offset + bold, offset);
            text.text(pos, Align2::LEFT_CENTER, title, font.clone(), color);
        }
    }
}

/// Windows 98 close, maximise and minimise boxes, right to left. They're only
/// decoration. Returns the area they cover.
fn caption_boxes(painter: &Painter, bar: Rect) -> Rect {
    let size = egui::vec2(16.0, 14.0);
    let ink = OVERLOAD.text;
    let mut right = bar.right() - 3.0;
    let mut covered = Rect::NOTHING;
    for i in 0..3 {
        let r = Rect::from_min_size(Pos2::new(right - size.x, bar.center().y - size.y / 2.0), size);
        painter.rect_filled(r, 0.0, FACE);
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, OVERLOAD.card_stroke), StrokeKind::Inside);
        bevel(painter, r.shrink(1.0), true);
        let c = r.center();
        match i {
            // Close: an X.
            0 => {
                let stroke = Stroke::new(1.5, ink);
                painter.line_segment([c + egui::vec2(-3.5, -3.0), c + egui::vec2(3.5, 3.0)], stroke);
                painter.line_segment([c + egui::vec2(-3.5, 3.0), c + egui::vec2(3.5, -3.0)], stroke);
            }
            // Maximise: a window outline with a thick top edge.
            1 => {
                let w = Rect::from_center_size(c, egui::vec2(8.0, 7.0));
                painter.rect_stroke(w, 0.0, Stroke::new(1.0, ink), StrokeKind::Inside);
                painter.hline(w.x_range(), w.top() + 1.5, Stroke::new(1.0, ink));
            }
            // Minimise: a bar along the bottom.
            _ => {
                painter.hline(c.x - 3.5..=c.x + 2.5, c.y + 3.0, Stroke::new(2.0, ink));
            }
        }
        covered = covered.union(r);
        // Windows 98 leaves a gap between close and the other two.
        right = r.left() - if i == 0 { 2.0 } else { 0.0 };
    }
    covered
}

/// A Windows 98 edge just inside `rect`: light along the top and left and
/// shaded along the bottom and right when `raised`, the other way round when
/// sunken.
pub fn bevel(painter: &Painter, rect: Rect, raised: bool) {
    let (light, shade) = if raised {
        (BEVEL_LIGHT, BEVEL_SHADE)
    } else {
        (BEVEL_SHADE, BEVEL_LIGHT)
    };
    // Centre the 1-pixel lines on pixel rows.
    let r = rect.shrink(0.5);
    let (tl, br) = (Stroke::new(1.0, light), Stroke::new(1.0, shade));
    painter.line_segment([r.left_bottom(), r.left_top()], tl);
    painter.line_segment([r.left_top(), r.right_top()], tl);
    painter.line_segment([r.right_top(), r.right_bottom()], br);
    painter.line_segment([r.right_bottom(), r.left_bottom()], br);
}

/// Fill `rect` with a gradient through `stops`: left to right when
/// `horizontal`, else top to bottom.
pub fn gradient(painter: &Painter, rect: Rect, stops: &[Color32], horizontal: bool) {
    let mut mesh = Mesh::default();
    let last = stops.len().saturating_sub(1).max(1) as f32;
    for (i, &color) in stops.iter().enumerate() {
        let t = i as f32 / last;
        let (a, b) = if horizontal {
            let x = rect.left() + t * rect.width();
            (Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom()))
        } else {
            let y = rect.top() + t * rect.height();
            (Pos2::new(rect.left(), y), Pos2::new(rect.right(), y))
        };
        mesh.colored_vertex(a, color);
        mesh.colored_vertex(b, color);
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(Shape::mesh(mesh));
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

/// Small rounded "status chip" like Mainsail's printer state badge. In
/// Overload, a sunken square field like a Windows 98 status bar.
pub fn status_chip(ui: &mut Ui, text: &str, color: Color32) {
    let p = pal(ui.ctx());
    let (fill, outline, corner): (Color32, Color32, u8) = if p.retro {
        (Color32::WHITE, p.card_stroke, 0)
    } else {
        (color.gamma_multiply(0.18), color.gamma_multiply(0.7), 12)
    };
    let chip = Frame::NONE
        .fill(fill)
        .stroke(Stroke::new(1.0, outline))
        .corner_radius(CornerRadius::same(corner))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                if p.retro {
                    // A square "pixel" status light.
                    ui.painter().rect_filled(rect, 0.0, color);
                } else {
                    ui.painter().circle_filled(rect.center(), 4.0, color);
                }
                ui.label(RichText::new(text).size(12.5).color(color));
            });
        });
    if p.retro {
        bevel(ui.painter(), chip.response.rect.shrink(1.0), false);
    }
}

#[cfg(test)]
mod tests {
    use super::ThemeChoice;
    use eframe::egui::epaint::text::{Fonts, TextOptions};
    use eframe::egui::{self, CornerRadius, FontDefinitions, FontFamily, FontId, Theme};
    use std::collections::BTreeSet;

    /// Every non-ASCII character in the UI's string literals must exist in
    /// the bundled fonts, otherwise it shows up as an empty box. Checked for
    /// the normal font and for Overload's pixel font with its fallbacks.
    #[test]
    fn every_symbol_in_ui_text_has_a_glyph() {
        let mut fonts = Fonts::new(TextOptions::default(), super::fonts());
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
        for family in [FontFamily::Proportional, super::pixel_family()] {
            let font = FontId::new(14.0, family);
            for source in sources {
                for c in string_literal_chars(source) {
                    if !c.is_ascii() && !fonts.has_glyph(&font, c) {
                        missing.insert((format!("{:?}", font.family), c));
                    }
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

    /// The pixel font really is in the pixel family, ahead of the fallbacks.
    #[test]
    fn pixel_family_leads_with_the_pixel_font() {
        let fonts = super::fonts();
        let family = &fonts.families[&super::pixel_family()];
        assert_eq!(family[0], super::PIXEL_FONT_NAME);
        assert!(family.len() > 1, "no fallback fonts for icons");
        assert!(fonts.font_data.contains_key(super::PIXEL_FONT_NAME));
    }

    /// Overload borrows egui's light style slot and gives it back unchanged.
    #[test]
    fn overload_borrows_and_returns_the_light_style() {
        let ctx = egui::Context::default();
        super::install(&ctx, ThemeChoice::Light);
        let light = ctx.style_of(Theme::Light);
        assert!(!super::pal(&ctx).retro);

        super::set_choice(&ctx, ThemeChoice::Overload);
        assert!(super::pal(&ctx).retro);
        assert_eq!(ctx.theme(), Theme::Light);
        let overload = ctx.style_of(Theme::Light);
        assert_eq!(overload.visuals.window_corner_radius, CornerRadius::ZERO);
        assert_eq!(
            overload.text_styles[&egui::TextStyle::Body].family,
            super::pixel_family()
        );

        super::set_choice(&ctx, ThemeChoice::Dark);
        assert!(!super::pal(&ctx).retro);
        super::set_choice(&ctx, ThemeChoice::Light);
        // Not the whole Style: its number formatter only equals itself.
        let back = ctx.style_of(Theme::Light);
        assert_eq!(back.visuals, light.visuals);
        assert_eq!(back.text_styles, light.text_styles);
        assert_eq!(back.spacing, light.spacing);
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
