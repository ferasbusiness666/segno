//! Pages: songs, albums, artists, playlists, search, downloads, settings, now playing, welcome,
//! and the mini player.

use crate::store::LockExt;
use crate::app::{Action, App, DragSongs, Route};
use crate::art::{ArtKey, ArtSize};
use crate::icons;
use crate::library;
use crate::store::{self, SortBy};
use crate::theme::{self, *};
use crate::widgets::{self as w, PillKind};
use crate::ytdlp::{JobStatus, Search, Setup, YtItem};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Response, Sense, Stroke, Ui, Vec2};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct RowMode {
    pub art: bool,
    pub album_col: bool,
    /// Show track numbers instead of the list position (albums).
    pub track_numbers: bool,
    pub playlist: Option<u64>,
    /// Rows can be dragged up and down to set your own order (Songs list).
    pub reorder: bool,
}

const MENU_W: f32 = 236.0;

pub fn menu_item(ui: &mut Ui, icon: &str, label: &str) -> Response {
    let wdt = MENU_W;
    let (rect, resp) = ui.allocate_exact_size(vec2(wdt, 32.0), Sense::click());
    let (h, _) = w::hover_press(ui, &resp);
    let p = ui.painter();
    if h > 0.0 {
        p.rect_filled(rect, R_SM + 2.0, with_alpha(Color32::WHITE, 0.08 * h));
    }
    icons::glyph(p, pos2(rect.left() + 18.0, rect.center().y), icon, 16.0, TEXT_2);
    w::text(p, pos2(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, label, body(), TEXT, wdt - 48.0);
    resp
}

fn menu_item_danger(ui: &mut Ui, icon: &str, label: &str) -> Response {
    let wdt = MENU_W;
    let (rect, resp) = ui.allocate_exact_size(vec2(wdt, 32.0), Sense::click());
    let (h, _) = w::hover_press(ui, &resp);
    let p = ui.painter();
    if h > 0.0 {
        p.rect_filled(rect, R_SM + 2.0, with_alpha(ACCENT, 0.14 * h));
    }
    icons::glyph(p, pos2(rect.left() + 18.0, rect.center().y), icon, 16.0, ACCENT_HI);
    w::text(p, pos2(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, label, body(), ACCENT_HI, wdt - 48.0);
    resp
}

fn menu_sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(MENU_W, 9.0), Sense::hover());
    ui.painter().line_segment([pos2(r.left() + 6.0, r.center().y), pos2(r.right() - 6.0, r.center().y)], Stroke::new(1.0, HAIRLINE));
}

impl App {
    // -----------------------------------------------------------------------------------------
    // Song context menu

    pub fn song_menu(&mut self, ui: &mut Ui, id: u64, playlist: Option<(u64, usize)>) {
        ui.set_max_width(MENU_W);
        if menu_item(ui, icons::LIST_START, "Play next").clicked() {
            self.actions.push(Action::PlayNext(vec![id]));
            ui.close();
        }
        if menu_item(ui, icons::LIST_END, "Add to queue").clicked() {
            self.actions.push(Action::AddToQueue(vec![id]));
            ui.close();
        }
        menu_sep(ui);
        let loved = self.loved.contains(&id);
        if menu_item(ui, icons::HEART, if loved { "Remove from Loved songs" } else { "Love" }).clicked() {
            self.actions.push(Action::ToggleLove(id));
            ui.close();
        }
        let sub_label = (
            egui::RichText::new(icons::LIST_PLUS).font(FontId::new(16.0, icon_family())).color(TEXT_2),
            egui::RichText::new("Add to playlist").font(body()).color(TEXT),
        );
        ui.scope(|ui| {
            ui.spacing_mut().button_padding = vec2(11.0, 7.0);
            ui.spacing_mut().icon_spacing = 11.0;
            ui.set_min_width(MENU_W);
            ui.menu_button(sub_label, |ui| {
            ui.set_max_width(MENU_W);
            if menu_item(ui, icons::PLUS, "New playlist").clicked() {
                self.actions.push(Action::NewPlaylist { with: vec![id] });
                ui.close();
            }
            if !self.playlists.is_empty() {
                menu_sep(ui);
            }
            let pls: Vec<(u64, String)> = self.playlists.iter().map(|p| (p.id, p.name.clone())).collect();
            for (pid, name) in pls {
                if menu_item(ui, icons::LIST_MUSIC, &name).clicked() {
                    self.actions.push(Action::AddToPlaylist { pid, ids: vec![id] });
                    ui.close();
                }
            }
            });
        });
        menu_sep(ui);
        if let Some(t) = self.lib.get(id).cloned() {
            if let Some(ak) = library::album_key(&t) {
                if menu_item(ui, icons::DISC, "Go to album").clicked() {
                    self.actions.push(Action::Go(Route::Album(ak)));
                    ui.close();
                }
            }
            if menu_item(ui, icons::USER, "Go to artist").clicked() {
                self.actions.push(Action::Go(Route::Artist(library::artist_key(t.primary_artist()))));
                ui.close();
            }
        }
        if menu_item(ui, icons::FOLDER_OPEN, "Show in folder").clicked() {
            self.actions.push(Action::Reveal(id));
            ui.close();
        }
        menu_sep(ui);
        if let Some((pid, index)) = playlist {
            if menu_item(ui, icons::X, "Remove from this playlist").clicked() {
                self.actions.push(Action::RemoveFromPlaylist { pid, index });
                ui.close();
            }
        }
        if menu_item_danger(ui, icons::TRASH, "Delete from disk").clicked() {
            self.actions.push(Action::AskDeleteFile(id));
            ui.close();
        }
    }

    // -----------------------------------------------------------------------------------------
    // The page frame: color wash, top bar with back/forward, route transition.

    fn page_wash(&self) -> Option<Color32> {
        let color = match &self.route {
            Route::Album(k) => self.lib.album(*k).map(|a| a.color),
            Route::Artist(k) => self.lib.artist(*k).map(|a| a.color),
            Route::Playlist(pid) => Some(
                self.playlist(*pid)
                    .and_then(|p| p.tracks.iter().find_map(|path| self.lib.by_path(path).and_then(|t| t.color))),
            ),
            Route::Loved => return Some(Color32::from_rgb(0x5A, 0x1E, 0x33)),
            Route::NowPlaying => Some(self.now_info().and_then(|t| t.color)),
            Route::Songs | Route::Albums | Route::Artists => return Some(Color32::from_rgb(0x24, 0x22, 0x2E)),
            _ => return None,
        };
        Some(theme::wash_from_art(color.flatten()))
    }

    pub fn page(&mut self, ui: &mut Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        ui.painter().rect_filled(rect, R_LG, PANEL);
        // Animate the wash between pages instead of snapping.
        if let Some(target) = self.page_wash() {
            let id = Id::new("wash");
            let r = ctx.animate_value_with_time(id.with("r"), target.r() as f32, T_SLOW);
            let g = ctx.animate_value_with_time(id.with("g"), target.g() as f32, T_SLOW);
            let b = ctx.animate_value_with_time(id.with("b"), target.b() as f32, T_SLOW);
            let c = Color32::from_rgb(r as u8, g as u8, b as u8);
            let h = if self.route == Route::NowPlaying { rect.height() } else { 360.0f32.min(rect.height()) };
            let wr = Rect::from_min_size(rect.min, vec2(rect.width(), h));
            let stops: &[(f32, Color32)] = if self.route == Route::NowPlaying {
                &[(0.0, c), (0.65, lerp_color(c, PANEL, 0.7)), (1.0, PANEL)]
            } else {
                &[(0.0, c), (0.55, lerp_color(c, PANEL, 0.62)), (1.0, PANEL)]
            };
            w::vgradient(ui.painter(), wr, R_LG, stops);
        }

        let now = ctx.input(|i| i.time);
        let t = ((now - self.route_t) as f32 / T).clamp(0.0, 1.0);
        let a = egui::emath::easing::cubic_out(t);
        if t < 1.0 {
            ctx.request_repaint();
        }
        let content = rect.translate(vec2(0.0, (1.0 - a) * 8.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content)
                .id_salt(("page", format!("{:?}", self.route)))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect);
        child.multiply_opacity(a);
        let scroll = match self.route.clone() {
            Route::Songs => self.songs_page(&mut child),
            Route::Albums => self.albums_page(&mut child),
            Route::Album(k) => self.album_page(&mut child, k),
            Route::Artists => self.artists_page(&mut child),
            Route::Artist(k) => self.artist_page(&mut child, k),
            Route::Playlist(pid) => self.playlist_page(&mut child, Some(pid)),
            Route::Loved => self.playlist_page(&mut child, None),
            Route::Search => self.search_page(&mut child),
            Route::Downloads => self.downloads_page(&mut child),
            Route::Settings => self.settings_page(&mut child),
            Route::NowPlaying => self.now_playing_page(&mut child),
        };
        self.top_bar(ui, rect, scroll);
    }

