//! Import playlists from Spotify, YouTube and other sites, many at once: paste playlist links,
//! a YouTube channel or a Spotify profile (all their public playlists), or songs copied out of
//! the Spotify app. Everything found is listed to tick, then saved as playlists that play
//! online until you download them.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui};

use crate::app::{Action, App, Route};
use crate::art::ArtKey;
use crate::icons;
use crate::store::LockExt;
use crate::theme::*;
use crate::widgets::{self as w, PillKind};
use crate::ytdlp::{self, PlaylistInfo, Resolver, YtItem};

// -------------------------------------------------------------------------------------------
// What was pasted

/// One thing to look up.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A playlist or album link on any site.
    Link(String),
    /// A YouTube channel: its public playlists.
    Channel(String),
    /// A Spotify profile: its public playlists.
    Profile(String),
    /// Songs copied out of Spotify (Ctrl+A, Ctrl+C copies one link per song).
    Songs(Vec<String>),
}

/// Everything worth looking up in pasted text, in order, once each.
pub fn sources(text: &str) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    for raw in text.split_whitespace() {
        let tok = raw.trim_matches(|c: char| matches!(c, ',' | ';' | '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']'));
        if !ytdlp::looks_like_link(tok) {
            continue;
        }
        let url = ytdlp::full_url(tok);
        if !crate::spotify::track_ids_in(&url).is_empty() {
            continue; // songs: gathered below
        }
        let src = spotify_profile(&url).map(Source::Profile).or_else(|| youtube_channel(&url).map(Source::Channel)).unwrap_or(Source::Link(url));
        if !out.contains(&src) {
            out.push(src);
        }
    }
    let songs = crate::spotify::track_ids_in(text);
    if !songs.is_empty() {
        out.push(Source::Songs(songs));
    }
    out
}

/// "open.spotify.com/user/ID" (or "spotify:user:ID") → the profile page.
fn spotify_profile(url: &str) -> Option<String> {
    let rest = url.split_once("spotify.com/").map(|(_, r)| r).or_else(|| url.strip_prefix("spotify:"))?;
    let rest = if rest.starts_with("intl-") { rest.split_once('/')?.1 } else { rest };
    let id = rest.strip_prefix("user/").or_else(|| rest.strip_prefix("user:"))?;
    let id: String = id.chars().take_while(|c| !matches!(c, '/' | '?' | '#' | ':')).collect();
    (!id.is_empty()).then(|| format!("https://open.spotify.com/user/{id}"))
}

/// A YouTube (or YouTube Music) channel link → its Playlists tab.
fn youtube_channel(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    let after = ["youtube.com/"].iter().find_map(|h| lower.find(h).map(|i| i + h.len()))?;
    let path = &url[after..];
    let mut segs = path.split(['/', '?', '#']).filter(|s| !s.is_empty());
    let first = segs.next()?;
    let base = if first.starts_with('@') {
        first.to_string()
    } else if matches!(first, "channel" | "c" | "user") {
        format!("{first}/{}", segs.next()?)
    } else {
        return None;
    };
    Some(format!("https://www.youtube.com/{base}/playlists"))
}

/// A link cut short for a message.
fn short(url: &str) -> String {
    let u = url.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    if u.chars().count() > 48 {
        format!("{}…", u.chars().take(47).collect::<String>())
    } else {
        u.to_string()
    }
}

// -------------------------------------------------------------------------------------------
// Looking things up

/// A playlist found, waiting to be ticked.
#[derive(Clone)]
pub struct Found {
    pub info: PlaylistInfo,
    /// Its songs, when already read (a channel's playlists are read when imported).
    pub songs: Option<Vec<YtItem>>,
    pub picked: bool,
}

pub enum Stage {
    Start,
    Looking(String),
    Pick { found: Vec<Found>, problems: Vec<String> },
    Importing { done: usize, total: usize },
    Done { playlists: usize, songs: usize, problems: Vec<String> },
}

