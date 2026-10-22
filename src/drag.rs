//! Dragging songs. The row lifts out of its list and follows the pointer. In a list you can
//! reorder, it stays in line and the other rows slide apart to make room; over the queue it
//! flies across and takes the queue's shape; let go, it settles into its new place.
//!
//! A list calls `drag_layout` before drawing its rows, `row_rect` for each row, `drag_scroll`
//! inside its scroll area and `drag_drop` after the rows. `draw_drag` (from the overlays)
//! moves and paints the lifted row.

use crate::app::{Action, App, DragSongs};
use crate::art::{ArtKey, ArtSize};
use crate::theme::*;
use crate::widgets as w;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Painter, Rangef, Rect, Ui, Vec2};

/// A list songs can be reordered in (and, for some, dropped into).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DragList {
    Songs,
    Playlist(u64),
    /// The songs after the one playing (row 0 = next).
    Queue,
}

/// The song being carried.
pub struct Lift {
    song: u64,
    count: usize,
    /// The list and row it was picked up from, if that list can be reordered.
    src: Option<(DragList, usize)>,
    /// Where the pointer holds the row.
    grab: Vec2,
    /// The row it came from (it flies back there if it's let go nowhere).
    home: Rect,
    /// Its row was on screen this frame (if not, a cancelled drag fades out instead).
    home_seen: bool,
    /// Where the lifted row is drawn now.
    ghost: Rect,
    trail: [Rect; 3],
    since: f64,
}

/// A dropped song easing from where it was let go into its new row. The row itself waits
/// until it lands; the drag layer draws it on the way.
pub struct Settle {
    key: Id,
    index: usize,
    song: u64,
    from: Rect,
    /// Where its row is on screen this frame (set by the list).
    to: Option<Rect>,
    at: f64,
}

/// The lifted row after it was let go somewhere that isn't a list (a sidebar playlist, the
/// queue button), or on its way home after being let go nowhere.
pub struct Fly {
    song: u64,
    count: usize,
    from: Rect,
    to: Rect,
    at: f64,
    /// Taken somewhere: shrinks and fades. Otherwise it lands back in its row.
    fade: bool,
}

/// What a list needs to know while it draws its rows.
pub struct DragState {
    pub list: DragList,
    key: Id,
    /// A song is being carried over this list.
    pub hovering: bool,
    /// The row the song came from, when it's being moved inside this list.
    pub own: Option<usize>,
    /// Where it would land (its index after the drop).
    pub gap: Option<usize>,
    pub n: usize,
    row_h: f32,
    /// Rows with a small heading above them (sorted), `head_h` tall. A heading moves with its row.
    heads: Vec<usize>,
    head_h: f32,
}

const SETTLE: f32 = 0.24;
const FLY: f32 = 0.26;
const LIFT_IN: f32 = 0.14;

impl DragState {
    /// Headings at or above row `i`, leaving out the carried row's own.
    fn heads_to(&self, i: usize, skip: Option<usize>) -> f32 {
        let n = self.heads.partition_point(|&h| h <= i);
        let skipped = skip.map(|s| s <= i && self.heads.binary_search(&s).is_ok()).unwrap_or(false);
        (n - skipped as usize) as f32 * self.head_h
    }

    /// Where row `i` starts (below its heading, if any), from the top of the list.
    pub fn row_y(&self, i: usize) -> f32 {
        i as f32 * self.row_h + self.heads_to(i, None)
    }

    /// Where row `i` goes while a song is carried over the list.
    fn moved_y(&self, i: usize) -> f32 {
        match (self.hovering, self.own, self.gap) {
            (true, Some(s), Some(t)) => {
                let j = if i > s { i - 1 } else { i };
                let j = if j >= t { j + 1 } else { j };
                j as f32 * self.row_h + self.heads_to(i, Some(s))
            }
            (true, None, Some(t)) => (i + (i >= t) as usize) as f32 * self.row_h + self.heads_to(i, None),
            _ => self.row_y(i),
        }
    }

    /// Where the carried song would sit if dropped at `t` (above row `t`'s heading).
    fn drop_y(&self, t: usize) -> f32 {
        let heads = match self.own {
            Some(s) if t > s => self.heads_to(t, Some(s)),
            _ => self.heads_to(t.saturating_sub(1), None) * (t > 0) as u8 as f32,
        };
        t as f32 * self.row_h + heads
    }

    /// Whether row `i`'s heading shows (not while its row is being carried).
    pub fn head_shown(&self, i: usize) -> bool {
        !self.hidden(i)
    }

