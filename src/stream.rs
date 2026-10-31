//! Songs that play from their site (YouTube, SoundCloud…) without being downloaded: in saved
//! playlists, search results and suggestions. The audio thread asks for each one's file just
//! before it plays (and the next one while the current plays), so they play like files.

use crate::app::{Action, App, Transient};
use crate::player::{Cmd, QItem};
use crate::store::{self, stream_entry, stream_id, stream_key, LockExt};
use crate::ytdlp::{self, YtItem};
use std::collections::HashSet;
use std::path::PathBuf;

impl App {
    /// Remember a song that plays from its site, so lists, the queue and the player bar can
    /// show it. Returns its track id.
    pub(crate) fn register_stream(&mut self, item: YtItem) -> u64 {
        let id = stream_id(&item.id);
        self.stream_reg.lk().insert(id, item.clone());
        let entry = self.transient.entry(id).or_insert(Transient { item: item.clone(), art: None, color: None, path: None });
        // Keep the newest details (a search result can be richer than a saved copy).
        if !item.title.is_empty() {
            entry.item = item;
        }
        id
    }

    /// Every playlist's streamed songs, registered at startup and after a playlist changes.
    pub(crate) fn register_playlist_streams(&mut self) {
        let items: Vec<YtItem> = self.playlists.iter().flat_map(|p| p.streams.values().cloned()).collect();
        for it in items {
            self.register_stream(it);
        }
    }

    /// A queue entry for a streamed song: its cached file if there is one, else a placeholder
    /// the player waits on.
    pub(crate) fn stream_qitem(&self, id: u64) -> Option<QItem> {
        let tr = self.transient.get(&id)?;
        let path = tr
            .path
            .clone()
            .filter(|p| p.is_file())
            .or_else(|| ytdlp::cached(&tr.item))
            .unwrap_or_else(|| stream_entry(&tr.item.id));
        Some(QItem::stream(id, path))
    }

    pub(crate) fn is_stream(&self, id: u64) -> bool {
        self.lib.get(id).is_none() && self.transient.contains_key(&id)
    }

    /// Play one song from its site right away (a search result or a suggestion).
    pub(crate) fn play_stream(&mut self, item: YtItem) {
        let id = self.register_stream(item);
        if let Some(q) = self.stream_qitem(id) {
            self.player.send(Cmd::PlayList { items: vec![q], start: 0, shuffle: None });
        }
    }

    pub(crate) fn on_fetched(&mut self, item: YtItem, path: PathBuf, art: Option<(u64, [u8; 3])>) {
        let id = self.register_stream(item);
        if let Some(tr) = self.transient.get_mut(&id) {
            tr.path = Some(path);
            if art.is_some() {
                tr.art = art.map(|a| a.0);
                tr.color = art.map(|a| a.1);
            }
        }
    }

    pub(crate) fn on_fetch_failed(&mut self, item: YtItem, msg: String) {
        // The audio thread already moved on; say why if it was the song you were waiting for.
        let id = stream_id(&item.id);
        if self.current_id() == Some(id) {
            let name = self.info_for(id).map(|t| t.title).unwrap_or_default();
            self.toast(format!("Couldn't play “{name}”: {msg}"));
        }
    }

    /// Streamed songs that are now in the library (downloaded here or found in the folder)
    /// become the files themselves, in the same place in every playlist.
    pub(crate) fn resolve_streams(&mut self) {
        let mut changed = false;
        for p in self.playlists.iter_mut() {
            if p.streams.is_empty() {
                continue;
            }
            for entry in p.tracks.iter_mut() {
                let Some(key) = stream_key(entry).map(str::to_string) else { continue };
                if let Some(t) = self.lib.by_key.get(&key).and_then(|id| self.lib.get(*id)) {
                    *entry = t.path.clone();
                    p.streams.remove(&key);
                    changed = true;
                }
            }
        }
        if changed {
            store::save_playlists(&self.playlists);
        }
    }

    /// A download finished: its streamed entries become the file right away (the library
    /// catches up on its next scan).
    pub(crate) fn stream_downloaded(&mut self, key: &str, path: &std::path::Path) {
        let entry = stream_entry(key);
        let mut changed = false;
        for p in self.playlists.iter_mut() {
            if let Some(i) = p.tracks.iter().position(|t| *t == entry) {
                if p.tracks.contains(&path.to_path_buf()) {
                    p.tracks.remove(i);
                } else {
                    p.tracks[i] = path.to_path_buf();
                }
                p.streams.remove(key);
                changed = true;
            }
        }
        if changed {
            store::save_playlists(&self.playlists);
        }
    }