/// From the importing thread to the app, which owns the playlists.
pub enum Msg {
    Save(PlaylistInfo, Vec<YtItem>),
    Finished(Vec<String>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Platform {
    Spotify,
    YouTube,
    YouTubeMusic,
    Other,
}

impl Platform {
    const ALL: [Platform; 4] = [Platform::Spotify, Platform::YouTube, Platform::YouTubeMusic, Platform::Other];

    fn label(self) -> &'static str {
        match self {
            Platform::Spotify => "Spotify",
            Platform::YouTube => "YouTube",
            Platform::YouTubeMusic => "YouTube Music",
            Platform::Other => "SoundCloud & more",
        }
    }

    /// (what you want, how to get the link).
    fn steps(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Platform::Spotify => &[
                ("One playlist", "Open it, click ··· then Share → Copy link to playlist."),
                ("All your public playlists", "Click your picture → Profile, then ··· → Share → Copy link to profile. The playlists shown on your profile come in."),
                ("Liked Songs or a private playlist", "In the Spotify app, open it, click any song, press Ctrl+A then Ctrl+C, and paste here."),
            ],
            Platform::YouTube => &[
                ("One playlist", "Open it and copy the address from your browser, or click Share → Copy."),
                ("All your public playlists", "Click your picture → View your channel and copy the address. Only Public playlists show; set the others to Public first if you want them."),
            ],
            Platform::YouTubeMusic => &[
                ("One playlist", "Open it, click the three dots → Share → Copy link."),
                ("All your playlists", "Your YouTube Music playlists live on your YouTube channel too: follow the YouTube steps."),
            ],
            Platform::Other => &[
                ("SoundCloud", "Open the playlist and copy the address, or click Share → Copy link."),
                ("Bandcamp", "Open the album and copy the address."),
                ("Other sites", "Most sites that play music work: copy the playlist's address and paste it here."),
            ],
        }
    }
}

pub struct Import {
    pub text: String,
    pub stage: Arc<Mutex<Stage>>,
    pub help: Platform,
    /// Bumped by each new look, so an older one can't overwrite it.
    gen: Arc<AtomicU64>,
    tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    /// Saved so far in this import.
    pub saved: usize,
    pub saved_songs: usize,
}

impl Default for Import {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            text: String::new(),
            stage: Arc::new(Mutex::new(Stage::Start)),
            help: Platform::Spotify,
            gen: Arc::new(AtomicU64::new(0)),
            tx,
            rx,
            saved: 0,
            saved_songs: 0,
        }
    }
}

