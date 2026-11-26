//! The layer range slider beside the preview, like a slicer's: a vertical
//! track with layer 1 at the bottom and a handle at each end of the range of
//! layers shown.

use eframe::egui::{Align2, Pos2, Rect, Sense, Stroke, TextStyle, Ui, Vec2};

use crate::preview::layer_color;
use crate::theme::Palette;

/// Width of the slider's strip, points.
pub const WIDTH: f32 = 46.0;
/// A handle: a pill with the layer number on it.
const HANDLE: Vec2 = Vec2::new(30.0, 18.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Low,
    High,
}

/// The track: layer 1 at the bottom, the last layer at the top.
struct Track {
    top: f32,
    bottom: f32,
    layers: usize,
}

impl Track {
    fn new(rect: Rect, layers: usize) -> Self {
        // Room at each end for a handle nudged clear of the other.
        let margin = HANDLE.y + 2.0;
        let top = rect.top() + margin;
        Self {
            top,
            bottom: (rect.bottom() - margin).max(top + 1.0),
            layers,
        }
    }

    fn y(&self, layer: usize) -> f32 {
        self.bottom - (self.bottom - self.top) * layer as f32 / (self.layers - 1) as f32
    }

    fn layer_at(&self, y: f32) -> usize {
        let t = (self.bottom - y) / (self.bottom - self.top);
        (t * (self.layers - 1) as f32).round().clamp(0.0, (self.layers - 1) as f32) as usize
    }
}

/// The handle a press at `layer` takes hold of: the nearer one, or the one
/// it's beyond. None while both handles sit on `layer`: which way the drag
/// goes decides.
fn grab((lo, hi): (usize, usize), layer: usize) -> Option<Handle> {
    if lo == hi && layer == lo {
        None
    } else if layer >= hi {
        Some(Handle::High)
    } else if layer <= lo || layer - lo < hi - layer {
        Some(Handle::Low)
    } else {
        Some(Handle::High)
    }
}

/// Move `handle` to `layer`, never past the other one.
fn move_to(range: &mut (usize, usize), handle: Handle, layer: usize) {
    match handle {
        Handle::High => range.1 = layer.max(range.0),
        Handle::Low => range.0 = layer.min(range.1),
    }
}