    fn top_bar(&mut self, ui: &mut Ui, rect: Rect, scroll: (f32, String)) {
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), TOPBAR_H));
        let (offset, title) = scroll;
        let solid = ((offset - 90.0) / 60.0).clamp(0.0, 1.0);
        if solid > 0.0 {
            let c = self.page_wash().map(|c| lerp_color(c, PANEL, 0.35)).unwrap_or(PANEL);
            ui.painter().rect_filled(
                bar,
                egui::CornerRadius { nw: R_LG as u8, ne: R_LG as u8, sw: 0, se: 0 },
                with_alpha(c, solid),
            );
        }
        let mut x = bar.left() + PAD_X - 4.0;
        for (icon, enabled, action, tip) in [
            (icons::CHEVRON_LEFT, self.can_back(), Action::Back, "Back"),
            (icons::CHEVRON_RIGHT, self.can_fwd(), Action::Forward, "Forward"),
        ] {
            let r = Rect::from_center_size(pos2(x + 16.0, bar.center().y), Vec2::splat(32.0));
            let resp = ui.interact(r, Id::new(("nav", tip)), if enabled { Sense::click() } else { Sense::hover() });
            let (h, _) = w::hover_press(ui, &resp);
            ui.painter().circle_filled(r.center(), 16.0, with_alpha(Color32::BLACK, 0.35 + 0.15 * h));
            icons::glyph(ui.painter(), r.center(), icon, 18.0, if enabled { lerp_color(TEXT_2, TEXT, h) } else { TEXT_3 });
            if enabled && resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                self.actions.push(action);
            }
            x += 40.0;
        }
        if solid > 0.0 && !title.is_empty() {
            w::text(
                ui.painter(),
                pos2(x + S2, bar.center().y),
                Align2::LEFT_CENTER,
                &title,
                FontId::new(18.0, bold()),
                with_alpha(TEXT, solid),
                bar.right() - x - PAD_X,
            );
        }
    }

    /// A long list with a header, drawn lazily: only the rows on screen are built each frame.
    fn list_page(
        &mut self,
        ui: &mut Ui,
        salt: impl egui::AsIdSalt,
        header_h: f32,
        rows: usize,
        row_h: f32,
        mut header: impl FnMut(&mut App, &mut Ui, Rect),
        mut row: impl FnMut(&mut App, &mut Ui, Rect, usize),
    ) -> f32 {
        let out = egui::ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let width = ui.available_width();
                let total = TOPBAR_H + header_h + rows as f32 * row_h + S12 + S6;
                ui.set_height(total);
                let origin = ui.max_rect().min;
                let hr = Rect::from_min_size(origin + vec2(0.0, TOPBAR_H), vec2(width, header_h));
                if viewport.min.y < TOPBAR_H + header_h {
                    let mut hu = ui.new_child(egui::UiBuilder::new().max_rect(hr).layout(egui::Layout::top_down(egui::Align::Min)));
                    header(self, &mut hu, hr);
                }
                let start = TOPBAR_H + header_h;
                let first = (((viewport.min.y - start) / row_h).floor().max(0.0)) as usize;
                let last = ((((viewport.max.y - start) / row_h).ceil().max(0.0)) as usize).min(rows);
                for i in first..last {
                    let r = Rect::from_min_size(origin + vec2(PAD_X - S2, start + i as f32 * row_h), vec2(width - (PAD_X - S2) * 2.0, row_h));
                    let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("row", i)));
                    row(self, &mut ru, r, i);
                }
            });
        out.state.offset.y
    }

    fn column_header(&self, ui: &Ui, rect: Rect, mode: RowMode) {
        let p = ui.painter();
        let y = rect.center().y;
        let left = rect.left() + PAD_X - S2;
        let right = rect.right() - PAD_X + S2;
        w::text(p, pos2(left + 22.0, y), Align2::CENTER_CENTER, "#", small(), TEXT_3, 30.0);
        let title_x = left + if mode.art { 44.0 + 40.0 + S3 - 40.0 } else { 44.0 };
        w::text(p, pos2(title_x + if mode.art { 0.0 } else { 0.0 }, y), Align2::LEFT_CENTER, "Title", small(), TEXT_3, 200.0);
        if mode.album_col && rect.width() > 760.0 {
            let ax = left + (right - left) * 0.55;
            w::text(p, pos2(ax, y), Align2::LEFT_CENTER, "Album", small(), TEXT_3, 200.0);
        }
        icons::glyph(p, pos2(right - 34.0, y), icons::CLOCK, 14.0, TEXT_3);
        p.line_segment([pos2(left, rect.bottom() - 0.5), pos2(right, rect.bottom() - 0.5)], Stroke::new(1.0, HAIRLINE));
    }

    /// One song row. `list` is what plays (in order) when the row is played.
    fn track_row(&mut self, ui: &mut Ui, rect: Rect, list: &Arc<Vec<u64>>, i: usize, mode: RowMode, pl_index: Option<usize>) {
        let Some(&id) = list.get(i) else { return };
        let Some(t) = self.lib.get(id).cloned() else { return };
        let resp = ui.interact(rect, Id::new(("track", id, i, mode.playlist)), Sense::click_and_drag());
        let (h, _) = w::hover_press(ui, &resp);
        let is_cur = self.current_id() == Some(id);
        let selected = self.selected == Some(id);
        let p = ui.painter().clone();
        let bg = if selected { with_alpha(Color32::WHITE, 0.10) } else { with_alpha(Color32::WHITE, 0.06 * h) };
        if selected || h > 0.0 {
            p.rect_filled(rect, R_MD - 2.0, bg);
        }
        let y = rect.center().y;
        let mut x = rect.left() + S2;
        let hovering = resp.hovered() || h > 0.5 || self.song_menu_open_for(ui, id);

        // Number / play column
        let num_c = pos2(x + 14.0, y);
        let mut play_hit = false;
        let num_r = Rect::from_center_size(num_c, vec2(28.0, 28.0));
        if hovering {
            let pr = ui.interact(num_r, resp.id.with("play"), Sense::click());
            let (ph, _) = w::hover_press(ui, &pr);
            if is_cur && self.ps.playing {
                icons::pause(&p, num_c, 12.0, lerp_color(TEXT, ACCENT, ph));
            } else {
                icons::play(&p, num_c, 12.0, lerp_color(TEXT, ACCENT, ph));
            }
            if pr.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                play_hit = true;
            }
        } else if is_cur && self.ps.playing {
            icons::now_playing_bars(&p, num_c, 13.0, ACCENT);
        } else {
            let n = if mode.track_numbers && t.track_no > 0 { t.track_no as usize } else { i + 1 };
            w::text(&p, num_c, Align2::CENTER_CENTER, &n.to_string(), body(), if is_cur { ACCENT } else { TEXT_2 }, 40.0);
        }
        x += 36.0;

        // Art
        if mode.art {
            let ar = Rect::from_min_size(pos2(x, y - 20.0), vec2(40.0, 40.0));
            let tex = self.art.cover(t.art, ArtSize::S);
            w::art(&p, ar, tex, R_SM, Color32::WHITE);
            x = ar.right() + S3;
        } else {
            x += S1;
        }

        let right = rect.right() - S2;
        let dur_x = right - 4.0;
        let heart_c = pos2(right - 74.0, y);
        let more_c = pos2(right - 108.0, y);
        let text_end = more_c.x - 20.0;
        let album_on = mode.album_col && rect.width() > 760.0;
        let album_x = rect.left() + (rect.width()) * 0.55;
        let title_w = (if album_on { album_x - S4 } else { text_end }) - x;
        let tc = if is_cur { ACCENT } else { TEXT };
        w::text(&p, pos2(x, y - 9.0), Align2::LEFT_CENTER, &t.title, body(), tc, title_w);
        let artist = if t.artist.is_empty() { "Unknown artist" } else { t.artist.as_str() };
        w::text(&p, pos2(x, y + 10.0), Align2::LEFT_CENTER, artist, small(), TEXT_2, title_w);
        if album_on {
            w::text(&p, pos2(album_x, y), Align2::LEFT_CENTER, &t.album, small(), TEXT_2, text_end - album_x - S2);
        }
        w::text(&p, pos2(dur_x, y), Align2::RIGHT_CENTER, &fmt_time(t.duration), small(), TEXT_2, 60.0);

        // Love (always visible when loved, otherwise on hover) and more.
        let loved = self.loved.contains(&id);
        if loved || hovering {
            let lr = Rect::from_center_size(heart_c, Vec2::splat(30.0));
            let lresp = ui.interact(lr, resp.id.with("love"), Sense::click());
            if self.love_button(ui, lr, lresp, id).clicked() {
                self.actions.push(Action::ToggleLove(id));
            }
        }
        let mut menu_open_at: Option<egui::Pos2> = None;
        if hovering {
            let mr = Rect::from_center_size(more_c, Vec2::splat(30.0));
            let mresp = ui.interact(mr, resp.id.with("more"), Sense::click());
            let mresp = w::icon_button_at(ui, mr, mresp, icons::ELLIPSIS, false, "More");
            if mresp.clicked() {
                menu_open_at = Some(mr.left_bottom());
            }
        }

        // Interactions
        if play_hit {
            if is_cur {
                self.actions.push(Action::Toggle);
            } else {
                self.actions.push(Action::PlayIds { ids: list.to_vec(), start: i, shuffle: None });
            }
        } else if resp.double_clicked() {
            self.actions.push(Action::PlayIds { ids: list.to_vec(), start: i, shuffle: None });
        } else if resp.clicked() {
            self.selected = Some(id);
        }
        if selected && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !ui.ctx().egui_wants_keyboard_input() {
            self.actions.push(Action::PlayIds { ids: list.to_vec(), start: i, shuffle: None });
        }

        // Drag: onto a sidebar playlist, or within this playlist to reorder.
        if resp.drag_started() {
            egui::DragAndDrop::set_payload(
                ui.ctx(),
                DragSongs {
                    ids: vec![id],
                    from_playlist: mode.playlist.zip(pl_index),
                    from_queue: None,
                    from_songs: mode.reorder.then_some(i),
                },
            );
        }
        if let (Some(pid), Some(idx)) = (mode.playlist, pl_index) {
            if let Some(d) = resp.dnd_hover_payload::<DragSongs>() {
                if d.from_playlist.map(|(p, _)| p) == Some(pid) {
                    let above = ui.input(|i| i.pointer.hover_pos().map(|p| p.y < rect.center().y).unwrap_or(true));
                    let ly = if above { rect.top() } else { rect.bottom() };
                    ui.painter().line_segment([pos2(rect.left(), ly), pos2(rect.right(), ly)], Stroke::new(2.0, ACCENT));
                }
            }
            if let Some(d) = resp.dnd_release_payload::<DragSongs>() {
                if let Some((p, from)) = d.from_playlist {
                    if p == pid {
                        let above = ui.input(|i| i.pointer.hover_pos().map(|p| p.y < rect.center().y).unwrap_or(true));
                        let mut to = if above { idx } else { idx + 1 };
                        if from < to {
                            to -= 1;
                        }
                        self.actions.push(Action::MoveInPlaylist { pid, from, to });
                    }
                }
            }
        }

        if mode.reorder {
            if let Some(d) = resp.dnd_hover_payload::<DragSongs>() {
                if d.from_songs.is_some() {
                    let above = ui.input(|i| i.pointer.hover_pos().map(|p| p.y < rect.center().y).unwrap_or(true));
                    let ly = if above { rect.top() } else { rect.bottom() };
                    ui.painter().line_segment([pos2(rect.left(), ly), pos2(rect.right(), ly)], Stroke::new(2.0, ACCENT));
                }
            }
            if let Some(d) = resp.dnd_release_payload::<DragSongs>() {
                if let Some(from) = d.from_songs {
                    let above = ui.input(|i| i.pointer.hover_pos().map(|p| p.y < rect.center().y).unwrap_or(true));
                    let mut to = if above { i } else { i + 1 };
                    if from < to {
                        to -= 1;
                    }
                    self.actions.push(Action::MoveInSongs { from, to });
                }
            }
        }
        // A grip on hover in lists you can reorder, so it's clear rows can be dragged.
        if (mode.reorder || mode.playlist.is_some()) && hovering {
            // Inside the row's left padding, clear of the play button.
            icons::glyph(&p, pos2(rect.left() + 7.0, rect.center().y), icons::GRIP, 12.0, TEXT_3);
        }

        let pl = mode.playlist.zip(pl_index);
        resp.context_menu(|ui| self.song_menu(ui, id, pl));
        if let Some(pos) = menu_open_at {
            let now = ui.input(|i| i.time);
            ui.ctx().memory_mut(|m| m.data.insert_temp(Id::new("open-song-menu"), (id, pos, pl, now)));
        }
        self.floating_song_menu(ui, id);
    }

    /// The "…" button opens the same menu as right-click, anchored to the button.
    fn floating_song_menu(&mut self, ui: &mut Ui, id: u64) {
        type Open = (u64, egui::Pos2, Option<(u64, usize)>, f64);
        let key = Id::new("open-song-menu");
        let Some((mid, pos, pl, opened)) = ui.ctx().memory(|m| m.data.get_temp::<Open>(key)) else {
            return;
        };
        if mid != id {
            return;
        }
        let before = self.actions.len();
        let area = egui::Area::new(Id::new(("song-menu-area", id)))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    self.song_menu(ui, id, pl);
                })
            });
        let now = ui.input(|i| i.time);
        // Ignore the click that opened it.
        let settled = now - opened > 0.15;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let in_menu = pointer
            .map(|p| area.response.rect.contains(p) || ui.ctx().layer_id_at(p).map(|l| l.order == egui::Order::Foreground).unwrap_or(false))
            .unwrap_or(false);
        let clicked_elsewhere = settled && ui.input(|i| i.pointer.any_pressed()) && !in_menu;
        let chose = self.actions.len() > before;
        if clicked_elsewhere || chose || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            ui.ctx().memory_mut(|m| m.data.remove::<Open>(key));
        }
    }

    fn song_menu_open_for(&self, ui: &Ui, id: u64) -> bool {
        ui.ctx()
            .memory(|m| m.data.get_temp::<(u64, egui::Pos2, Option<(u64, usize)>, f64)>(Id::new("open-song-menu")))
            .map(|o| o.0 == id)
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------------------------------
    // Songs

    fn songs_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let ids = self.songs();
        if ids.is_empty() {
            self.library_empty(ui);
            return (0.0, String::new());
        }
        let mode = RowMode { art: true, album_col: true, track_numbers: false, playlist: None, reorder: true };
        let total = self.lib.total_duration(&ids);
        let n = ids.len();
        let scanning = self.lib.scanning;
        let list = ids.clone();
        let off = self.list_page(
            ui,
            "songs",
            150.0,
            n,
            ROW_H,
            |app, ui, r| {
                let p = ui.painter().clone();
                w::text(&p, pos2(r.left() + PAD_X, r.top() + 22.0), Align2::LEFT_CENTER, "Songs", h1(), TEXT, 400.0);
                let meta = if scanning {
                    format!("{} · Looking for new music…", theme::songs_and_length(n, total))
                } else {
                    theme::songs_and_length(n, total)
                };
                w::text(&p, pos2(r.left() + PAD_X, r.top() + 54.0), Align2::LEFT_CENTER, &meta, small(), TEXT_2, r.width() - 2.0 * PAD_X);
                let br = Rect::from_min_size(pos2(r.left() + PAD_X, r.top() + 76.0), vec2(r.width() - 2.0 * PAD_X, 36.0));
                let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(egui::Layout::left_to_right(egui::Align::Center)));
                if w::pill(&mut bu, "Play", Some("play"), PillKind::Primary).clicked() {
                    app.actions.push(Action::PlayIds { ids: list.to_vec(), start: 0, shuffle: Some(false) });
                }
                if w::pill(&mut bu, "Shuffle", Some(icons::SHUFFLE), PillKind::Secondary).clicked() {
                    app.actions.push(Action::PlayIds { ids: list.to_vec(), start: 0, shuffle: Some(true) });
                }
                bu.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    app.sort_menu(ui);
                });
                let ch = Rect::from_min_size(pos2(r.left(), r.bottom() - 32.0), vec2(r.width(), 32.0));
                app.column_header(ui, ch, mode);
            },
            |app, ui, r, i| app.track_row(ui, r, &ids, i, mode, None),
        );
        (off, "Songs".into())
    }

    fn sort_menu(&mut self, ui: &mut Ui) {
        let label = match self.settings.sort {
            SortBy::Added => "Recently added",
            SortBy::Title => "Title",
            SortBy::Artist => "Artist",
            SortBy::Album => "Album",
            SortBy::Custom => "Your order",
        };
        let resp = w::pill(ui, label, Some(icons::ARROW_DOWN_UP), PillKind::Ghost);
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_max_width(MENU_W);
            for (s, l) in [
                (SortBy::Added, "Recently added"),
                (SortBy::Title, "Title"),
                (SortBy::Artist, "Artist"),
                (SortBy::Album, "Album"),
                (SortBy::Custom, "Your order"),
            ] {
                let icon = if self.settings.sort == s { icons::CHECK } else { "" };
                if menu_item(ui, icon, l).clicked() {
                    self.settings.sort = s;
                    self.settings.save();
                    ui.close();
                }
            }
        });
    }

    fn library_empty(&mut self, ui: &mut Ui) {
        ui.add_space(TOPBAR_H + S8);
        if self.lib.scanning || !self.lib.loaded_once {
            ui.vertical_centered(|ui| {
                ui.add_space(S12);
                let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                w::spinner(ui, r.center(), 14.0, TEXT_2);
                ui.add_space(S3);
                ui.label(egui::RichText::new("Looking for music…").font(body()).color(TEXT_2));
            });
            return;
        }
        let folder = self
            .settings
            .music_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let body_text = format!("Add songs to {folder}, or find some on YouTube and download them here.");
        if w::empty_state(ui, icons::MUSIC, "Your library is empty", &body_text, Some(("Search YouTube", Some(icons::SEARCH)))) {
            self.actions.push(Action::Go(Route::Search));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Albums & artists

    fn grid_metrics(width: f32, min_card: f32) -> (usize, f32) {
        let avail = width - 2.0 * PAD_X;
        let gap = S5;
        let cols = (((avail + gap) / (min_card + gap)).floor() as usize).max(2);
        let card = (avail - gap * (cols as f32 - 1.0)) / cols as f32;
        (cols, card)
    }

    fn albums_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let n = self.lib.albums.len();
        if n == 0 {
            ui.add_space(TOPBAR_H + S8);
            let hint = "Albums show up when songs have album info. Songs from YouTube Music usually include it.";
            if w::empty_state(ui, icons::DISC, "No albums yet", hint, Some(("Browse songs", None))) {
                self.actions.push(Action::Go(Route::Songs));
            }
            return (0.0, String::new());
        }
        let (cols, card) = Self::grid_metrics(ui.available_width(), 168.0);
        let row_h = card + 64.0;
        let rows = n.div_ceil(cols);
        let off = self.list_page(
            ui,
            "albums",
            96.0,
            rows,
            row_h,
            |_app, ui, r| {
                let p = ui.painter();
                w::text(p, pos2(r.left() + PAD_X, r.top() + 22.0), Align2::LEFT_CENTER, "Albums", h1(), TEXT, 400.0);
                w::text(p, pos2(r.left() + PAD_X, r.top() + 54.0), Align2::LEFT_CENTER, &theme::plural(n, "album", "albums"), small(), TEXT_2, 400.0);
            },
            |app, ui, r, row| {
                for c in 0..cols {
                    let i = row * cols + c;
                    if i >= n {
                        break;
                    }
                    let x = r.left() + S2 + c as f32 * (card + S5);
                    let cr = Rect::from_min_size(pos2(x, r.top()), vec2(card, row_h - S2));
                    let (key, title, artist, art) = {
                        let a = &app.lib.albums[i];
                        (a.key, a.title.clone(), a.artist.clone(), a.art)
                    };
                    app.card(ui, cr, card, Id::new(("album", key)), art, &title, &artist, false, Route::Album(key));
                }
            },
        );
        (off, "Albums".into())
    }

    fn artists_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let n = self.lib.artists.len();
        if n == 0 {
            self.library_empty(ui);
            return (0.0, String::new());
        }
        let (cols, card) = Self::grid_metrics(ui.available_width(), 160.0);
        let row_h = card + 64.0;
        let rows = n.div_ceil(cols);
        let off = self.list_page(
            ui,
            "artists",
            96.0,
            rows,
            row_h,
            |_app, ui, r| {
                let p = ui.painter();
                w::text(p, pos2(r.left() + PAD_X, r.top() + 22.0), Align2::LEFT_CENTER, "Artists", h1(), TEXT, 400.0);
                w::text(p, pos2(r.left() + PAD_X, r.top() + 54.0), Align2::LEFT_CENTER, &theme::plural(n, "artist", "artists"), small(), TEXT_2, 400.0);
            },
            |app, ui, r, row| {
                for c in 0..cols {
                    let i = row * cols + c;
                    if i >= n {
                        break;
                    }
                    let x = r.left() + S2 + c as f32 * (card + S5);
                    let cr = Rect::from_min_size(pos2(x, r.top()), vec2(card, row_h - S2));
                    let (key, name, songs, art) = {
                        let a = &app.lib.artists[i];
                        (a.key, a.name.clone(), a.tracks.len(), a.art)
                    };
                    app.card(ui, cr, card, Id::new(("artist", key)), art, &name, &theme::plural(songs, "song", "songs"), true, Route::Artist(key));
                }
            },
        );
        (off, "Artists".into())
    }

    /// Grid card: art, title, subtitle; a play button rises in on hover.
    #[allow(clippy::too_many_arguments)]
    fn card(&mut self, ui: &mut Ui, rect: Rect, art_size: f32, id: Id, art: Option<u64>, title: &str, sub: &str, round: bool, route: Route) {
        let resp = ui.interact(rect, id, Sense::click());
        let (h, pr) = w::hover_press(ui, &resp);
        let p = ui.painter().clone();
        let surface = rect.expand(S2);
        w::hover_surface(&p, surface, h);
        let ar = Rect::from_min_size(rect.min, vec2(art_size, art_size));
        let ar = w::scale_rect(ar, 1.0 - 0.02 * pr);
        let tex = self.art.cover(art, ArtSize::M);
        let radius = if round { art_size / 2.0 } else { R_MD - 2.0 };
        w::soft_shadow(&p, ar, radius, 0.35 + 0.35 * h);
        w::art(&p, ar, tex, radius, Color32::WHITE);
        w::text(&p, pos2(rect.left(), ar.bottom() + 18.0), Align2::LEFT_CENTER, title, body_strong(), TEXT, art_size);
        w::text(&p, pos2(rect.left(), ar.bottom() + 38.0), Align2::LEFT_CENTER, sub, small(), TEXT_2, art_size);

        // Floating play button: fades in and rises 8 pt.
        let mut play_clicked = false;
        if h > 0.01 {
            let c = pos2(ar.right() - 30.0, ar.bottom() - 30.0 + (1.0 - h) * 8.0);
            let br = Rect::from_center_size(c, Vec2::splat(48.0));
            let presp = ui.interact(br, id.with("play"), Sense::click());
            w::paint_play_circle(ui, br, &presp, false, false, h);
            if presp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                play_clicked = true;
            }
        }
        if play_clicked {
            let ids = match &route {
                Route::Album(k) => self.lib.album(*k).map(|a| a.tracks.clone()).unwrap_or_default(),
                Route::Artist(k) => self.lib.artist(*k).map(|a| a.tracks.clone()).unwrap_or_default(),
                _ => vec![],
            };
            self.actions.push(Action::PlayIds { ids, start: 0, shuffle: Some(false) });
        } else if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            self.actions.push(Action::Go(route));
        }
    }

    /// Big header for album / artist / playlist pages.
    #[allow(clippy::too_many_arguments)]
    fn detail_header(
        &mut self,
        ui: &mut Ui,
        r: Rect,
        kind: &str,
        title: &str,
        meta: &str,
        art: Option<u64>,
        round: bool,
        ids: &Arc<Vec<u64>>,
        playlist: Option<u64>,
        loved_tile: bool,
    ) {
        let p = ui.painter().clone();
        let size = 200.0f32.min(r.width() * 0.3);
        let ar = Rect::from_min_size(pos2(r.left() + PAD_X, r.top() + S2), vec2(size, size));
        let radius = if round { size / 2.0 } else { R_MD };
        w::soft_shadow(&p, ar, radius, 1.0);
        if loved_tile {
            w::vgradient(&p, ar, R_MD, &[(0.0, ACCENT_HI), (1.0, Color32::from_rgb(0x8A, 0x1C, 0x3A))]);
            icons::heart_filled(&p, ar.center(), size * 0.36, Color32::WHITE);
        } else if let Some(pid) = playlist.filter(|_| art.is_none()) {
            self.collage(ui, ar, pid);
        } else {
            let tex = self.art.cover(art, ArtSize::L);
            w::art(&p, ar, tex, radius, Color32::WHITE);
        }
        let tx = ar.right() + S6;
        let tw = r.right() - PAD_X - tx;
        w::text(&p, pos2(tx, ar.top() + size * 0.42), Align2::LEFT_CENTER, kind, small_strong(), TEXT, tw);

        // Title (or the rename field for playlists).
        let renaming = matches!(&self.rename, Some((pid, _, _)) if Some(*pid) == playlist);
        let title_y = ar.top() + size * 0.42 + 34.0;
        if renaming {
            let tr = Rect::from_min_size(pos2(tx - 6.0, title_y - 26.0), vec2(tw.min(560.0), 52.0));
            let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(tr));
            let (pid, mut name, first) = self.rename.clone().unwrap();
            ui.painter().rect_filled(tr, R_MD, with_alpha(Color32::BLACK, 0.3));
            let te = egui::TextEdit::singleline(&mut name)
                .id(Id::new(("rename", pid)))
                .font(FontId::new(32.0, bold()))
                .text_color(TEXT)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::symmetric(6, 6))
                .desired_width(tr.width() - 12.0)
                .show(&mut cu);
            if first {
                te.response.request_focus();
                let mut state = te.state.clone();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(name.chars().count()),
                )));
                state.store(ui.ctx(), te.response.id);
            }
            self.rename = Some((pid, name, false));
            if te.response.lost_focus() {
                self.actions.push(Action::CommitRename);
            }
        } else {
            let tr = w::text(&p, pos2(tx, title_y), Align2::LEFT_CENTER, title, display(), TEXT, tw);
            if let Some(pid) = playlist {
                let resp = ui.interact(tr, Id::new(("title", pid)), Sense::click()).on_hover_text("Click to rename");
                if resp.clicked() {
                    self.actions.push(Action::StartRename(pid));
                }
            }
        }
        w::text(&p, pos2(tx, title_y + 34.0), Align2::LEFT_CENTER, meta, small(), TEXT_2, tw);

        // Actions
        let ay = ar.bottom() + S6 + 28.0;
        let br = Rect::from_min_size(pos2(r.left() + PAD_X, ay - 28.0), vec2(r.width() - 2.0 * PAD_X, 56.0));
        let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(egui::Layout::left_to_right(egui::Align::Center)));
        bu.spacing_mut().item_spacing.x = S4;
        let playing_this = self.ps.playing && self.current_id().map(|c| ids.contains(&c)).unwrap_or(false) && {
            // Only show pause if the queue came from this list.
            self.ps.queue.len() == ids.len()
        };
        if !ids.is_empty() {
            if w::play_circle(&mut bu, 56.0, playing_this, false).clicked() {
                if playing_this {
                    self.actions.push(Action::Toggle);
                } else {
                    self.actions.push(Action::PlayIds { ids: ids.to_vec(), start: 0, shuffle: Some(false) });
                }
            }
            if w::icon_button(&mut bu, 40.0, icons::SHUFFLE, false, "Shuffle play").clicked() {
                self.actions.push(Action::PlayIds { ids: ids.to_vec(), start: 0, shuffle: Some(true) });
            }
        }
        if let Some(pid) = playlist {
            let more = w::icon_button(&mut bu, 40.0, icons::ELLIPSIS, false, "More");
            egui::Popup::menu(&more).show(|ui| {
                ui.set_max_width(MENU_W);
                if menu_item(ui, icons::PENCIL, "Rename").clicked() {
                    self.actions.push(Action::StartRename(pid));
                    ui.close();
                }
                if !ids.is_empty() && menu_item(ui, icons::LIST_END, "Add all to queue").clicked() {
                    self.actions.push(Action::AddToQueue(ids.to_vec()));
                    ui.close();
                }
                if menu_item_danger(ui, icons::TRASH, "Delete playlist").clicked() {
                    self.actions.push(Action::AskDeletePlaylist(pid));
                    ui.close();
                }
            });
        }
    }

    fn collage(&mut self, ui: &Ui, rect: Rect, pid: u64) {
        let arts: Vec<u64> = {
            let mut v = Vec::new();
            if let Some(p) = self.playlist(pid) {
                for path in &p.tracks {
                    if let Some(a) = self.lib.by_path(path).and_then(|t| t.art) {
                        if !v.contains(&a) {
                            v.push(a);
                        }
                    }
                    if v.len() == 4 {
                        break;
                    }
                }
            }
            v
        };
        let p = ui.painter();
        if arts.len() < 4 {
            let tex = self.art.cover(arts.first().copied(), ArtSize::L);
            if arts.is_empty() {
                p.rect_filled(rect, R_MD, RAISED_2);
                icons::glyph(p, rect.center(), icons::LIST_MUSIC, rect.width() * 0.3, TEXT_3);
            } else {
                w::art(p, rect, tex, R_MD, Color32::WHITE);
            }
            return;
        }
        let half = rect.width() / 2.0;
        for (i, a) in arts.iter().enumerate() {
            let r = Rect::from_min_size(rect.min + vec2((i % 2) as f32 * half, (i / 2) as f32 * half), Vec2::splat(half));
            let tex = self.art.cover(Some(*a), ArtSize::M);
            let cr = match i {
                0 => egui::CornerRadius { nw: R_MD as u8, ..Default::default() },
                1 => egui::CornerRadius { ne: R_MD as u8, ..Default::default() },
                2 => egui::CornerRadius { sw: R_MD as u8, ..Default::default() },
                _ => egui::CornerRadius { se: R_MD as u8, ..Default::default() },
            };
            match tex {
                Some(t) => {
                    p.add(egui::epaint::RectShape::filled(r, cr, Color32::WHITE).with_texture(t, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
                }
                None => {
                    p.rect_filled(r, cr, RAISED_2);
                }
            }
        }
    }

    fn album_page(&mut self, ui: &mut Ui, key: u64) -> (f32, String) {
        let Some(a) = self.lib.album(key) else {
            return self.missing(ui);
        };
        let ids = Arc::new(a.tracks.clone());
        let title = a.title.clone();
        let mut meta = a.artist.clone();
        if a.year > 0 {
            meta.push_str(&format!(" · {}", a.year));
        }
        meta.push_str(&format!(" · {}", theme::songs_and_length(ids.len(), self.lib.total_duration(&ids))));
        let art = a.art;
        let artist_key = library::artist_key(&a.artist);
        let mode = RowMode { art: false, album_col: false, track_numbers: true, playlist: None, reorder: false };
        let n = ids.len();
        let ids2 = ids.clone();
        let t2 = title.clone();
        let off = self.list_page(
            ui,
            ("album", key),
            352.0,
            n,
            ROW_H,
            |app, ui, r| {
                app.detail_header(ui, r, "Album", &t2, &meta, art, false, &ids2, None, false);
                // The artist name in the meta line opens the artist.
                let _ = artist_key;
                let ch = Rect::from_min_size(pos2(r.left(), r.bottom() - 32.0), vec2(r.width(), 32.0));
                app.column_header(ui, ch, mode);
            },
            |app, ui, r, i| app.track_row(ui, r, &ids, i, mode, None),
        );
        (off, title)
    }

    fn artist_page(&mut self, ui: &mut Ui, key: u64) -> (f32, String) {
        let Some(a) = self.lib.artist(key) else {
            return self.missing(ui);
        };
        let ids = Arc::new(a.tracks.clone());
        let name = a.name.clone();
        let mut meta = theme::songs_and_length(ids.len(), self.lib.total_duration(&ids));
        if !a.albums.is_empty() {
            meta = format!("{} · {}", theme::plural(a.albums.len(), "album", "albums"), meta);
        }
        let art = a.art;
        let mode = RowMode { art: true, album_col: true, track_numbers: false, playlist: None, reorder: false };
        let n = ids.len();
        let ids2 = ids.clone();
        let n2 = name.clone();
        let off = self.list_page(
            ui,
            ("artist", key),
            352.0,
            n,
            ROW_H,
            |app, ui, r| {
                app.detail_header(ui, r, "Artist", &n2, &meta, art, true, &ids2, None, false);
                let ch = Rect::from_min_size(pos2(r.left(), r.bottom() - 32.0), vec2(r.width(), 32.0));
                app.column_header(ui, ch, mode);
            },
            |app, ui, r, i| app.track_row(ui, r, &ids, i, mode, None),
        );
        (off, name)
    }

    fn missing(&mut self, ui: &mut Ui) -> (f32, String) {
        ui.add_space(TOPBAR_H + S8);
        if w::empty_state(ui, icons::INFO, "This isn't here anymore", "The songs may have been moved or deleted.", Some(("Go to Songs", None))) {
            self.actions.push(Action::Go(Route::Songs));
        }
        (0.0, String::new())
    }

    fn playlist_page(&mut self, ui: &mut Ui, pid: Option<u64>) -> (f32, String) {
        let (title, entries): (String, Vec<(usize, u64)>) = match pid {
            Some(id) => {
                let Some(p) = self.playlist(id) else {
                    return self.missing(ui);
                };
                (p.name.clone(), self.playlist_ids(id))
            }
            None => ("Loved songs".to_string(), self.loved_ids().into_iter().enumerate().collect()),
        };
        let ids: Arc<Vec<u64>> = Arc::new(entries.iter().map(|e| e.1).collect());
        let pl_index: Vec<usize> = entries.iter().map(|e| e.0).collect();
        let meta = theme::songs_and_length(ids.len(), self.lib.total_duration(&ids));
        let art = None;
        let mode = RowMode { art: true, album_col: true, track_numbers: false, playlist: pid, reorder: false };
        let n = ids.len();
        let ids2 = ids.clone();
        let t2 = title.clone();
        let kind = if pid.is_some() { "Playlist" } else { "Your favorites" };
        let empty = n == 0;
        let header_h = if empty { 352.0 + 220.0 } else { 352.0 };
        let off = self.list_page(
            ui,
            ("playlist", pid),
            header_h,
            n,
            ROW_H,
            |app, ui, r| {
                let hr = Rect::from_min_size(r.min, vec2(r.width(), 352.0));
                let mut hu = ui.new_child(egui::UiBuilder::new().max_rect(hr));
                app.detail_header(&mut hu, hr, kind, &t2, &meta, art, false, &ids2, pid, pid.is_none());
                if empty {
                    let er = Rect::from_min_max(pos2(r.left(), hr.bottom() - 40.0), r.max);
                    let mut eu = ui.new_child(egui::UiBuilder::new().max_rect(er).layout(egui::Layout::top_down(egui::Align::Center)));
                    let (t, b) = if pid.is_some() {
                        ("This playlist is empty", "Right-click any song and choose Add to playlist, or drag songs onto it in the sidebar.")
                    } else {
                        ("No loved songs yet", "Press the heart next to any song and it lands here.")
                    };
                    if w::empty_state(&mut eu, if pid.is_some() { icons::LIST_MUSIC } else { icons::HEART }, t, b, Some(("Browse songs", None))) {
                        app.actions.push(Action::Go(Route::Songs));
                    }
                } else {
                    let ch = Rect::from_min_size(pos2(r.left(), r.bottom() - 32.0), vec2(r.width(), 32.0));
                    app.column_header(ui, ch, mode);
                }
            },
            |app, ui, r, i| {
                let pi = pl_index.get(i).copied();
                app.track_row(ui, r, &ids, i, mode, if pid.is_some() { pi } else { None })
            },
        );
        (off, title)
    }

    // -----------------------------------------------------------------------------------------
    // Search

    fn setup_banner(&mut self, ui: &mut Ui) {
        // Only talk about setup once the person has asked for something from YouTube.
        if !self.yt_requested && self.route != Route::Downloads {
            return;
        }
        let setup = self.yt.shared.lk().setup.clone();
        match setup {
            Setup::Working { label, progress } => {
                let wdt = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(wdt, 64.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, R_MD, RAISED);
                let pct = progress.map(|v| format!(" {}%", (v * 100.0).round() as u32)).unwrap_or_default();
                w::text(p, pos2(r.left() + S4, r.top() + 22.0), Align2::LEFT_CENTER, &format!("{label}…{pct}"), body_strong(), TEXT, wdt - 32.0);
                w::text(p, pos2(r.left() + S4, r.top() + 42.0), Align2::LEFT_CENTER, "This happens once. You can keep listening meanwhile.", small(), TEXT_2, wdt - 32.0);
                let track = Rect::from_min_size(pos2(r.left() + S4, r.bottom() - 8.0), vec2(wdt - 32.0, 3.0));
                p.rect_filled(track, 2, RAISED_2);
                if let Some(v) = progress {
                    p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * v, 3.0)), 2, ACCENT);
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                ui.add_space(S4);
            }
            Setup::Failed(msg) => {
                let wdt = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(wdt, 64.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, R_MD, RAISED);
                icons::glyph(p, pos2(r.left() + 26.0, r.center().y), icons::CIRCLE_ALERT, 20.0, WARN);
                w::text(p, pos2(r.left() + 48.0, r.top() + 22.0), Align2::LEFT_CENTER, "Downloads aren't set up yet", body_strong(), TEXT, wdt - 200.0);
                w::text(p, pos2(r.left() + 48.0, r.top() + 42.0), Align2::LEFT_CENTER, &msg, small(), TEXT_2, wdt - 200.0);
                let br = Rect::from_min_size(pos2(r.right() - 130.0, r.center().y - 18.0), vec2(116.0, 36.0));
                let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(br));
                if w::pill(&mut bu, "Try again", Some(icons::REFRESH), PillKind::Secondary).clicked() {
                    self.yt.shared.lk().setup = Setup::Unknown;
                    self.yt.prepare();
                }
                ui.add_space(S4);
            }
            _ => {}
        }
    }

    fn section_title(&self, ui: &mut Ui, s: &str) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        w::text(ui.painter(), pos2(r.left(), r.center().y + 4.0), Align2::LEFT_CENTER, s, h2(), TEXT, r.width());
    }

    fn search_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let q = self.search.trim().to_string();
        let out = egui::ScrollArea::vertical()
            .id_salt("search")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(TOPBAR_H);
                let full = ui.available_width();
                let mut inner = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_size(ui.cursor().min + vec2(PAD_X, 0.0), vec2(full - 2.0 * PAD_X, f32::INFINITY)))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                let ui2 = &mut inner;
                let (r, _) = ui2.allocate_exact_size(vec2(ui2.available_width(), 44.0), Sense::hover());
                w::text(ui2.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, "Search", h1(), TEXT, 400.0);
                ui2.add_space(S2);
                self.setup_banner(ui2);
                if q.is_empty() {
                    self.search_tips(ui2);
                } else {
                    self.search_results(ui2, &q);
                }
                let used = inner.min_rect().height();
                ui.add_space(used + S12);
            });
        (out.state.offset.y, "Search".into())
    }

    fn search_tips(&mut self, ui: &mut Ui) {
        ui.add_space(S4);
        let wdt = ui.available_width();
        let tips = [
            (icons::SEARCH, "Type a song or artist", "Matches from your library show up as you type. Press Enter to search YouTube too."),
            (icons::LINK, "Paste a YouTube link", "A song or a whole playlist. Playlists also become playlists here."),
            (icons::DOWNLOAD, "Click to listen, download to keep", "Any result plays right away. The download button saves it to your music folder with its cover art."),
        ];
        // Three across when each card has room for its text, otherwise one per row.
        let cols = if (wdt - 2.0 * S4) / 3.0 >= 220.0 { 3 } else { 1 };
        let card_w = if cols == 3 { (wdt - 2.0 * S4) / 3.0 } else { wdt };
        // Three across: icon on top. One per row: icon on the left, so cards stay short.
        let text_x = if cols == 3 { S4 } else { 64.0 };
        let text_w = card_w - text_x - S4;
        for row in tips.chunks(cols) {
            let galleys: Vec<_> = row
                .iter()
                .map(|(_, title, body_text)| {
                    let t = ui.painter().layout(title.to_string(), body_strong(), TEXT, text_w);
                    let b = ui.painter().layout(body_text.to_string(), small(), TEXT_2, text_w);
                    (t, b)
                })
                .collect();
            let text_top = if cols == 3 { 60.0 } else { S4 };
            let card_h = galleys
                .iter()
                .map(|(t, b)| text_top + t.size().y + 4.0 + b.size().y + S4)
                .fold(if cols == 3 { 0.0 } else { 64.0 }, f32::max);
            let (r_row, _) = ui.allocate_exact_size(vec2(wdt, card_h), Sense::hover());
            for (c, ((icon, _, _), (tg, bg))) in row.iter().zip(galleys).enumerate() {
                let r = Rect::from_min_size(pos2(r_row.left() + c as f32 * (card_w + S4), r_row.top()), vec2(card_w, card_h));
                let p = ui.painter();
                p.rect_filled(r, R_LG, RAISED);
                let ic = if cols == 3 { pos2(r.left() + 32.0, r.top() + 32.0) } else { pos2(r.left() + 32.0, r.center().y) };
                p.circle_filled(ic, 16.0, RAISED_2);
                icons::glyph(p, ic, icon, 16.0, ACCENT);
                let th = tg.size().y;
                let block = th + 4.0 + bg.size().y;
                let ty = if cols == 3 { r.top() + text_top } else { r.center().y - block / 2.0 };
                p.galley(pos2(r.left() + text_x, ty), tg, TEXT);
                p.galley(pos2(r.left() + text_x, ty + th + 4.0), bg, TEXT_2);
            }
            ui.add_space(S3);
        }
    }

    fn search_results(&mut self, ui: &mut Ui, q: &str) {
        // Library first: it's instant.
        let is_link = crate::ytdlp::looks_like_link(q);
        if !is_link {
            let hits = Arc::new(self.lib.search(q, 8));
            if !hits.is_empty() {
                self.section_title(ui, "In your library");
                let mode = RowMode { art: true, album_col: true, track_numbers: false, playlist: None, reorder: false };
                let row_w = ui.available_width();
                for i in 0..hits.len() {
                    let (r, _) = ui.allocate_exact_size(vec2(row_w, ROW_H), Sense::hover());
                    let r = r.expand2(vec2(S2, 0.0));
                    let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("hit", i)));
                    self.track_row(&mut ru, r, &hits, i, mode, None);
                }
                ui.add_space(S4);
            }
        }
        self.section_title(ui, if is_link { "From your link" } else { "YouTube" });
        let search = self.yt.shared.lk().search.clone();
        match search {
            Search::Idle => {
                if !is_link {
                    let wdt = ui.available_width();
                    let (r, resp) = ui.allocate_exact_size(vec2(wdt, 56.0), Sense::click());
                    let (h, _) = w::hover_press(ui, &resp);
                    let p = ui.painter();
                    p.rect_filled(r, R_MD, lerp_color(RAISED, RAISED_2, h));
                    icons::glyph(p, pos2(r.left() + 28.0, r.center().y), icons::SEARCH, 18.0, ACCENT);
                    w::text(p, pos2(r.left() + 52.0, r.center().y), Align2::LEFT_CENTER, &format!("Search YouTube for “{q}”"), body_strong(), TEXT, wdt - 160.0);
                    w::text(p, pos2(r.right() - S4, r.center().y), Align2::RIGHT_CENTER, "Enter ↵", small(), TEXT_3, 80.0);
                    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        self.actions.push(Action::YtSearch(q.to_string()));
                    }
                }
            }
            Search::Loading { query } => {
                let _ = query;
                // Skeleton rows in the shape of the results.
                let t = ui.input(|i| i.time) as f32;
                let pulse = 0.5 + 0.5 * (t * 3.0).sin();
                for i in 0..6 {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 68.0), Sense::hover());
                    let p = ui.painter();
                    let c = lerp_color(RAISED, RAISED_2, pulse * 0.6);
                    p.rect_filled(Rect::from_min_size(pos2(r.left(), r.top() + 7.0), vec2(96.0, 54.0)), R_SM + 2.0, c);
                    p.rect_filled(Rect::from_min_size(pos2(r.left() + 112.0, r.top() + 18.0), vec2(260.0 - i as f32 * 18.0, 12.0)), 6, c);
                    p.rect_filled(Rect::from_min_size(pos2(r.left() + 112.0, r.top() + 40.0), vec2(140.0, 10.0)), 5, c);
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
            }
            Search::Failed { msg, .. } => {
                if matches!(self.yt.shared.lk().setup, Setup::Failed(_)) {
                    // The setup banner above already explains it, with its own Try again.
                    return;
                }
                let wdt = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(wdt, 64.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, R_MD, RAISED);
                icons::glyph(p, pos2(r.left() + 26.0, r.center().y), icons::CIRCLE_ALERT, 20.0, WARN);
                w::text(p, pos2(r.left() + 48.0, r.top() + 22.0), Align2::LEFT_CENTER, "Couldn't search YouTube", body_strong(), TEXT, wdt - 200.0);
                w::text(p, pos2(r.left() + 48.0, r.top() + 42.0), Align2::LEFT_CENTER, &msg, small(), TEXT_2, wdt - 200.0);
                let br = Rect::from_min_size(pos2(r.right() - 130.0, r.center().y - 18.0), vec2(116.0, 36.0));
                let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(br));
                if w::pill(&mut bu, "Try again", Some(icons::REFRESH), PillKind::Secondary).clicked() {
                    self.actions.push(Action::YtSearch(q.to_string()));
                }
            }
            Search::Done { query, items, playlist } => {
                if !is_link && query.trim() != q {
                    // Typed more since the last search.
                    let wdt = ui.available_width();
                    let (r, resp) = ui.allocate_exact_size(vec2(wdt, 44.0), Sense::click());
                    let (h, _) = w::hover_press(ui, &resp);
                    p_hint(ui, r, h, q);
                    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        self.actions.push(Action::YtSearch(q.to_string()));
                    }
                    ui.add_space(S2);
                }
                if items.is_empty() {
                    ui.label(egui::RichText::new("No results. Try different words.").font(body()).color(TEXT_2));
                    return;
                }
                if let Some(pl) = &playlist {
                    let wdt = ui.available_width();
                    let (r, _) = ui.allocate_exact_size(vec2(wdt, 84.0), Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(r, R_LG, RAISED);
                    icons::glyph(p, pos2(r.left() + 36.0, r.center().y), icons::LIST_MUSIC, 22.0, ACCENT);
                    w::text(p, pos2(r.left() + 64.0, r.center().y - 10.0), Align2::LEFT_CENTER, &pl.title, body_strong(), TEXT, wdt - 280.0);
                    let sub = if pl.channel.is_empty() { theme::plural(items.len(), "song", "songs") } else { format!("{} · {}", pl.channel, theme::plural(items.len(), "song", "songs")) };
                    w::text(p, pos2(r.left() + 64.0, r.center().y + 10.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, wdt - 280.0);
                    let (active_ids, failed_ids): (Vec<String>, Vec<String>) = {
                        let sh = self.yt.shared.lk();
                        (
                            sh.jobs
                                .iter()
                                .filter(|j| !matches!(&j.status, JobStatus::Failed(_)) && !matches!(&j.status, JobStatus::Done(p) if self.download_gone(p)))
                                .map(|j| j.item.id.clone())
                                .collect(),
                            sh.jobs.iter().filter(|j| matches!(j.status, JobStatus::Failed(_))).map(|j| j.item.id.clone()).collect(),
                        )
                    };
                    let _ = failed_ids;
                    let need: Vec<YtItem> = items
                        .iter()
                        .filter(|i| !self.lib.yt_ids.contains(&i.id) && !active_ids.contains(&i.id))
                        .cloned()
                        .collect();
                    let in_progress = items.iter().any(|i| !self.lib.yt_ids.contains(&i.id) && active_ids.contains(&i.id));
                    let label = if !need.is_empty() {
                        if need.len() == items.len() { format!("Download all {}", items.len()) } else { format!("Download {} more", need.len()) }
                    } else if in_progress {
                        "Downloading…".to_string()
                    } else {
                        "All downloaded".to_string()
                    };
                    let bw = w::text_width(ui, &label, body_strong()) + 60.0;
                    let br = Rect::from_min_size(pos2(r.right() - bw - S4, r.center().y - 18.0), vec2(bw, 36.0));
                    let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(br));
                    let resp = w::pill(&mut bu, &label, Some(icons::DOWNLOAD), if need.is_empty() { PillKind::Secondary } else { PillKind::Primary });
                    if resp.clicked() && !need.is_empty() {
                        self.actions.push(Action::YtDownload { items: items.clone(), playlist_name: Some(pl.title.clone()) });
                    }
                    ui.add_space(S3);
                }
                for (i, item) in items.iter().enumerate() {
                    self.yt_row(ui, item, i);
                }
            }
        }
    }

    fn yt_row(&mut self, ui: &mut Ui, item: &YtItem, i: usize) {
        let wdt = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(wdt, 68.0), Sense::click());
        let (h, _) = w::hover_press(ui, &resp);
        let p = ui.painter().clone();
        w::hover_surface(&p, rect.expand2(vec2(S2, 0.0)), h);

        // State: in library / downloading / failed / ready to download. A finished download
        // whose file was deleted counts as not downloaded.
        let in_lib = self.lib.yt_ids.contains(&item.id);
        let job = self.yt.shared.lk().jobs.iter().find(|j| j.item.id == item.id).cloned();
        let status = job
            .as_ref()
            .map(|j| j.status.clone())
            .filter(|s| !matches!(s, JobStatus::Done(path) if self.download_gone(path)));
        let saved = in_lib || matches!(status, Some(JobStatus::Done(_)));
        let loading = !saved && matches!(self.yt.shared.lk().previews.get(&item.id), Some(crate::ytdlp::PreviewState::Loading(_)));
        let playing_this = self.playing_yt_id().as_deref() == Some(item.id.as_str());

        // Thumbnail, with play / loading / now-playing on top.
        let tr = Rect::from_min_size(pos2(rect.left(), rect.top() + 7.0), vec2(96.0, 54.0));
        let tex = self.art.get(ArtKey::Url(item.thumb_url()));
        w::thumb(&p, tr, tex, R_SM + 2.0);
        let veil = if loading || playing_this { 1.0 } else { h };
        if veil > 0.0 {
            p.rect_filled(tr, R_SM + 2.0, with_alpha(Color32::BLACK, 0.55 * veil));
            if loading {
                w::spinner(ui, tr.center(), 10.0, TEXT);
            } else if playing_this && h > 0.0 && self.ps.playing {
                icons::pause(&p, tr.center(), 16.0, with_alpha(TEXT, veil));
            } else if playing_this && self.ps.playing {
                icons::now_playing_bars(&p, tr.center(), 16.0, ACCENT);
            } else {
                icons::play(&p, tr.center(), 16.0, with_alpha(TEXT, veil));
            }
        } else if item.duration > 0.0 {
            let d = fmt_time(item.duration);
            let dw = w::text_width(ui, &d, small()) + 8.0;
            let dr = Rect::from_min_size(pos2(tr.right() - dw - 4.0, tr.bottom() - 20.0), vec2(dw, 16.0));
            p.rect_filled(dr, 4, with_alpha(Color32::BLACK, 0.75));
            w::text(&p, dr.center(), Align2::CENTER_CENTER, &d, small(), TEXT, 60.0);
        }
        let tx = tr.right() + S4;
        let tw = rect.right() - tx - 64.0;
        let title_c = if playing_this { ACCENT } else { TEXT };
        w::text(&p, pos2(tx, rect.center().y - 9.0), Align2::LEFT_CENTER, &item.title, body(), title_c, tw);
        let sub = if saved || !playing_this { item.channel.clone() } else { format!("{} · Playing from YouTube", item.channel) };
        w::text(&p, pos2(tx, rect.center().y + 11.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, tw);

        let br = Rect::from_center_size(pos2(rect.right() - 24.0, rect.center().y), Vec2::splat(40.0));
        let bresp = ui.interact(br, Id::new(("yt-dl", &item.id, i)), Sense::click());
        let (bh, bp) = w::hover_press(ui, &bresp);
        let c = br.center();
        match (&status, in_lib) {
            (_, true) | (Some(JobStatus::Done(_)), _) => {
                p.circle_filled(c, 15.0, with_alpha(ACCENT, 0.16));
                icons::glyph(&p, c, icons::CHECK, 16.0, ACCENT);
                let resp = bresp.on_hover_text("In your library");
                if resp_clicked(&resp) {
                    self.play_yt(&item.id, job.as_ref());
                }
            }
            (Some(JobStatus::Waiting), _) => {
                w::progress_ring(&p, c, 13.0, 0.0, ACCENT);
                let _ = bresp.on_hover_text("Waiting to download");
            }
            (Some(JobStatus::Downloading { pct, .. }), _) => {
                w::progress_ring(&p, c, 13.0, *pct, ACCENT);
                let _ = bresp.on_hover_text(format!("Downloading · {}%", (pct * 100.0).round() as u32));
            }
            (Some(JobStatus::Saving), _) => {
                w::progress_ring(&p, c, 13.0, 1.0, ACCENT);
                let _ = bresp.on_hover_text("Adding to your library");
            }
            (Some(JobStatus::Failed(msg)), _) => {
                icons::glyph(&p, c, icons::CIRCLE_ALERT, 20.0, WARN);
                let r = bresp.on_hover_text(format!("{msg}\nClick to try again.")).on_hover_cursor(CursorIcon::PointingHand);
                if r.clicked() {
                    if let Some(j) = &job {
                        self.actions.push(Action::YtRetry(j.uid));
                    }
                }
            }
            (None, false) => {
                p.circle_filled(c, 18.0 * (1.0 - 0.06 * bp), lerp_color(RAISED_2, Color32::from_rgb(0x36, 0x36, 0x3E), bh));
                icons::glyph(&p, c, icons::ARROW_DOWN_TO_LINE, 18.0, lerp_color(TEXT, ACCENT_HI, bh));
                let r = bresp.on_hover_text("Download").on_hover_cursor(CursorIcon::PointingHand);
                if r.clicked() {
                    self.actions.push(Action::YtDownload { items: vec![item.clone()], playlist_name: None });
                }
            }
        }
        // Clicking the row plays it: from your library if it's there, otherwise straight from
        // YouTube without saving.
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            if playing_this {
                self.actions.push(Action::Toggle);
            } else if saved {
                self.play_yt(&item.id, job.as_ref());
            } else if !loading {
                self.actions.push(Action::Preview(item.clone()));
            }
        }
    }

    fn play_yt(&mut self, yt_id: &str, job: Option<&crate::ytdlp::Job>) {
        if let Some(t) = self.lib.tracks.iter().find(|t| t.yt_id.as_deref() == Some(yt_id)) {
            self.actions.push(Action::PlayIds { ids: vec![t.id], start: 0, shuffle: None });
        } else if let Some(JobStatus::Done(path)) = job.map(|j| &j.status) {
            self.actions.push(Action::PlayPath(path.clone()));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Downloads

    fn downloads_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let jobs = self.yt.shared.lk().jobs.clone();
        let active = jobs.iter().filter(|j| matches!(j.status, JobStatus::Waiting | JobStatus::Downloading { .. } | JobStatus::Saving)).count();
        let finished = jobs.len() - active;
        let out = egui::ScrollArea::vertical()
            .id_salt("downloads")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(TOPBAR_H);
                let full = ui.available_width();
                let mut inner = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_size(ui.cursor().min + vec2(PAD_X, 0.0), vec2(full - 2.0 * PAD_X, f32::INFINITY)))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                let ui2 = &mut inner;
                let (r, _) = ui2.allocate_exact_size(vec2(ui2.available_width(), 44.0), Sense::hover());
                w::text(ui2.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, "Downloads", h1(), TEXT, 400.0);
                let meta = if active > 0 {
                    format!("{} downloading", group_digits(active))
                } else if jobs.is_empty() {
                    "Songs you download from YouTube show up here".into()
                } else {
                    "All done".into()
                };
                let (r, _) = ui2.allocate_exact_size(vec2(ui2.available_width(), 18.0), Sense::hover());
                w::text(ui2.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &meta, small(), TEXT_2, r.width());
                ui2.add_space(S4);
                ui2.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = S2;
                    if w::pill(ui, "Search YouTube", Some(icons::SEARCH), PillKind::Primary).clicked() {
                        self.actions.push(Action::Go(Route::Search));
                    }
                    if w::pill(ui, "Open music folder", Some(icons::FOLDER_OPEN), PillKind::Secondary).clicked() {
                        self.actions.push(Action::OpenFolder);
                    }
                    if finished > 0 && w::pill(ui, "Clear finished", None, PillKind::Ghost).clicked() {
                        self.actions.push(Action::YtClearFinished);
                    }
                });
                ui2.add_space(S5);
                self.setup_banner(ui2);
                if jobs.is_empty() {
                    w::empty_state(ui2, icons::DOWNLOAD, "Nothing downloading", "Search for a song or paste a YouTube link, then press the download button next to it.", None);
                }
                for j in jobs.iter().rev() {
                    self.job_row(ui2, j);
                }
                let used = inner.min_rect().height();
                ui.add_space(used + S12);
            });
        (out.state.offset.y, "Downloads".into())
    }

    fn job_row(&mut self, ui: &mut Ui, j: &crate::ytdlp::Job) {
        let wdt = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(wdt, 72.0), Sense::click());
        let (h, _) = w::hover_press(ui, &resp);
        let p = ui.painter().clone();
        w::hover_surface(&p, rect.expand2(vec2(S2, 0.0)), h);
        let tr = Rect::from_min_size(pos2(rect.left(), rect.top() + 9.0), vec2(96.0, 54.0));
        let tex = self.art.get(ArtKey::Url(j.item.thumb_url()));
        w::thumb(&p, tr, tex, R_SM + 2.0);
        let tx = tr.right() + S4;
        let tw = rect.right() - tx - 120.0;
        w::text(&p, pos2(tx, rect.top() + 24.0), Align2::LEFT_CENTER, &j.item.title, body(), TEXT, tw);
        let (line, color, frac) = match &j.status {
            JobStatus::Waiting => ("Waiting".to_string(), TEXT_2, None),
            JobStatus::Downloading { pct, speed } => {
                let mut s = format!("Downloading · {}%", (pct * 100.0).round() as u32);
                if !speed.is_empty() {
                    s.push_str(&format!(" · {speed}"));
                }
                (s, TEXT_2, Some(*pct))
            }
            JobStatus::Saving => ("Adding cover art and tags".to_string(), TEXT_2, Some(1.0)),
            JobStatus::Done(p) if self.download_gone(p) => ("No longer in your music folder".to_string(), TEXT_3, None),
            JobStatus::Done(_) => ("Saved to your library".to_string(), TEXT_2, None),
            JobStatus::Failed(m) => (m.clone(), WARN, None),
        };
        w::text(&p, pos2(tx, rect.top() + 44.0), Align2::LEFT_CENTER, &line, small(), color, tw);
        if let Some(f) = frac {
            let bar = Rect::from_min_size(pos2(tx, rect.top() + 58.0), vec2(tw.min(420.0), 3.0));
            p.rect_filled(bar, 2, RAISED_2);
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, 3.0)), 2, ACCENT);
        }
        // Right-side actions
        let mut x = rect.right() - 20.0;
        let mut btn = |ui: &mut Ui, icon: &str, tip: &str, key: &str| -> bool {
            let r = Rect::from_center_size(pos2(x, rect.center().y), Vec2::splat(34.0));
            x -= 40.0;
            let resp = ui.interact(r, Id::new(("job", j.uid, key)), Sense::click());
            w::icon_button_at(ui, r, resp, icon, false, tip).clicked()
        };
        match &j.status {
            JobStatus::Waiting | JobStatus::Downloading { .. } => {
                if btn(ui, icons::X, "Cancel", "cancel") {
                    self.actions.push(Action::YtCancel(j.uid));
                }
            }
            JobStatus::Failed(_) => {
                if btn(ui, icons::X, "Remove", "remove") {
                    self.actions.push(Action::YtCancel(j.uid));
                }
                if btn(ui, icons::REFRESH, "Try again", "retry") {
                    self.actions.push(Action::YtRetry(j.uid));
                }
            }
            JobStatus::Done(path) if self.download_gone(path) => {
                if btn(ui, icons::X, "Remove", "remove") {
                    self.actions.push(Action::YtCancel(j.uid));
                }
                if btn(ui, icons::REFRESH, "Download again", "again") {
                    self.actions.push(Action::YtRetry(j.uid));
                }
            }
            JobStatus::Done(path) => {
                let path = path.clone();
                if btn(ui, icons::FOLDER_OPEN, "Show in folder", "reveal") {
                    crate::platform::reveal(&path);
                }
                let pr = Rect::from_center_size(pos2(x, rect.center().y), Vec2::splat(34.0));
                let presp = ui.interact(pr, Id::new(("job", j.uid, "play")), Sense::click());
                let (ph, _) = w::hover_press(ui, &presp);
                ui.painter().circle_filled(pr.center(), 16.0, lerp_color(RAISED_2, ACCENT, ph));
                icons::play(ui.painter(), pr.center(), 11.0, TEXT);
                if presp.on_hover_text("Play").on_hover_cursor(CursorIcon::PointingHand).clicked() || resp.double_clicked() {
                    let id = self.lib.by_path(&path).map(|t| t.id);
                    match id {
                        Some(id) => self.actions.push(Action::PlayIds { ids: vec![id], start: 0, shuffle: None }),
                        None => self.actions.push(Action::PlayPath(path)),
                    }
                }
            }
            JobStatus::Saving => {}
        }
    }

    // -----------------------------------------------------------------------------------------
    // Settings

    fn settings_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let out = egui::ScrollArea::vertical()
            .id_salt("settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(TOPBAR_H);
                let full = ui.available_width();
                let colw = (full - 2.0 * PAD_X).min(720.0);
                let mut inner = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_size(ui.cursor().min + vec2(PAD_X, 0.0), vec2(colw, f32::INFINITY)))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                let ui2 = &mut inner;
                let (r, _) = ui2.allocate_exact_size(vec2(colw, 44.0), Sense::hover());
                w::text(ui2.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, "Settings", h1(), TEXT, 400.0);
                ui2.add_space(S5);

                // Music folder
                let folder = self.settings.music_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
                self.settings_card(ui2, "Music folder", "Segno plays everything in this folder, including subfolders, and saves downloads here.", |app, ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                    ui.painter().rect_filled(r, R_MD, BG);
                    icons::glyph(ui.painter(), pos2(r.left() + 20.0, r.center().y), icons::FOLDER_OPEN, 16.0, TEXT_2);
                    w::text(ui.painter(), pos2(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, &folder, body(), TEXT, r.width() - 52.0);
                    ui.add_space(S3);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = S2;
                        if w::pill(ui, "Change folder", None, PillKind::Secondary).clicked() {
                            app.actions.push(Action::PickFolder);
                        }
                        if w::pill(ui, "Open in Explorer", None, PillKind::Ghost).clicked() {
                            app.actions.push(Action::OpenFolder);
                        }
                        if w::pill(ui, "Check for new music", None, PillKind::Ghost).clicked() {
                            app.actions.push(Action::Rescan);
                        }
                    });
                });

                // Playback
                let mut auto = self.settings.autoplay;
                self.settings_card(ui2, "Playback", "", |app, ui| {
                    if toggle_row(ui, "Keep playing when the queue ends", "Plays on through your Songs list, so the music doesn't stop.", &mut auto) {
                        app.actions.push(Action::SetAutoplay(auto));
                    }
                });

                // Close behavior
                let mut v = self.settings.close_to_tray;
                self.settings_card(ui2, "When you close the window", "", |app, ui| {
                    if toggle_row(ui, "Keep playing in the tray while music is playing", "If nothing is playing, closing the window quits Segno.", &mut v) {
                        app.actions.push(Action::SetCloseToTray(v));
                    }
                });

                // Downloads
                let (version, setup) = {
                    let s = self.yt.shared.lk();
                    (s.version.clone(), s.setup.clone())
                };
                let status = match (&setup, &version) {
                    (Setup::Working { label, .. }, _) => format!("{label}…"),
                    (Setup::Failed(m), _) => m.clone(),
                    (_, Some(v)) => format!("yt-dlp {v} · saves M4A with cover art"),
                    _ => "Sets itself up the first time you search or download.".into(),
                };
                self.settings_card(ui2, "Downloads", &status, |app, ui| {
                    if version.is_some() {
                        if w::pill(ui, "Update yt-dlp", Some(icons::REFRESH), PillKind::Secondary).clicked() {
                            app.actions.push(Action::YtUpdate);
                            app.actions.push(Action::Toast("Updating yt-dlp".into()));
                        }
                    } else if !matches!(setup, Setup::Working { .. }) && w::pill(ui, "Set up now", Some(icons::DOWNLOAD), PillKind::Secondary).clicked() {
                        app.yt_requested = true;
                        app.yt.shared.lk().setup = Setup::Unknown;
                        app.yt.prepare();
                    }
                });

                // Updates
                {
                    use crate::update::{State, VERSION};
                    let st = self.updater.state();
                    let status = match &st {
                        State::Idle => format!("You're on Segno {VERSION}."),
                        State::Checking => "Checking for updates…".to_string(),
                        State::UpToDate => format!("You have the latest version, Segno {VERSION}."),
                        State::Downloading { version, pct } => format!("Downloading Segno {version} · {}%", (pct * 100.0).round() as u32),
                        State::Ready { version } => format!("Segno {version} is ready. It starts the next time you open Segno."),
                        State::Available { version } => format!("Segno {version} is out. Get it from the download page."),
                        State::Failed(m) => m.clone(),
                    };
                    let page = self.updater.page();
                    let mut auto = self.settings.auto_update;
                    self.settings_card(ui2, "Updates", &status, |app, ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = S2;
                            match &st {
                                State::Ready { .. } => {
                                    let settled = ui.input(|i| i.time) - app.update_ready_at > 0.6;
                                    if w::pill(ui, "Restart now", Some(icons::REFRESH), PillKind::Primary).clicked() && settled {
                                        app.actions.push(Action::RestartToUpdate);
                                    }
                                    if w::pill(ui, "What's new", None, PillKind::Ghost).clicked() {
                                        app.actions.push(Action::OpenUrl(page.clone()));
                                    }
                                }
                                State::Available { version } => {
                                    if w::pill(ui, &format!("Get Segno {version}"), Some(icons::DOWNLOAD), PillKind::Primary).clicked() {
                                        app.actions.push(Action::OpenUrl(page.clone()));
                                    }
                                }
                                State::Checking | State::Downloading { .. } => {
                                    let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                                    w::spinner(ui, r.center(), 9.0, TEXT_2);
                                }
                                _ => {
                                    if w::pill(ui, "Check for updates", Some(icons::REFRESH), PillKind::Secondary).clicked() {
                                        app.actions.push(Action::CheckUpdates);
                                    }
                                }
                            }
                        });
                        ui.add_space(S3);
                        if toggle_row(ui, "Update automatically", "Checks once a day and downloads new versions in the background.", &mut auto) {
                            app.actions.push(Action::SetAutoUpdate(auto));
                        }
                    });
                }

                // Shortcuts
                self.settings_card(ui2, "Keyboard shortcuts", "", |_app, ui| {
                    for (k, d) in [
                        ("Space", "Play or pause"),
                        ("Ctrl + →  /  Ctrl + ←", "Next / previous song"),
                        ("Ctrl + ↑  /  Ctrl + ↓", "Volume up / down"),
                        ("Ctrl + F", "Search"),
                        ("Ctrl + M", "Mini player"),
                        ("Ctrl + B", "Fold or unfold the sidebar"),
                        ("Alt + ←", "Back"),
                        ("Double-click a song", "Play it"),
                    ] {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                        w::text(ui.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, d, body(), TEXT_2, r.width() * 0.5);
                        w::text(ui.painter(), pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, k, body_strong(), TEXT, r.width() * 0.5);
                    }
                });

                self.settings_card(ui2, "About", &format!("Segno {}. Icons by Lucide, type set in Inter. Downloads use yt-dlp. Only download music you have the right to.", crate::update::VERSION), |_a, _u| {});
                let used = inner.min_rect().height();
                ui.add_space(used + S12);
            });
        (out.state.offset.y, "Settings".into())
    }

    fn settings_card(&mut self, ui: &mut Ui, title: &str, sub: &str, content: impl FnOnce(&mut App, &mut Ui)) {
        let frame = egui::Frame::new().fill(RAISED).corner_radius(R_LG).inner_margin(S5);
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).font(body_strong()).color(TEXT));
            if !sub.is_empty() {
                ui.add_space(2.0);
                ui.label(egui::RichText::new(sub).font(small()).color(TEXT_2));
            }
            ui.add_space(S3);
            content(self, ui);
        });
        ui.add_space(S4);
    }

    // -----------------------------------------------------------------------------------------
    // Now Playing

    fn now_playing_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let rect = ui.max_rect();
        let Some(t) = self.now_info() else {
            ui.add_space(TOPBAR_H + S8);
            if w::empty_state(ui, icons::MUSIC, "Nothing playing", "Pick a song and it shows up here, big.", Some(("Browse songs", None))) {
                self.actions.push(Action::Go(Route::Songs));
            }
            return (0.0, String::new());
        };
        let wide = rect.width() > 860.0;
        let avail_h = rect.height() - TOPBAR_H - S8;
        let left_w = if wide { rect.width() * 0.56 } else { rect.width() };
        let art_s = (avail_h - 140.0).min(left_w - 2.0 * PAD_X - S8).min(460.0).max(160.0);
        let cx = rect.left() + left_w / 2.0;
        let top = rect.top() + TOPBAR_H + ((avail_h - art_s - 120.0) / 2.0).max(0.0);
        let ar = Rect::from_min_size(pos2(cx - art_s / 2.0, top), Vec2::splat(art_s));
        let p = ui.painter().clone();
        // Art "breathes" slightly smaller while paused (one short transition, not a loop).
        let play_t = ui.ctx().animate_bool_with_time(Id::new("np-playing"), self.ps.playing, T_SLOW);
        let ar_s = w::scale_rect(ar, 0.92 + 0.08 * play_t);
        w::soft_shadow(&p, ar_s, R_LG, 0.6 + 0.6 * play_t);
        let tex = self.art.cover(t.art, ArtSize::L);
        w::art(&p, ar_s, tex, R_LG, Color32::WHITE);
        w::text(&p, pos2(cx, ar.bottom() + 40.0), Align2::CENTER_CENTER, &t.title, FontId::new(24.0, bold()), TEXT, left_w - 2.0 * PAD_X);
        let sub = if t.preview.is_some() {
            format!("{} · From YouTube", t.artist)
        } else if t.album.is_empty() {
            t.artist.clone()
        } else {
            format!("{} · {}", t.artist, t.album)
        };
        let sr = w::text(&p, pos2(cx, ar.bottom() + 70.0), Align2::CENTER_CENTER, &sub, body(), TEXT_2, left_w - 2.0 * PAD_X);
        if let Some(key) = t.artist_key {
            let resp = ui.interact(sr, Id::new("np-artist"), Sense::click());
            if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                self.actions.push(Action::Go(Route::Artist(key)));
            }
        }
        let lr = Rect::from_center_size(pos2(cx, ar.bottom() + 104.0), Vec2::splat(36.0));
        let lresp = ui.interact(lr, Id::new("np-love"), Sense::click());
        match &t.preview {
            None => {
                if self.love_button(ui, lr, lresp, t.id).clicked() {
                    self.actions.push(Action::ToggleLove(t.id));
                }
            }
            Some(item) => self.save_preview_button(ui, lr, lresp, item),
        }

        if wide {
            let qr = Rect::from_min_max(pos2(rect.left() + left_w, rect.top() + TOPBAR_H + S4), pos2(rect.right() - PAD_X, rect.bottom() - S4));
            p.rect_filled(qr, R_LG, with_alpha(Color32::BLACK, 0.22));
            let inner = qr.shrink(S4);
            w::text(&p, pos2(inner.left(), inner.top() + 12.0), Align2::LEFT_CENTER, "Up next", h2(), TEXT, inner.width());
            let fit = ((inner.height() - 40.0) / 56.0).max(0.0) as usize;
            // Queue first; when it runs out, the library songs that play after it.
            let mut up: Vec<(Option<usize>, u64)> = self.ps.queue.iter().enumerate().skip(self.ps.index + 1).take(fit).map(|(i, q)| (Some(i), q.id)).collect();
            let from_queue = up.len();
            if up.len() < fit {
                let more = self.autoplay_preview(fit - up.len());
                up.extend(more.into_iter().map(|id| (None, id)));
            }
            if up.is_empty() {
                w::text(&p, pos2(inner.left(), inner.top() + 48.0), Align2::LEFT_CENTER, "That's the last song.", small(), TEXT_2, inner.width());
            }
            let mut y = inner.top() + 36.0;
            for (k, (qi, id)) in up.into_iter().enumerate() {
                if k == from_queue && qi.is_none() {
                    if k > 0 {
                        y += S2;
                    }
                    w::text(&p, pos2(inner.left(), y + 10.0), Align2::LEFT_CENTER, "Then from your library", small_strong(), TEXT_3, inner.width());
                    y += 24.0;
                }
                let r = Rect::from_min_size(pos2(inner.left() - S2, y), vec2(inner.width() + S4, 52.0));
                y += 56.0;
                if r.bottom() > inner.bottom() + S2 {
                    break;
                }
                let resp = ui.interact(r, Id::new(("np-up", qi, id)), Sense::click());
                let (h, _) = w::hover_press(ui, &resp);
                w::hover_surface(&p, r, h * 0.8);
                let tt = self.info_for(id);
                let a = Rect::from_min_size(pos2(r.left() + S2, r.top() + 6.0), vec2(40.0, 40.0));
                let tex = self.art.cover(tt.as_ref().and_then(|t| t.art), ArtSize::S);
                w::art(&p, a, tex, R_SM, Color32::WHITE);
                if let Some(tt) = tt {
                    let c = if qi.is_some() { TEXT } else { TEXT_2 };
                    w::text(&p, pos2(a.right() + S3, r.center().y - 8.0), Align2::LEFT_CENTER, &tt.title, body(), c, r.width() - 80.0);
                    w::text(&p, pos2(a.right() + S3, r.center().y + 9.0), Align2::LEFT_CENTER, &tt.artist, small(), TEXT_2, r.width() - 80.0);
                }
                if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    match qi {
                        Some(qi) => self.actions.push(Action::JumpTo(qi)),
                        None => self.actions.push(Action::PlayFromAutoplay(id)),
                    }
                }
            }
        }
        (0.0, String::new())
    }

    // -----------------------------------------------------------------------------------------
    // Welcome (first run: one question)

    pub fn welcome(&mut self, ui: &mut Ui, full: Rect) {
        let p = ui.painter().clone();
        w::vgradient(&p, full, 0.0, &[(0.0, Color32::from_rgb(0x3A, 0x18, 0x26)), (0.6, BG), (1.0, BG)]);
        let card = Rect::from_center_size(full.center(), vec2(520.0f32.min(full.width() - 48.0), 400.0));
        let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(card).layout(egui::Layout::top_down(egui::Align::Center)));
        let ui = &mut cu;
        let (r, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
        crate::paint_logo(ui.painter(), r);
        ui.add_space(S5);
        ui.label(egui::RichText::new("Welcome to Segno").font(FontId::new(30.0, bold())).color(TEXT));
        ui.add_space(S2);
        let msg = "Pick the folder where your music lives. Segno plays everything in it and saves your downloads there.";
        let mut job = egui::text::LayoutJob::simple(msg.to_string(), body(), TEXT_2, card.width() - 40.0);
        job.halign = egui::Align::Center;
        let g = ui.painter().layout_job(job);
        let (r, _) = ui.allocate_exact_size(vec2(card.width(), g.size().y), Sense::hover());
        ui.painter().galley(pos2(r.center().x, r.top()), g, TEXT_2);
        ui.add_space(S6);
        let def = store::default_music_dir();
        let (r, _) = ui.allocate_exact_size(vec2(card.width() - 40.0, 44.0), Sense::hover());
        ui.painter().rect_filled(r, R_MD, RAISED);
        icons::glyph(ui.painter(), pos2(r.left() + 22.0, r.center().y), icons::FOLDER_OPEN, 18.0, ACCENT);
        w::text(ui.painter(), pos2(r.left() + 44.0, r.center().y), Align2::LEFT_CENTER, &def.display().to_string(), body(), TEXT, r.width() - 56.0);
        ui.add_space(S5);
        ui.horizontal(|ui| {
            let total = 168.0 + 190.0 + S2;
            ui.add_space((ui.available_width() - total) / 2.0);
            ui.spacing_mut().item_spacing.x = S2;
            if w::pill(ui, "Use this folder", None, PillKind::Primary).clicked() {
                self.actions.push(Action::SetFolder(def.clone()));
            }
            if w::pill(ui, "Choose another folder", None, PillKind::Secondary).clicked() {
                self.actions.push(Action::PickFolder);
            }
        });
        ui.add_space(S4);
        ui.label(egui::RichText::new("You can change this later in Settings.").font(small()).color(TEXT_3));
    }

    // -----------------------------------------------------------------------------------------
    // Mini player: art fills the window; controls fade in on hover.

    pub fn mini_ui(&mut self, ui: &mut Ui) -> f32 {
        let full = ui.max_rect();
        let p = ui.painter().clone();
        let t = self.now_info();
        let color = theme::wash_from_art(t.as_ref().and_then(|t| t.color));
        p.rect_filled(full, 0.0, color);
        let tex = self.art.cover(t.as_ref().and_then(|t| t.art), ArtSize::L);
        if t.as_ref().and_then(|t| t.art).is_some() {
            w::art(&p, full, tex, 0.0, Color32::WHITE);
        } else {
            icons::glyph(&p, full.center(), icons::MUSIC, 64.0, with_alpha(TEXT, 0.5));
        }
        let bg = ui.interact(full, Id::new("mini-bg"), Sense::click_and_drag());
        if bg.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if bg.double_clicked() {
            self.actions.push(Action::ToggleMini);
        }
        let hovering = ui.ctx().input(|i| i.pointer.has_pointer()) && ui.rect_contains_pointer(full);
        let h = ui.ctx().animate_bool_with_time(Id::new("mini-hover"), hovering || !self.ps.playing, T);
        if h > 0.0 {
            w::vgradient(&p, full, 0.0, &[(0.0, with_alpha(Color32::BLACK, 0.35 * h)), (0.45, with_alpha(Color32::BLACK, 0.15 * h)), (1.0, with_alpha(Color32::BLACK, 0.85 * h))]);
            let c = full.center() + vec2(0.0, 6.0);
            let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_center_size(c, vec2(180.0, 56.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            cu.multiply_opacity(h);
            cu.spacing_mut().item_spacing.x = S4;
            if w::transport_button_c(&mut cu, 40.0, w::Transport::Prev, "Previous", TEXT).clicked() {
                self.actions.push(Action::Prev);
            }
            if w::play_circle(&mut cu, 52.0, self.ps.playing, true).clicked() {
                self.actions.push(Action::Toggle);
            }
            if w::transport_button_c(&mut cu, 40.0, w::Transport::Next, "Next", TEXT).clicked() {
                self.actions.push(Action::Next);
            }
            if let Some(t) = &t {
                w::text(&p, pos2(full.left() + S4, full.bottom() - 40.0), Align2::LEFT_CENTER, &t.title, body_strong(), with_alpha(TEXT, h), full.width() - 32.0);
                w::text(&p, pos2(full.left() + S4, full.bottom() - 21.0), Align2::LEFT_CENTER, &t.artist, small(), with_alpha(TEXT_2, h), full.width() - 32.0);
            }
            let er = Rect::from_center_size(pos2(full.right() - 22.0, full.top() + 22.0), Vec2::splat(32.0));
            let eresp = ui.interact(er, Id::new("mini-expand"), Sense::click());
            let (eh, _) = w::hover_press(ui, &eresp);
            p.circle_filled(er.center(), 16.0, with_alpha(Color32::BLACK, (0.35 + 0.2 * eh) * h));
            icons::glyph(&p, er.center(), icons::MAXIMIZE, 15.0, with_alpha(TEXT, h));
            if eresp.on_hover_text("Back to full player").on_hover_cursor(CursorIcon::PointingHand).clicked() {
                self.actions.push(Action::ToggleMini);
            }
        }
        // Progress line, always visible.
        let dur = self.now_duration();
        let frac = if dur > 0.0 { (self.pos / dur).clamp(0.0, 1.0) } else { 0.0 };
        let line = Rect::from_min_size(pos2(full.left(), full.bottom() - 3.0), vec2(full.width(), 3.0));
        p.rect_filled(line, 0.0, with_alpha(Color32::WHITE, 0.2));
        p.rect_filled(Rect::from_min_size(line.min, vec2(full.width() * frac, 3.0)), 0.0, ACCENT);
        full.width()
    }
}

fn resp_clicked(r: &Response) -> bool {
    r.clicked()
}

fn p_hint(ui: &Ui, r: Rect, h: f32, q: &str) {
    let p = ui.painter();
    p.rect_filled(r, R_MD, lerp_color(RAISED, RAISED_2, h));
    icons::glyph(p, pos2(r.left() + 24.0, r.center().y), icons::SEARCH, 16.0, ACCENT);
    w::text(p, pos2(r.left() + 46.0, r.center().y), Align2::LEFT_CENTER, &format!("Search YouTube for “{q}”"), body(), TEXT, r.width() - 140.0);
    w::text(p, pos2(r.right() - S4, r.center().y), Align2::RIGHT_CENTER, "Enter ↵", small(), TEXT_3, 80.0);
}

/// A settings row with a switch. Returns true when toggled.
fn toggle_row(ui: &mut Ui, label: &str, sub: &str, value: &mut bool) -> bool {
    let wdt = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(wdt, 44.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("t"), *value, T);
    let p = ui.painter();
    w::text(p, pos2(rect.left(), rect.top() + 12.0), Align2::LEFT_CENTER, label, body(), TEXT, wdt - 80.0);
    if !sub.is_empty() {
        w::text(p, pos2(rect.left(), rect.top() + 32.0), Align2::LEFT_CENTER, sub, small(), TEXT_2, wdt - 80.0);
    }
    let sw = Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), vec2(44.0, 24.0));
    p.rect_filled(sw, 12, lerp_color(RAISED_2, ACCENT, t));
    let knob_x = sw.left() + 12.0 + 20.0 * egui::emath::easing::cubic_out(t);
    p.circle_filled(pos2(knob_x, sw.center().y), 9.5, Color32::WHITE);
    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        *value = !*value;
        return true;
    }
    false
}

use egui::FontId;