impl Import {
    /// Look up everything in the text box.
    fn look(&self, resolver: Resolver, ctx: egui::Context) {
        let sources = sources(&self.text);
        if sources.is_empty() {
            *self.stage.lk() = Stage::Pick {
                found: Vec::new(),
                problems: vec!["There's no link in what you pasted. See the steps above for where to find one.".into()],
            };
            return;
        }
        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        let (stage, gens) = (self.stage.clone(), self.gen.clone());
        *stage.lk() = Stage::Looking("Looking…".into());
        std::thread::spawn(move || {
            let current = || gens.load(Ordering::SeqCst) == gen;
            let note = |s: String| {
                if current() {
                    *stage.lk() = Stage::Looking(s);
                    ctx.request_repaint();
                }
            };
            let (mut found, mut problems) = (Vec::<Found>::new(), Vec::<String>::new());
            for src in &sources {
                if !current() {
                    return;
                }
                match src {
                    Source::Link(url) => {
                        note(format!("Reading {}…", short(url)));
                        match resolver.link(url) {
                            Ok((items, Some(info))) if !items.is_empty() => found.push(Found { info, songs: Some(items), picked: true }),
                            Ok((_, Some(info))) => problems.push(format!("“{}” has no songs Segno can play.", info.title)),
                            Ok(_) => problems.push(format!("{}: that's a single song, not a playlist. Open it in Search to play or save it.", short(url))),
                            Err(e) => problems.push(format!("{}: {e}", short(url))),
                        }
                    }
                    Source::Channel(url) => {
                        note("Reading the channel's playlists…".into());
                        match resolver.channel_playlists(url) {
                            Ok((name, lists)) if lists.is_empty() => problems.push(format!(
                                "{} has no public playlists. On YouTube, set the playlists you want to Public, then try again.",
                                if name.is_empty() { "This channel".to_string() } else { name }
                            )),
                            Ok((name, lists)) => found.extend(lists.into_iter().map(|l| Found {
                                info: PlaylistInfo { title: l.title, channel: name.clone(), url: l.url, cover: l.cover, site: "YouTube".into() },
                                songs: None,
                                picked: true,
                            })),
                            Err(e) => problems.push(format!("{}: {e}", short(url))),
                        }
                    }
                    Source::Profile(url) => {
                        note("Reading the Spotify profile…".into());
                        match crate::spotify::profile_playlists(url) {
                            Ok(ids) => {
                                for (k, id) in ids.iter().enumerate() {
                                    if !current() {
                                        return;
                                    }
                                    note(format!("Reading Spotify playlist {} of {}…", k + 1, ids.len()));
                                    match resolver.link(&format!("https://open.spotify.com/playlist/{id}")) {
                                        Ok((items, Some(info))) if !items.is_empty() => found.push(Found { info, songs: Some(items), picked: true }),
                                        Ok(_) => {}
                                        Err(e) => problems.push(e),
                                    }
                                }
                            }
                            Err(e) => problems.push(e),
                        }
                    }
                    Source::Songs(ids) => {
                        let (items, missed) = read_spotify_songs(&resolver, ids, |done| note(format!("Reading songs from Spotify: {done} of {}…", ids.len())));
                        if missed > 0 {
                            problems.push(format!("{} of the copied songs couldn't be read from Spotify.", missed));
                        }
                        if !items.is_empty() {
                            let key = crate::store::hash_bytes(ids.join(",").as_bytes());
                            let cover = items.iter().find_map(|i| (!i.thumb.is_empty()).then(|| i.thumb.clone())).unwrap_or_default();
                            found.push(Found {
                                info: PlaylistInfo {
                                    title: "Songs from Spotify".into(),
                                    channel: String::new(),
                                    url: format!("spotify:songs:{key:016x}"),
                                    cover,
                                    site: "Spotify".into(),
                                },
                                songs: Some(items),
                                picked: true,
                            });
                        }
                    }
                }
            }
            let mut seen = std::collections::HashSet::new();
            found.retain(|f| seen.insert(f.info.url.clone()));
            if current() {
                *stage.lk() = Stage::Pick { found, problems };
                ctx.request_repaint();
            }
        });
    }

    /// Save the ticked playlists (reading the songs of those not read yet).
    fn import(&mut self, resolver: Resolver, ctx: egui::Context) {
        let picks: Vec<Found> = match &*self.stage.lk() {
            Stage::Pick { found, .. } => found.iter().filter(|f| f.picked).cloned().collect(),
            _ => return,
        };
        if picks.is_empty() {
            return;
        }
        self.saved = 0;
        self.saved_songs = 0;
        self.gen.fetch_add(1, Ordering::SeqCst);
        let total = picks.len();
        *self.stage.lk() = Stage::Importing { done: 0, total };
        let (stage, tx) = (self.stage.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let mut problems = Vec::new();
            for (k, f) in picks.into_iter().enumerate() {
                let read = match f.songs {
                    Some(songs) => Ok((songs, Some(f.info.clone()))),
                    None => resolver.link(&f.info.url),
                };
                match read {
                    Ok((songs, _)) if songs.is_empty() => problems.push(format!("“{}” has no songs Segno can play.", f.info.title)),
                    // Keep the name and picture from the list; the site's own may be blank.
                    Ok((songs, info)) => {
                        let mut keep = f.info;
                        if let Some(i) = info.filter(|_| keep.cover.is_empty()) {
                            keep.cover = i.cover;
                        }
                        let _ = tx.send(Msg::Save(keep, songs));
                    }
                    Err(e) => problems.push(format!("“{}”: {e}", f.info.title)),
                }
                *stage.lk() = Stage::Importing { done: k + 1, total };
                ctx.request_repaint();
            }
            let _ = tx.send(Msg::Finished(problems));
            ctx.request_repaint();
        });
    }
}

