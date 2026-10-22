//! The queue list, shared by the queue panel, the Now Playing page and full screen: drag to
//! reorder, remove, add songs from the library, and the library songs that play after it.

use crate::app::{Action, App, DragSongs};
use crate::art::ArtSize;
use crate::drag::DragList;
use crate::icons;
use crate::player::{Origin, QItem};
use crate::store::Repeat;
use crate::theme::*;
use crate::views::{menu_item, menu_sep};
use crate::widgets as w;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Response, Sense, Ui, Vec2};
use std::collections::HashSet;

const ROW: f32 = 56.0;
/// A group heading inside the queue ("Added by you", "From Road Trip Mix"…).
const HEAD_H: f32 = 30.0;

/// Hover for rows that sit on a colored backdrop: a light veil instead of a gray block.
fn glass_hover(p: &egui::Painter, rect: Rect, h: f32) {
    if h > 0.0 {
        p.rect_filled(rect, R_MD, with_alpha(Color32::WHITE, 0.07 * h));
    }
}

/// Where the "Add songs" box puts what you pick.
#[derive(Clone, Copy)]
pub enum AddTo {
    Queue,
    Playlist(u64),
}

impl App {
    /// The queue inside `list_r`. `show_current` lists the playing song first (the side panel);
    /// the Now Playing views already show it big. `salt` keeps ids apart between places.
    pub(crate) fn queue_list(&mut self, ui: &mut Ui, list_r: Rect, show_current: bool, salt: &str) {
        let key = Id::new((salt, "q-drag"));
        let Some(cur) = self.ps.current().cloned() else {
            // Songs dropped here start playing.
            let ds = self.drag_layout(ui, DragList::Queue, key, list_r, list_r.x_range(), list_r.top() + S8, ROW, 0, true);
            let hl = ui.ctx().animate_bool_with_time(key.with("empty-h"), ds.hovering, T_FAST);
            if hl > 0.0 {
                ui.painter().rect_filled(list_r.shrink(S1), R_LG, with_alpha(ACCENT, 0.10 * hl));
                ui.painter().rect_stroke(list_r.shrink(S1), R_LG, egui::Stroke::new(1.0, with_alpha(ACCENT, 0.5 * hl)), egui::StrokeKind::Inside);
            }
            ui.add_space(S8);
            w::empty_state(ui, icons::LIST_MUSIC, "Nothing queued", "Play a song, or drag songs here to line them up.", None);
            self.drag_drop(ui, &ds);
            return;
        };
        let p = ui.painter().clone();
        let mut y = list_r.top();
        if show_current {
            w::section_label(&p, pos2(list_r.left() + S1, y + 10.0), "Now playing");
            y += 22.0;
            let r = Rect::from_min_size(pos2(list_r.left(), y), vec2(list_r.width(), ROW));
            self.queue_row(ui, r, &cur, self.ps.index, true, salt, false);
            y += ROW + S3;
        }
        let base = self.ps.index + 1;
        let n = self.ps.queue.len().saturating_sub(base);
        // The queue in groups: what you added, the rest of what you pressed play on, then what
        // Segno adds from your library. Each group starts with a heading.
        let origins: Vec<Origin> = self.ps.queue[base.min(self.ps.queue.len())..].iter().map(|q| q.origin).collect();
        let mut heads: Vec<usize> = Vec::new();
        let mut head_kind: Vec<Origin> = Vec::new();
        for (k, o) in origins.iter().enumerate() {
            if k == 0 || origins[k - 1] != *o {
                heads.push(k);
                head_kind.push(*o);
            }
        }

        // Header: how many are lined up, then Add and Clear.
        let hr = Rect::from_min_size(pos2(list_r.left(), y), vec2(list_r.width(), 28.0));
        let label = match (show_current, n) {
            (true, 0) => "Next up".to_string(),
            (true, n) => format!("Next up · {n}"),
            (false, 0) => "Nothing lined up".to_string(),
            (false, 1) => "1 song".to_string(),
            (false, n) => format!("{n} songs"),
        };
        w::section_label(&p, pos2(hr.left() + S1, hr.center().y), &label);
        let ar = Rect::from_center_size(pos2(hr.right() - 14.0, hr.center().y), Vec2::splat(28.0));
        let aresp = ui.interact(ar, Id::new((salt, "q-add")), Sense::click());
        let aresp = w::icon_button_at(ui, ar, aresp, icons::PLUS, false, "Add songs");
        if aresp.clicked() {
            self.add_query.clear();
            self.add_focus = true;
        }
        let queued: HashSet<u64> = self.ps.queue.iter().skip(base).map(|q| q.id).collect();
        self.add_songs_popup(&aresp, AddTo::Queue, &queued);
        if n > 0 {
            let tw = w::text_width(ui, "Clear", small_strong()) + S2;
            let cb = Rect::from_min_size(pos2(ar.left() - S1 - tw, hr.top() + 2.0), vec2(tw, 24.0));
            let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(cb));
            if w::link_button(&mut cu, "Clear").on_hover_text("Clear the songs lined up").clicked() {
                self.actions.push(Action::ClearQueue);
            }
        }
        y = hr.bottom();
        let body = Rect::from_min_max(pos2(list_r.left(), y), list_r.max);

