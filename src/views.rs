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
    /// Show when each song was added (Songs list).
    pub added: bool,
    /// Every song here is by this artist (album and artist pages): don't repeat the name on
    /// each row unless a song is by someone else.
    pub one_artist: Option<u64>,
    /// Show the position number (off for search results, where the order isn't a list).
    pub numbers: bool,
}

impl RowMode {
    pub fn list() -> Self {
        Self { art: true, album_col: true, track_numbers: false, playlist: None, reorder: false, added: false, one_artist: None, numbers: true }
    }
}

/// Where each column of a song list sits. The header and every row use the same numbers, so
/// they always line up.
#[derive(Clone, Copy)]
struct Cols {
    num: f32,
    art: Option<f32>,
    text: f32,
    title_w: f32,
    album: Option<(f32, f32)>,
    added: Option<(f32, f32)>,
    more: f32,
    heart: f32,
    dur_right: f32,
}

fn song_cols(left: f32, right: f32, mode: RowMode) -> Cols {
    let (art, text) = if mode.art { (Some(left + 44.0), left + 44.0 + 40.0 + S3) } else { (None, left + 48.0) };
    let more = right - 116.0;
    let heart = right - 82.0;
    let w = right - left;
    // Text columns end before the hover buttons (more and love) start.
    let mut end = more - 24.0;
    let added = if mode.added && w > if mode.album_col { 900.0 } else { 600.0 } {
        let x = end - 104.0;
        end = x - S4;
        Some((x, 104.0))
    } else {
        None
    };
    let album = if mode.album_col && w > 720.0 {
        let x = text + (end - text) * 0.52;
        let a = (x, end - x);
        end = x - S4;
        Some(a)
    } else {
        None
    };
    Cols { num: left + 22.0, art, text, title_w: end - text, album, added, more, heart, dur_right: right - 12.0 }
}

/// "Today", "Yesterday", "3 days ago", "2 weeks ago", then the date ("12 Mar", "12 Mar 2025").
fn when_added(secs: u64) -> String {
    if secs == 0 {
        return String::new();
    }
    let now = crate::store::now_secs();
    let days = now.saturating_sub(secs) / 86_400;
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => format!("{days} days ago"),
        7..=13 => "1 week ago".into(),
        14..=29 => format!("{} weeks ago", days / 7),
        _ => {
            let (y, m, d) = civil_from_days((secs / 86_400) as i64);
            let (ny, _, _) = civil_from_days((now / 86_400) as i64);
            const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            let mon = MONTHS[(m as usize).clamp(1, 12) - 1];
            if y == ny { format!("{d} {mon}") } else { format!("{d} {mon} {y}") }
        }
    }
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub(crate) const MENU_W: f32 = 236.0;

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

struct RowInfo {
    title: String,
    artist: String,
    primary: String,
    album: String,
    duration: f32,
    added: u64,
    track_no: u32,
    art: Option<u64>,
    /// Set when the song isn't downloaded and plays from its site.
    stream: Option<YtItem>,
}

impl RowInfo {
    fn primary_artist(&self) -> &str {
        &self.primary
    }
}

pub(crate) fn menu_sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(MENU_W, 9.0), Sense::hover());
    ui.painter().line_segment([pos2(r.left() + 6.0, r.center().y), pos2(r.right() - 6.0, r.center().y)], Stroke::new(1.0, HAIRLINE));
}

impl App {
    /// What a song row shows: a library song, or one that plays from its site.
    fn row_info(&self, id: u64) -> Option<RowInfo> {
        if let Some(t) = self.lib.get(id) {
            return Some(RowInfo {
                title: t.title.clone(),
                artist: t.artist.clone(),
                primary: t.primary_artist().to_string(),
                album: t.album.clone(),
                duration: t.duration,
                added: t.added,
                track_no: t.track_no,
                art: t.art,
                stream: None,
            });
        }
        let tr = self.transient.get(&id)?;
        let tags = crate::ytdlp::clean_meta(&serde_json::Value::Null, &tr.item);
        Some(RowInfo {
            title: tags.title,
            primary: tags.artist.clone(),
            artist: tags.artist,
            album: tags.album,
            duration: tr.item.duration,
            added: 0,
            track_no: 0,
            art: tr.art,
            stream: Some(tr.item.clone()),
        })
    }