/// Read songs copied out of Spotify, four at a time. Returns the songs (in order) and how many
/// couldn't be read.
fn read_spotify_songs(resolver: &Resolver, ids: &[String], progress: impl Fn(usize) + Sync) -> (Vec<YtItem>, usize) {
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let slots: Vec<Mutex<Option<YtItem>>> = ids.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..ids.len().min(4) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(id) = ids.get(i) else { break };
                if let Ok((items, _)) = resolver.link(&format!("https://open.spotify.com/track/{id}")) {
                    *slots[i].lk() = items.into_iter().next();
                }
                progress(done.fetch_add(1, Ordering::Relaxed) + 1);
            });
        }
    });
    let items: Vec<YtItem> = slots.into_iter().filter_map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner())).collect();
    let missed = ids.len() - items.len();
    (items, missed)
}

// -------------------------------------------------------------------------------------------
// The app's side

impl App {
    /// Save what the importing thread has read.
    pub(crate) fn sync_import(&mut self) {
        while let Ok(msg) = self.import.rx.try_recv() {
            match msg {
                Msg::Save(info, songs) => {
                    self.save_playlist(&info, &songs);
                    self.import.saved += 1;
                    self.import.saved_songs += songs.len();
                }
                Msg::Finished(problems) => {
                    let (playlists, songs) = (self.import.saved, self.import.saved_songs);
                    *self.import.stage.lk() = Stage::Done { playlists, songs, problems };
                    if playlists > 0 {
                        self.toast(format!("Imported {}", plural(playlists, "playlist", "playlists")));
                    }
                }
            }
        }
    }

