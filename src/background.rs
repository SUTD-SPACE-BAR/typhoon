//! The window background: the faded Space Bar logo, scaled to fill the
//! window. Overload instead tiles a pixel-art pattern, with no logo.
//!
//! Dark theme uses the normal logo (light rocket body); light theme uses the
//! inverted one (dark body), as the Space Bar Assets README prescribes for
//! dark and light backgrounds.

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions};

use crate::theme::Palette;

const LOGO_SVG: &str = include_str!("../assets/spacebar-logo.svg");
/// Overload's background tile: pixel art, one SVG unit per pixel, with edges
/// that wrap so it repeats seamlessly.
const TILE_SVG: &str = include_str!("../assets/overload-tile.svg");

/// Pixel size the logo SVG is rasterised at (once, on startup).
const RASTER_SIZE: u32 = 1024;
/// The tile's size in pixels (its SVG units).
const TILE_PIXELS: u32 = 16;
/// Points per tile pixel on screen. Whole device pixels at 100 to 200 %
/// display scaling, so the pixel art stays crisp.
const TILE_PIXEL_POINTS: f32 = 4.0;
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
    /// Overload's tile, sampled with sharp (nearest) pixels and repeating.
    tile: Option<TextureHandle>,
}

impl Background {
    pub fn new(ctx: &egui::Context) -> Self {
        let load = |name: &str, svg: &str, size: u32, options: TextureOptions| {
            rasterize(svg, size).map(|img| ctx.load_texture(name, img, options))
        };
        Self {
            dark: load("spacebar-logo", LOGO_SVG, RASTER_SIZE, TextureOptions::LINEAR),
            light: load(
                "spacebar-logo-inverted",
                &inverted_svg(),
                RASTER_SIZE,
                TextureOptions::LINEAR,
            ),
            tile: load("overload-tile", TILE_SVG, TILE_PIXELS, TextureOptions::NEAREST_REPEAT),
        }
    }

    /// Fill `rect` with the theme's background: the watermark centred in it,
    /// or Overload's tiles (and no watermark).
    pub fn paint(&self, painter: &egui::Painter, rect: Rect, p: &Palette, opacity: f32) {
        painter.rect_filled(rect, 0.0, p.background);
        if p.retro {
            if let Some(tile) = &self.tile {
                // One quad: the texture repeats, starting from the window's corner.
                let side = TILE_PIXELS as f32 * TILE_PIXEL_POINTS;
                let uv = Rect::from_min_max(
                    Pos2::ZERO,
                    Pos2::new(rect.width() / side, rect.height() / side),
                );
                painter.image(tile.id(), rect, uv, Color32::WHITE);
            }
            return;
        }
        let texture = if p.dark { &self.dark } else { &self.light };
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// The Overload tile renders to its pixel grid, fully opaque (so the
    /// repeats leave no gaps) and with no blended edge colours (crisp pixels).
    #[test]
    fn overload_tile_is_crisp_opaque_pixel_art() {
        let tile = super::rasterize(super::TILE_SVG, super::TILE_PIXELS).expect("tile SVG renders");
        assert_eq!(tile.size, [16, 16]);
        assert!(tile.pixels.iter().all(|c| c.a() == 255), "tile has transparent pixels");
        let colours: BTreeSet<[u8; 4]> = tile.pixels.iter().map(|c| c.to_array()).collect();
        assert_eq!(colours.len(), 3, "expected pink, lavender and blue only: {colours:?}");
    }
}
