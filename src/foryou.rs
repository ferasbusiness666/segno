//! "For you": songs like the ones you've been playing, found through YouTube's mixes.

use crate::app::{Action, App, Route};
use crate::art::ArtKey;
use crate::icons;
use crate::store::{self, now_secs, LockExt};
use crate::theme::*;
use crate::views::{menu_item, MENU_W};
use crate::widgets::{self as w, PillKind};
use crate::ytdlp::{self, JobStatus, PreviewState, Seed, Suggest, YtItem};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Picks older than this are refreshed when you open the page.
const STALE_SECS: u64 = 6 * 3600;

/// One song you played (the newest are at the end).
#[derive(Clone, Serialize, Deserialize)]
pub struct Played {
    /// YouTube id, when the song came from YouTube.
    pub yt: Option<String>,
    pub artist: String,
    pub title: String,
    pub at: u64,
}

fn history_path() -> PathBuf {
    store::data_dir().join("history.json")
}

pub fn load_history() -> Vec<Played> {
    store::load_json(&history_path()).unwrap_or_default()
}

impl App {
    /// Remember a song once it actually starts playing. Called every frame; cheap.
    pub(crate) fn note_played(&mut self) {
        if !self.ps.playing {
            return;
        }
        let Some(id) = self.current_id() else { return };
        if self.played_noted == Some(id) {
            return;
        }
        self.played_noted = Some(id);
        let (yt, artist, title) = match self.lib.get(id) {
            Some(t) => (t.yt_id.clone(), t.primary_artist().to_string(), t.title.clone()),
            None => match self.transient.get(&id) {
                Some(tr) => {
                    let (a, t) = ytdlp::guess_artist_title(&tr.item);
                    (Some(tr.item.id.clone()), a, t)
                }
                None => return,
            },
        };
        // Only real YouTube ids can seed a mix.
        let yt = yt.filter(|k| k.len() == 11 && !k.contains(':'));
        if title.is_empty() {
            return;
        }
        self.history.push(Played { yt, artist, title, at: now_secs() });
        if self.history.len() > 300 {
            let extra = self.history.len() - 300;
            self.history.drain(..extra);
        }
        store::save_json(&history_path(), &self.history);
    }

    /// Up to four songs to find more like: from the artists you play most lately (newer plays
    /// count more), with a different mix each time you ask for new picks.
    fn pick_seeds(&self) -> Vec<Seed> {
        let mut score: HashMap<String, f32> = HashMap::new();
        let mut latest: HashMap<String, &Played> = HashMap::new();
        for (rank, p) in self.history.iter().rev().take(150).enumerate() {
            let key = p.artist.to_lowercase();
            *score.entry(key.clone()).or_default() += 1.0 / (1.0 + rank as f32 / 25.0);
            latest.entry(key).or_insert(p);
        }
        let mut seeds: Vec<(f32, Seed)> = latest
            .into_iter()
            .map(|(k, p)| {
                let s = score.get(&k).copied().unwrap_or(0.0);
                let find = if p.artist.is_empty() { p.title.clone() } else { format!("{} {}", p.artist, p.title) };
                (s, Seed { id: p.yt.clone(), find, artist: p.artist.clone() })
            })
            .collect();
        // Not much history yet: loved songs, then the newest in the library.
        if seeds.len() < 4 {
            let have: HashSet<String> = seeds.iter().map(|(_, s)| s.artist.to_lowercase()).collect();
            let mut extra: Vec<&crate::library::Track> = self.lib.tracks.iter().filter(|t| self.loved.contains(&t.id)).collect();
            let mut newest: Vec<&crate::library::Track> = self.lib.tracks.iter().collect();
            newest.sort_by(|a, b| b.added.cmp(&a.added));
            extra.extend(newest.into_iter().take(30));
            let mut taken = have;
            for t in extra {
                let a = t.primary_artist().to_string();
                if seeds.len() >= 6 || !taken.insert(a.to_lowercase()) {
                    continue;
                }
                let yt = t.yt_id.clone().filter(|k| k.len() == 11 && !k.contains(':'));
                let find = if a.is_empty() { t.title.clone() } else { format!("{a} {}", t.title) };
                seeds.push((0.1, Seed { id: yt, find, artist: a }));
            }
        }
        seeds.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut seeds: Vec<Seed> = seeds.into_iter().map(|(_, s)| s).collect();
        // Your top two artists always; the other two rotate through the rest.
        if seeds.len() > 4 {
            let rest: Vec<Seed> = seeds.split_off(2);
            let n = rest.len();
            let start = (self.suggest_turn as usize * 2) % n;
            for k in 0..2.min(n) {
                seeds.push(rest[(start + k) % n].clone());
            }
        }
        seeds
    }