    /// The row being carried (it's drawn under the pointer instead).
    pub fn hidden(&self, i: usize) -> bool {
        self.hovering && self.own == Some(i)
    }

    /// The row being carried, while it's away from its list (drawn faded in its place).
    pub fn away(&self, i: usize) -> bool {
        !self.hovering && self.own == Some(i)
    }

    /// One more row of room, for a song coming in from somewhere else.
    pub fn extra(&self) -> usize {
        (self.hovering && self.own.is_none() && self.gap.is_some()) as usize
    }
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(a.min.lerp(b.min, t), a.max.lerp(b.max, t))
}

fn carrying(ctx: &egui::Context) -> bool {
    egui::DragAndDrop::has_payload_of_type::<DragSongs>(ctx)
}

impl App {
    /// Pick a song up from its row (call when the row's drag starts, after setting the payload).
    pub(crate) fn lift_song(&mut self, ui: &Ui, song: u64, src: Option<(DragList, usize)>, row: Rect) {
        let p = ui.ctx().input(|i| i.pointer.press_origin().or(i.pointer.latest_pos())).unwrap_or(row.center());
        let now = ui.input(|i| i.time);
        self.lift = Some(Lift { song, count: 1, src, grab: p - row.min, home: row, home_seen: true, ghost: row, trail: [row; 3], since: now });
        self.settle = None;
        self.fly = None;
    }