        // After the queue, the library carries on (unless the queue repeats).
        let auto = if self.ps.repeat == Repeat::Off { self.autoplay_preview(5) } else { Vec::new() };
        // No new heading when the queue already ends with songs from the library.
        let auto_head = !auto.is_empty() && origins.last() != Some(&Origin::Library);
        let auto_h = if auto.is_empty() { 0.0 } else { (if auto_head { HEAD_H } else { 0.0 }) + auto.len() as f32 * ROW };
        let end_h = if n == 0 && auto.is_empty() { 44.0 } else { 0.0 };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
        egui::ScrollArea::vertical()
            .id_salt((salt, "q-scroll"))
            .auto_shrink([false, false])
            .show_viewport(&mut child, |ui, vp| {
                let top = ui.max_rect().top();
                let left = ui.max_rect().left();
                let wdt = ui.max_rect().width();
                let changed = self.queue_changed_at(ui, salt);
                // Carrying a song over the queue: the rows part where it would go.
                let rows_x = egui::Rangef::new(left, left + wdt);
                let ds = self.drag_layout_with_heads(ui, DragList::Queue, key, list_r, rows_x, top, ROW, n, true, heads.clone(), HEAD_H);
                let total = ds.row_y(n) + ds.extra() as f32 * ROW + auto_h + end_h + S2;
                ui.set_min_height(total);
                // Only the rows on screen are built, so a queue of thousands stays cheap.
                let mut k = ((vp.min.y / ROW).floor().max(0.0) as usize).min(n);
                while k > 0 && ds.row_y(k - 1) + ROW > vp.min.y {
                    k -= 1;
                }
                k = k.saturating_sub(1);
                while k < n && ds.row_y(k) < vp.max.y + ROW {
                    // Rows glide to their new spot when songs are added, removed or played.
                    let uid = self.ps.queue[base + k].uid;
                    let (y, appear) = self.glide_y(ui, Id::new((salt, "q-glide", uid)), ds.row_y(k), changed);
                    let r = Rect::from_min_size(pos2(left, top + y), vec2(wdt, ROW));
                    let (r, draw) = self.row_place(ui, &ds, k, r);
                    if let Ok(h) = heads.binary_search(&k) {
                        if ds.head_shown(k) {
                            let hr = Rect::from_min_max(pos2(left, r.top() - HEAD_H), pos2(left + wdt, r.top()));
                            self.group_heading(ui, hr, head_kind[h]);
                        }
                    }
                    if draw {
                        let q = self.ps.queue[base + k].clone();
                        if appear < 1.0 {
                            let mut fu = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("q-new", q.uid)));
                            fu.multiply_opacity(appear);
                            self.queue_row(&fu, r, &q, base + k, false, salt, ds.away(k));
                        } else {
                            self.queue_row(ui, r, &q, base + k, false, salt, ds.away(k));
                        }
                    }
                    k += 1;
                }
                // What comes after the queue moves with it while songs are dragged in or out.
                let er = Rect::from_min_size(pos2(left, top + ds.row_y(n)), vec2(wdt, 1.0));
                let mut y = self.row_place(ui, &ds, n, er).0.top();
                if n == 0 && auto.is_empty() {
                    w::text(ui.painter(), pos2(left + S1, y + 18.0), Align2::LEFT_CENTER, "That's the last song.", small(), TEXT_3, wdt);
                }
                if !auto.is_empty() {
                    if auto_head {
                        let hr = Rect::from_min_size(pos2(left, y), vec2(wdt, HEAD_H));
                        self.group_heading(ui, hr, Origin::Library);
                        y += HEAD_H;
                    }
                    for id in &auto {
                        let (gy, _) = self.glide_y(ui, Id::new((salt, "auto-glide", *id)), y - top, f64::NEG_INFINITY);
                        let r = Rect::from_min_size(pos2(left, top + gy), vec2(wdt, ROW));
                        if r.bottom() >= vp.min.y + top && r.top() <= vp.max.y + top {
                            self.autoplay_row(ui, r, *id, salt);
                        }
                        y += ROW;
                    }
                }
                self.drag_scroll(ui, ui.clip_rect());
                self.drag_drop(ui, &ds);
            });
    }

    /// A queue row's spot (from the top of the list), gliding from where it was last time.
    /// Big jumps (a new queue) and the moment after a drop (the rows are already in place) don't
    /// glide.
    /// Also returns how far a song that just joined the queue has faded in.
    fn glide_y(&self, ui: &Ui, id: Id, target: f32, changed: f64) -> (f32, f32) {
        let ctx = ui.ctx();
        let now = ctx.input(|i| i.time);
        let last: Option<f32> = ctx.data(|d| d.get_temp(id.with("last")));
        ctx.data_mut(|d| d.insert_temp(id.with("last"), target));
        let jump = last.map(|l| (l - target).abs() > ROW * 6.0).unwrap_or(true);
        let hold = now - self.flip_hold < 0.4;
        let y = ctx.animate_value_with_time(id, target, if jump || hold { 0.0 } else { 0.24 });
        // New in the queue (not just scrolled into view): fade in.
        let born_key = id.with("born");
        if last.is_none() && now - changed < 0.3 {
            ctx.data_mut(|d| d.insert_temp(born_key, now));
        }
        let alpha = match ctx.data(|d| d.get_temp::<f64>(born_key)) {
            Some(b) => {
                let t = ((now - b) / 0.3).clamp(0.0, 1.0) as f32;
                if t >= 1.0 {
                    ctx.data_mut(|d| d.remove::<f64>(born_key));
                } else {
                    ctx.request_repaint();
                }
                t
            }
            None => 1.0,
        };
        (y, alpha)
    }

    /// When the queue last changed (songs added, removed, or the next one started).
    fn queue_changed_at(&self, ui: &Ui, salt: &str) -> f64 {
        let ctx = ui.ctx();
        let now = ctx.input(|i| i.time);
        let sig = (self.ps.queue.len(), self.ps.index, self.ps.queue.get(self.ps.index + 1).map(|q| q.uid));
        let key = Id::new((salt, "q-sig"));
        let (last, at): ((usize, usize, Option<u64>), f64) = ctx.data(|d| d.get_temp(key)).unwrap_or((sig, f64::NEG_INFINITY));
        if last != sig {
            ctx.data_mut(|d| d.insert_temp(key, (sig, now)));
            return now;
        }
        ctx.data_mut(|d| d.insert_temp(key, (sig, at)));
        at
    }

    /// The heading over a group of queued songs.
    fn group_heading(&self, ui: &Ui, r: Rect, kind: Origin) {
        let (icon, text, color) = match kind {
            Origin::Added => (icons::USER, "Added by you".to_string(), ACCENT),
            Origin::List => (
                icons::LIST_MUSIC,
                match &self.play_from {
                    Some(f) => format!("From {f}"),
                    None => "From your list".to_string(),
                },
                TEXT_3,
            ),
            Origin::Library => (icons::MUSIC, "From your library".to_string(), TEXT_3),
        };
        let p = ui.painter();
        let y = r.bottom() - 12.0;
        icons::glyph(p, pos2(r.left() + S1 + 7.0, y), icon, 13.0, color);
        w::text(p, pos2(r.left() + S1 + 20.0, y), Align2::LEFT_CENTER, &text, small_strong(), TEXT_2, r.width() - 32.0);
    }

    /// Row hover that still counts when the pointer is on a button inside the row.
    fn row_hover(ui: &Ui, rect: Rect, id: Id) -> f32 {
        let over = ui.rect_contains_pointer(rect) && !egui::DragAndDrop::has_any_payload(ui.ctx());
        ui.ctx().animate_bool_with_time(id.with("h"), over, T_FAST)
    }

    /// Cover that plays on click (a play glyph shows on hover).
    fn row_art(&mut self, ui: &Ui, art_r: Rect, art: Option<u64>, id: Id, h: f32, dim: f32, current: bool) -> Response {
        let p = ui.painter();
        let tex = self.art.cover(art, ArtSize::S);
        w::art(p, art_r, tex, R_SM, with_alpha(Color32::WHITE, dim));
        let resp = ui.interact(art_r, id.with("art"), Sense::click());
        let ah = ui.ctx().animate_bool_with_time(id.with("ah"), resp.hovered(), T_FAST);
        if current {
            p.rect_filled(art_r, R_SM, with_alpha(Color32::BLACK, 0.5));
            if self.ps.waiting && ah == 0.0 {
                w::spinner(ui, art_r.center(), 8.0, TEXT);
            } else if ah > 0.0 || !self.ps.playing {
                if self.ps.playing {
                    icons::pause(p, art_r.center(), 13.0, TEXT);
                } else {
                    icons::play(p, art_r.center(), 13.0, TEXT);
                }
            } else {
                icons::now_playing_bars(p, art_r.center(), 14.0, ACCENT);
            }
        } else if h > 0.0 {
            p.rect_filled(art_r, R_SM, with_alpha(Color32::BLACK, 0.45 * h));
            icons::play(p, art_r.center(), 12.0 + 2.0 * ah, with_alpha(TEXT, h));
        }
        let tip = if current { if self.ps.playing { "Pause" } else { "Play" } } else { "Play now" };
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip)
    }

    fn queue_row(&mut self, ui: &Ui, rect: Rect, q: &QItem, index: usize, current: bool, salt: &str, away: bool) {
        let id = Id::new((salt, "q-row", q.uid));
        let resp = ui.interact(rect, id, if current { Sense::click() } else { Sense::click_and_drag() });
        // Carried out of the queue: it stays faded in its place until it's dropped.
        let held = away;
        let h = Self::row_hover(ui, rect, id);
        let p = ui.painter().clone();
        if held {
            p.rect_filled(rect, R_MD, with_alpha(Color32::WHITE, 0.04));
        } else {
            glass_hover(&p, rect, h);
        }
        let dim = if held { 0.35 } else { 1.0 };
        let info = self.info_for(q.id);
        let art_r = Rect::from_min_size(pos2(rect.left() + S2, rect.center().y - 20.0), vec2(40.0, 40.0));
        if self.row_art(ui, art_r, info.as_ref().and_then(|t| t.art), id, h, dim, current).clicked() {
            self.actions.push(if current { Action::Toggle } else { Action::JumpTo(index) });
        }
        let tx = art_r.right() + S3;
        let tw = rect.right() - tx - 48.0;
        let (title, artist, dur) = match &info {
            Some(t) => (t.title.clone(), t.artist.clone(), t.duration),
            None => (q.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), String::new(), 0.0),
        };
        let tc = if current { ACCENT } else { TEXT };
        w::text(&p, pos2(tx, rect.center().y - 9.0), Align2::LEFT_CENTER, &title, body(), with_alpha(tc, dim), tw);
        w::text(&p, pos2(tx, rect.center().y + 9.0), Align2::LEFT_CENTER, &artist, small(), with_alpha(TEXT_2, dim), tw);
        // Right side: the length, or a remove button while hovered.
        if !current && h > 0.05 {
            let xr = Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), Vec2::splat(28.0));
            let xresp = ui.interact(xr, id.with("x"), Sense::click());
            if w::icon_button_at(ui, xr, xresp, icons::X, false, "Remove from queue").clicked() {
                self.actions.push(Action::RemoveFromQueue(q.uid));
            }
        } else if dur > 0.0 {
            w::text(&p, pos2(rect.right() - S3, rect.center().y), Align2::RIGHT_CENTER, &fmt_time(dur), small(), with_alpha(TEXT_3, dim), 60.0);
        }
        if !current && resp.drag_started() {
            egui::DragAndDrop::set_payload(ui.ctx(), DragSongs { ids: vec![q.id], from_queue: Some(index) });
            let k = index.saturating_sub(self.ps.index + 1);
            self.lift_song(ui, q.id, Some((DragList::Queue, k)), rect);
        }
        if !current && resp.hovered() && !held {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if resp.double_clicked() {
            self.actions.push(if current { Action::Toggle } else { Action::JumpTo(index) });
        }
        let song = q.id;
        // Library songs and songs that play from their site both have a menu.
        let in_lib = self.lib.get(song).is_some() || self.is_stream(song);
        let uid = q.uid;
        resp.context_menu(|ui| {
            if !current {
                if menu_item(ui, icons::X, "Remove from queue").clicked() {
                    self.actions.push(Action::RemoveFromQueue(uid));
                    ui.close();
                }
                if in_lib {
                    menu_sep(ui);
                }
            }
            if in_lib {
                self.song_menu(ui, song, None);
            }
        });
    }

    /// A library song that plays once the queue runs out. Drag it into the queue, or add it.
    fn autoplay_row(&mut self, ui: &Ui, rect: Rect, song: u64, salt: &str) {
        let Some(info) = self.info_for(song) else { return };
        let id = Id::new((salt, "auto", song));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let h = Self::row_hover(ui, rect, id);
        let p = ui.painter().clone();
        glass_hover(&p, rect, h);
        let art_r = Rect::from_min_size(pos2(rect.left() + S2, rect.center().y - 20.0), vec2(40.0, 40.0));
        if self.row_art(ui, art_r, info.art, id, h, 0.7 + 0.3 * h, false).clicked() {
            self.actions.push(Action::PlayFromAutoplay(song));
        }
        let tx = art_r.right() + S3;
        let tw = rect.right() - tx - 48.0;
        w::text(&p, pos2(tx, rect.center().y - 9.0), Align2::LEFT_CENTER, &info.title, body(), lerp_color(TEXT_2, TEXT, h), tw);
        w::text(&p, pos2(tx, rect.center().y + 9.0), Align2::LEFT_CENTER, &info.artist, small(), TEXT_3, tw);
        if h > 0.05 {
            let ar = Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), Vec2::splat(28.0));
            let aresp = ui.interact(ar, id.with("add"), Sense::click());
            if w::icon_button_at(ui, ar, aresp, icons::PLUS, false, "Add to queue").clicked() {
                self.actions.push(Action::AddToQueue(vec![song]));
            }
        } else if info.duration > 0.0 {
            w::text(&p, pos2(rect.right() - S3, rect.center().y), Align2::RIGHT_CENTER, &fmt_time(info.duration), small(), TEXT_3, 60.0);
        }
        if resp.drag_started() {
            egui::DragAndDrop::set_payload(ui.ctx(), DragSongs { ids: vec![song], from_queue: None });
            self.lift_song(ui, song, None, rect);
        }
        if resp.double_clicked() {
            self.actions.push(Action::PlayFromAutoplay(song));
        }
        if self.lib.get(song).is_some() {
            resp.context_menu(|ui| self.song_menu(ui, song, None));
        }
    }

    /// "+" next to the queue or a playlist: find songs in the library and add them to the end.
    pub(crate) fn add_songs_popup(&mut self, button: &Response, target: AddTo, have: &HashSet<u64>) {
        let add = move |ids: Vec<u64>| match target {
            AddTo::Queue => Action::AddToQueue(ids),
            AddTo::Playlist(pid) => Action::AddToPlaylist { pid, ids },
        };
        let width = 340.0;
        egui::Popup::menu(button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(4.0)
            .width(width)
            .show(|ui| {
                ui.set_width(width);
                let out = w::search_box(ui, &mut self.add_query, "Find songs in your library", Id::new("q-add-search"), width);
                if self.add_focus {
                    out.response.request_focus();
                    self.add_focus = false;
                }
                let q = self.add_query.trim().to_string();
                let ids: Vec<u64> = if q.is_empty() {
                    // Nothing typed yet: the newest songs.
                    let mut v: Vec<(u64, u64)> = self.lib.tracks.iter().map(|t| (t.added, t.id)).collect();
                    let k = v.len().min(7);
                    if k > 0 {
                        v.select_nth_unstable_by(k - 1, |a, b| b.0.cmp(&a.0));
                        v.truncate(k);
                        v.sort_by(|a, b| b.0.cmp(&a.0));
                    }
                    v.into_iter().map(|(_, id)| id).collect()
                } else {
                    self.lib.search(&q, 7)
                };
                if out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(first) = ids.first() {
                        self.actions.push(add(vec![*first]));
                    }
                    out.response.request_focus();
                }
                ui.add_space(S2);
                let caption = if q.is_empty() { "Recently added" } else if ids.is_empty() { "No songs match" } else { "Songs" };
                let (cr, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
                w::section_label(ui.painter(), pos2(cr.left() + S1, cr.center().y), caption);
                for id in ids {
                    let Some(info) = self.info_for(id) else { continue };
                    let (r, resp) = ui.allocate_exact_size(vec2(width, 48.0), Sense::click());
                    let (h, _) = w::hover_press(ui, &resp);
                    let p = ui.painter().clone();
                    if h > 0.0 {
                        p.rect_filled(r, R_SM + 2.0, with_alpha(Color32::WHITE, 0.08 * h));
                    }
                    let ar = Rect::from_min_size(pos2(r.left() + 6.0, r.center().y - 18.0), vec2(36.0, 36.0));
                    let tex = self.art.cover(info.art, ArtSize::S);
                    w::art(&p, ar, tex, R_SM, Color32::WHITE);
                    let tx = ar.right() + S3;
                    let tw = r.right() - tx - 40.0;
                    w::text(&p, pos2(tx, r.center().y - 8.0), Align2::LEFT_CENTER, &info.title, body(), TEXT, tw);
                    w::text(&p, pos2(tx, r.center().y + 9.0), Align2::LEFT_CENTER, &info.artist, small(), TEXT_2, tw);
                    let c = pos2(r.right() - 20.0, r.center().y);
                    let there = have.contains(&id);
                    if there {
                        icons::glyph(&p, c, icons::CHECK, 16.0, ACCENT);
                    } else {
                        icons::glyph(&p, c, icons::PLUS, 16.0, lerp_color(TEXT_2, TEXT, h));
                    }
                    let tip = match (target, there) {
                        (AddTo::Queue, true) => "In the queue · click to add again",
                        (AddTo::Queue, false) => "Add to the end of the queue",
                        (AddTo::Playlist(_), true) => "Already in this playlist",
                        (AddTo::Playlist(_), false) => "Add to this playlist",
                    };
                    if resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip).clicked() {
                        if !(there && matches!(target, AddTo::Playlist(_))) {
                            self.actions.push(add(vec![id]));
                        }
                        // Keep typing right away for the next song.
                        ui.memory_mut(|m| m.request_focus(Id::new("q-add-search")));
                    }
                }
            });
    }
}