    pub(crate) fn refresh_suggestions(&mut self) {
        let seeds = self.pick_seeds();
        if seeds.is_empty() {
            return;
        }
        self.suggest_turn += 1;
        let mut have_ids: HashSet<String> = self.lib.yt_ids.iter().cloned().collect();
        have_ids.extend(self.yt.shared.lk().jobs.iter().map(|j| j.item.id.clone()));
        let have_keys: HashSet<String> = self.lib.tracks.iter().map(|t| ytdlp::song_key(t.primary_artist(), &t.title)).collect();
        self.yt.suggest(seeds, have_ids, have_keys);
    }

    pub(crate) fn for_you_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let state = self.yt.shared.lk().suggest.clone();
        let can_seed = !self.history.is_empty() || !self.lib.tracks.is_empty();
        let stale = match &state {
            Suggest::Idle => true,
            Suggest::Ready { at, .. } => now_secs().saturating_sub(*at) > STALE_SECS,
            _ => false,
        };
        if stale && can_seed && !self.suggest_auto_tried {
            self.suggest_auto_tried = true;
            self.refresh_suggestions();
        }
        let (items, from, loading, failed): (Vec<YtItem>, Vec<String>, bool, Option<String>) = match state {
            Suggest::Idle => (Vec::new(), Vec::new(), false, None),
            Suggest::Loading { had, from } => (had, from, true, None),
            Suggest::Ready { items, from, .. } => (items, from, false, None),
            Suggest::Failed(m) => (Vec::new(), Vec::new(), false, Some(m)),
        };
        if !can_seed {
            ui.add_space(TOPBAR_H + S8);
            let body = "Play a few songs and Segno finds more like them here.";
            if w::empty_state(ui, icons::SPARKLES, "Nothing to go on yet", body, Some(("Search YouTube", Some(icons::SEARCH)))) {
                self.actions.push(Action::Go(Route::Search));
            }
            return (0.0, String::new());
        }