    /// Before a list draws its rows. `zone` is where the list takes the song (on screen), `rows_x`
    /// its rows' left and right edges, `top` where row 0 is.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn drag_layout(&mut self, ui: &Ui, list: DragList, key: Id, zone: Rect, rows_x: Rangef, top: f32, row_h: f32, n: usize, takes_others: bool) -> DragState {
        self.drag_layout_with_heads(ui, list, key, zone, rows_x, top, row_h, n, takes_others, Vec::new(), 0.0)
    }

    /// `drag_layout` for a list with headings between its rows (the queue's groups).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn drag_layout_with_heads(
        &mut self,
        ui: &Ui,
        list: DragList,
        key: Id,
        zone: Rect,
        rows_x: Rangef,
        top: f32,
        row_h: f32,
        n: usize,
        takes_others: bool,
        heads: Vec<usize>,
        head_h: f32,
    ) -> DragState {
        let mut ds = DragState { list, key, hovering: false, own: None, gap: None, n, row_h, heads, head_h };
        let Some(lift) = &self.lift else { return ds };
        if !carrying(ui.ctx()) {
            return ds;
        }
        ds.own = lift.src.filter(|(l, _)| *l == list).map(|(_, i)| i);
        let Some(p) = ui.ctx().input(|i| i.pointer.latest_pos()) else { return ds };
        if !zone.contains(p) || (ds.own.is_none() && !takes_others) {
            return ds;
        }
        ds.hovering = true;
        let ghost_top = p.y - lift.grab.y.min(row_h);
        let last = if ds.own.is_some() { n.saturating_sub(1) } else { n };
        // The closest spot (headings push rows down, so it's at most a few rows above a plain
        // division).
        let y = ghost_top - top;
        let est = ((y / row_h).floor().max(0.0) as usize).min(last);
        let lo = est.saturating_sub(ds.heads.len() + 1);
        let mut best = (est, f32::MAX);
        for t in lo..=(est + 1).min(last) {
            let d = (ds.drop_y(t) - y).abs();
            if d < best.1 {
                best = (t, d);
            }
        }
        ds.gap = Some(best.0);
        // In line with this list's rows: same edges, only following the pointer up and down.
        self.drag_aim = Some(Rect::from_x_y_ranges(rows_x, ghost_top..=ghost_top + row_h));
        ds
    }

    /// Where row `i` (at `r` in its own place) is drawn this frame, moved aside to make room,
    /// and whether to draw it (not while it's in the air: carried, or still landing).
    pub(crate) fn row_place(&mut self, ui: &Ui, ds: &DragState, i: usize, r: Rect) -> (Rect, bool) {
        if let Some(l) = &mut self.lift {
            // Keep track of where its row is, so a cancelled drag lands back in the right spot
            // even after scrolling.
            if ds.own == Some(i) {
                l.home = r;
                l.home_seen = true;
            }
        }
        if ds.hidden(i) {
            return (r, false);
        }
        let target = ds.moved_y(i) - ds.row_y(i);
        let key = (ds.key, i);
        let cur = self.row_shift.get(&key).copied().unwrap_or(0.0);
        let mut shift = 0.0;
        if target != 0.0 || cur != 0.0 {
            let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
            let k = 1.0 - (-dt * 22.0).exp();
            shift = cur + (target - cur) * k;
            if (shift - target).abs() < 0.5 {
                shift = target;
            } else {
                ui.ctx().request_repaint();
            }
            if shift == 0.0 {
                self.row_shift.remove(&key);
            } else {
                self.row_shift.insert(key, shift);
            }
        }
        let r = r.translate(vec2(0.0, shift));
        if let Some(s) = &mut self.settle {
            if s.key == ds.key && s.index == i {
                s.to = Some(r);
                return (r, false);
            }
        }
        (r, true)
    }

    /// `row_place` for lists that only need the rows they draw.
    pub(crate) fn row_rect(&mut self, ui: &Ui, ds: &DragState, i: usize, r: Rect) -> Option<Rect> {
        let (r, draw) = self.row_place(ui, ds, i, r);
        draw.then_some(r)
    }

    /// Inside a list's scroll area while a song is carried: the mouse wheel still scrolls, and
    /// holding the song near the top or bottom edge scrolls too.
    pub(crate) fn drag_scroll(&self, ui: &mut Ui, visible: Rect) {
        if self.lift.is_none() || !carrying(ui.ctx()) {
            return;
        }
        let Some(p) = ui.ctx().input(|i| i.pointer.latest_pos()) else { return };
        if !visible.x_range().contains(p.x) {
            return;
        }
        // Scroll areas ignore the wheel while something is dragged, so pass it on here.
        let wheel = ui.input(|i| i.smooth_scroll_delta);
        if visible.contains(p) && wheel != Vec2::ZERO {
            ui.scroll_with_delta(vec2(0.0, wheel.y));
        }
        if p.y < visible.top() - 40.0 || p.y > visible.bottom() + 40.0 {
            return;
        }
        let edge = 60.0;
        let up = ((visible.top() + edge - p.y) / edge).clamp(0.0, 1.0);
        let down = ((p.y - (visible.bottom() - edge)) / edge).clamp(0.0, 1.0);
        if up > 0.0 || down > 0.0 {
            // Faster the closer it is to the edge.
            let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
            ui.scroll_with_delta(vec2(0.0, (up.powf(1.5) - down.powf(1.5)) * 1400.0 * dt));
            ui.ctx().request_repaint();
        }
    }

    /// After a list's rows: if the song was let go over it, move it or put it in.
    pub(crate) fn drag_drop(&mut self, ui: &Ui, ds: &DragState) {
        if !ds.hovering || !ui.input(|i| i.pointer.any_released()) {
            return;
        }
        let Some(d) = egui::DragAndDrop::take_payload::<DragSongs>(ui.ctx()) else { return };
        let t = ds.gap.unwrap_or(ds.n);
        match (ds.list, ds.own) {
            (DragList::Songs, Some(s)) => {
                if s != t {
                    self.actions.push(Action::MoveInSongs { from: s, to: t });
                }
            }
            (DragList::Playlist(pid), Some(s)) => {
                let rows = self.playlist_ids(pid);
                if let (Some(a), Some(b)) = (rows.get(s), rows.get(t)) {
                    if a.0 != b.0 {
                        self.actions.push(Action::MoveInPlaylist { pid, from: a.0, to: b.0 });
                    }
                }
            }
            (DragList::Playlist(pid), None) => {
                let rows = self.playlist_ids(pid);
                let at = rows.get(t).map(|r| r.0);
                self.actions.push(Action::InsertInPlaylist { pid, at, ids: d.ids.clone() });
            }
            (DragList::Queue, Some(s)) => {
                let base = self.ps.index + 1;
                if s != t {
                    self.actions.push(Action::MoveQueue { from: base + s, to: base + t });
                }
            }
            (DragList::Queue, None) => {
                let base = if self.ps.current().is_some() { self.ps.index + 1 } else { self.ps.queue.len() };
                self.actions.push(Action::InsertInQueue { at: base + t, ids: d.ids.clone() });
            }
            (DragList::Songs, None) => {}
        }
        let now = ui.input(|i| i.time);
        if let Some(l) = self.lift.take() {
            self.settle = Some(Settle { key: ds.key, index: t, song: l.song, from: l.ghost, to: None, at: now });
        }
        // The rows already moved to where the new order puts them.
        self.row_shift.clear();
        self.flip_hold = now;
    }

    /// Moves and paints the lifted row (from the overlays, after every list had its turn).
    pub(crate) fn draw_drag(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.05);
        let aim = self.drag_aim.take();
        let layer = egui::LayerId::new(egui::Order::Tooltip, Id::new("drag-ghost"));
        let painter = ctx.layer_painter(layer);
        // A dropped song landing in its row: the lift and shadow fade as it gets there.
        if let Some(s) = &mut self.settle {
            let t = ((now - s.at) as f32 / SETTLE).clamp(0.0, 1.0);
            // Right after the drop its row isn't drawn yet: hold it where it was let go.
            let to = s.to.take().or((now - s.at < 0.05).then_some(s.from));
            let (song, from) = (s.song, s.from);
            match to {
                Some(to) if t < 1.0 => {
                    let e = egui::emath::easing::cubic_out(t);
                    self.paint_ghost(&painter, lerp_rect(from, to, e), song, 1, 1.0 - e, 1.0, true);
                    ctx.request_repaint();
                }
                _ => self.settle = None,
            }
        }
        let payload = egui::DragAndDrop::payload::<DragSongs>(ctx);
        let released = ctx.input(|i| i.pointer.any_released());
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        // Something started a drag without lifting a row: carry a small card.
        if self.lift.is_none() && !released {
            if let (Some(d), Some(p)) = (&payload, pointer) {
                if let Some(&song) = d.ids.first() {
                    let r = Rect::from_min_size(p - vec2(28.0, 26.0), vec2(280.0, 52.0));
                    self.lift = Some(Lift { song, count: d.ids.len(), src: None, grab: vec2(28.0, 26.0), home: r, home_seen: false, ghost: r, trail: [r; 3], since: now });
                }
            }
        }
        if self.lift.is_some() {
            if payload.is_none() || released {
                let l = self.lift.take().unwrap();
                // Taken by a playlist or the queue button: it shrinks into it. Let go nowhere
                // or Esc: back to where it came from.
                let taken = payload.is_none() && released;
                // Its row scrolled out of sight: nowhere to land, so it fades where it is.
                let lost = !taken && l.src.is_some() && !l.home_seen;
                let to = if taken || lost { w::scale_rect(l.ghost, if taken { 0.5 } else { 0.9 }) } else { l.home };
                self.fly = Some(Fly { song: l.song, count: l.count, from: l.ghost, to, at: now, fade: taken || lost });
            } else {
                let l = self.lift.as_ref().unwrap();
                let p = pointer.unwrap_or(l.ghost.center());
                // Not over a list: a compact card under the pointer.
                let aim = aim.unwrap_or_else(|| Rect::from_min_size(p - vec2(28.0, 26.0), vec2(280.0, 52.0)));
                let l = self.lift.as_mut().unwrap();
                if l.src.is_some() {
                    l.home_seen = false;
                }
                let prev = l.ghost;
                // Up and down it keeps up with the pointer; across and in size it glides (that's
                // the flight into the queue).
                let kx = 1.0 - (-dt * 15.0).exp();
                let ky = 1.0 - (-dt * 50.0).exp();
                let x0 = egui::lerp(prev.min.x..=aim.min.x, kx);
                let wdt = egui::lerp(prev.width()..=aim.width(), kx);
                let y0 = egui::lerp(prev.min.y..=aim.min.y, ky);
                let hgt = egui::lerp(prev.height()..=aim.height(), kx);
                let g = Rect::from_min_size(pos2(x0, y0), vec2(wdt, hgt));
                l.trail = [prev, l.trail[0], l.trail[1]];
                l.ghost = g;
                let moving = (g.min - aim.min).length() > 0.3 || (g.size() - aim.size()).length() > 0.3;
                let lift_t = egui::emath::easing::cubic_out(((now - l.since) as f32 / LIFT_IN).clamp(0.0, 1.0));
                let (song, count, trail) = (l.song, l.count, l.trail);
                // Motion blur while it flies sideways fast (into or out of the queue).
                let speed = (g.center().x - prev.center().x).abs() / dt;
                if speed > 400.0 {
                    // Faint copies smeared along the path of the last couple of frames.
                    let a = ((speed - 400.0) / 1600.0).clamp(0.0, 1.0);
                    let path = [trail[2], trail[1], trail[0], g];
                    for seg in 0..3 {
                        for k in 0..4 {
                            let f = k as f32 / 4.0;
                            let age = (3 - seg) as f32 - f; // how many frames back
                            let r = lerp_rect(path[seg], path[seg + 1], f);
                            self.paint_ghost(&painter, r, song, count, lift_t, a * 0.16 * (1.0 - age / 3.0), false);
                        }
                    }
                }
                self.paint_ghost(&painter, g, song, count, lift_t, 1.0, true);
                ctx.set_cursor_icon(CursorIcon::Grabbing);
                if moving || lift_t < 1.0 || speed > 1.0 {
                    ctx.request_repaint();
                }
            }
        }
        if let Some(f) = &self.fly {
            let t = ((now - f.at) as f32 / FLY).clamp(0.0, 1.0);
            if t >= 1.0 {
                self.fly = None;
            } else {
                let e = egui::emath::easing::cubic_out(t);
                let r = lerp_rect(f.from, f.to, e);
                let (song, count, home) = (f.song, f.count, !f.fade);
                // Going home it stays solid and lands; taken, it shrinks and fades.
                let alpha = if home { 1.0 - egui::emath::easing::cubic_in(t) * 0.6 } else { 1.0 - e };
                self.paint_ghost(&painter, r, song, count, 1.0 - e, alpha, true);
                ctx.request_repaint();
            }
        }
    }

    /// The lifted row: a raised card with the song's cover, title, artist and length, laid out
    /// like the list row it came from.
    #[allow(clippy::too_many_arguments)]
    fn paint_ghost(&mut self, p: &Painter, r: Rect, song: u64, count: usize, lift: f32, alpha: f32, shadow: bool) {
        if alpha <= 0.01 || r.width() < 8.0 || r.height() < 4.0 {
            return;
        }
        // Lifted: a touch bigger and casting a shadow, as if it's closer to you.
        let r = r.expand2(vec2(5.0, 2.0) * lift);
        if shadow {
            let sh = egui::epaint::Shadow {
                offset: [0, (3.0 + 9.0 * lift) as i8],
                blur: (10.0 + 22.0 * lift) as u8,
                spread: 0,
                color: Color32::from_black_alpha((170.0 * alpha) as u8),
            };
            p.add(sh.as_shape(r, R_MD));
        }
        // Landing, it turns back into a plain row.
        p.rect_filled(r, R_MD, with_alpha(lerp_color(RAISED, RAISED_2, lift), alpha * (0.45 + 0.55 * lift)));
        p.rect_stroke(r, R_MD, egui::Stroke::new(1.0, with_alpha(Color32::WHITE, (0.05 + 0.05 * lift) * alpha)), egui::StrokeKind::Inside);
        let info = self.info_for(song);
        // A wide song-list row has its number column first; the queue doesn't.
        let wide = ((r.width() - 380.0) / 160.0).clamp(0.0, 1.0);
        let side = (r.height() - 14.0).clamp(12.0, 40.0);
        let ar = Rect::from_min_size(pos2(r.left() + egui::lerp(8.0..=44.0, wide) + 5.0 * lift, r.center().y - side / 2.0), Vec2::splat(side));
        let stream_thumb = match &info {
            Some(t) if t.art.is_none() => t.preview.as_ref().and_then(|it| it.thumb_url()),
            _ => None,
        };
        match stream_thumb {
            Some(u) => {
                let tex = self.art.get(ArtKey::Url(u));
                w::thumb_square(p, ar, tex, R_SM);
                if alpha < 1.0 {
                    p.rect_filled(ar, R_SM, with_alpha(RAISED, 1.0 - alpha));
                }
            }
            None => {
                let tex = self.art.cover(info.as_ref().and_then(|t| t.art), ArtSize::S);
                w::art(p, ar, tex, R_SM, with_alpha(Color32::WHITE, alpha));
            }
        }
        let tx = ar.right() + S3;
        let right = r.right() - 12.0 - 5.0 * lift;
        let badge = count > 1;
        let tw = (right - tx - if badge { 80.0 } else { 52.0 }).max(0.0);
        let y = r.center().y;
        if let Some(t) = &info {
            w::text(p, pos2(tx, y - 9.0), Align2::LEFT_CENTER, &t.title, body(), with_alpha(TEXT, alpha), tw);
            w::text(p, pos2(tx, y + 10.0), Align2::LEFT_CENTER, &t.artist, small(), with_alpha(TEXT_2, alpha), tw);
            if !badge && t.duration > 0.0 && r.width() > 200.0 {
                w::text(p, pos2(right, y), Align2::RIGHT_CENTER, &fmt_time(t.duration), small(), with_alpha(TEXT_2, alpha), 60.0);
            }
        }
        if badge {
            let label = format!("{count} songs");
            let br = Rect::from_min_size(pos2(right - 68.0, y - 11.0), vec2(68.0, 22.0));
            p.rect_filled(br, 11.0, with_alpha(ACCENT, alpha));
            w::text(p, br.center(), Align2::CENTER_CENTER, &label, small_strong(), with_alpha(ON_ACCENT, alpha), 64.0);
        }
    }
}