    pub(crate) fn import_page(&mut self, ui: &mut Ui) -> (f32, String) {
        let out = egui::ScrollArea::vertical().id_salt("import").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(TOPBAR_H);
            let full = ui.available_width();
            let colw = (full - 2.0 * PAD_X).min(720.0);
            let mut col = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_size(ui.cursor().min + vec2(PAD_X, 0.0), vec2(colw, f32::INFINITY)))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            self.import_column(&mut col, colw);
            let h = col.min_rect().height();
            ui.add_space(h + S12);
        });
        (out.state.offset.y, "Import playlists".into())
    }

    fn import_column(&mut self, ui: &mut Ui, wdt: f32) {
        let (r, _) = ui.allocate_exact_size(vec2(wdt, 44.0), Sense::hover());
        w::text(ui.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, "Import playlists", h1(), TEXT, wdt);
        let (r, _) = ui.allocate_exact_size(vec2(wdt, 22.0), Sense::hover());
        w::text(
            ui.painter(),
            pos2(r.left(), r.center().y),
            Align2::LEFT_CENTER,
            "Bring your playlists from Spotify, YouTube and other sites. Songs play online until you download them.",
            body(),
            TEXT_2,
            wdt,
        );
        ui.add_space(S5);

        // Where to find the link, for each site.
        let card = egui::Frame::new().fill(RAISED).corner_radius(R_LG).inner_margin(S5);
        card.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new("Where to find the link").font(body_strong()).color(TEXT));
            ui.add_space(S3);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(S2, S2);
                for p in Platform::ALL {
                    let kind = if self.import.help == p { PillKind::Secondary } else { PillKind::Ghost };
                    if w::pill(ui, p.label(), None, kind).clicked() {
                        self.import.help = p;
                    }
                }
            });
            ui.add_space(S3);
            let inner = ui.available_width();
            for (what, how) in self.import.help.steps() {
                ui.label(egui::RichText::new(*what).font(small_strong()).color(TEXT));
                let g = ui.painter().layout(how.to_string(), body(), TEXT_2, inner);
                let (r, _) = ui.allocate_exact_size(vec2(inner, g.size().y), Sense::hover());
                ui.painter().galley(r.min, g, TEXT_2);
                ui.add_space(S3);
            }
        });
        ui.add_space(S4);

        // The paste box.
        let busy = matches!(&*self.import.stage.lk(), Stage::Looking(_) | Stage::Importing { .. });
        let card = egui::Frame::new().fill(RAISED).corner_radius(R_LG).inner_margin(S5);
        card.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new("Paste links").font(body_strong()).color(TEXT));
            ui.add_space(2.0);
            ui.label(egui::RichText::new("One or many: playlists, your YouTube channel, your Spotify profile, or songs copied from Spotify.").font(small()).color(TEXT_2));
            ui.add_space(S3);
            let id = Id::new("import-text");
            let focused = ui.memory(|m| m.has_focus(id));
            egui::Frame::new()
                .fill(if focused { RAISED_2 } else { with_alpha(Color32::BLACK, 0.18) })
                .corner_radius(R_MD)
                .stroke(egui::Stroke::new(1.0, with_alpha(Color32::WHITE, if focused { 0.22 } else { 0.06 })))
                .inner_margin(S3)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("import-text-scroll").max_height(160.0).show(ui, |ui| {
                        egui::TextEdit::multiline(&mut self.import.text)
                            .id(id)
                            .hint_text(egui::RichText::new("https://open.spotify.com/playlist/…\nhttps://www.youtube.com/@yourname").color(TEXT_3))
                            .frame(egui::Frame::NONE)
                            .font(body())
                            .text_color(TEXT)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .show(ui);
                    });
                });
            ui.add_space(S3);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy && !self.import.text.trim().is_empty(), |ui| {
                    if w::pill(ui, "Find playlists", Some(icons::SEARCH), PillKind::Primary).clicked() {
                        let (resolver, ctx) = (self.yt.resolver(), ui.ctx().clone());
                        self.import.look(resolver, ctx);
                    }
                });
                if !self.import.text.is_empty() && !busy && w::pill(ui, "Clear", None, PillKind::Ghost).clicked() {
                    self.import.text.clear();
                    *self.import.stage.lk() = Stage::Start;
                }
            });
        });
        ui.add_space(S5);

        self.import_results(ui, wdt);
    }

    fn import_results(&mut self, ui: &mut Ui, wdt: f32) {
        let stage = self.import.stage.clone();
        let mut st = stage.lk();
        match &mut *st {
            Stage::Start => {}
            Stage::Looking(note) => busy_row(ui, wdt, note),
            Stage::Importing { done, total } => busy_row(ui, wdt, &format!("Importing {} of {}…", (*done + 1).min(*total), total)),
            Stage::Pick { found, problems } => {
                if !found.is_empty() {
                    let picked = found.iter().filter(|f| f.picked).count();
                    let (r, _) = ui.allocate_exact_size(vec2(wdt, 28.0), Sense::hover());
                    w::text(ui.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &format!("Found {}", plural(found.len(), "playlist", "playlists")), h2(), TEXT, wdt - 140.0);
                    let all = picked == found.len();
                    let lr = Rect::from_min_max(pos2(r.right() - 130.0, r.top()), r.max);
                    let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(lr).layout(egui::Layout::right_to_left(egui::Align::Center)));
                    if w::link_button(&mut lu, if all { "Select none" } else { "Select all" }).clicked() {
                        for f in found.iter_mut() {
                            f.picked = !all;
                        }
                    }
                    ui.add_space(S2);
                    for (k, f) in found.iter_mut().enumerate() {
                        let here = self.playlists.iter().any(|p| p.source.as_ref().is_some_and(|s| s.url == f.info.url));
                        if found_row(ui, wdt, k, f, here, &mut self.art) {
                            f.picked = !f.picked;
                        }
                    }
                    ui.add_space(S4);
                    let label = format!("Import {}", plural(picked, "playlist", "playlists"));
                    let go = ui.add_enabled_ui(picked > 0, |ui| w::pill(ui, &label, Some(icons::DOWNLOAD), PillKind::Primary)).inner;
                    if go.clicked() {
                        drop(st);
                        let (resolver, ctx) = (self.yt.resolver(), ui.ctx().clone());
                        self.import.import(resolver, ctx);
                        return;
                    }
                }
                problem_list(ui, wdt, problems);
            }
            Stage::Done { playlists, songs, problems } => {
                let (r, _) = ui.allocate_exact_size(vec2(wdt, 40.0), Sense::hover());
                let p = ui.painter();
                p.circle_filled(pos2(r.left() + 16.0, r.center().y), 16.0, with_alpha(ACCENT, 0.18));
                icons::glyph(p, pos2(r.left() + 16.0, r.center().y), icons::CHECK, 16.0, ACCENT);
                let msg = if *playlists == 0 {
                    "Nothing was imported".to_string()
                } else {
                    format!("Imported {} with {}", plural(*playlists, "playlist", "playlists"), plural(*songs, "song", "songs"))
                };
                w::text(p, pos2(r.left() + 44.0, r.center().y), Align2::LEFT_CENTER, &msg, h2(), TEXT, wdt - 44.0);
                ui.add_space(S2);
                if *playlists > 0 {
                    ui.label(egui::RichText::new("They're in the sidebar under Playlists.").font(body()).color(TEXT_2));
                    ui.add_space(S3);
                }
                if w::pill(ui, "Import more", Some(icons::PLUS), PillKind::Secondary).clicked() {
                    *st = Stage::Start;
                    drop(st);
                    self.import.text.clear();
                    return;
                }
                problem_list(ui, wdt, problems);
            }
        }
    }

    /// The "+" next to Playlists: start a new one or import.
    pub(crate) fn new_playlist_menu(&mut self, ui: &mut Ui) {
        ui.set_max_width(260.0);
        if choice(ui, icons::PLUS, "New playlist", "Start an empty one").clicked() {
            self.actions.push(Action::NewPlaylist { with: vec![] });
            ui.close();
        }
        if choice(ui, icons::DOWNLOAD, "Import playlists", "From Spotify, YouTube and more").clicked() {
            self.actions.push(Action::Go(Route::Import));
            ui.close();
        }
    }
}