    // -----------------------------------------------------------------------------------------
    // Song context menu

    pub fn song_menu(&mut self, ui: &mut Ui, id: u64, playlist: Option<(u64, usize)>) {
        ui.set_max_width(MENU_W);
        let in_lib = self.lib.get(id).is_some();
        // A song that plays from its site (not downloaded).
        let stream = if in_lib { None } else { self.transient.get(&id).map(|t| t.item.clone()) };
        if menu_item(ui, icons::LIST_START, "Play next").clicked() {
            self.actions.push(Action::PlayNext(vec![id]));
            ui.close();
        }
        if menu_item(ui, icons::LIST_END, "Add to queue").clicked() {
            self.actions.push(Action::AddToQueue(vec![id]));
            ui.close();
        }
        menu_sep(ui);
        if let Some(item) = &stream {
            let busy = self.yt.shared.lk().jobs.iter().any(|j| {
                j.item.id == item.id && matches!(j.status, JobStatus::Waiting | JobStatus::Downloading { .. } | JobStatus::Saving)
            });
            if !busy && menu_item(ui, icons::ARROW_DOWN_TO_LINE, "Download").clicked() {
                self.actions.push(Action::DownloadStream(id));
                ui.close();
            }
        } else {
            let loved = self.loved.contains(&id);
            if menu_item(ui, icons::HEART, if loved { "Remove from Loved songs" } else { "Love" }).clicked() {
                self.actions.push(Action::ToggleLove(id));
                ui.close();
            }
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
        if let Some(item) = &stream {
            if menu_item(ui, icons::LINK, &format!("Open on {}", item.source())).clicked() {
                self.actions.push(Action::OpenUrl(item.page()));
                ui.close();
            }
        } else if menu_item(ui, icons::FOLDER_OPEN, "Show in folder").clicked() {
            self.actions.push(Action::Reveal(id));
            ui.close();
        }
        if playlist.is_some() || in_lib {
            menu_sep(ui);
        }
        if let Some((pid, index)) = playlist {
            if menu_item(ui, icons::X, "Remove from this playlist").clicked() {
                self.actions.push(Action::RemoveFromPlaylist { pid, index });
                ui.close();
            }
        }
        if in_lib && menu_item_danger(ui, icons::TRASH, "Delete from disk").clicked() {
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
            Route::Playlist(pid) => Some(self.playlist_art(*pid).1),
            Route::Loved => return Some(Color32::from_rgb(0x5A, 0x1E, 0x33)),
            Route::NowPlaying => Some(self.now_info().and_then(|t| t.color)),
            Route::Songs | Route::Albums | Route::Artists => return Some(Color32::from_rgb(0x24, 0x22, 0x2E)),
            Route::ForYou => return Some(Color32::from_rgb(0x2C, 0x1F, 0x4A)),
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

        self.sticky_cols = false;
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
            Route::ForYou => self.for_you_page(&mut child),
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
            // A soft shadow so the list reads as passing under the bar, not cut off by it.
            // (Pinned column titles draw their own, lower down.)
            if !self.sticky_cols {
                let sh = Rect::from_min_size(bar.left_bottom(), vec2(bar.width(), 10.0));
                w::vgradient(ui.painter(), sh, 0.0, &[(0.0, with_alpha(Color32::BLACK, 0.28 * solid)), (1.0, with_alpha(Color32::BLACK, 0.0))]);
            }
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
    pub(crate) fn list_page(
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
        let c = song_cols(left, right, mode);
        w::text(p, pos2(c.num, y), Align2::CENTER_CENTER, "#", small(), TEXT_3, 30.0);
        w::text(p, pos2(c.art.unwrap_or(c.text), y), Align2::LEFT_CENTER, "Title", small(), TEXT_3, 200.0);
        if let Some((x, w)) = c.album {
            w::text(p, pos2(x, y), Align2::LEFT_CENTER, "Album", small(), TEXT_3, w);
        }
        if let Some((x, w)) = c.added {
            w::text(p, pos2(x, y), Align2::LEFT_CENTER, "Date added", small(), TEXT_3, w);
        }
        // The clock's right edge lines up with the durations.
        icons::glyph(p, pos2(c.dur_right - 7.0, y), icons::CLOCK, 14.0, TEXT_3);
        p.line_segment([pos2(left, rect.bottom() - 0.5), pos2(right, rect.bottom() - 0.5)], Stroke::new(1.0, HAIRLINE));
    }

    /// Once the column titles scroll up to the top bar, pin them under it.
    fn sticky_columns(&mut self, ui: &Ui, off: f32, header_h: f32, mode: RowMode) {
        if off < header_h - 32.0 {
            return;
        }
        let page = ui.max_rect();
        let r = Rect::from_min_size(pos2(page.left(), page.top() + TOPBAR_H), vec2(page.width(), 36.0));
        let c = self.page_wash().map(|c| lerp_color(c, PANEL, 0.35)).unwrap_or(PANEL);
        ui.painter().rect_filled(r, 0.0, c);
        self.column_header(ui, r, mode);
        let sh = Rect::from_min_size(r.left_bottom(), vec2(r.width(), 10.0));
        w::vgradient(ui.painter(), sh, 0.0, &[(0.0, with_alpha(Color32::BLACK, 0.28)), (1.0, with_alpha(Color32::BLACK, 0.0))]);
        self.sticky_cols = true;
    }

    /// Whether enough songs in a list have an album name to make the Album column worth showing.
    pub fn list_has_albums(&self, ids: &[u64]) -> bool {
        if ids.is_empty() {
            return false;
        }
        let step = (ids.len() / 300).max(1);
        let (mut seen, mut with) = (0usize, 0usize);
        for id in ids.iter().step_by(step) {
            seen += 1;
            if self.lib.get(*id).map(|t| !t.album.is_empty()).unwrap_or(false) {
                with += 1;
            }
        }
        with * 4 >= seen
    }

    /// One song row. `list` is what plays (in order) when the row is played.
    fn track_row(&mut self, ui: &mut Ui, rect: Rect, list: &Arc<Vec<u64>>, i: usize, mode: RowMode, pl_index: Option<usize>) {
        let Some(&id) = list.get(i) else { return };
        let Some(t) = self.row_info(id) else { return };
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
        let menu_open = self.song_menu_open_for(ui, id);
        let hovering = resp.hovered() || h > 0.5 || menu_open;
        // How far the hover controls have faded in (they cross-fade, never pop).
        let hv = if menu_open { 1.0 } else { h };

        let cols = song_cols(rect.left(), rect.right(), mode);
        // Number / play column: the number fades out as the play button fades in.
        let num_c = pos2(cols.num, y);
        let mut play_hit = false;
        let num_r = Rect::from_center_size(num_c, vec2(28.0, 28.0));
        if hv > 0.01 {
            let pr = ui.interact(num_r, resp.id.with("play"), if hovering { Sense::click() } else { Sense::hover() });
            let (ph, _) = w::hover_press(ui, &pr);
            let c = with_alpha(lerp_color(TEXT, ACCENT, ph), hv);
            if is_cur && self.ps.playing {
                icons::pause(&p, num_c, 12.0, c);
            } else {
                icons::play(&p, num_c, 12.0, c);
            }
            if hovering && pr.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                play_hit = true;
            }
        }
        if hv < 0.99 {
            let a = 1.0 - hv;
            if is_cur && self.ps.waiting {
                w::spinner(ui, num_c, 7.0, with_alpha(ACCENT, a));
            } else if is_cur && self.ps.playing {
                icons::now_playing_bars(&p, num_c, 13.0, with_alpha(ACCENT, a));
            } else if mode.numbers {
                let n = if mode.track_numbers && t.track_no > 0 { t.track_no as usize } else { i + 1 };
                let c = if is_cur { ACCENT } else { TEXT_2 };
                w::text(&p, num_c, Align2::CENTER_CENTER, &n.to_string(), body(), with_alpha(c, a), 40.0);
            }
        }

        // Art (a song that plays from its site shows its thumbnail until it has a cover)
        if let Some(ax) = cols.art {
            let ar = Rect::from_min_size(pos2(ax, y - 20.0), vec2(40.0, 40.0));
            match (&t.art, &t.stream) {
                (None, Some(item)) => {
                    let tex = item.thumb_url().and_then(|u| self.art.get(ArtKey::Url(u)));
                    w::thumb_square(&p, ar, tex, R_SM);
                }
                _ => {
                    let tex = self.art.cover(t.art, ArtSize::S);
                    w::art(&p, ar, tex, R_SM, Color32::WHITE);
                }
            }
        }
        let x = cols.text;

        let heart_c = pos2(cols.heart, y);
        let more_c = pos2(cols.more, y);
        let title_w = cols.title_w;
        let tc = if is_cur { ACCENT } else { TEXT };
        // On an album or artist page the artist is already in the header; only name it on rows
        // by someone else.
        let show_artist = match mode.one_artist {
            Some(k) => library::artist_key(t.primary_artist()) != k,
            None => true,
        };
        if show_artist {
            w::text(&p, pos2(x, y - 9.0), Align2::LEFT_CENTER, &t.title, body(), tc, title_w);
            let artist = if t.artist.is_empty() { "Unknown artist" } else { t.artist.as_str() };
            w::text(&p, pos2(x, y + 10.0), Align2::LEFT_CENTER, artist, small(), TEXT_2, title_w);
        } else {
            w::text(&p, pos2(x, y), Align2::LEFT_CENTER, &t.title, body(), tc, title_w);
        }
        if let Some((ax, aw)) = cols.album {
            w::text(&p, pos2(ax, y), Align2::LEFT_CENTER, &t.album, small(), TEXT_2, aw);
        }
        if let Some((dx, dw)) = cols.added {
            let when = if t.stream.is_some() { "Not downloaded".to_string() } else { when_added(t.added) };
            w::text(&p, pos2(dx, y), Align2::LEFT_CENTER, &when, small(), TEXT_2, dw);
        }
        w::text(&p, pos2(cols.dur_right, y), Align2::RIGHT_CENTER, &fmt_time(t.duration), small(), TEXT_2, 60.0);

        // Love (always visible when loved, otherwise on hover) and more, fading in with the hover.
        // A song that plays from its site has a download button there instead, always shown, so
        // you can tell at a glance what's on your computer.
        if let Some(item) = &t.stream {
            let lr = Rect::from_center_size(heart_c, Vec2::splat(30.0));
            let lresp = ui.interact(lr, resp.id.with("save"), Sense::click());
            self.save_preview_button(ui, lr, lresp, item);
        } else {
            let loved = self.loved.contains(&id);
            let mut fade = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("hover-controls"));
            fade.multiply_opacity(if loved { 1.0 } else { hv });
            if loved || hv > 0.01 {
                let lr = Rect::from_center_size(heart_c, Vec2::splat(30.0));
                let lresp = fade.interact(lr, resp.id.with("love"), if loved || hovering { Sense::click() } else { Sense::hover() });
                if self.love_button(&fade, lr, lresp, id).clicked() {
                    self.actions.push(Action::ToggleLove(id));
                }
            }
        }
        let mut fade2 = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("hover-more"));
        fade2.multiply_opacity(hv);
        let mut menu_open_at: Option<egui::Pos2> = None;
        if hv > 0.01 {
            let mr = Rect::from_center_size(more_c, Vec2::splat(30.0));
            let mresp = fade2.interact(mr, resp.id.with("more"), if hovering { Sense::click() } else { Sense::hover() });
            let mresp = w::icon_button_at(&fade2, mr, mresp, icons::ELLIPSIS, false, "More");
            if hovering && mresp.clicked() {
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
        let mode = RowMode { reorder: true, added: true, album_col: self.list_has_albums(&ids), ..RowMode::list() };
        let total = self.lib.total_duration(&ids);
        let n = ids.len();
        let scanning = self.lib.scanning;
        let list = ids.clone();
        let off = self.list_page(
            ui,
            "songs",
            188.0,
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
                // The same play and shuffle buttons as album and playlist pages.
                let br = Rect::from_min_size(pos2(r.left() + PAD_X, r.top() + 80.0), vec2(r.width() - 2.0 * PAD_X, 56.0));
                let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(egui::Layout::left_to_right(egui::Align::Center)));
                bu.spacing_mut().item_spacing.x = S4;
                app.play_buttons(&mut bu, &list);
                bu.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    app.sort_menu(ui);
                });
                let ch = Rect::from_min_size(pos2(r.left(), r.bottom() - 32.0), vec2(r.width(), 32.0));
                app.column_header(ui, ch, mode);
            },
            |app, ui, r, i| app.track_row(ui, r, &ids, i, mode, None),
        );
        self.sticky_columns(ui, off, 188.0, mode);
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

    pub(crate) fn grid_metrics(width: f32, min_card: f32) -> (usize, f32) {
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
        let (h, pr) = w::group_hover_press(ui, &resp, rect);
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
        // Playlists: click the picture to choose your own.
        if let Some(pid) = playlist {
            let resp = ui.interact(ar, Id::new(("cover", pid)), Sense::click());
            let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), T_FAST);
            if h > 0.0 {
                p.rect_filled(ar, radius, with_alpha(Color32::BLACK, 0.55 * h));
                icons::glyph(&p, ar.center() - vec2(0.0, 12.0), icons::IMAGE, 30.0, with_alpha(TEXT, h));
                w::text(&p, ar.center() + vec2(0.0, 22.0), Align2::CENTER_TOP, "Choose picture", body_strong(), with_alpha(TEXT, h), size);
            }
            if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                self.actions.push(Action::PickPlaylistCover(pid));
            }
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
        if !ids.is_empty() {
            self.play_buttons(&mut bu, ids);
        }
        if let Some(pid) = playlist {
            let add = w::icon_button(&mut bu, 40.0, icons::PLUS, false, "Add songs");
            if add.clicked() {
                self.add_query.clear();
                self.add_focus = true;
            }
            let have: std::collections::HashSet<u64> = ids.iter().copied().collect();
            self.add_songs_popup(&add, crate::queue::AddTo::Playlist(pid), &have);
            let more = w::icon_button(&mut bu, 40.0, icons::ELLIPSIS, false, "More");
            // Songs that still play from their site: one button keeps them all.
            let waiting = {
                let busy: std::collections::HashSet<String> = self
                    .yt
                    .shared
                    .lk()
                    .jobs
                    .iter()
                    .filter(|j| matches!(j.status, JobStatus::Waiting | JobStatus::Downloading { .. } | JobStatus::Saving))
                    .map(|j| j.item.id.clone())
                    .collect();
                self.playlist(pid)
                    .map(|p| p.tracks.iter().filter_map(|t| store::stream_key(t)).filter(|k| !busy.contains(*k)).count())
                    .unwrap_or(0)
            };
            if waiting > 0 {
                bu.add_space(S1);
                let label = format!("Download {waiting}");
                if w::pill(&mut bu, &label, Some(icons::ARROW_DOWN_TO_LINE), PillKind::Secondary)
                    .on_hover_text("Keep these songs on your computer so they play without internet")
                    .clicked()
                {
                    self.actions.push(Action::DownloadPlaylist(pid));
                }
            }
            egui::Popup::menu(&more).show(|ui| {
                ui.set_max_width(MENU_W);
                if menu_item(ui, icons::PENCIL, "Rename").clicked() {
                    self.actions.push(Action::StartRename(pid));
                    ui.close();
                }
                if menu_item(ui, icons::IMAGE, "Choose picture…").clicked() {
                    self.actions.push(Action::PickPlaylistCover(pid));
                    ui.close();
                }
                if art.is_some() && menu_item(ui, icons::X, "Remove picture").clicked() {
                    self.actions.push(Action::RemovePlaylistCover(pid));
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

    /// The big play button and shuffle, used at the top of every list of songs.
    fn play_buttons(&mut self, ui: &mut Ui, ids: &Arc<Vec<u64>>) {
        // Pause shows only while the queue came from this list.
        let playing_this = self.ps.playing
            && self.ps.queue.len() == ids.len()
            && self.current_id().map(|c| ids.contains(&c)).unwrap_or(false);
        if w::play_circle(ui, 56.0, playing_this, false).on_hover_text(if playing_this { "Pause" } else { "Play" }).clicked() {
            if playing_this {
                self.actions.push(Action::Toggle);
            } else {
                self.actions.push(Action::PlayIds { ids: ids.to_vec(), start: 0, shuffle: Some(false) });
            }
        }
        if w::icon_button(ui, 40.0, icons::SHUFFLE, false, "Shuffle play").clicked() {
            self.actions.push(Action::PlayIds { ids: ids.to_vec(), start: 0, shuffle: Some(true) });
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
        let mode = RowMode { art: false, album_col: false, track_numbers: true, one_artist: Some(artist_key), ..RowMode::list() };
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
        self.sticky_columns(ui, off, 352.0, mode);
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
        let mode = RowMode { one_artist: Some(key), album_col: self.list_has_albums(&ids), ..RowMode::list() };
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
        self.sticky_columns(ui, off, 352.0, mode);
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
        // Songs that play from their site count too.
        let stream_len: f32 = ids.iter().filter(|i| self.lib.get(**i).is_none()).filter_map(|i| self.transient.get(i)).map(|t| t.item.duration).sum();
        let mut meta = theme::songs_and_length(ids.len(), self.lib.total_duration(&ids) + stream_len);
        let not_downloaded = ids.iter().filter(|i| self.lib.get(**i).is_none()).count();
        // An imported playlist still filling up.
        let (downloading, failed) = match pid {
            Some(id) => {
                let sh = self.yt.shared.lk();
                let mine = sh.jobs.iter().filter(|j| j.playlist == Some(id));
                let (a, f) = mine.fold((0, 0), |(a, f), j| match j.status {
                    JobStatus::Waiting | JobStatus::Downloading { .. } | JobStatus::Saving => (a + 1, f),
                    JobStatus::Failed(_) => (a, f + 1),
                    _ => (a, f),
                });
                (a, f)
            }
            None => (0, 0),
        };
        if downloading > 0 {
            meta = format!("{meta} · downloading {downloading}");
        } else if not_downloaded > 0 {
            meta = format!("{meta} · {not_downloaded} not downloaded");
        }
        let art = pid.and_then(|id| self.playlist(id)).and_then(|p| p.cover);
        let mode = RowMode { playlist: pid, album_col: self.list_has_albums(&ids), ..RowMode::list() };
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
                if failed > 0 {
                    if let Some(id) = pid {
                        // Songs that didn't download: say so next to the play buttons, with a retry.
                        let label = format!("{} didn't download · Try again", theme::plural(failed, "song", "songs"));
                        let lw = w::text_width(ui, &label, body_strong()) + 52.0;
                        let br = Rect::from_min_size(pos2(r.right() - PAD_X - lw, hr.top() + 200.0f32.min(r.width() * 0.3) + S6 + 10.0), vec2(lw, 36.0));
                        let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(br));
                        if w::pill(&mut bu, &label, Some(icons::REFRESH), PillKind::Ghost).clicked() {
                            app.actions.push(Action::RetryPlaylist(id));
                        }
                    }
                }
                if empty && downloading > 0 {
                    let er = Rect::from_min_max(pos2(r.left(), hr.bottom() - 40.0), r.max);
                    let mut eu = ui.new_child(egui::UiBuilder::new().max_rect(er).layout(egui::Layout::top_down(egui::Align::Center)));
                    let body = "Songs show up here as they finish downloading. You can leave this page; it keeps going.";
                    if w::empty_state(&mut eu, icons::DOWNLOAD, "Downloading your playlist", body, Some(("See downloads", None))) {
                        app.actions.push(Action::Go(Route::Downloads));
                    }
                } else if empty {
                    let er = Rect::from_min_max(pos2(r.left(), hr.bottom() - 40.0), r.max);
                    let mut eu = ui.new_child(egui::UiBuilder::new().max_rect(er).layout(egui::Layout::top_down(egui::Align::Center)));
                    let (t, b) = if pid.is_some() {
                        ("This playlist is empty", "Press + above to find songs, or drag songs onto the playlist in the sidebar.")
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
        if !empty {
            self.sticky_columns(ui, off, header_h, mode);
        }
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
                let mode = RowMode { album_col: false, numbers: false, ..RowMode::list() };
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
        let link_site = if is_link { crate::ytdlp::site_from_url(q) } else { "YouTube".into() };
        self.section_title(ui, &if is_link { format!("From {link_site}") } else { "YouTube".to_string() });
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
                let head = if is_link { "Couldn't open this link" } else { "Couldn't search YouTube" };
                w::text(p, pos2(r.left() + 48.0, r.top() + 22.0), Align2::LEFT_CENTER, head, body_strong(), TEXT, wdt - 200.0);
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
                    self.link_playlist_card(ui, pl, &items);
                }
                for (i, item) in items.iter().enumerate() {
                    self.yt_row(ui, item, i);
                }
            }
        }
    }

    /// The header for a playlist or album link: its picture, name, and the two ways to keep it.
    fn link_playlist_card(&mut self, ui: &mut Ui, pl: &crate::ytdlp::PlaylistInfo, items: &[YtItem]) {
        let wdt = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(wdt, 104.0), Sense::hover());
        let p = ui.painter().clone();
        p.rect_filled(r, R_LG, RAISED);
        let ar = Rect::from_min_size(pos2(r.left() + S4, r.top() + S4), Vec2::splat(72.0));
        let tex = (!pl.cover.is_empty()).then(|| self.art.get(ArtKey::Url(pl.cover.clone()))).flatten();
        if tex.is_some() {
            w::art(&p, ar, tex, R_SM + 2.0, Color32::WHITE);
        } else {
            p.rect_filled(ar, R_SM + 2.0, RAISED_2);
            icons::glyph(&p, ar.center(), icons::LIST_MUSIC, 26.0, TEXT_2);
        }
        let saved = self
            .playlists
            .iter()
            .find(|x| x.source.as_ref().map(|s| s.url == pl.url).unwrap_or(false))
            .map(|x| x.id);
        let (active, have): (Vec<String>, usize) = {
            let sh = self.yt.shared.lk();
            (
                sh.jobs
                    .iter()
                    .filter(|j| matches!(j.status, JobStatus::Waiting | JobStatus::Downloading { .. } | JobStatus::Saving))
                    .map(|j| j.item.id.clone())
                    .collect(),
                items.iter().filter(|i| self.lib.yt_ids.contains(&i.id)).count(),
            )
        };
        let need: Vec<YtItem> = items.iter().filter(|i| !self.lib.yt_ids.contains(&i.id) && !active.contains(&i.id)).cloned().collect();

        // Buttons first, so the text knows how much room it has.
        let save_label = if saved.is_some() { "Saved" } else { "Save playlist" };
        let dl_label = if !need.is_empty() {
            if have == 0 { format!("Download all {}", items.len()) } else { format!("Download {} more", need.len()) }
        } else if items.iter().any(|i| active.contains(&i.id)) {
            "Downloading…".to_string()
        } else {
            "All downloaded".to_string()
        };
        let sw = w::text_width(ui, save_label, body_strong()) + 60.0;
        let dw = w::text_width(ui, &dl_label, body_strong()) + 60.0;
        let by = r.center().y - 18.0;
        let save_r = Rect::from_min_size(pos2(r.right() - S4 - sw, by), vec2(sw, 36.0));
        let dl_r = Rect::from_min_size(pos2(save_r.left() - S2 - dw, by), vec2(dw, 36.0));
        let mut su = ui.new_child(egui::UiBuilder::new().max_rect(save_r));
        let save_icon = if saved.is_some() { icons::CHECK } else { icons::LIST_PLUS };
        let kind = if saved.is_some() { PillKind::Secondary } else { PillKind::Primary };
        let save_tip = if saved.is_some() {
            "Open it in your playlists".to_string()
        } else {
            "Adds it to your playlists. The songs play online until you download them.".to_string()
        };
        let sresp = w::pill(&mut su, save_label, Some(save_icon), kind).on_hover_text(save_tip);
        if sresp.clicked() {
            match saved {
                Some(pid) => self.actions.push(Action::Go(Route::Playlist(pid))),
                None => self.actions.push(Action::SavePlaylist { info: pl.clone(), items: items.to_vec() }),
            }
        }
        let mut du = ui.new_child(egui::UiBuilder::new().max_rect(dl_r));
        let dresp = w::pill(&mut du, &dl_label, Some(icons::DOWNLOAD), PillKind::Secondary)
            .on_hover_text("Downloads the songs without making a playlist");
        if dresp.clicked() && !need.is_empty() {
            self.actions.push(Action::YtDownload { items: need.clone(), playlist_name: None });
        }

        let tx = ar.right() + S4;
        let tw = dl_r.left() - S4 - tx;
        let kind = if pl.site == "Spotify" && items.len() > 1 && pl.channel.is_empty() { "Album" } else { "Playlist" };
        w::text(&p, pos2(tx, r.center().y - 20.0), Align2::LEFT_CENTER, &format!("{} {}", pl.site, kind.to_lowercase()), small_strong(), TEXT_2, tw);
        w::text(&p, pos2(tx, r.center().y), Align2::LEFT_CENTER, &pl.title, FontId::new(18.0, bold()), TEXT, tw);
        let mut sub = theme::plural(items.len(), "song", "songs");
        if !pl.channel.is_empty() {
            sub = format!("{} · {sub}", pl.channel);
        }
        if have > 0 {
            sub = format!("{sub} · {have} in your library");
        }
        w::text(&p, pos2(tx, r.center().y + 21.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, tw);
        ui.add_space(S3);
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
        let tex = item.thumb_url().and_then(|u| self.art.get(ArtKey::Url(u)));
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
        let mut sub = item.channel.clone();
        if !item.is_youtube() {
            sub = if sub.is_empty() { item.source().to_string() } else { format!("{} · {sub}", item.source()) };
        }
        if playing_this && !saved {
            sub = format!("{sub} · Playing");
        }
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

    pub(crate) fn play_yt(&mut self, yt_id: &str, job: Option<&crate::ytdlp::Job>) {
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
                if !jobs.is_empty() {
                    let meta = if active > 0 { format!("{} downloading", group_digits(active)) } else { "All done".into() };
                    let (r, _) = ui2.allocate_exact_size(vec2(ui2.available_width(), 18.0), Sense::hover());
                    w::text(ui2.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &meta, small(), TEXT_2, r.width());
                    ui2.add_space(S4);
                    ui2.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = S2;
                        if w::pill(ui, "Open music folder", Some(icons::FOLDER_OPEN), PillKind::Secondary).clicked() {
                            self.actions.push(Action::OpenFolder);
                        }
                        if finished > 0 && w::pill(ui, "Clear finished", None, PillKind::Ghost).clicked() {
                            self.actions.push(Action::YtClearFinished);
                        }
                    });
                }
                ui2.add_space(S5);
                self.setup_banner(ui2);
                if jobs.is_empty()
                    && w::empty_state(
                        ui2,
                        icons::DOWNLOAD,
                        "Nothing downloaded yet",
                        "Search for a song or paste a YouTube link, then press the download button next to it.",
                        Some(("Search YouTube", Some(icons::SEARCH))),
                    )
                {
                    self.actions.push(Action::Go(Route::Search));
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
        let tex = j.item.thumb_url().and_then(|u| self.art.get(ArtKey::Url(u)));
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
                    ui.add_space(S3);
                    let mut spin = app.settings.spin_record;
                    if toggle_row(ui, "Spin the record on Now Playing", "Turn off to save a little battery while that screen is open.", &mut spin) {
                        app.settings.spin_record = spin;
                        app.settings.save();
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
                    (_, Some(v)) => format!("yt-dlp {v} · saves songs with cover art"),
                    _ => "Sets itself up the first time you search or download.".into(),
                };
                self.settings_card(ui2, "Downloads from YouTube and other sites", &status, |app, ui| {
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

                // Updates (the Store version gets them from the Store)
                if crate::update::STORE {
                    let status = format!("You're on Segno {}. Updates come from the Microsoft Store.", crate::update::VERSION);
                    self.settings_card(ui2, "Updates", &status, |_app, _ui| {});
                } else {
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
                        if toggle_row(ui, "Update automatically", "Checks every few hours and downloads new versions in the background.", &mut auto) {
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
