//! Now Playing: the record slides out of its sleeve and turns while the song plays, with the
//! queue beside it. Full screen (F11) shows the same over a blurred copy of the cover.

use crate::app::{Action, App, NowInfo, Route};
use crate::art::{ArtKey, ArtSize};
use crate::icons;
use crate::lyrics;
use crate::store::Repeat;
use crate::theme::{self, *};
use crate::widgets as w;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Id, Pos2, Rect, Response, Sense, Ui, Vec2};
use std::sync::Arc;
use std::time::Duration;

/// Seconds without touching the mouse before the full-screen controls fade out.
const IDLE_HIDE: f64 = 3.0;
/// How far the record's center sits right of the sleeve's center (× sleeve width):
/// paused, it shows a good slice; playing, it slides out about halfway.
const OUT_PAUSED: f32 = 0.30;
const OUT_PLAYING: f32 = 0.56;
const RECORD_R: f32 = 0.48;
/// The sleeve plus the record pulled all the way out, as a multiple of the sleeve width.
const SPREAD: f32 = 0.5 + OUT_PLAYING + RECORD_R;

/// A round dark button like the back/forward buttons.
fn round_button(ui: &Ui, rect: Rect, id: Id, icon: &str, active: bool, tip: &str) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let (h, pr) = w::hover_press(ui, &resp);
    let p = ui.painter();
    p.circle_filled(rect.center(), rect.width() / 2.0, with_alpha(Color32::BLACK, 0.35 + 0.15 * h));
    let c = if active { ACCENT } else { lerp_color(TEXT_2, TEXT, h) };
    icons::glyph(p, rect.center(), icon, rect.width() * 0.5 * (1.0 + 0.08 * h - 0.08 * pr), c);
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip)
}

impl App {
    /// Turn the record (33⅓ rpm, easing up to speed and down to a stop like a turntable).
    pub(crate) fn turn_record(&mut self, ctx: &egui::Context) -> f32 {
        let now = ctx.input(|i| i.time);
        let dt = (now - self.vinyl_last).clamp(0.0, 0.1) as f32;
        self.vinyl_last = now;
        let turning = self.ps.playing && !self.ps.waiting && self.settings.spin_record;
        let spin = ctx.animate_bool_with_time(Id::new("vinyl-spin"), turning, 0.8);
        let tau = std::f32::consts::TAU;
        self.vinyl_angle = (self.vinyl_angle + dt * spin * 3.49) % tau;
        self.backdrop_angle = (self.backdrop_angle + dt * spin * 0.035) % tau;
        self.vinyl_angle
    }

    /// The record and its sleeve, centered on `c`. Clicking either plays or pauses. While the
    /// song changes, the old cover and label fade into the new ones.
    fn draw_record(&mut self, ui: &Ui, c: Pos2, s: f32, t: &NowInfo, angle: f32, swap: Option<&(NowInfo, f32)>) {
        let out = ui.ctx().animate_bool_with_time(Id::new("vinyl-out"), self.ps.playing, 0.7);
        let out = egui::emath::easing::cubic_in_out(out);
        let sleeve = Rect::from_min_size(pos2(c.x - s * SPREAD / 2.0, c.y - s / 2.0), Vec2::splat(s));
        let r = s * RECORD_R;
        let rc = sleeve.center() + vec2(s * (OUT_PAUSED + (OUT_PLAYING - OUT_PAUSED) * out), 0.0);
        let p = ui.painter();
        let tex = self.art.cover(t.art, ArtSize::L);
        let label_of = |i: &NowInfo| if i.color.is_some() { theme::wash_from_art(i.color) } else { ACCENT_LO };
        let old = swap.map(|(o, e)| (self.art.cover(o.art, ArtSize::L), label_of(o), *e));
        w::vinyl(p, rc, r, tex, label_of(t), angle);
        if let Some((otex, olabel, e)) = old {
            w::vinyl_label(p, rc, r, otex, olabel, angle, 1.0 - e);
        }
        w::soft_shadow(p, sleeve, 6.0, 1.1);
        match old {
            // The new cover comes in over the old one.
            Some((otex, _, e)) => {
                w::art(p, sleeve, otex, 6.0, Color32::WHITE);
                w::art(p, sleeve, tex, 6.0, with_alpha(Color32::WHITE, e));
            }
            None => w::art(p, sleeve, tex, 6.0, Color32::WHITE),
        }
        p.rect_stroke(sleeve, 6.0, egui::Stroke::new(1.0, with_alpha(Color32::WHITE, 0.07)), egui::StrokeKind::Inside);
        let hit = Rect::from_min_max(sleeve.min, pos2(rc.x + r, sleeve.bottom()));
        let resp = ui.interact(hit, Id::new("np-record"), Sense::click());
        let tip = if self.ps.playing { "Pause" } else { "Play" };
        // No tooltip in full screen: it would sit on top of the record once the controls hide.
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        let resp = if self.immersive { resp } else { resp.on_hover_text(tip) };
        if resp.clicked() {
            self.actions.push(Action::Toggle);
        }
    }