/// A menu choice with a line under it.
fn choice(ui: &mut Ui, icon: &str, label: &str, sub: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(260.0, 48.0), Sense::click());
    let (h, _) = w::hover_press(ui, &resp);
    let p = ui.painter();
    if h > 0.0 {
        p.rect_filled(rect, R_SM + 2.0, with_alpha(Color32::WHITE, 0.08 * h));
    }
    icons::glyph(p, pos2(rect.left() + 20.0, rect.center().y), icon, 18.0, TEXT_2);
    w::text(p, pos2(rect.left() + 44.0, rect.top() + 16.0), Align2::LEFT_CENTER, label, body_strong(), TEXT, rect.width() - 52.0);
    w::text(p, pos2(rect.left() + 44.0, rect.top() + 33.0), Align2::LEFT_CENTER, sub, small(), TEXT_2, rect.width() - 52.0);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn busy_row(ui: &mut Ui, wdt: f32, note: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(wdt, 36.0), Sense::hover());
    w::spinner(ui, pos2(r.left() + 12.0, r.center().y), 9.0, ACCENT);
    w::text(ui.painter(), pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, note, body(), TEXT_2, wdt - 32.0);
    ui.ctx().request_repaint();
}

/// One found playlist: tick box, picture, name, where it's from. Returns true when clicked.
fn found_row(ui: &mut Ui, wdt: f32, k: usize, f: &Found, here: bool, art: &mut crate::art::ArtCache) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(wdt, 56.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let (h, _) = w::hover_press(ui, &resp);
    let p = ui.painter();
    w::hover_surface(p, r, h);
    let on = ui.ctx().animate_bool_with_time(Id::new(("import-pick", k)), f.picked, T_FAST);
    let bx = Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), vec2(20.0, 20.0));
    p.rect_filled(bx, 5.0, lerp_color(with_alpha(Color32::WHITE, 0.06), ACCENT, on));
    p.rect_stroke(bx, 5.0, egui::Stroke::new(1.5, with_alpha(Color32::WHITE, 0.25 * (1.0 - on))), egui::StrokeKind::Inside);
    if on > 0.01 {
        icons::glyph(p, bx.center(), icons::CHECK, 14.0, with_alpha(Color32::WHITE, on));
    }
    let tr = Rect::from_min_size(pos2(bx.right() + S4, r.center().y - 20.0), vec2(71.0, 40.0));
    let tex = (!f.info.cover.is_empty()).then(|| art.get(ArtKey::Url(f.info.cover.clone()))).flatten();
    if tex.is_some() {
        w::thumb(p, tr, tex, R_SM);
    } else {
        p.rect_filled(tr, R_SM, RAISED_2);
        icons::glyph(p, tr.center(), icons::LIST_MUSIC, 18.0, TEXT_3);
    }
    let tx = tr.right() + S4;
    let tw = r.right() - tx - S3;
    w::text(p, pos2(tx, r.center().y - 9.0), Align2::LEFT_CENTER, &f.info.title, body_strong(), TEXT, tw);
    let mut sub = f.info.site.clone();
    if let Some(s) = &f.songs {
        sub = format!("{sub} · {}", plural(s.len(), "song", "songs"));
    }
    if !f.info.channel.is_empty() {
        sub = format!("{sub} · {}", f.info.channel);
    }
    if here {
        sub = format!("{sub} · Already imported: adds any new songs");
    }
    w::text(p, pos2(tx, r.center().y + 10.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, tw);
    resp.clicked()
}