/// The slider in `rect`, for layers named `labels`; returns whether `range`
/// changed. `chips` dims the layers switched off by their chip.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    range: &mut (usize, usize),
    labels: &[String],
    chips: &[bool],
    p: &Palette,
) -> bool {
    let layers = labels.len();
    if layers < 2 {
        return false;
    }
    let before = *range;
    let id = ui.make_persistent_id("layer_slider");
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let track = Track::new(rect, layers);

    // The handle held: picked where the button went down, once that turns
    // into a drag or a click (a press alone moves nothing), and kept until
    // the drag ends.
    let held_id = id.with("held");
    if response.drag_started() || response.clicked() {
        let pressed = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos());
        let held = pressed.and_then(|pos| grab(*range, track.layer_at(pos.y)));
        ui.data_mut(|d| d.insert_temp(held_id, held));
    }
    if (response.dragged() || response.clicked())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let layer = track.layer_at(pos.y);
        let mut held = ui.data(|d| d.get_temp::<Option<Handle>>(held_id)).flatten();
        if held.is_none() && range.0 == range.1 && layer != range.0 {
            // Both handles on one layer: the drag's direction picks one.
            held = Some(if layer > range.1 { Handle::High } else { Handle::Low });
            ui.data_mut(|d| d.insert_temp(held_id, held));
        }
        if let Some(handle) = held {
            move_to(range, handle, layer);
        }
    }
    if response.double_clicked() {
        *range = (0, layers - 1);
    }

    // The track, the part of it shown, and a tick per layer in its colour.
    let painter = ui.painter();
    let x = rect.center().x;
    let (lo, hi) = *range;
    painter.line_segment(
        [Pos2::new(x, track.y(0)), Pos2::new(x, track.y(layers - 1))],
        Stroke::new(2.0, p.card_stroke),
    );
    painter.line_segment([Pos2::new(x, track.y(lo)), Pos2::new(x, track.y(hi))], Stroke::new(3.0, p.accent));
    for i in 0..layers {
        let on = (lo..=hi).contains(&i) && chips.get(i).copied().unwrap_or(true);
        let color = if on { layer_color(i) } else { layer_color(i).gamma_multiply(0.25) };
        let tick = Rect::from_center_size(Pos2::new(x - 11.0, track.y(i)), Vec2::new(6.0, 3.0));
        painter.rect_filled(tick, p.corner(1), color);
    }

    // The handles, nudged apart where they'd overlap. While the slider is in
    // use, each says what its layer is, out to the left over the view.
    let (mut y_lo, mut y_hi) = (track.y(lo), track.y(hi));
    let overlap = HANDLE.y + 2.0 - (y_lo - y_hi);
    if lo != hi && overlap > 0.0 {
        y_lo += overlap / 2.0;
        y_hi -= overlap / 2.0;
    }
    let handles = if lo == hi { vec![(hi, y_hi)] } else { vec![(lo, y_lo), (hi, y_hi)] };
    let font = TextStyle::Small.resolve(ui.style());
    let tags = response.hovered() || response.dragged();
    for (layer, y) in handles {
        let pill = Rect::from_center_size(Pos2::new(x, y), HANDLE);
        painter.rect_filled(pill, p.corner(9), p.accent_fill);
        painter.text(pill.center(), Align2::CENTER_CENTER, (layer + 1).to_string(), font.clone(), p.on_accent);
        if tags {
            let galley = painter.layout_no_wrap(format!("{} · {}", layer + 1, labels[layer]), font.clone(), p.text);
            let at = Pos2::new(pill.left() - 8.0 - galley.size().x, y - galley.size().y / 2.0);
            painter.rect_filled(Rect::from_min_size(at, galley.size()).expand(4.0), p.corner(4), p.card);
            painter.galley(at, galley, p.text);
        }
    }
    response.on_hover_text(
        "Drag the handles to choose which layers are shown. ↑/↓ move the top one; double-click shows them all.",
    );
    *range != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_maps_layers_both_ways() {
        let track = Track::new(Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH, 300.0)), 10);
        for i in 0..10 {
            assert_eq!(track.layer_at(track.y(i)), i);
        }
        assert!(track.y(0) > track.y(9), "layer 1 is at the bottom");
        assert_eq!(track.layer_at(-50.0), 9);
        assert_eq!(track.layer_at(1000.0), 0);
    }

    #[test]
    fn grabs_the_nearer_handle() {
        assert_eq!(grab((2, 7), 8), Some(Handle::High));
        assert_eq!(grab((2, 7), 1), Some(Handle::Low));
        assert_eq!(grab((2, 7), 6), Some(Handle::High));
        assert_eq!(grab((2, 7), 3), Some(Handle::Low));
        assert_eq!(grab((4, 4), 4), None);
        assert_eq!(grab((4, 4), 5), Some(Handle::High));
        assert_eq!(grab((4, 4), 3), Some(Handle::Low));
    }

    /// For each (from, to): press on layer `from`, drag to layer `to` and
    /// let go, as a mouse would, frame by frame (from = to is a click). The
    /// range that leaves.
    fn drags(mut range: (usize, usize), layers: usize, moves: &[(usize, usize)]) -> (usize, usize) {
        use eframe::egui::{Context, Event, PointerButton, RawInput};
        let ctx = Context::default();
        let labels: Vec<String> = (0..layers).map(|i| format!("layer {}", i + 1)).collect();
        let chips = vec![true; layers];
        let rect = Rect::from_min_size(Pos2::new(300.0, 50.0), Vec2::new(WIDTH, 400.0));
        let track = Track::new(rect, layers);
        let at = |layer: f32| {
            let (a, b) = (track.y(0), track.y(layers - 1));
            Pos2::new(rect.center().x, a + (b - a) * layer / (layers - 1) as f32)
        };
        let mut time = 0.0;
        let mut frame = |events: Vec<Event>, range: &mut (usize, usize)| {
            time += 1.0 / 60.0;
            let input = RawInput {
                events,
                time: Some(time),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let p = crate::theme::pal(ui.ctx());
                show(ui, rect, range, &labels, &chips, &p);
            });
            output.textures_delta.clear();
        };
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for &(from, to) in moves {
            let (start, end) = (at(from as f32), at(to as f32));
            frame(vec![Event::PointerMoved(start)], &mut range);
            frame(vec![button(start, true)], &mut range);
            if from != to {
                for k in 1..=30 {
                    let layer = from as f32 + (to as f32 - from as f32) * k as f32 / 30.0;
                    frame(vec![Event::PointerMoved(at(layer))], &mut range);
                }
            }
            frame(vec![button(end, false)], &mut range);
            frame(Vec::new(), &mut range);
        }
        range
    }

    fn drag(range: (usize, usize), layers: usize, from: usize, to: usize) -> (usize, usize) {
        drags(range, layers, &[(from, to)])
    }

    #[test]
    fn handles_drag_both_ways() {
        assert_eq!(drag((0, 2), 3, 2, 0), (0, 0), "top handle down");
        assert_eq!(drag((0, 2), 3, 0, 2), (2, 2), "bottom handle up");
        assert_eq!(drag((0, 9), 10, 9, 4), (0, 4), "top handle down");
        assert_eq!(drag((0, 9), 10, 0, 6), (6, 9), "bottom handle up");
        assert_eq!(drag((2, 6), 10, 6, 8), (2, 8), "top handle up");
        assert_eq!(drag((2, 6), 10, 2, 0), (0, 6), "bottom handle down");
    }

    /// Both handles on one layer: the way the drag goes picks the handle.
    #[test]
    fn joined_handles_part_either_way() {
        assert_eq!(drag((4, 4), 10, 4, 7), (4, 7));
        assert_eq!(drag((4, 4), 10, 4, 1), (1, 4));
    }

    /// A click on the track brings the nearer handle there.
    #[test]
    fn clicks_move_the_nearer_handle() {
        assert_eq!(drag((2, 9), 10, 6, 6), (2, 6));
        assert_eq!(drag((2, 9), 10, 4, 4), (4, 9));
    }

    /// The handle held in one drag doesn't carry over to the next.
    #[test]
    fn each_drag_picks_its_own_handle() {
        assert_eq!(drags((0, 9), 10, &[(0, 3), (9, 5)]), (3, 5));
        assert_eq!(drags((0, 9), 10, &[(9, 5), (0, 3)]), (3, 5));
        assert_eq!(drags((0, 9), 10, &[(9, 2), (2, 7)]), (0, 7));
    }

    #[test]
    fn handles_never_cross() {
        let mut range = (2, 7);
        move_to(&mut range, Handle::High, 1);
        assert_eq!(range, (2, 2));
        move_to(&mut range, Handle::Low, 5);
        assert_eq!(range, (2, 2));
        move_to(&mut range, Handle::High, 9);
        move_to(&mut range, Handle::Low, 0);
        assert_eq!(range, (0, 9));
    }
}