    /// Title, then "artist · album" (the artist opens their page). Centered on `c`. While the
    /// song changes, the old title rises and fades as the new one comes up from below.
    fn song_titles(&mut self, ui: &Ui, t: &NowInfo, c: Pos2, max_w: f32, size: f32, swap: Option<&(NowInfo, f32)>) -> f32 {
        let mut e = 1.0;
        if let Some((old, k)) = swap {
            e = *k;
            let a = (1.0 - e * 2.5).max(0.0);
            let oc = c - vec2(0.0, 16.0 * e);
            let p = ui.painter();
            w::text(p, oc, Align2::CENTER_TOP, &old.title, FontId::new(size, bold()), with_alpha(TEXT, a), max_w);
            let sub_size = (size * 0.6).max(14.0);
            w::text(p, pos2(oc.x, oc.y + size * 1.35), Align2::CENTER_TOP, &old.artist, FontId::proportional(sub_size), with_alpha(TEXT_2, a), max_w);
        }
        let a = ((e - 0.2) / 0.8).clamp(0.0, 1.0);
        let c = c + vec2(0.0, 16.0 * (1.0 - e));
        let mut painter = ui.painter().clone();
        painter.multiply_opacity(a);
        let p = &painter;
        w::text(p, c, Align2::CENTER_TOP, &t.title, FontId::new(size, bold()), TEXT, max_w);
        let sub = if t.preview.is_some() {
            let site = t.preview.as_ref().map(|i| i.source()).unwrap_or_default();
            format!("{} · From {}", t.artist, site)
        } else if t.album.is_empty() {
            t.artist.clone()
        } else {
            format!("{} · {}", t.artist, t.album)
        };
        let sub_size = (size * 0.6).max(14.0);
        let sy = c.y + size * 1.35;
        let sr = w::text(p, pos2(c.x, sy), Align2::CENTER_TOP, &sub, FontId::proportional(sub_size), TEXT_2, max_w);
        if let Some(key) = t.artist_key {
            let resp = ui.interact(sr, Id::new(("np-artist", size as u32)), Sense::click());
            if resp.hovered() {
                p.line_segment([sr.left_bottom(), sr.right_bottom()], egui::Stroke::new(1.0, TEXT_2));
            }
            if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                if self.immersive {
                    self.actions.push(Action::ToggleImmersive);
                }
                self.actions.push(Action::Go(Route::Artist(key)));
            }
        }
        sr.bottom() - 16.0 * (1.0 - e)
    }

    /// Love (or download, for a song that isn't saved yet) and a "…" menu, centered on `c`.
    fn song_actions(&mut self, ui: &mut Ui, t: &NowInfo, c: Pos2, salt: &str) {
        let in_lib = self.lib.get(t.id).is_some();
        let n = if in_lib { 2.0 } else { 1.0 };
        let mut x = c.x - (n - 1.0) * 22.0;
        let lr = Rect::from_center_size(pos2(x, c.y), Vec2::splat(36.0));
        let lresp = ui.interact(lr, Id::new((salt, "love")), Sense::click());
        match &t.preview {
            None => {
                if self.love_button(ui, lr, lresp, t.id).clicked() {
                    self.actions.push(Action::ToggleLove(t.id));
                }
            }
            Some(item) => self.save_preview_button(ui, lr, lresp, item),
        }
        x += 44.0;
        if in_lib {
            let mr = Rect::from_center_size(pos2(x, c.y), Vec2::splat(36.0));
            let mresp = ui.interact(mr, Id::new((salt, "more")), Sense::click());
            let mresp = w::icon_button_at(ui, mr, mresp, icons::ELLIPSIS, false, "More");
            let id = t.id;
            egui::Popup::menu(&mresp).show(|ui| self.song_menu(ui, id, None));
        }
    }

    /// The cover, blurred, filling `rect`.
    fn blurred_cover(&mut self, p: &egui::Painter, rect: Rect, art: Option<u64>, alpha: f32, turn: Option<f32>) {
        let Some(k) = art else { return };
        let Some(tex) = self.art.get(ArtKey::Blur(k)) else { return };
        let tint = with_alpha(Color32::WHITE, alpha);
        match turn {
            // Big enough that the corners never show while it turns.
            Some(a) => w::rotated_image(p, rect.center(), rect.width().max(rect.height()) * 1.6, tex, a, tint),
            None => {
                // Cover the rect without stretching: crop the square to its shape.
                let (wd, ht) = (rect.width(), rect.height());
                let uv = if wd > ht {
                    let k = ht / wd / 2.0;
                    Rect::from_min_max(pos2(0.0, 0.5 - k), pos2(1.0, 0.5 + k))
                } else {
                    let k = wd / ht / 2.0;
                    Rect::from_min_max(pos2(0.5 - k, 0.0), pos2(0.5 + k, 1.0))
                };
                p.add(egui::epaint::RectShape::filled(rect, R_LG, tint).with_texture(tex, uv));
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // The page

    pub(crate) fn now_playing_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let rect = ui.max_rect();
        let Some(t) = self.now_info() else {
            ui.add_space(TOPBAR_H + S8);
            if w::empty_state(ui, icons::MUSIC, "Nothing playing", "Pick a song and it shows up here, on a record.", Some(("Browse songs", None))) {
                self.actions.push(Action::Go(Route::Songs));
            }
            return (0.0, String::new());
        };
        let angle = self.turn_record(ui.ctx());
        let p = ui.painter().clone();
        // A new song: everything cross-fades over half a second.
        let swap = self.song_swap(ui.ctx(), 0.5);
        match &swap {
            Some((old, e)) => {
                self.blurred_cover(&p, rect, old.art, 0.38 * (1.0 - e), None);
                self.blurred_cover(&p, rect, t.art, 0.38 * e, None);
            }
            None => self.blurred_cover(&p, rect, t.art, 0.38, None),
        }
        w::vgradient(&p, rect, R_LG, &[(0.0, with_alpha(Color32::BLACK, 0.10)), (0.6, with_alpha(Color32::BLACK, 0.30)), (1.0, with_alpha(Color32::BLACK, 0.55))]);

        // Top right: Up next and full screen.
        let by = rect.top() + TOPBAR_H / 2.0;
        let fr = Rect::from_center_size(pos2(rect.right() - PAD_X - 12.0, by), Vec2::splat(32.0));
        if round_button(ui, fr, Id::new("np-full"), icons::MAXIMIZE, false, "Full screen (F11)").clicked() {
            self.actions.push(Action::ToggleImmersive);
        }
        let qr = fr.translate(vec2(-40.0, 0.0));
        let lm = self.lyrics_mode();
        let qtip = if self.np_queue && !lm { "Hide Up next" } else { "Show Up next" };
        if round_button(ui, qr, Id::new("np-queue"), icons::LIST_MUSIC, self.np_queue && !lm, qtip).clicked() {
            self.toggle_side(false, false);
        }
        if self.settings.show_lyrics {
            let ltip = if self.np_queue && lm { "Hide lyrics" } else { "Show lyrics" };
            if round_button(ui, qr.translate(vec2(-40.0, 0.0)), Id::new("np-lyrics"), icons::LYRICS, self.np_queue && lm, ltip).clicked() {
                self.toggle_side(false, true);
            }
        }

        let body = Rect::from_min_max(pos2(rect.left() + PAD_X, rect.top() + TOPBAR_H), pos2(rect.right() - PAD_X, rect.bottom() - S5));
        // Lyrics take more room than Up next; the change of width glides.
        let side_goal = if lm { (body.width() * 0.5).clamp(300.0, 680.0) } else { (body.width() * 0.4).clamp(280.0, 400.0) };
        let q_w = ui.ctx().animate_value_with_time(Id::new("np-side-w"), side_goal, T_SLOW);
        let fits = body.width() - q_w - S6 >= 300.0;
        // Up next slides in from the right; the record moves over to make room.
        let qt = egui::emath::easing::cubic_out(ui.ctx().animate_bool_with_time(Id::new("np-q-open"), self.np_queue && fits, T_SLOW));
        let stage = Rect::from_min_max(body.min, pos2(body.right() - (q_w + S6) * qt, body.bottom()));
        let info_h = 120.0;
        let s = ((stage.width() - S4) / SPREAD).min(stage.height() - info_h - S6).clamp(96.0, 420.0);
        let top = stage.top() + ((stage.height() - s - info_h) / 2.0).max(S2);
        let c = pos2(stage.center().x, top + s / 2.0);
        self.draw_record(ui, c, s, &t, angle, swap.as_ref());
        let bottom = self.song_titles(ui, &t, pos2(c.x, top + s + S8), stage.width(), 26.0, swap.as_ref());
        self.song_actions(ui, &t, pos2(c.x, bottom + 28.0), "np");

        if qt > 0.01 {
            let qr = Rect::from_min_max(pos2(body.right() - q_w, body.top() + S1), body.max).translate(vec2((1.0 - qt) * (q_w + PAD_X + S4), 0.0));
            if lm {
                // Lyrics float on the background, no panel behind them.
                let lr = qr.shrink2(vec2(S2, 0.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr).layout(egui::Layout::top_down(egui::Align::Min)));
                child.multiply_opacity(qt);
                self.lyrics_panel(&mut child, lr, (side_goal / 20.0).clamp(20.0, 32.0).round(), "np");
            } else {
                p.rect_filled(qr, R_LG, with_alpha(Color32::BLACK, 0.28 * qt));
                p.rect_stroke(qr, R_LG, egui::Stroke::new(1.0, with_alpha(Color32::WHITE, 0.05 * qt)), egui::StrokeKind::Inside);
                let inner = qr.shrink2(vec2(S3, S4));
                w::text(&p, pos2(inner.left() + S1, inner.top() + 12.0), Align2::LEFT_CENTER, "Up next", h2(), with_alpha(TEXT, qt), inner.width());
                let lr = Rect::from_min_max(pos2(inner.left(), inner.top() + 34.0), inner.max);
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr).layout(egui::Layout::top_down(egui::Align::Min)));
                child.multiply_opacity(qt);
                self.queue_list(&mut child, lr, false, "np");
            }
        }
        (0.0, String::new())
    }

    // -----------------------------------------------------------------------------------------
    // Lyrics

    /// The lyrics in `area`, straight on the background: the line being sung is bright and
    /// stays in place while the rest slide past and fade out at the edges. Click a line to jump
    /// there; the wheel looks ahead, and a few seconds later the song takes over again.
    pub(crate) fn lyrics_panel(&mut self, ui: &mut Ui, area: Rect, size: f32, salt: &str) {
        /// Light a line up a moment early, the way people read ahead when they sing along.
        const LEAD: f32 = 0.2;
        const INSET: f32 = 4.0;
        let p = ui.painter().clone();
        let ctx = ui.ctx().clone();
        let note = |main: &str, sub: &str| {
            w::text(&p, area.center() - vec2(0.0, 10.0), Align2::CENTER_CENTER, main, h2(), TEXT, area.width());
            w::text(&p, area.center() + vec2(0.0, 16.0), Align2::CENTER_CENTER, sub, small(), TEXT_2, area.width());
        };
        // Until the song playing has been looked up, whatever is in `lyrics` is another song's.
        let current = self.lyrics_for.is_some() && self.lyrics_for == self.current_id();
        let l = match (&self.lyrics, current) {
            (lyrics::State::Found(l), true) => Arc::clone(l),
            (lyrics::State::Missing, true) => {
                note("No lyrics found for this song", "Or put a .lrc file with the same name next to it.");
                if retry_button(ui, area, salt) {
                    self.actions.push(Action::RetryLyrics);
                }
                return;
            }
            (lyrics::State::Failed, true) => {
                note("Couldn't load lyrics", "Check your internet. Segno tries again by itself too.");
                if retry_button(ui, area, salt) {
                    self.actions.push(Action::RetryLyrics);
                }
                // The app asks again after half a minute; wake up for it.
                ctx.request_repaint_after(Duration::from_secs(31));
                return;
            }
            _ => return note("Looking for lyrics…", ""),
        };
        // Plain lyrics aren't timed: they sit still, start at the top and scroll by hand.
        let plain = !l.synced;
        let id = self.lyrics_for.unwrap_or(0);
        let now = ctx.input(|i| i.time);
        let pos = self.seek_preview.unwrap_or(self.pos);
        let idx = l.line_at(pos + LEAD);

        // Break the lines into rows once per song and size, not every frame.
        let font = FontId::new(size, theme::bold());
        let wrap = ((area.width() - 2.0 * INSET) / 8.0).floor().max(5.0) * 8.0;
        let key = (id, l.lines.len(), size.to_bits(), wrap.to_bits(), ctx.pixels_per_point().to_bits());
        let lay = match self.lyr.layout.take() {
            Some(lay) if lay.key == key => lay,
            _ => {
                self.lyr.shown = None;
                LyricsLayout::new(ui, &l, &font, wrap, key)
            }
        };

        // The line being sung cross-fades with the one before it.
        if idx != self.lyr.line {
            self.lyr.prev = self.lyr.line;
            self.lyr.line = idx;
            self.lyr.changed = now;
        }
        let fade = ((now - self.lyr.changed) / 0.3).clamp(0.0, 1.0) as f32;
        if fade < 1.0 {
            ctx.request_repaint();
        }

        // Keep the line being sung in place; the wheel looks ahead or back for a moment.
        let ai = idx.unwrap_or(0);
        let center = if plain { 0.0 } else { lay.lines[ai].top + lay.lines[ai].rows.len() as f32 * lay.row_h / 2.0 };
        if ui.rect_contains_pointer(area) {
            let d = ctx.input(|i| i.smooth_scroll_delta.y);
            if d != 0.0 {
                let (lo, hi) = if plain { (0.0, (lay.height - area.height() * 0.6).max(0.0)) } else { (-center, lay.height - center) };
                self.lyr.off = (self.lyr.off - d).clamp(lo, hi);
                self.lyr.touched = now;
            }
        }
        if self.lyr.off != 0.0 && !plain {
            let quiet = now - self.lyr.touched;
            if quiet > 3.0 {
                self.lyr.off = 0.0;
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(3.05 - quiet));
            }
        }
        let goal = center + self.lyr.off;
        let shown = match self.lyr.shown {
            Some(s) if (goal - s).abs() > 0.5 => {
                let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.05);
                ctx.request_repaint();
                s + (goal - s) * (1.0 - (-dt * 9.0).exp())
            }
            _ => goal,
        };
        self.lyr.shown = Some(shown);

        let anchor = if plain { area.top() + 44.0 } else { area.top() + area.height() * 0.42 };
        let edge_fade = (area.height() * 0.18).clamp(40.0, 160.0);
        let clip = p.with_clip_rect(area.intersect(p.clip_rect()));
        if plain {
            w::text(&p, pos2(area.left() + INSET, area.top() + 12.0), Align2::LEFT_CENTER, "Not timed to the song", small(), TEXT_2, area.width());
        }
        for (i, line) in lay.lines.iter().enumerate() {
            let h = line.rows.len() as f32 * lay.row_h;
            let top = anchor + line.top - shown;
            if top + h <= area.top() {
                continue;
            }
            if top >= area.bottom() {
                break;
            }
            let on = if Some(i) == self.lyr.line {
                fade
            } else if Some(i) == self.lyr.prev {
                1.0 - fade
            } else {
                0.0
            };
            let dim = (0.5 - 0.05 * (i as f32 - ai as f32).abs()).max(0.3);
            let mid = top + h / 2.0;
            // Lines fade out toward the top and bottom instead of being cut off (plain lyrics
            // only fade at the top once scrolled).
            let from_top = if plain && shown < 1.0 { edge_fade } else { mid - area.top() };
            let edge = (from_top / edge_fade).min((area.bottom() - mid) / edge_fade).clamp(0.0, 1.0);
            let edge = edge * edge * (3.0 - 2.0 * edge);
            let a = if plain {
                0.85
            } else {
                // Timed lines: click one to jump there.
                let hit = Rect::from_min_size(pos2(area.left(), top), vec2(area.width(), h)).intersect(area);
                let resp = ui.interact(hit, Id::new(("lyr-line", salt, i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                if resp.clicked() {
                    self.actions.push(Action::Seek(l.lines[i].0));
                    self.lyr.off = 0.0;
                }
                (egui::lerp(dim..=1.0, on) + if resp.hovered() { 0.2 } else { 0.0 }).min(1.0)
            } * edge;
            for (k, row) in line.rows.iter().enumerate() {
                // line_job puts right-to-left words in the right order.
                let g = p.layout_job(theme::line_job(row, font.clone(), Color32::PLACEHOLDER, wrap));
                let x = if line.rtl { area.right() - INSET - g.size().x } else { area.left() + INSET };
                clip.galley(pos2(x, top + k as f32 * lay.row_h), g, with_alpha(TEXT, a));
            }
        }
        // Wake up for the next line (the record may not be turning to do it for us).
        if self.ps.playing && !plain {
            let wait = l.next_start(idx).map_or(1.0, |n| (n - pos - LEAD).max(0.0));
            ctx.request_repaint_after(Duration::from_secs_f32(wait.clamp(0.016, 1.0)));
        }
        self.lyr.layout = Some(lay);
    }

    // -----------------------------------------------------------------------------------------
    // Shared controls (also used by the player bar)

    /// Seek bar in `seek`. Returns (position, length) to label it with.
    pub(crate) fn seek_bar(&mut self, ui: &mut Ui, seek: Rect, id: Id) -> (f32, f32) {
        let dur = self.now_duration();
        let pos = self.seek_preview.unwrap_or(self.pos);
        let frac = if dur > 0.0 { pos / dur } else { 0.0 };
        let out = w::slider(ui, seek, id, frac, true);
        if let Some(v) = out.value {
            self.seek_preview = Some(v * dur);
        }
        if out.released {
            if let Some(p) = self.seek_preview.take() {
                if self.ps.current().is_some() {
                    self.actions.push(Action::Seek(p));
                    self.pos = p;
                }
            }
        }
        if !out.dragging && !out.released && out.value.is_none() {
            self.seek_preview = None;
        }
        (self.seek_preview.unwrap_or(self.pos), dur)
    }

    /// Shuffle, previous, play, next, repeat, centered on `c`. `big` is the full-screen size.
    pub(crate) fn transport_row(&mut self, ui: &mut Ui, c: Pos2, big: bool, salt: &str) {
        let (side, skip, play, gap) = if big { (40.0, 48.0, 64.0, S5) } else { (32.0, 36.0, 40.0, S3) };
        let wd = side * 2.0 + skip * 2.0 + play + gap * 4.0;
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt((salt, "transport"))
                .max_rect(Rect::from_center_size(c, vec2(wd + 2.0, play)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing.x = gap;
        let shuffle_tip = if self.ps.shuffle { "Shuffle is on" } else { "Shuffle" };
        if w::icon_button(&mut row, side, icons::SHUFFLE, self.ps.shuffle, shuffle_tip).clicked() {
            self.actions.push(Action::ToggleShuffle);
        }
        let base = if big { TEXT } else { TEXT_2 };
        if w::transport_button_c(&mut row, skip, w::Transport::Prev, "Previous (Ctrl+←)", base).clicked() {
            self.actions.push(Action::Prev);
        }
        if w::play_circle(&mut row, play, self.ps.playing, true).clicked() {
            self.actions.push(Action::Toggle);
        }
        if w::transport_button_c(&mut row, skip, w::Transport::Next, "Next (Ctrl+→)", base).clicked() {
            self.actions.push(Action::Next);
        }
        let (rep_icon, rep_tip) = match self.ps.repeat {
            Repeat::Off => (icons::REPEAT, "Repeat"),
            Repeat::All => (icons::REPEAT, "Repeating all"),
            Repeat::One => (icons::REPEAT_1, "Repeating this song"),
        };
        if w::icon_button(&mut row, side, rep_icon, self.ps.repeat != Repeat::Off, rep_tip).clicked() {
            self.actions.push(Action::CycleRepeat);
        }
    }

    /// Mute button at `icon_c` and a volume slider in `slider` (the wheel works over it too).
    pub(crate) fn volume_control(&mut self, ui: &mut Ui, slider: Rect, icon_c: Pos2, salt: &str) {
        let shown = if self.ps.muted { 0.0 } else { self.ps.volume };
        let vo = w::slider(ui, slider, Id::new((salt, "volume")), shown, false);
        if vo.hovered {
            let dy: f32 = ui.input(|i| {
                i.events
                    .iter()
                    .map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } => delta.y,
                        _ => 0.0,
                    })
                    .sum()
            });
            if dy.abs() > 0.0 {
                let v = (self.ps.volume + dy.signum() * 0.05).clamp(0.0, 1.0);
                self.actions.push(Action::Volume(v));
                self.ps.volume = v;
            }
        }
        if let Some(v) = vo.value {
            if (v - self.ps.volume).abs() > 0.002 || self.ps.muted {
                self.actions.push(Action::Volume(v));
                self.ps.volume = v;
                self.ps.muted = false;
            }
        }
        let icon = if self.ps.muted || self.ps.volume <= 0.001 {
            icons::VOLUME_X
        } else if self.ps.volume < 0.35 {
            icons::VOLUME_0
        } else if self.ps.volume < 0.7 {
            icons::VOLUME_1
        } else {
            icons::VOLUME_2
        };
        let vr = Rect::from_center_size(icon_c, Vec2::splat(32.0));
        let vresp = ui.interact(vr, Id::new((salt, "mute")), Sense::click());
        if w::icon_button_at(ui, vr, vresp, icon, false, if self.ps.muted { "Unmute" } else { "Mute" }).clicked() {
            self.actions.push(Action::ToggleMute);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Full screen

    pub(crate) fn immersive_ui(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let p = ui.painter().clone();
        let t = self.now_info();
        let angle = self.turn_record(&ctx);

        // Background: the cover, blurred and slowly turning, under a dark veil.
        let swap = self.song_swap(&ctx, 0.6);
        let wash = theme::wash_from_art(t.as_ref().and_then(|t| t.color));
        let wash = match &swap {
            Some((old, e)) => lerp_color(theme::wash_from_art(old.color), wash, *e),
            None => wash,
        };
        p.rect_filled(full, 0.0, lerp_color(wash, BG, 0.4));
        let turn = Some(self.backdrop_angle);
        match &swap {
            Some((old, e)) => {
                self.blurred_cover(&p, full, old.art, 0.9 * (1.0 - e), turn);
                self.blurred_cover(&p, full, t.as_ref().and_then(|t| t.art), 0.9 * e, turn);
            }
            None => self.blurred_cover(&p, full, t.as_ref().and_then(|t| t.art), 0.9, turn),
        }
        w::vgradient(&p, full, 0.0, &[(0.0, with_alpha(Color32::BLACK, 0.45)), (0.45, with_alpha(Color32::BLACK, 0.32)), (1.0, with_alpha(Color32::BLACK, 0.78))]);
        let bg = ui.interact(full, Id::new("imm-bg"), Sense::click());
        if bg.double_clicked() {
            self.actions.push(Action::ToggleImmersive);
        }

        // The controls fade after a few seconds without the mouse moving (while playing).
        let active = ctx.input(|i| {
            i.pointer.delta() != Vec2::ZERO || i.pointer.any_down() || i.smooth_scroll_delta != Vec2::ZERO || !i.keys_down.is_empty()
        });
        if active || !self.ps.playing || egui::DragAndDrop::has_any_payload(&ctx) {
            self.imm_active = now;
        }
        let quiet = now - self.imm_active;
        let idle = quiet > IDLE_HIDE;
        let ch = ctx.animate_bool_with_time(Id::new("imm-chrome"), !idle, 0.45);
        let hide_cursor = idle && ch < 0.01;
        if !idle {
            ctx.request_repaint_after(Duration::from_secs_f64(IDLE_HIDE - quiet + 0.05));
        }

        let pad = (full.height() * 0.06).clamp(S6, 64.0);
        let area = full.shrink(pad);
        let lm = self.lyrics_mode();
        let side_goal = if lm { (area.width() * 0.5).clamp(360.0, 820.0) } else { (full.width() * 0.3).clamp(320.0, 440.0) };
        let q_w = ctx.animate_value_with_time(Id::new("imm-side-w"), side_goal, T_SLOW);
        let fits = area.width() - q_w - pad >= 420.0;
        // Up next slides in from the right; the record moves over to make room.
        let qt = egui::emath::easing::cubic_out(ctx.animate_bool_with_time(Id::new("imm-q-open"), self.imm_queue && fits, T_SLOW));
        let stage = Rect::from_min_max(area.min, pos2(area.right() - (q_w + pad) * qt, area.bottom()));

        // Top corners: Up next and leave full screen.
        if ch > 0.02 {
            let mut top = ui.new_child(egui::UiBuilder::new().max_rect(full).id_salt("imm-top"));
            top.multiply_opacity(ch);
            let er = Rect::from_center_size(pos2(full.right() - S6 - 20.0, full.top() + S6 + 20.0), Vec2::splat(40.0));
            if round_button(&top, er, Id::new("imm-exit"), icons::MINIMIZE, false, "Leave full screen (Esc)").clicked() {
                self.actions.push(Action::ToggleImmersive);
            }
            let qr = er.translate(vec2(-48.0, 0.0));
            let lm = self.lyrics_mode();
            let tip = if self.imm_queue && !lm { "Hide Up next" } else { "Show Up next" };
            if round_button(&top, qr, Id::new("imm-queue"), icons::LIST_MUSIC, self.imm_queue && !lm, tip).clicked() {
                self.toggle_side(true, false);
            }
            if self.settings.show_lyrics {
                let ltip = if self.imm_queue && lm { "Hide lyrics" } else { "Show lyrics" };
                if round_button(&top, qr.translate(vec2(-48.0, 0.0)), Id::new("imm-lyrics"), icons::LYRICS, self.imm_queue && lm, ltip).clicked() {
                    self.toggle_side(true, true);
                }
            }
        }

        let Some(t) = t else {
            if hide_cursor {
                ctx.set_cursor_icon(CursorIcon::None);
            }
            icons::glyph(&p, stage.center() - vec2(0.0, 30.0), icons::MUSIC, 64.0, TEXT_3);
            w::text(&p, stage.center() + vec2(0.0, 30.0), Align2::CENTER_CENTER, "Nothing playing", h2(), TEXT_2, stage.width());
            return;
        };

        // Record + title + controls sit together in the middle. When the controls fade out,
        // the record and title glide down to the middle on their own (and back up with them).
        const WITH_CONTROLS: f32 = 230.0;
        const TITLE_ONLY: f32 = 104.0;
        let s = (stage.width() / SPREAD).min(stage.height() - WITH_CONTROLS - S6).clamp(120.0, 560.0);
        let info_h = egui::lerp(TITLE_ONLY..=WITH_CONTROLS, egui::emath::easing::cubic_in_out(ch));
        let top = stage.top() + ((stage.height() - s - info_h) / 2.0).max(0.0);
        let c = pos2(stage.center().x, top + s / 2.0);
        self.draw_record(ui, c, s, &t, angle, swap.as_ref());
        let title_size = if s > 400.0 { 32.0 } else { 26.0 };
        let tb = self.song_titles(ui, &t, pos2(c.x, top + s + 40.0), stage.width(), title_size, swap.as_ref());

        // Seek bar, times, then the transport row with love on the left and volume on the right.
        let cw = (s * SPREAD).clamp(360.0, 620.0).min(stage.width());
        let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(full).id_salt("imm-controls"));
        // Squared: the controls show up once the record has mostly moved up for them, and
        // fade first when they hide.
        cu.multiply_opacity(ch * ch);
        if ch > 0.02 {
            let sy = tb + 30.0;
            let seek = Rect::from_center_size(pos2(c.x, sy), vec2(cw, 12.0));
            let (pos, dur) = self.seek_bar(&mut cu, seek, Id::new("imm-seek"));
            let tp = cu.painter().clone();
            w::text(&tp, pos2(seek.left(), sy + 18.0), Align2::LEFT_CENTER, &fmt_time(pos), small(), TEXT_2, 80.0);
            let left = if dur > 0.0 { format!("-{}", fmt_time((dur - pos).max(0.0))) } else { String::new() };
            w::text(&tp, pos2(seek.right(), sy + 18.0), Align2::RIGHT_CENTER, &left, small(), TEXT_2, 80.0);
            let ry = sy + 66.0;
            self.transport_row(&mut cu, pos2(c.x, ry), true, "imm");
            let lr = Rect::from_center_size(pos2(seek.left() + 18.0, ry), Vec2::splat(36.0));
            let lresp = cu.interact(lr, Id::new("imm-love"), Sense::click());
            match &t.preview {
                None => {
                    if self.love_button(&cu, lr, lresp, t.id).clicked() {
                        self.actions.push(Action::ToggleLove(t.id));
                    }
                }
                Some(item) => self.save_preview_button(&cu, lr, lresp, item),
            }
            if cw >= 460.0 {
                let vol = Rect::from_min_max(pos2(seek.right() - 84.0, ry - 6.0), pos2(seek.right(), ry + 6.0));
                self.volume_control(&mut cu, vol, pos2(vol.left() - 22.0, ry), "imm");
            }
        }

        if qt > 0.01 {
            let qr = Rect::from_min_max(pos2(area.right() - q_w, area.top() + 56.0), area.max).translate(vec2((1.0 - qt) * (q_w + pad + S6), 0.0));
            if lm {
                // Lyrics float on the background, no panel behind them.
                let lr = Rect::from_min_max(pos2(qr.left(), area.top() + 40.0), qr.max);
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr).id_salt("imm-q").layout(egui::Layout::top_down(egui::Align::Min)));
                child.multiply_opacity(qt);
                self.lyrics_panel(&mut child, lr, (side_goal / 18.0).clamp(26.0, 42.0).round(), "imm");
            } else {
                p.rect_filled(qr, 16.0, with_alpha(Color32::BLACK, 0.34 * qt));
                p.rect_stroke(qr, 16.0, egui::Stroke::new(1.0, with_alpha(Color32::WHITE, 0.06 * qt)), egui::StrokeKind::Inside);
                let inner = qr.shrink2(vec2(S3, S4));
                w::text(&p, pos2(inner.left() + S1, inner.top() + 12.0), Align2::LEFT_CENTER, "Up next", h2(), with_alpha(TEXT, qt), inner.width());
                let lr = Rect::from_min_max(pos2(inner.left(), inner.top() + 34.0), inner.max);
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr).id_salt("imm-q").layout(egui::Layout::top_down(egui::Align::Min)));
                child.multiply_opacity(qt);
                self.queue_list(&mut child, lr, false, "imm");
                if ui.rect_contains_pointer(qr) {
                    self.imm_active = now;
                }
            }
        }
        // Last, so nothing drawn above sets the pointer back.
        if hide_cursor {
            ctx.set_cursor_icon(CursorIcon::None);
        }
    }
}