        let sub = match from.len() {
            0 if loading => "Finding songs you might like…".to_string(),
            0 => "New music based on what you play.".to_string(),
            1 => format!("Because you play {}", from[0]),
            2 => format!("Because you play {} and {}", from[0], from[1]),
            _ => format!("Because you play {}, {} and more", from[0], from[1]),
        };
        let (cols, card) = Self::grid_metrics(ui.available_width(), 168.0);
        let row_h = card + 66.0;
        let n = items.len();
        let rows = n.div_ceil(cols);
        let header_h = 112.0;
        let page = ui.max_rect();
        let off = self.list_page(
            ui,
            "for-you",
            header_h,
            rows,
            row_h,
            None,
            |app, ui, r| {
                let p = ui.painter().clone();
                w::text(&p, pos2(r.left() + PAD_X, r.top() + 22.0), Align2::LEFT_CENTER, "For you", h1(), TEXT, r.width() - 260.0);
                w::text(&p, pos2(r.left() + PAD_X, r.top() + 56.0), Align2::LEFT_CENTER, &sub, body(), TEXT_2, r.width() - 2.0 * PAD_X - 180.0);
                let br = Rect::from_min_size(pos2(r.right() - PAD_X - 136.0, r.top() + 4.0), vec2(136.0, 36.0));
                let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(egui::Layout::right_to_left(egui::Align::Center)));
                if loading {
                    let (sr, _) = bu.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                    w::spinner(&bu, sr.center(), 9.0, TEXT_2);
                } else if w::pill(&mut bu, "New picks", Some(icons::REFRESH), PillKind::Secondary).on_hover_text("Find a fresh set").clicked() {
                    app.suggest_auto_tried = true;
                    app.refresh_suggestions();
                }
            },
            |app, ui, r, row| {
                for c in 0..cols {
                    let i = row * cols + c;
                    if i >= n {
                        break;
                    }
                    let x = r.left() + S2 + c as f32 * (card + S5);
                    let cr = Rect::from_min_size(pos2(x, r.top()), vec2(card, row_h - S2));
                    app.suggest_card(ui, cr, card, &items[i]);
                }
            },
        );
        // Nothing on screen yet: say why, below the header.
        if n == 0 {
            let area = Rect::from_min_max(pos2(page.left(), page.top() + TOPBAR_H + header_h), page.max);
            let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Center)));
            cu.add_space(S12);
            if loading {
                let (sr, _) = cu.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                w::spinner(&cu, sr.center(), 14.0, ACCENT);
                cu.add_space(S3);
                let label = match &self.yt.shared.lk().setup {
                    ytdlp::Setup::Working { label, .. } => format!("{label}…"),
                    _ => "Finding songs you might like…".to_string(),
                };
                cu.label(egui::RichText::new(label).font(body()).color(TEXT_2));
            } else if let Some(msg) = failed {
                if w::empty_state(&mut cu, icons::CIRCLE_ALERT, "Couldn't get suggestions", &msg, Some(("Try again", Some(icons::REFRESH)))) {
                    self.refresh_suggestions();
                }
            }
        }
        (off, "For you".into())
    }

    fn suggest_card(&mut self, ui: &mut Ui, rect: Rect, size: f32, item: &YtItem) {
        let id = Id::new(("sugg", &item.id));
        let resp = ui.interact(rect, id, Sense::click());
        let (h, pr) = w::group_hover_press(ui, &resp, rect);
        let p = ui.painter().clone();
        w::hover_surface(&p, rect.expand(S2), h);
        let ar = w::scale_rect(Rect::from_min_size(rect.min, Vec2::splat(size)), 1.0 - 0.02 * pr);
        let tex = item.thumb_url().and_then(|u| self.art.get(ArtKey::Url(u)));
        w::soft_shadow(&p, ar, R_MD - 2.0, 0.35 + 0.35 * h);
        w::thumb_square(&p, ar, tex, R_MD - 2.0);

        let in_lib = self.lib.yt_ids.contains(&item.id);
        let job = self.yt.shared.lk().jobs.iter().find(|j| j.item.id == item.id).cloned();
        let status = job.as_ref().map(|j| j.status.clone()).filter(|s| !matches!(s, JobStatus::Done(path) if self.download_gone(path)));
        let saved = in_lib || matches!(status, Some(JobStatus::Done(_)));
        let loading = !saved && matches!(self.yt.shared.lk().previews.get(&item.id), Some(PreviewState::Loading(_)));
        let playing_this = self.playing_yt_id().as_deref() == Some(item.id.as_str());

        // Bottom right: play (on hover), a spinner while it loads, bars while it plays.
        let pc = pos2(ar.right() - 30.0, ar.bottom() - 30.0);
        if loading {
            p.circle_filled(pc, 22.0, with_alpha(Color32::BLACK, 0.6));
            w::spinner(ui, pc, 9.0, TEXT);
        } else if playing_this && !(h > 0.01) {
            p.circle_filled(pc, 22.0, with_alpha(Color32::BLACK, 0.6));
            if self.ps.playing {
                self.np_bars(ui, &p, pc, 16.0, ACCENT);
            } else {
                icons::play(&p, pc, 14.0, TEXT);
            }
        } else if h > 0.01 {
            let c = pc + vec2(0.0, (1.0 - h) * 8.0);
            let br = Rect::from_center_size(c, Vec2::splat(48.0));
            let presp = ui.interact(br, id.with("play"), Sense::click());
            w::paint_play_circle(ui, br, &presp, playing_this && self.ps.playing, false, h);
            if presp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                self.play_suggestion(item, playing_this, saved, loading, job.as_ref());
            }
        }

        // Top right: download state (always shown once it's saving or saved).
        let dc = pos2(ar.right() - 22.0, ar.top() + 22.0);
        let dr = Rect::from_center_size(dc, Vec2::splat(32.0));
        let shown = h.max(if status.is_some() || saved { 1.0 } else { 0.0 });
        if shown > 0.01 {
            let dresp = ui.interact(dr, id.with("dl"), Sense::click());
            let (dh, _) = w::hover_press(ui, &dresp);
            p.circle_filled(dc, 16.0, with_alpha(Color32::BLACK, (0.55 + 0.2 * dh) * shown));
            match (&status, saved) {
                (_, true) => {
                    icons::glyph(&p, dc, icons::CHECK, 15.0, with_alpha(ACCENT, shown));
                    let _ = dresp.on_hover_text("In your library");
                }
                (Some(JobStatus::Waiting), _) => {
                    w::progress_ring(&p, dc, 10.0, 0.0, ACCENT);
                    let _ = dresp.on_hover_text("Waiting to download");
                }
                (Some(JobStatus::Downloading { pct, .. }), _) => {
                    w::progress_ring(&p, dc, 10.0, *pct, ACCENT);
                    let _ = dresp.on_hover_text(format!("Downloading · {}%", (pct * 100.0).round() as u32));
                }
                (Some(JobStatus::Saving), _) => {
                    w::progress_ring(&p, dc, 10.0, 1.0, ACCENT);
                    let _ = dresp.on_hover_text("Adding to your library");
                }
                (Some(JobStatus::Failed(msg)), _) => {
                    icons::glyph(&p, dc, icons::CIRCLE_ALERT, 16.0, WARN);
                    if dresp.on_hover_text(format!("{msg}\nClick to try again.")).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        if let Some(j) = &job {
                            self.actions.push(Action::YtRetry(j.uid));
                        }
                    }
                }
                _ => {
                    icons::glyph(&p, dc, icons::ARROW_DOWN_TO_LINE, 15.0, with_alpha(lerp_color(TEXT, ACCENT_HI, dh), shown));
                    if dresp.on_hover_text("Download").on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        self.actions.push(Action::YtDownload { items: vec![item.clone()], playlist_name: None });
                    }
                }
            }
        }

        let (artist, title) = ytdlp::guess_artist_title(item);
        let tc = if playing_this { ACCENT } else { TEXT };
        w::text(&p, pos2(rect.left(), ar.bottom() + 18.0), Align2::LEFT_CENTER, &title, body_strong(), tc, size);
        w::text(&p, pos2(rect.left(), ar.bottom() + 38.0), Align2::LEFT_CENTER, &artist, small(), TEXT_2, size);

        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            self.play_suggestion(item, playing_this, saved, loading, job.as_ref());
        }
        let page = item.page();
        resp.context_menu(|ui| {
            ui.set_max_width(MENU_W);
            if !saved && menu_item(ui, icons::ARROW_DOWN_TO_LINE, "Download").clicked() {
                self.actions.push(Action::YtDownload { items: vec![item.clone()], playlist_name: None });
                ui.close();
            }
            if menu_item(ui, icons::LINK, "Open on YouTube").clicked() {
                self.actions.push(Action::OpenUrl(page.clone()));
                ui.close();
            }
        });
    }

    fn play_suggestion(&mut self, item: &YtItem, playing_this: bool, saved: bool, loading: bool, job: Option<&ytdlp::Job>) {
        if playing_this {
            self.actions.push(Action::Toggle);
        } else if saved {
            self.play_yt(&item.id, job);
        } else if !loading {
            self.actions.push(Action::Preview(item.clone()));
        }
    }
}