fn problem_list(ui: &mut Ui, wdt: f32, problems: &[String]) {
    if problems.is_empty() {
        return;
    }
    ui.add_space(S4);
    for msg in problems {
        let g = ui.painter().layout(msg.clone(), body(), TEXT_2, wdt - 28.0);
        let (r, _) = ui.allocate_exact_size(vec2(wdt, g.size().y.max(20.0)), Sense::hover());
        icons::glyph(ui.painter(), pos2(r.left() + 9.0, r.top() + 10.0), icons::CIRCLE_ALERT, 15.0, TEXT_3);
        ui.painter().galley(pos2(r.left() + 28.0, r.top()), g, TEXT_2);
        ui.add_space(S2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_was_pasted() {
        let text = "Check these:\nhttps://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=x,\nyoutube.com/@SomeName\n\
                    https://music.youtube.com/channel/UCabc123\nhttps://www.youtube.com/playlist?list=PLxyz\n\
                    https://open.spotify.com/user/someuser?si=1\nhttps://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC\n\
                    https://open.spotify.com/track/7qiZfU4dY1lWllzX7mPBI3\nhttps://www.youtube.com/playlist?list=PLxyz";
        assert_eq!(
            sources(text),
            vec![
                Source::Link("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=x".into()),
                Source::Channel("https://www.youtube.com/@SomeName/playlists".into()),
                Source::Channel("https://www.youtube.com/channel/UCabc123/playlists".into()),
                Source::Link("https://www.youtube.com/playlist?list=PLxyz".into()),
                Source::Profile("https://open.spotify.com/user/someuser".into()),
                Source::Songs(vec!["4uLU6hMCjMI75M1A2tKUQC".into(), "7qiZfU4dY1lWllzX7mPBI3".into()]),
            ]
        );
        assert!(sources("just some words").is_empty());
    }

    #[test]
    fn channel_links() {
        assert_eq!(youtube_channel("https://www.youtube.com/@name/videos"), Some("https://www.youtube.com/@name/playlists".into()));
        assert_eq!(youtube_channel("https://youtube.com/c/Name"), Some("https://www.youtube.com/c/Name/playlists".into()));
        assert_eq!(youtube_channel("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), None);
        assert_eq!(youtube_channel("https://www.youtube.com/playlist?list=PLx"), None);
        assert_eq!(spotify_profile("https://open.spotify.com/intl-de/user/abc.def?si=2"), Some("https://open.spotify.com/user/abc.def".into()));
        assert_eq!(spotify_profile("spotify:user:abc"), Some("https://open.spotify.com/user/abc".into()));
        assert_eq!(spotify_profile("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"), None);
    }
}