/// "Try again" under the lyrics panel's message.
fn retry_button(ui: &mut Ui, area: Rect, salt: &str) -> bool {
    let r = Rect::from_center_size(area.center() + vec2(0.0, 64.0), vec2(area.width(), 36.0));
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("lyr-retry", salt)).layout(egui::Layout::top_down(egui::Align::Center)));
    w::pill(&mut b, "Try again", Some(icons::REFRESH), w::PillKind::Ghost).clicked()
}

/// What the lyrics panel keeps between frames.
#[derive(Default)]
pub(crate) struct LyricsView {
    /// The lines broken into rows, for one song at one size.
    layout: Option<LyricsLayout>,
    /// Where the list is scrolled to; it eases toward the line being sung. None: jump there.
    shown: Option<f32>,
    /// A wheel scroll away from the line being sung, and when it last moved.
    off: f32,
    touched: f64,
    /// The line being sung, the one before it, and when it changed (they cross-fade).
    line: Option<usize>,
    prev: Option<usize>,
    changed: f64,
}

struct LyricsLayout {
    /// Song, number of lines, font size, row width and screen scale it was made for.
    key: (u64, usize, u32, u32, u32),
    lines: Vec<LaidLine>,
    row_h: f32,
    height: f32,
}

struct LaidLine {
    rows: Vec<String>,
    /// From the top of the first line.
    top: f32,
    /// Arabic or Hebrew: rows line up on the right.
    rtl: bool,
}

impl LyricsLayout {
    fn new(ui: &Ui, l: &lyrics::Lyrics, font: &FontId, wrap: f32, key: (u64, usize, u32, u32, u32)) -> Self {
        let p = ui.painter();
        let width = |s: &str| p.layout_no_wrap(s.to_string(), font.clone(), Color32::PLACEHOLDER).size().x;
        let row_h = ui.fonts_mut(|f| f.row_height(font));
        let gap = font.size * 0.9;
        let (mut y, mut rtl) = (0.0, false);
        let lines = l
            .lines
            .iter()
            .map(|(_, text)| {
                // A pause sits on the same side as the words before it.
                let rows = if text.is_empty() {
                    vec![if l.synced { "· · ·".to_string() } else { String::new() }]
                } else {
                    rtl = theme::has_rtl(text);
                    lyrics::wrap_words(text, wrap, width)
                };
                let line = LaidLine { top: y, rtl, rows };
                y += line.rows.len() as f32 * row_h + gap;
                line
            })
            .collect();
        Self { key, lines, row_h, height: y }
    }
}
