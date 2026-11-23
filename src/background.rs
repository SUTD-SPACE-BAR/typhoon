//! Faded Space Bar logo painted behind every panel, scaled to fill the window.
//!
//! Dark theme uses the normal logo (light rocket body); light theme uses the
//! inverted one (dark body), as the Space Bar Assets README prescribes for
//! dark and light backgrounds.

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions};

const LOGO_SVG: &str = include_str!("../assets/spacebar-logo.svg");

/// Pixel size the SVG is rasterised at (once, on startup).
const RASTER_SIZE: u32 = 1024;
/// Default watermark opacity, 0..=1 (adjustable in Preferences).
pub const DEFAULT_OPACITY: f32 = 0.10;
/// The logo is square. Its side is the larger of these two, so it fills wide
/// and squarish windows alike without being cropped too hard:
/// at least this fraction of the window's shorter side...
const MIN_SHORT_SIDE: f32 = 1.15;
/// ...and at least this fraction of the window's longer side.
const MIN_LONG_SIDE: f32 = 0.85;

/// The inverted logo: Space Bar Grey and White swapped.
fn inverted_svg() -> String {
    LOGO_SVG
        .replace("#f2f2f2", "#__WHITE__")
        .replace("#262626", "#f2f2f2")
        .replace("#__WHITE__", "#262626")
}

/// Rasterise an SVG to an RGBA image at `size` x `size` pixels.
fn rasterize(svg: &str, size: u32) -> Option<ColorImage> {
    let tree =
        resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    let svg_size = tree.size();
    let scale = size as f32 / svg_size.width().max(svg_size.height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(ColorImage::from_rgba_premultiplied(
        [size as usize, size as usize],
        pixmap.data(),
    ))
}

/// The normal logo, for the window icon.
pub fn rasterize_logo(size: u32) -> Option<ColorImage> {
    rasterize(LOGO_SVG, size)
}

pub struct Background {
    dark: Option<TextureHandle>,
    light: Option<TextureHandle>,
}

impl Background {
    pub fn new(ctx: &egui::Context) -> Self {
        let load = |name: &str, svg: &str| {
            rasterize(svg, RASTER_SIZE).map(|img| ctx.load_texture(name, img, TextureOptions::LINEAR))
        };
        Self {
            dark: load("spacebar-logo", LOGO_SVG),
            light: load("spacebar-logo-inverted", &inverted_svg()),
        }
    }

    /// Fill `rect` with the background colour and draw the watermark centred in it.
    pub fn paint(&self, painter: &egui::Painter, rect: Rect, fill: Color32, dark: bool, opacity: f32) {
        painter.rect_filled(rect, 0.0, fill);
        let texture = if dark { &self.dark } else { &self.light };
        let Some(texture) = texture else { return };
        if opacity <= 0.0 {
            return;
        }
        let short = rect.width().min(rect.height());
        let long = rect.width().max(rect.height());
        let side = (short * MIN_SHORT_SIDE).max(long * MIN_LONG_SIDE);
        // Square, so the logo keeps its proportions.
        let quad = Rect::from_center_size(rect.center(), egui::vec2(side, side));
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(texture.id(), quad, uv, Color32::WHITE.gamma_multiply(opacity));
    }
}