    /// The songs of a playlist that aren't downloaded yet (in playlist order).
    pub(crate) fn playlist_streams(&self, pid: u64) -> Vec<YtItem> {
        let Some(p) = self.playlist(pid) else { return Vec::new() };
        p.tracks.iter().filter_map(|t| stream_key(t).and_then(|k| p.streams.get(k)).cloned()).collect()
    }

    /// Download every streamed song of a playlist; each takes its file's place when done.
    pub(crate) fn download_playlist(&mut self, pid: u64) {
        let busy: HashSet<String> = self
            .yt
            .shared
            .lock()
            .map(|s| {
                s.jobs
                    .iter()
                    .filter(|j| matches!(j.status, ytdlp::JobStatus::Waiting | ytdlp::JobStatus::Downloading { .. } | ytdlp::JobStatus::Saving))
                    .map(|j| j.item.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let need: Vec<YtItem> = self.playlist_streams(pid).into_iter().filter(|i| !busy.contains(&i.id)).collect();
        if need.is_empty() {
            return;
        }
        let n = need.len();
        self.yt_requested = true;
        self.yt.download(need, Some(pid));
        self.toast(format!("Downloading {}", crate::theme::plural(n, "song", "songs")));
    }

    /// Download one streamed song (from a row, its menu, or the player bar).
    pub(crate) fn download_stream(&mut self, id: u64) {
        if let Some(tr) = self.transient.get(&id) {
            let item = tr.item.clone();
            self.actions.push(Action::YtDownload { items: vec![item], playlist_name: None });
        }
    }
}

// -------------------------------------------------------------------------------------------
// "Wrong song? Pick another": choose the YouTube video a Spotify song plays

/// YouTube's results for the picker, filled in by a background search.
type Results = std::sync::Arc<std::sync::Mutex<Option<Result<Vec<YtItem>, String>>>>;

/// The open "Pick another" window.
pub(crate) struct Picker {
    /// The song (its track id) and what it is on Spotify.
    id: u64,
    item: YtItem,
    /// YouTube's results, once they arrive.
    results: Results,
}

impl App {
    /// Open "Pick another" for a Spotify song and look its name up on YouTube.
    pub(crate) fn open_picker(&mut self, id: u64) {
        let Some(item) = self.transient.get(&id).map(|t| t.item.clone()).filter(|i| !i.find.is_empty()) else { return };
        let results: Results = std::sync::Arc::new(std::sync::Mutex::new(None));
        let (slot, resolver, ctx) = (results.clone(), self.yt.resolver(), self.ctx.clone());
        // Wider than the first guess: no "audio", so live and other uploads show too.
        let words = item.find.trim_end_matches(" audio").to_string();
        std::thread::spawn(move || {
            let found = resolver.search(&words);
            *slot.lk() = Some(found);
            ctx.request_repaint();
        });
        self.picker = Some(Picker { id, item, results });
    }

    /// Play this video for the song from now on: everywhere the song is saved, and right away
    /// if it's playing.
    pub(crate) fn choose_video(&mut self, id: u64, video: String) {
        let Some(key) = self.transient.get(&id).map(|t| t.item.id.clone()) else { return };
        let mut changed = false;
        for p in &mut self.playlists {
            if let Some(it) = p.streams.get_mut(&key) {
                it.video = video.clone();
                changed = true;
            }
        }
        if changed {
            store::save_playlists(&self.playlists);
        }
        if let Some(t) = self.transient.get_mut(&id) {
            t.item.video = video.clone();
            t.path = None;
            self.stream_reg.lk().insert(id, t.item.clone());
        }
        // Every queue entry waits for the new file; the song playing starts over with it.
        self.player.send(Cmd::Fetched { id, path: stream_entry(&key) });
        if self.current_id() == Some(id) {
            self.player.send(Cmd::JumpTo(self.ps.index));
        }
        self.toast("Got it: this song plays the one you picked from now on");
    }

    pub(crate) fn picker_ui(&mut self, ctx: &egui::Context) {
        use crate::art::ArtKey;
        use crate::theme::*;
        use crate::widgets::{self as w, PillKind};
        use egui::{pos2, vec2, Align2, CursorIcon, Id, Rect, Sense};
        let Some(pk) = self.picker.as_ref() else { return };
        let (id, item, results) = (pk.id, pk.item.clone(), pk.results.lk().clone());
        let current = ytdlp::current_video(&item);
        let mut chosen: Option<String> = None;
        let mut close = false;
        let modal = egui::Modal::new(Id::new("pick-video"))
            .frame(egui::Frame::new().fill(OVERLAY).corner_radius(R_LG).inner_margin(S6).stroke(egui::Stroke::new(1.0, HAIRLINE)))
            .show(ctx, |ui| {
                let wdt = 560.0;
                ui.set_width(wdt);
                ui.label(egui::RichText::new("Pick the right song").font(h2()).color(TEXT));
                ui.add_space(S1);
                let len = if item.duration > 0.0 { format!(" · {}", fmt_time(item.duration)) } else { String::new() };
                ui.label(egui::RichText::new(format!("“{}” by {}{len} on Spotify", item.title, item.channel)).font(body()).color(TEXT_2));
                ui.add_space(S4);
                match &results {
                    None => {
                        let (r, _) = ui.allocate_exact_size(vec2(wdt, 60.0), Sense::hover());
                        w::spinner(ui, pos2(r.left() + 12.0, r.center().y), 9.0, ACCENT);
                        w::text(ui.painter(), pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, "Looking on YouTube…", body(), TEXT_2, wdt);
                        ui.ctx().request_repaint();
                    }
                    Some(Err(e)) => {
                        ui.label(egui::RichText::new(e).font(body()).color(TEXT_2));
                    }
                    Some(Ok(list)) if list.is_empty() => {
                        ui.label(egui::RichText::new("YouTube found nothing for this song.").font(body()).color(TEXT_2));
                    }
                    Some(Ok(list)) => {
                        egui::ScrollArea::vertical().id_salt("pick-video-list").max_height(420.0).show(ui, |ui| {
                            for v in list.iter().filter(|v| v.is_youtube()).take(12) {
                                let (r, resp) = ui.allocate_exact_size(vec2(wdt, 64.0), Sense::click());
                                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                                let (h, _) = w::hover_press(ui, &resp);
                                let p = ui.painter();
                                w::hover_surface(p, r, h);
                                let tr = Rect::from_min_size(pos2(r.left() + S2, r.center().y - 27.0), vec2(96.0, 54.0));
                                let tex = v.thumb_url().and_then(|u| self.art.get(ArtKey::Url(u)));
                                w::thumb(p, tr, tex, R_SM);
                                let tx = tr.right() + S4;
                                let tw = r.right() - tx - S3;
                                w::text(p, pos2(tx, r.center().y - 10.0), Align2::LEFT_CENTER, &v.title, body_strong(), TEXT, tw);
                                let mut sub = v.channel.clone();
                                if v.duration > 0.0 {
                                    sub = format!("{sub} · {}", fmt_time(v.duration));
                                }
                                let same_len = item.duration > 0.0 && v.duration > 0.0 && (item.duration - v.duration).abs() <= 3.0;
                                let note = if current.as_deref() == Some(v.id.as_str()) {
                                    Some(("Playing this one", TEXT))
                                } else if same_len {
                                    Some(("Same length as on Spotify", ACCENT))
                                } else {
                                    None
                                };
                                let sr = w::text(p, pos2(tx, r.center().y + 11.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, tw);
                                if let Some((label, color)) = note {
                                    w::text(p, pos2(sr.right() + S3, r.center().y + 11.0), Align2::LEFT_CENTER, label, small_strong(), color, (tx + tw - sr.right() - S3).max(0.0));
                                }
                                if resp.clicked() {
                                    chosen = Some(v.id.clone());
                                }
                            }
                        });
                    }
                }
                ui.add_space(S4);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w::pill(ui, "Cancel", None, PillKind::Ghost).clicked() {
                        close = true;
                    }
                });
            });
        if modal.should_close() {
            close = true;
        }
        if let Some(video) = chosen {
            self.choose_video(id, video);
            close = true;
        }
        if close {
            self.picker = None;
        }
    }
}
