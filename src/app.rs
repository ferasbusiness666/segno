//! App state, the frame layout (sidebar, page, queue, player bar), and everything that reacts to
//! the player, the library and downloads.

use crate::store::LockExt;
use crate::art::{ArtCache, ArtSize};
use crate::library::{self, Library, ScanCmd};
use crate::platform::{self, MediaAction, TrayAction};
use crate::player::{self, Cmd, PlayerHandle, QItem};
use crate::store::{self, now_secs, path_id, Playlist, Repeat, Settings};
use crate::theme::{self, *};
use crate::widgets::{self as w, PillKind};
use crate::update::{self, Updater};
use crate::ytdlp::{self, Yt, YtItem};
use crate::icons;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Key, Rect, Response, Sense, Stroke, Ui, ViewportCommand};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Songs,
    Albums,
    Album(u64),
    Artists,
    Artist(u64),
    Playlist(u64),
    Loved,
    Search,
    Downloads,
    Settings,
    NowPlaying,
}

pub enum Action {
    PlayIds { ids: Vec<u64>, start: usize, shuffle: Option<bool> },
    PlayPath(PathBuf),
    Toggle,
    Next,
    Prev,
    Seek(f32),
    Volume(f32),
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    PlayNext(Vec<u64>),
    AddToQueue(Vec<u64>),
    InsertInQueue { at: usize, ids: Vec<u64> },
    PlayFromAutoplay(u64),
    Preview(YtItem),
    SetAutoplay(bool),
    RemoveFromQueue(u64),
    MoveQueue { from: usize, to: usize },
    JumpTo(usize),
    ClearQueue,
    Go(Route),
    Back,
    Forward,
    ToggleLove(u64),
    NewPlaylist { with: Vec<u64> },
    AddToPlaylist { pid: u64, ids: Vec<u64> },
    RemoveFromPlaylist { pid: u64, index: usize },
    MoveInPlaylist { pid: u64, from: usize, to: usize },
    MoveInSongs { from: usize, to: usize },
    FoldSidebar(bool),
    StartRename(u64),
    CommitRename,
    AskDeletePlaylist(u64),
    DeletePlaylist(u64),
    Reveal(u64),
    AskDeleteFile(u64),
    DeleteFile(u64),
    YtSearch(String),
    YtDownload { items: Vec<YtItem>, playlist_name: Option<String> },
    YtRetry(u64),
    YtCancel(u64),
    YtClearFinished,
    YtUpdate,
    PickFolder,
    SetFolder(PathBuf),
    OpenFolder,
    Rescan,
    ToggleMini,
    ToggleQueue,
    SetCloseToTray(bool),
    Toast(String),
    Undo,
    CloseDialog,
    QuitApp,
    CheckUpdates,
    RestartToUpdate,
    SetAutoUpdate(bool),
    OpenUrl(String),
    HideToTray,
}

pub struct Toast {
    pub text: String,
    pub at: f64,
    pub undo: Option<Undo>,
}

pub enum Undo {
    /// Not an undo: "Restart" to finish installing an update.
    Restart,
    PlaylistRemove { pid: u64, index: usize, path: PathBuf },
    PlaylistDelete(Playlist, usize),
}

pub enum Dialog {
    DeleteFile(u64),
    DeletePlaylist(u64),
    CloseChoice,
}

/// Drag payload: songs being dragged (onto a playlist in the sidebar, or within a playlist).
#[derive(Clone)]
pub struct DragSongs {
    pub ids: Vec<u64>,
    pub from_playlist: Option<(u64, usize)>,
    pub from_queue: Option<usize>,
    /// Position in the Songs list (reordering the library by hand).
    pub from_songs: Option<usize>,
}

/// What the player bar / Now Playing / media overlay show for a song, whether it's in the library
/// or played straight from YouTube.
#[derive(Clone)]
pub struct NowInfo {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f32,
    pub art: Option<u64>,
    pub color: Option<[u8; 3]>,
    /// Set when the song isn't in the library (played from YouTube before downloading).
    pub preview: Option<YtItem>,
    pub artist_key: Option<u64>,
}

#[derive(Clone)]
pub struct Transient {
    pub item: YtItem,
    pub art: Option<u64>,
    pub color: Option<[u8; 3]>,
}

pub struct App {
    pub player: PlayerHandle,
    pub ps: player::State,
    pub pos: f32,
    pub scan_tx: std::sync::mpsc::Sender<ScanCmd>,
    lib_rx: Receiver<library::LibEvent>,
    pub lib: Library,
    pub art: ArtCache,
    pub yt: Yt,
    pub updater: Updater,
    update_seen: update::State,
    /// When the update became ready (so a click meant for the button underneath isn't taken as "Restart now").
    pub update_ready_at: f64,
    pub settings: Settings,
    pub playlists: Vec<Playlist>,
    pub loved: HashSet<u64>,
    /// Songs playing from YouTube that aren't downloaded, keyed by the id of their cache file.
    pub transient: std::collections::HashMap<u64, Transient>,
    autoplay_synced: (u64, store::SortBy),
    pub song_order: Vec<PathBuf>,
    order_version: u64,

    hwnd: Option<isize>,
    _tray: Option<platform::Tray>,
    tray_rx: Receiver<TrayAction>,
    media: Option<platform::Media>,
    media_tx: std::sync::mpsc::Sender<MediaAction>,
    media_rx: Receiver<MediaAction>,
    media_seen: (Option<u64>, bool, u64),
    platform_ready: bool,
    visible: bool,

    pub route: Route,
    back: Vec<Route>,
    fwd: Vec<Route>,
    pub route_t: f64,
    pub search: String,
    pub search_id: Id,
    pub focus_search: bool,
    pub yt_last_link: String,
    pub yt_requested: bool,
    link_pending: Option<(String, f64)>,
    pub queue_open: bool,
    pub mini: bool,
    normal_size: Option<egui::Vec2>,
    normal_pos: Option<egui::Pos2>,
    was_maximized: bool,
    quitting: bool,
    pub toasts: Vec<Toast>,
    pub dialog: Option<Dialog>,
    pub rename: Option<(u64, String, bool)>,
    pub selected: Option<u64>,
    pub actions: Vec<Action>,
    songs_cache: (u64, store::SortBy, Arc<Vec<u64>>),
    pub seek_preview: Option<f32>,
    settings_dirty: Option<Instant>,
    last_save: Instant,
    resume_restored: bool,
    cjk_loaded: bool,
    pub ctx: egui::Context,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install_fonts(&ctx);
        theme::install_style(&ctx);

        let repaint = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint()) as Arc<dyn Fn() + Send + Sync>
        };
        let player = player::spawn(repaint.clone());
        let (scan_tx, lib_rx) = library::spawn_scanner(repaint.clone());
        let yt = Yt::new(repaint.clone());
        let settings = Settings::load();
        let updater = Updater::new(repaint.clone(), settings.auto_update, settings.update_checked);
        let playlists = store::load_playlists();
        let loved = settings.loved.iter().map(|p| path_id(p)).collect();

        player.send(Cmd::SetVolume(settings.volume));
        player.send(Cmd::SetMuted(settings.muted));
        player.send(Cmd::SetRepeat(settings.repeat));
        player.send(Cmd::SetAutoplay(settings.autoplay));
        std::thread::spawn(ytdlp::clean_previews);
        if let Some(d) = &settings.music_dir {
            let _ = scan_tx.send(ScanCmd::SetDir(d.clone()));
            yt.set_music_dir(d.clone());
        }

        let (tray_tx, tray_rx) = channel();
        {
            let ctx2 = ctx.clone();
            platform::listen_for_second_launch(tray_tx.clone(), move || ctx2.request_repaint());
        }
        let tray = {
            let ctx2 = ctx.clone();
            let (rgba, size) = crate::app_icon(32);
            platform::Tray::new(rgba, size, tray_tx, move || ctx2.request_repaint())
        };
        let (media_tx, media_rx) = channel();

        let mut app = App {
            player,
            ps: Default::default(),
            pos: 0.0,
            scan_tx,
            lib_rx,
            lib: Library::default(),
            art: ArtCache::new(ctx.clone()),
            yt,
            updater,
            update_seen: update::State::Idle,
            update_ready_at: 0.0,
            settings,
            playlists,
            loved,
            transient: std::collections::HashMap::new(),
            autoplay_synced: (u64::MAX, store::SortBy::Added),
            song_order: store::load_song_order(),
            order_version: 0,
            hwnd: None,
            _tray: tray,
            tray_rx,
            media: None,
            media_tx,
            media_rx,
            media_seen: (None, false, 0),
            platform_ready: false,
            visible: true,
            route: Route::Songs,
            back: Vec::new(),
            fwd: Vec::new(),
            route_t: 0.0,
            search: String::new(),
            search_id: Id::new("search-box"),
            focus_search: false,
            yt_last_link: String::new(),
            yt_requested: false,
            link_pending: None,
            queue_open: false,
            mini: false,
            normal_size: None,
            normal_pos: None,
            was_maximized: false,
            quitting: false,
            toasts: Vec::new(),
            dialog: None,
            rename: None,
            selected: None,
            actions: Vec::new(),
            songs_cache: (u64::MAX, store::SortBy::Added, Arc::new(Vec::new())),
            seek_preview: None,
            settings_dirty: None,
            last_save: Instant::now(),
            resume_restored: false,
            cjk_loaded: false,
            ctx: ctx.clone(),
        };
        // Restore the queue and position from last time (paused, ready to continue).
        let r = app.settings.resume.clone();
        if !r.queue.is_empty() {
            let mk = |v: &Vec<PathBuf>| v.iter().map(|p| QItem::new(path_id(p), p.clone())).collect::<Vec<_>>();
            let queue = mk(&r.queue);
            // Keep uids shared between queue and original for the same entries.
            let mut original = Vec::new();
            let mut used = vec![false; queue.len()];
            for p in &r.original {
                if let Some(i) = queue.iter().enumerate().position(|(i, q)| !used[i] && &q.path == p) {
                    used[i] = true;
                    original.push(queue[i].clone());
                }
            }
            app.player.send(Cmd::SetShuffle(false));
            app.player.send(Cmd::Restore {
                queue,
                original,
                index: r.index,
                position: r.position,
            });
            app.player.state.lk().shuffle = app.settings.shuffle;
        } else if app.settings.shuffle {
            app.player.state.lk().shuffle = true;
        }
        app.resume_restored = true;
        // First start after an update: say so once.
        if app.settings.last_version != update::VERSION {
            if !app.settings.last_version.is_empty() {
                app.toast(format!("Updated to Segno {}", update::VERSION));
            }
            app.settings.last_version = update::VERSION.to_string();
            app.settings.save();
        }
        app
    }

    // -----------------------------------------------------------------------------------------
    // Helpers

    pub fn qitems(&self, ids: &[u64]) -> Vec<QItem> {
        ids.iter()
            .filter_map(|id| self.lib.get(*id).map(|t| QItem::new(t.id, t.path.clone())))
            .collect()
    }

    pub fn current_id(&self) -> Option<u64> {
        self.ps.current().map(|q| q.id)
    }

    pub fn info_for(&self, id: u64) -> Option<NowInfo> {
        if let Some(t) = self.lib.get(id) {
            return Some(NowInfo {
                id,
                title: t.title.clone(),
                artist: if t.artist.is_empty() { "Unknown artist".into() } else { t.artist.clone() },
                album: t.album.clone(),
                duration: t.duration,
                art: t.art,
                color: t.color,
                preview: None,
                artist_key: Some(library::artist_key(t.primary_artist())),
            });
        }
        let tr = self.transient.get(&id)?;
        let tags = ytdlp::clean_meta(&serde_json::Value::Null, &tr.item);
        Some(NowInfo {
            id,
            title: tags.title,
            artist: tags.artist,
            album: String::new(),
            duration: tr.item.duration,
            art: tr.art,
            color: tr.color,
            preview: Some(tr.item.clone()),
            artist_key: None,
        })
    }

    /// The YouTube id of what's playing, whether it's a downloaded song or a preview.
    pub fn playing_yt_id(&self) -> Option<String> {
        let id = self.current_id()?;
        match self.lib.get(id) {
            Some(t) => t.yt_id.clone(),
            None => self.transient.get(&id).map(|t| t.item.id.clone()),
        }
    }

    /// A finished download whose file has since been deleted or moved away.
    pub fn download_gone(&self, path: &std::path::Path) -> bool {
        self.lib.by_path(path).is_none() && !path.exists()
    }

    pub fn now_info(&self) -> Option<NowInfo> {
        self.current_id().and_then(|id| self.info_for(id))
    }

    /// The next few library songs that will play when the queue runs out.
    pub fn autoplay_preview(&mut self, n: usize) -> Vec<u64> {
        if !self.settings.autoplay {
            return Vec::new();
        }
        let songs = self.songs();
        if songs.is_empty() {
            return Vec::new();
        }
        let cur = self.current_id();
        let queued: HashSet<u64> = self.ps.queue.iter().map(|q| q.id).collect();
        let start = cur.and_then(|c| songs.iter().position(|i| *i == c)).map(|p| p + 1).unwrap_or(0);
        let mut out = Vec::new();
        for k in 0..songs.len() {
            let id = songs[(start + k) % songs.len()];
            if Some(id) == cur || queued.contains(&id) {
                continue;
            }
            out.push(id);
            if out.len() >= n {
                break;
            }
        }
        out
    }

    /// Hand the player the library order so it can keep going after the queue ends.
    fn sync_autoplay(&mut self) {
        let key = (self.lib.version.wrapping_add(self.order_version << 32), self.settings.sort);
        if key == self.autoplay_synced {
            return;
        }
        self.autoplay_synced = key;
        let songs = self.songs();
        let list: Vec<(u64, PathBuf)> = songs
            .iter()
            .filter_map(|id| self.lib.get(*id).map(|t| (t.id, t.path.clone())))
            .collect();
        *self.player.library.lk() = list;
    }

    pub fn songs(&mut self) -> Arc<Vec<u64>> {
        let ver = self.lib.version.wrapping_add(self.order_version << 32);
        if self.songs_cache.0 != ver || self.songs_cache.1 != self.settings.sort {
            let ids = if self.settings.sort == store::SortBy::Custom {
                // Your own order; songs added since then go on top.
                let ordered: Vec<u64> = self.song_order.iter().filter_map(|p| self.lib.by_path(p).map(|t| t.id)).collect();
                let known: HashSet<u64> = ordered.iter().copied().collect();
                let mut v: Vec<u64> = self
                    .lib
                    .sorted_ids(store::SortBy::Added)
                    .into_iter()
                    .filter(|id| !known.contains(id))
                    .collect();
                v.extend(ordered);
                v
            } else {
                self.lib.sorted_ids(self.settings.sort)
            };
            self.songs_cache = (ver, self.settings.sort, Arc::new(ids));
        }
        self.songs_cache.2.clone()
    }

    pub fn playlist(&self, pid: u64) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.id == pid)
    }

    /// Track ids of a playlist, skipping files that are gone.
    pub fn playlist_ids(&self, pid: u64) -> Vec<(usize, u64)> {
        self.playlist(pid)
            .map(|p| {
                p.tracks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, path)| self.lib.by_path(path).map(|t| (i, t.id)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn loved_ids(&self) -> Vec<u64> {
        self.settings
            .loved
            .iter()
            .filter_map(|p| self.lib.by_path(p).map(|t| t.id))
            .collect()
    }

    pub fn duration_of(&self, id: u64) -> f32 {
        self.info_for(id).map(|t| t.duration).unwrap_or(0.0)
    }

    pub fn now_duration(&self) -> f32 {
        if self.ps.duration > 0.0 {
            self.ps.duration
        } else {
            self.current_id().map(|i| self.duration_of(i)).unwrap_or(0.0)
        }
    }

    pub fn go(&mut self, r: Route) {
        if self.route != r {
            let prev = std::mem::replace(&mut self.route, r);
            self.back.push(prev);
            if self.back.len() > 50 {
                self.back.remove(0);
            }
            self.fwd.clear();
            self.route_t = self.ctx.input(|i| i.time);
        }
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_fwd(&self) -> bool {
        !self.fwd.is_empty()
    }

    /// React to the updater: offer a restart when a new version finished downloading.
    fn sync_updates(&mut self) {
        let st = self.updater.state();
        if st == self.update_seen {
            return;
        }
        if let update::State::Ready { version } = &st {
            let at = self.ctx.input(|i| i.time);
            self.update_ready_at = at;
            if !self.quitting {
                self.toasts.push(Toast {
                    text: format!("Segno {version} is ready"),
                    at,
                    undo: Some(Undo::Restart),
                });
            }
        }
        let checked = self.updater.last_checked.load(std::sync::atomic::Ordering::Relaxed);
        if checked != self.settings.update_checked {
            self.settings.update_checked = checked;
            self.mark_dirty();
        }
        self.update_seen = st;
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        let at = self.ctx.input(|i| i.time);
        self.toasts.retain(|t| t.undo.is_some());
        self.toasts.push(Toast {
            text: text.into(),
            at,
            undo: None,
        });
    }

    fn mark_dirty(&mut self) {
        if self.settings_dirty.is_none() {
            self.settings_dirty = Some(Instant::now());
        }
    }

    fn save_resume(&mut self) {
        let s = self.player.snapshot();
        self.settings.resume = store::Resume {
            queue: s.queue.iter().map(|q| q.path.clone()).collect(),
            original: s.original.iter().map(|q| q.path.clone()).collect(),
            index: s.index,
            position: self.player.position(),
        };
        self.settings.volume = s.volume;
        self.settings.muted = s.muted;
        self.settings.shuffle = s.shuffle;
        self.settings.repeat = s.repeat;
    }

    fn remember_window(&mut self) {
        if self.mini {
            return;
        }
        let (size, maxed) = self.ctx.input(|i| (i.viewport().inner_rect.map(|r| r.size()), i.viewport().maximized));
        if let Some(m) = maxed {
            self.settings.maximized = m;
        }
        if let Some(sz) = size {
            if !self.settings.maximized && sz.x >= 600.0 && sz.y >= 400.0 {
                self.settings.window_size = Some([sz.x, sz.y]);
            }
        }
    }

    pub fn save_all(&mut self) {
        self.remember_window();
        if self.resume_restored {
            self.save_resume();
        }
        self.settings.save();
        store::save_playlists(&self.playlists);
        self.settings_dirty = None;
        self.last_save = Instant::now();
    }

    fn set_folder(&mut self, dir: PathBuf) {
        self.settings.music_dir = Some(dir.clone());
        let _ = self.scan_tx.send(ScanCmd::SetDir(dir.clone()));
        self.yt.set_music_dir(dir);
        self.settings.save();
    }

    // -----------------------------------------------------------------------------------------
    // Background events (also runs while the window is hidden)

    fn logic_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.lib_rx.try_recv() {
            if !self.cjk_loaded && ev.tracks.iter().any(|t| theme::has_cjk(&t.title) || theme::has_cjk(&t.artist)) {
                theme::add_cjk_fallback(ctx);
                self.cjk_loaded = true;
            }
            self.lib.set_tracks(ev.tracks);
            self.lib.scanning = ev.scanning;
            self.lib.loaded_once = true;
        }
        while let Ok(ev) = self.yt.events.try_recv() {
            match ev {
                ytdlp::YtEvent::Downloaded { path, playlist } => {
                    if let Some(pid) = playlist {
                        if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                            if !p.tracks.contains(&path) {
                                p.tracks.push(path.clone());
                            }
                        }
                        store::save_playlists(&self.playlists);
                    }
                    let _ = self.scan_tx.send(ScanCmd::Rescan);
                    // If that song is playing as a preview right now, nothing changes for the
                    // listener; the library copy takes over next time.
                    if self.route != Route::Downloads && self.yt.active_count() == 0 {
                        let name = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.toast(format!("Downloaded “{name}”"));
                    }
                }
                ytdlp::YtEvent::PreviewReady { item, path, art } => {
                    let id = path_id(&path);
                    self.transient.insert(
                        id,
                        Transient {
                            item,
                            art: art.map(|a| a.0),
                            color: art.map(|a| a.1),
                        },
                    );
                    self.player.send(Cmd::PlayList {
                        items: vec![QItem::new(id, path)],
                        start: 0,
                        shuffle: None,
                    });
                }
                ytdlp::YtEvent::PreviewFailed { msg } => {
                    self.toast(format!("Couldn't play it: {msg}"));
                }
            }
        }
        self.sync_autoplay();
        self.sync_updates();
        while let Ok(a) = self.tray_rx.try_recv() {
            match a {
                TrayAction::Show => self.show_window(ctx),
                TrayAction::Toggle => self.player.send(Cmd::Toggle),
                TrayAction::Next => self.player.send(Cmd::Next),
                TrayAction::Prev => self.player.send(Cmd::Prev),
                TrayAction::Quit => {
                    self.quitting = true;
                    self.save_all();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
        while let Ok(a) = self.media_rx.try_recv() {
            match a {
                MediaAction::Toggle => self.player.send(Cmd::Toggle),
                MediaAction::Play => self.player.send(Cmd::Play),
                MediaAction::Pause => self.player.send(Cmd::Pause),
                MediaAction::Next => self.player.send(Cmd::Next),
                MediaAction::Prev => self.player.send(Cmd::Prev),
                MediaAction::Stop => self.player.send(Cmd::Pause),
                MediaAction::SeekTo(s) => self.player.send(Cmd::Seek(s)),
                MediaAction::Raise => self.show_window(ctx),
            }
        }
        self.ps = self.player.snapshot();
        self.pos = self.player.position();

        // Keep the Windows media overlay in sync (only when something changed).
        let cur = self.current_id();
        let key = (cur, self.ps.playing, self.ps.version);
        if key != self.media_seen {
            let track_changed = key.0 != self.media_seen.0;
            let state_changed = key.1 != self.media_seen.1 || track_changed;
            self.media_seen = key;
            let info = cur.and_then(|id| self.info_for(id)).map(|t| (t.title, t.artist, t.album, t.duration, t.art));
            let dur = self.now_duration();
            let pos = self.pos;
            let has = self.ps.current().is_some();
            let playing = self.ps.playing;
            if let Some(m) = &mut self.media {
                if track_changed {
                    m.set_track(info.as_ref().map(|(ti, ar, al, d, art)| platform::NowPlayingInfo {
                        title: ti,
                        artist: ar,
                        album: al,
                        duration: if dur > 0.0 { dur } else { *d },
                        cover: art.map(|k| crate::art::art_file(k, ArtSize::L)),
                    }));
                }
                if state_changed || track_changed {
                    m.set_state(if has { Some(playing) } else { None }, pos);
                }
            }
            if state_changed {
                self.mark_dirty();
            }
            if track_changed {
                if let Some(tray) = &self._tray {
                    let tip = match &info {
                        Some((t, a, ..)) if !a.is_empty() => format!("{t} · {a}"),
                        Some((t, ..)) => t.clone(),
                        None => "Segno".into(),
                    };
                    tray.set_tooltip(&tip);
                }
                if let Some((t, a, ..)) = &info {
                    let title = if a.is_empty() { format!("{t} · Segno") } else { format!("{t} · {a}") };
                    ctx.send_viewport_cmd(ViewportCommand::Title(title));
                }
                self.mark_dirty();
            }
            if let Some(err) = self.ps.error.clone() {
                self.toast(err);
                self.player.state.lk().error = None;
            }
        }

        // While playing, remember the position now and then (in case Windows shuts down).
        if self.ps.playing && self.last_save.elapsed() > Duration::from_secs(30) {
            self.mark_dirty();
        }
        if let Some(t) = self.settings_dirty {
            if t.elapsed() > Duration::from_secs(3) {
                self.save_all();
            }
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        if let Some(h) = self.hwnd {
            platform::bring_to_front(h);
        }
        self.visible = true;
    }

    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        self.save_all();
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        self.visible = false;
    }

    // -----------------------------------------------------------------------------------------
    // Actions

    fn run_actions(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            self.run(ctx, a);
        }
    }

    fn run(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::PlayIds { ids, start, shuffle } => {
                let items = self.qitems(&ids);
                if items.is_empty() {
                    return;
                }
                let start_id = ids.get(start).copied();
                let start = start_id
                    .and_then(|id| items.iter().position(|q| q.id == id))
                    .unwrap_or(0);
                self.player.send(Cmd::PlayList { items, start, shuffle });
                self.mark_dirty();
            }
            Action::PlayPath(p) => {
                let id = path_id(&p);
                self.player.send(Cmd::PlayList {
                    items: vec![QItem::new(id, p)],
                    start: 0,
                    shuffle: None,
                });
            }
            Action::Toggle | Action::Next if self.ps.queue.is_empty() => {
                // Nothing queued yet: play the library from the top.
                let ids = self.songs().to_vec();
                if !ids.is_empty() {
                    self.run(ctx, Action::PlayIds { ids, start: 0, shuffle: None });
                }
            }
            Action::Toggle => self.player.send(Cmd::Toggle),
            Action::Next => self.player.send(Cmd::Next),
            Action::InsertInQueue { at, ids } => {
                let n = ids.len();
                let items = self.qitems(&ids);
                let empty = self.ps.queue.is_empty();
                self.player.send(Cmd::InsertAt { at, items });
                if empty {
                    self.player.send(Cmd::Play);
                }
                self.toast(if n == 1 { "Added to queue".to_string() } else { format!("Added {n} songs to queue") });
            }
            Action::PlayFromAutoplay(id) => {
                let items = self.qitems(&[id]);
                let at = self.ps.index + 1;
                self.player.send(Cmd::InsertAt { at, items });
                self.player.send(Cmd::JumpTo(at));
            }
            Action::Preview(item) => {
                self.yt_requested = true;
                self.yt.preview(item);
            }
            Action::SetAutoplay(v) => {
                self.settings.autoplay = v;
                self.player.send(Cmd::SetAutoplay(v));
                self.mark_dirty();
            }
            Action::Prev => self.player.send(Cmd::Prev),
            Action::Seek(s) => self.player.send(Cmd::Seek(s)),
            Action::Volume(v) => {
                self.player.send(Cmd::SetVolume(v));
                self.settings.volume = v;
                self.mark_dirty();
            }
            Action::ToggleMute => {
                let m = !self.ps.muted;
                self.player.send(Cmd::SetMuted(m));
                self.mark_dirty();
            }
            Action::ToggleShuffle => {
                let on = !self.ps.shuffle;
                self.player.send(Cmd::SetShuffle(on));
                self.settings.shuffle = on;
                self.mark_dirty();
            }
            Action::CycleRepeat => {
                let r = self.ps.repeat.next();
                self.player.send(Cmd::SetRepeat(r));
                self.settings.repeat = r;
                self.mark_dirty();
            }
            Action::PlayNext(ids) => {
                let n = ids.len();
                let items = self.qitems(&ids);
                let empty = self.ps.queue.is_empty();
                self.player.send(Cmd::PlayNext(items));
                if empty {
                    self.player.send(Cmd::Play);
                }
                self.toast(if n == 1 { "Playing next".to_string() } else { format!("{n} songs playing next") });
            }
            Action::AddToQueue(ids) => {
                let n = ids.len();
                let items = self.qitems(&ids);
                let empty = self.ps.queue.is_empty();
                self.player.send(Cmd::AddToQueue(items));
                if empty {
                    self.player.send(Cmd::Play);
                }
                self.toast(if n == 1 { "Added to queue".to_string() } else { format!("Added {n} songs to queue") });
            }
            Action::RemoveFromQueue(uid) => self.player.send(Cmd::RemoveUid(uid)),
            Action::MoveQueue { from, to } => self.player.send(Cmd::Move { from, to }),
            Action::JumpTo(i) => self.player.send(Cmd::JumpTo(i)),
            Action::ClearQueue => self.player.send(Cmd::ClearUpcoming),
            Action::Go(r) => {
                if r == Route::Search {
                    self.focus_search = true;
                }
                self.go(r)
            }
            Action::Back => {
                if let Some(r) = self.back.pop() {
                    let cur = std::mem::replace(&mut self.route, r);
                    self.fwd.push(cur);
                    self.route_t = ctx.input(|i| i.time);
                }
            }
            Action::Forward => {
                if let Some(r) = self.fwd.pop() {
                    let cur = std::mem::replace(&mut self.route, r);
                    self.back.push(cur);
                    self.route_t = ctx.input(|i| i.time);
                }
            }
            Action::ToggleLove(id) => {
                if let Some(t) = self.lib.get(id) {
                    let p = t.path.clone();
                    if self.loved.remove(&id) {
                        self.settings.loved.retain(|x| x != &p);
                    } else {
                        self.loved.insert(id);
                        self.settings.loved.insert(0, p);
                    }
                    self.mark_dirty();
                }
            }
            Action::NewPlaylist { with } => {
                let n = self.playlists.len() + 1;
                let mut name = format!("Playlist {n}");
                let mut k = n;
                while self.playlists.iter().any(|p| p.name == name) {
                    k += 1;
                    name = format!("Playlist {k}");
                }
                let id = store::hash_bytes(format!("{}{}", now_secs(), name).as_bytes());
                let tracks = with.iter().filter_map(|i| self.lib.get(*i).map(|t| t.path.clone())).collect();
                self.playlists.push(Playlist {
                    id,
                    name: name.clone(),
                    tracks,
                    created: now_secs(),
                });
                store::save_playlists(&self.playlists);
                self.go(Route::Playlist(id));
                if with.is_empty() {
                    self.rename = Some((id, name, true));
                } else {
                    self.toast(format!("Added to “{name}”"));
                }
            }
            Action::AddToPlaylist { pid, ids } => {
                let paths: Vec<PathBuf> = ids.iter().filter_map(|i| self.lib.get(*i).map(|t| t.path.clone())).collect();
                if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                    let mut added = 0;
                    for path in paths {
                        if !p.tracks.contains(&path) {
                            p.tracks.push(path);
                            added += 1;
                        }
                    }
                    let name = p.name.clone();
                    store::save_playlists(&self.playlists);
                    self.toast(if added == 0 {
                        format!("Already in “{name}”")
                    } else {
                        format!("Added to “{name}”")
                    });
                }
            }
            Action::RemoveFromPlaylist { pid, index } => {
                if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                    if index < p.tracks.len() {
                        let path = p.tracks.remove(index);
                        let name = p.name.clone();
                        store::save_playlists(&self.playlists);
                        let at = ctx.input(|i| i.time);
                        self.toasts.clear();
                        self.toasts.push(Toast {
                            text: format!("Removed from “{name}”"),
                            at,
                            undo: Some(Undo::PlaylistRemove { pid, index, path }),
                        });
                    }
                }
            }
            Action::MoveInPlaylist { pid, from, to } => {
                if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                    if from < p.tracks.len() && to < p.tracks.len() && from != to {
                        let t = p.tracks.remove(from);
                        p.tracks.insert(to, t);
                        store::save_playlists(&self.playlists);
                    }
                }
            }
            Action::FoldSidebar(fold) => {
                self.settings.sidebar_collapsed = fold;
                self.settings.save();
            }
            Action::MoveInSongs { from, to } => {
                let mut v = self.songs().to_vec();
                if from < v.len() && from != to {
                    let id = v.remove(from);
                    v.insert(to.min(v.len()), id);
                    self.song_order = v.iter().filter_map(|i| self.lib.get(*i).map(|t| t.path.clone())).collect();
                    store::save_song_order(&self.song_order);
                    self.order_version += 1;
                    if self.settings.sort != store::SortBy::Custom {
                        self.settings.sort = store::SortBy::Custom;
                        self.settings.save();
                        self.toast("Songs are in your own order now");
                    }
                }
            }
            Action::StartRename(pid) => {
                if let Some(p) = self.playlist(pid) {
                    self.rename = Some((pid, p.name.clone(), true));
                }
            }
            Action::CommitRename => {
                if let Some((pid, name, _)) = self.rename.take() {
                    let name = name.trim().to_string();
                    if !name.is_empty() {
                        if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                            p.name = name;
                        }
                        store::save_playlists(&self.playlists);
                    }
                }
            }
            Action::AskDeletePlaylist(pid) => self.dialog = Some(Dialog::DeletePlaylist(pid)),
            Action::DeletePlaylist(pid) => {
                self.dialog = None;
                if let Some(i) = self.playlists.iter().position(|p| p.id == pid) {
                    let p = self.playlists.remove(i);
                    store::save_playlists(&self.playlists);
                    if self.route == Route::Playlist(pid) {
                        self.route = Route::Songs;
                    }
                    self.back.retain(|r| *r != Route::Playlist(pid));
                    let at = ctx.input(|i| i.time);
                    self.toasts.clear();
                    self.toasts.push(Toast {
                        text: format!("Deleted “{}”", p.name),
                        at,
                        undo: Some(Undo::PlaylistDelete(p, i)),
                    });
                }
            }
            Action::Reveal(id) => {
                if let Some(t) = self.lib.get(id) {
                    platform::reveal(&t.path);
                }
            }
            Action::AskDeleteFile(id) => self.dialog = Some(Dialog::DeleteFile(id)),
            Action::DeleteFile(id) => {
                self.dialog = None;
                if let Some(t) = self.lib.get(id).cloned() {
                    if self.current_id() == Some(id) {
                        if let Some(q) = self.ps.current() {
                            self.player.send(Cmd::RemoveUid(q.uid));
                        }
                        // Give the audio thread a moment to release the file.
                        std::thread::sleep(Duration::from_millis(150));
                    }
                    if platform::recycle(&t.path) {
                        // A deleted song shouldn't linger in Loved songs or playlists.
                        self.loved.remove(&id);
                        self.settings.loved.retain(|p| p != &t.path);
                        for pl in &mut self.playlists {
                            pl.tracks.retain(|p| p != &t.path);
                        }
                        store::save_playlists(&self.playlists);
                        self.mark_dirty();
                        self.toast(if cfg!(windows) {
                            format!("Moved “{}” to the Recycle Bin", t.title)
                        } else {
                            format!("Deleted “{}”", t.title)
                        });
                        let _ = self.scan_tx.send(ScanCmd::Rescan);
                    } else {
                        self.toast("Couldn't delete the file. It may be open in another app.");
                    }
                }
            }
            Action::YtSearch(q) => {
                self.yt_requested = true;
                if !q.trim().is_empty() {
                    self.yt.search(q);
                }
            }
            Action::YtDownload { items, playlist_name } => {
                self.yt_requested = true;
                let pid = playlist_name.map(|name| {
                    if let Some(p) = self.playlists.iter().find(|p| p.name == name) {
                        p.id
                    } else {
                        let id = store::hash_bytes(format!("{}{}", now_secs(), name).as_bytes());
                        self.playlists.push(Playlist {
                            id,
                            name,
                            tracks: Vec::new(),
                            created: now_secs(),
                        });
                        store::save_playlists(&self.playlists);
                        id
                    }
                });
                let n = items.len();
                self.yt.download(items, pid);
                if n > 1 {
                    self.toast(format!("Downloading {n} songs"));
                }
            }
            Action::YtRetry(uid) => self.yt.retry(uid),
            Action::YtCancel(uid) => self.yt.cancel(uid),
            Action::YtClearFinished => self.yt.clear_finished(),
            Action::YtUpdate => self.yt.update_now(),
            Action::PickFolder => {
                let start = self.settings.music_dir.clone().unwrap_or_else(store::default_music_dir);
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Choose your music folder")
                    .set_directory(&start)
                    .pick_folder()
                {
                    self.set_folder(dir);
                    self.toast("Music folder updated");
                }
            }
            Action::SetFolder(dir) => {
                let _ = std::fs::create_dir_all(&dir);
                self.set_folder(dir);
            }
            Action::Rescan => {
                let _ = self.scan_tx.send(ScanCmd::Rescan);
                self.toast("Checking your music folder");
            }
            Action::OpenFolder => {
                if let Some(d) = &self.settings.music_dir {
                    platform::open_folder(d);
                }
            }
            Action::ToggleMini => self.toggle_mini(ctx),
            Action::ToggleQueue => self.queue_open = !self.queue_open,
            Action::SetCloseToTray(v) => {
                self.settings.close_to_tray = v;
                self.mark_dirty();
            }
            Action::Toast(s) => self.toast(s),
            Action::Undo => {
                if let Some(t) = self.toasts.pop() {
                    match t.undo {
                        Some(Undo::Restart) => self.actions.push(Action::RestartToUpdate),
                        Some(Undo::PlaylistRemove { pid, index, path }) => {
                            if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                                let i = index.min(p.tracks.len());
                                p.tracks.insert(i, path);
                            }
                            store::save_playlists(&self.playlists);
                        }
                        Some(Undo::PlaylistDelete(p, i)) => {
                            let i = i.min(self.playlists.len());
                            self.playlists.insert(i, p);
                            store::save_playlists(&self.playlists);
                        }
                        None => {}
                    }
                }
            }
            Action::CloseDialog => self.dialog = None,
            Action::CheckUpdates => self.updater.check_now(),
            Action::RestartToUpdate => {
                self.save_all();
                match self.updater.restart() {
                    Ok(()) => {
                        self.quitting = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    Err(msg) => self.toast(msg),
                }
            }
            Action::SetAutoUpdate(v) => {
                self.settings.auto_update = v;
                self.updater.auto.store(v, std::sync::atomic::Ordering::Relaxed);
                self.mark_dirty();
            }
            Action::OpenUrl(u) => platform::open_url(&u),
            Action::QuitApp => {
                self.dialog = None;
                self.quitting = true;
                self.save_all();
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Action::HideToTray => {
                self.dialog = None;
                self.hide_to_tray(ctx);
            }
        }
    }

    fn toggle_mini(&mut self, ctx: &egui::Context) {
        self.mini = !self.mini;
        if self.mini {
            self.normal_size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
            self.normal_pos = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min));
            self.was_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            if self.was_maximized {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
            }
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(vec2(220.0, 220.0)));
            ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(300.0, 300.0)));
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            let pos = self.settings.mini_pos.map(|p| pos2(p[0], p[1])).or_else(|| {
                ctx.input(|i| i.viewport().monitor_size)
                    .map(|m| pos2(m.x - 300.0 - 24.0, m.y - 300.0 - 72.0))
            });
            if let Some(p) = pos {
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(p));
            }
        } else {
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                self.settings.mini_pos = Some([r.min.x, r.min.y]);
            }
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
            ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(vec2(880.0, 580.0)));
            let size = self.normal_size.unwrap_or(vec2(1180.0, 760.0));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            if let Some(p) = self.normal_pos {
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(p));
            }
            if self.was_maximized {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
            }
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            if let Some(h) = self.hwnd {
                platform::style_title_bar(h, [BG.r(), BG.g(), BG.b()], [TEXT_2.r(), TEXT_2.g(), TEXT_2.b()]);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Keyboard

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some()) && ctx.egui_wants_keyboard_input();
        ctx.input_mut(|i| {
            let cmd = egui::Modifiers::COMMAND;
            if i.consume_key(cmd, Key::F) {
                self.actions.push(Action::Go(Route::Search));
            }
            if i.consume_key(cmd, Key::ArrowRight) {
                self.actions.push(Action::Next);
            }
            if i.consume_key(cmd, Key::ArrowLeft) {
                self.actions.push(Action::Prev);
            }
            if i.consume_key(cmd, Key::ArrowUp) {
                self.actions.push(Action::Volume((self.ps.volume + 0.08).min(1.0)));
            }
            if i.consume_key(cmd, Key::ArrowDown) {
                self.actions.push(Action::Volume((self.ps.volume - 0.08).max(0.0)));
            }
            if i.consume_key(cmd, Key::M) {
                self.actions.push(Action::ToggleMini);
            }
            if i.consume_key(cmd, Key::B) {
                self.actions.push(Action::FoldSidebar(!self.settings.sidebar_collapsed));
            }
            if i.consume_key(egui::Modifiers::ALT, Key::ArrowLeft) {
                self.actions.push(Action::Back);
            }
            if !typing {
                if i.consume_key(egui::Modifiers::NONE, Key::Space) {
                    self.actions.push(Action::Toggle);
                }
                if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                    if self.mini {
                        self.actions.push(Action::ToggleMini);
                    } else if self.route == Route::NowPlaying {
                        self.actions.push(Action::Back);
                    }
                }
            }
        });
    }

    // -----------------------------------------------------------------------------------------
    // Battery: schedule the next frame only as often as the seek bar actually moves.

    fn schedule_repaint(&self, ctx: &egui::Context, bar_px: f32) {
        if !self.ps.playing || !self.visible {
            return;
        }
        // Minimized or fully covered by other windows: nothing to show, so don't draw.
        if ctx.input(|i| i.viewport().minimized.unwrap_or(false) || i.viewport().visible() == Some(false)) {
            return;
        }
        // One frame per second, landing just after the second ticks over: the time label
        // changes once a second and the seek bar moves a pixel or two. That's ~1 fps while playing.
        let _ = bar_px;
        let to_next_sec = 1.0 - self.pos.fract() + 0.03;
        ctx.request_repaint_after(Duration::from_secs_f32(to_next_sec.clamp(0.05, 1.05)));
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.platform_ready {
            self.platform_ready = true;
            self.hwnd = platform::hwnd_of(frame);
            if let Some(h) = self.hwnd {
                platform::style_title_bar(h, [BG.r(), BG.g(), BG.b()], [TEXT_2.r(), TEXT_2.g(), TEXT_2.b()]);
                let tx = self.media_tx.clone();
                let c = ctx.clone();
                self.media = platform::Media::new(h, tx, move || c.request_repaint());
            }
        }
        self.logic_events(ctx);
        let hidden = ctx.input(|i| i.viewport().visible() == Some(false));
        if hidden {
            self.visible = false;
        }
        // Closing the window: while music plays, keep going in the tray; otherwise quit.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            let playing = self.ps.playing;
            let has_tray = self._tray.is_some();
            if self.mini {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.toggle_mini(ctx);
            } else if playing && has_tray && self.settings.close_to_tray {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                if !self.settings.tray_hint_shown {
                    self.settings.tray_hint_shown = true;
                    self.dialog = Some(Dialog::CloseChoice);
                } else {
                    self.hide_to_tray(ctx);
                }
            } else {
                self.save_all();
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.visible = true;
        if std::env::var_os("SEGNO_FRAMES").is_some() {
            let id = Id::new("frame-count");
            let (n, start): (u32, f64) = ctx.data(|d| d.get_temp(id)).unwrap_or((0, ctx.input(|i| i.time)));
            let now = ctx.input(|i| i.time);
            if now - start >= 5.0 {
                eprintln!("frames in last {:.1}s: {}", now - start, n + 1);
                ctx.data_mut(|d| d.insert_temp(id, (0u32, now)));
            } else {
                ctx.data_mut(|d| d.insert_temp(id, (n + 1, start)));
            }
        }
        self.art.begin_frame(&ctx);
        self.shortcuts(&ctx);

        if self.mini {
            let bar = self.mini_ui(ui);
            self.run_actions(&ctx);
            self.schedule_repaint(&ctx, bar);
            return;
        }

        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, BG);

        if self.settings.music_dir.is_none() {
            self.welcome(ui, full);
            self.run_actions(&ctx);
            return;
        }

        if self.focus_search && self.settings.sidebar_collapsed {
            // The search box lives in the sidebar.
            self.settings.sidebar_collapsed = false;
            self.settings.save();
        }
        let q_t = ctx.animate_bool_with_time(Id::new("queue-open"), self.queue_open, T);
        let c_t = ctx.animate_bool_with_time(Id::new("side-folded"), self.settings.sidebar_collapsed, T);
        let g = GUTTER;
        let bar = Rect::from_min_max(pos2(full.left(), full.bottom() - BAR_H), full.max);

        // Panel widths: your sizes, unless the window is too narrow for them plus the page.
        let open_w = self.settings.sidebar_w.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let mut side_w = egui::lerp(open_w..=RAIL_W, egui::emath::easing::cubic_out(c_t));
        let mut queue_w = self.settings.queue_w.clamp(QUEUE_MIN, QUEUE_MAX);
        let q_e = egui::emath::easing::cubic_out(q_t);
        let room = full.width() - 3.0 * g - (queue_w + g) * q_e - side_w;
        if room < MAIN_MIN {
            let mut short = MAIN_MIN - room;
            let give = ((queue_w - QUEUE_MIN) * q_e).min(short).max(0.0);
            queue_w -= give / q_e.max(0.01);
            short -= give;
            if c_t < 1.0 {
                side_w -= (side_w - SIDEBAR_MIN.min(side_w)).min(short);
            }
        }
        let side = Rect::from_min_max(pos2(full.left() + g, full.top() + g), pos2(full.left() + g + side_w, bar.top()));
        let qw = (queue_w + g) * q_e;
        let main = Rect::from_min_max(pos2(side.right() + g, side.top()), pos2(full.right() - g - qw, bar.top()));

        if c_t < 0.5 {
            // Lay out at full width and let the panel edge clip it while it folds.
            let layout = Rect::from_min_size(side.min, vec2(side_w.max(egui::lerp(open_w..=RAIL_W, 0.5)), side.height()));
            self.sidebar(ui, side, layout);
        } else {
            self.rail(ui, side);
        }
        self.page(ui, main);
        if q_t > 0.0 {
            let qr = Rect::from_min_max(pos2(main.right() + g, main.top()), pos2(main.right() + g + queue_w, main.bottom()));
            self.queue_panel(ui, qr, q_t);
        }
        self.dividers(ui, full, side, main, q_t);
        let seek_w = self.player_bar(ui, bar);
        self.overlays(ui, full);
        self.run_actions(&ctx);
        self.schedule_repaint(&ctx, seek_w);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_all();
        // A downloaded update takes over now, so the next start is the new version.
        self.updater.apply_on_exit();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }
}

// ---------------------------------------------------------------------------------------------
// Frame pieces

impl App {
    fn sidebar(&mut self, ui: &mut Ui, rect: Rect, layout: Rect) {
        ui.painter().rect_filled(rect, R_LG, PANEL);
        let inner = layout.shrink2(vec2(S3, S4));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        child.shrink_clip_rect(rect.shrink(1.0));
        let ui = &mut child;

        // Wordmark, with the fold button on the right.
        let (r, _) = ui.allocate_exact_size(vec2(inner.width(), 28.0), Sense::hover());
        let mark = Rect::from_center_size(pos2(r.left() + 13.0, r.center().y), vec2(26.0, 26.0));
        crate::paint_logo(ui.painter(), mark);
        w::text(ui.painter(), pos2(mark.right() + S2 + 2.0, r.center().y), Align2::LEFT_CENTER, "Segno", FontId::new(17.0, bold()), TEXT, r.width() - 80.0);
        let fold_r = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), Vec2::splat(28.0));
        let fold = ui.interact(fold_r, Id::new("fold-sidebar"), Sense::click());
        if w::icon_button_at(ui, fold_r, fold, icons::PANEL_CLOSE, false, "Collapse sidebar").clicked() {
            self.actions.push(Action::FoldSidebar(true));
        }
        ui.add_space(S4);

        // Search
        let width = inner.width();
        let mut q = std::mem::take(&mut self.search);
        let out = w::search_box(ui, &mut q, "Search or paste a link", self.search_id, width);
        if self.focus_search {
            self.focus_search = false;
            out.response.request_focus();
        }
        if (out.response.gained_focus() || (out.response.has_focus() && !q.is_empty())) && self.route != Route::Search {
            self.actions.push(Action::Go(Route::Search));
        }
        if out.response.changed() {
            if self.route != Route::Search {
                self.actions.push(Action::Go(Route::Search));
            }
            if ytdlp::looks_like_link(&q) && q.trim() != self.yt_last_link {
                // Wait for typing to settle so a typed link doesn't start a lookup per keystroke.
                self.link_pending = Some((q.trim().to_string(), ui.input(|i| i.time)));
                ui.ctx().request_repaint_after(Duration::from_millis(400));
            }
            if q.trim().is_empty() {
                self.yt.clear_search();
            }
        }
        if out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !q.trim().is_empty() {
            self.actions.push(Action::YtSearch(q.trim().to_string()));
            self.actions.push(Action::Go(Route::Search));
        }
        if out.response.has_focus() {
            self.yt.prepare();
        }
        if let Some((link, at)) = self.link_pending.clone() {
            if ui.input(|i| i.time) - at > 0.35 {
                self.link_pending = None;
                if link == q.trim() && link != self.yt_last_link {
                    self.yt_last_link = link.clone();
                    self.actions.push(Action::YtSearch(link));
                }
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        }
        self.search = q;
        ui.add_space(S4);

        let r = self.route.clone();
        let nav = [
            (icons::MUSIC, "Songs", Route::Songs),
            (icons::DISC, "Albums", Route::Albums),
            (icons::USER, "Artists", Route::Artists),
        ];
        for (icon, label, route) in nav {
            let sel = r == route || matches!((&r, &route), (Route::Album(_), Route::Albums) | (Route::Artist(_), Route::Artists));
            if w::nav_item(ui, icon, label, sel, None).clicked() {
                self.actions.push(Action::Go(route));
            }
        }
        let active = self.yt.active_count();
        let badge = (active > 0).then(|| active.to_string());
        if w::nav_item(ui, icons::DOWNLOAD, "Downloads", r == Route::Downloads, badge).clicked() {
            self.actions.push(Action::Go(Route::Downloads));
        }

        ui.add_space(S5);
        let (lr, _) = ui.allocate_exact_size(vec2(inner.width(), 28.0), Sense::hover());
        w::section_label(ui.painter(), pos2(lr.left() + S3, lr.center().y), "Playlists");
        let add_rect = Rect::from_center_size(pos2(lr.right() - 14.0, lr.center().y), Vec2::splat(28.0));
        let add = ui.interact(add_rect, Id::new("new-playlist"), Sense::click());
        if w::icon_button_at(ui, add_rect, add, icons::PLUS, false, "New playlist").clicked() {
            self.actions.push(Action::NewPlaylist { with: vec![] });
        }
        ui.add_space(S1);

        // Settings pinned to the bottom; playlists scroll above it.
        let bottom_h = 36.0 + S2;
        let list_h = (ui.available_height() - bottom_h).max(40.0);
        egui::ScrollArea::vertical()
            .id_salt("sidebar-playlists")
            .max_height(list_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let loved_n = self.loved_ids().len();
                self.playlist_nav_item(ui, None, "Loved songs", loved_n, r == Route::Loved);
                let pls: Vec<(u64, String, usize)> = self
                    .playlists
                    .iter()
                    .map(|p| (p.id, p.name.clone(), p.tracks.iter().filter(|t| self.lib.by_path(t).is_some()).count()))
                    .collect();
                for (id, name, n) in pls {
                    self.playlist_nav_item(ui, Some(id), &name, n, r == Route::Playlist(id));
                }
            });
        let rest = ui.available_height() - 36.0;
        if rest > 0.0 {
            ui.add_space(rest);
        }
        let update_badge = matches!(self.updater.state(), update::State::Ready { .. } | update::State::Available { .. }).then(|| "Update".to_string());
        if w::nav_item(ui, icons::SETTINGS, "Settings", r == Route::Settings, update_badge).clicked() {
            self.actions.push(Action::Go(Route::Settings));
        }
    }

    /// The folded sidebar: icons only, names on hover.
    fn rail(&mut self, ui: &mut Ui, rect: Rect) {
        ui.painter().rect_filled(rect, R_LG, PANEL);
        let inner = rect.shrink2(vec2(S2, S4));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        child.shrink_clip_rect(rect.shrink(1.0));
        let ui = &mut child;
        let wdt = inner.width();
        let r = self.route.clone();

        // Logo; on hover it turns into the unfold button.
        let (lr, lresp) = ui.allocate_exact_size(vec2(wdt, 36.0), Sense::click());
        let hov = ui.ctx().animate_bool_with_time(lresp.id.with("h"), lresp.hovered(), T_FAST);
        let mark = Rect::from_center_size(lr.center(), Vec2::splat(26.0));
        if hov < 1.0 {
            crate::paint_logo(&ui.painter().with_clip_rect(ui.clip_rect()), mark);
        }
        if hov > 0.0 {
            let p = ui.painter();
            p.rect_filled(Rect::from_center_size(lr.center(), Vec2::splat(36.0)), R_MD, with_alpha(RAISED_2, hov));
            icons::glyph(p, lr.center(), icons::PANEL_OPEN, 18.0, with_alpha(TEXT, hov));
        }
        if lresp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Expand sidebar").clicked() {
            self.actions.push(Action::FoldSidebar(false));
        }
        ui.add_space(S3);

        let active = self.yt.active_count();
        let items = [
            (icons::SEARCH, "Search", Route::Search, 0),
            (icons::MUSIC, "Songs", Route::Songs, 0),
            (icons::DISC, "Albums", Route::Albums, 0),
            (icons::USER, "Artists", Route::Artists, 0),
            (icons::DOWNLOAD, "Downloads", Route::Downloads, active),
        ];
        let items = items.map(|(i, l, r, n)| (i, l, r, (n > 0).then(|| n.min(99).to_string())));
        for (icon, label, route, badge) in items {
            let sel = r == route || matches!((&r, &route), (Route::Album(_), Route::Albums) | (Route::Artist(_), Route::Artists));
            if self.rail_button(ui, icon, label, sel, badge).clicked() {
                self.actions.push(Action::Go(route));
            }
        }

        ui.add_space(S2);
        let (sr, _) = ui.allocate_exact_size(vec2(wdt, 1.0), Sense::hover());
        ui.painter().hline(sr.x_range().shrink(S3), sr.center().y, Stroke::new(1.0, HAIRLINE));
        ui.add_space(S2);
        if self.rail_button(ui, icons::PLUS, "New playlist", false, None).clicked() {
            self.actions.push(Action::NewPlaylist { with: vec![] });
        }
        ui.add_space(S1);

        let list_h = (ui.available_height() - 44.0 - S2).max(40.0);
        egui::ScrollArea::vertical()
            .id_salt("rail-playlists")
            .max_height(list_h)
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                let loved_n = self.loved_ids().len();
                self.rail_playlist(ui, None, "Loved songs", loved_n, r == Route::Loved);
                let pls: Vec<(u64, String, usize)> = self
                    .playlists
                    .iter()
                    .map(|p| (p.id, p.name.clone(), p.tracks.iter().filter(|t| self.lib.by_path(t).is_some()).count()))
                    .collect();
                for (id, name, n) in pls {
                    self.rail_playlist(ui, Some(id), &name, n, r == Route::Playlist(id));
                }
            });
        let rest = ui.available_height() - 44.0;
        if rest > 0.0 {
            ui.add_space(rest);
        }
        let update_dot = matches!(self.updater.state(), update::State::Ready { .. } | update::State::Available { .. });
        let tip = if update_dot { "Settings · update ready" } else { "Settings" };
        if self.rail_button(ui, icons::SETTINGS, tip, r == Route::Settings, update_dot.then(String::new)).clicked() {
            self.actions.push(Action::Go(Route::Settings));
        }
    }

    /// `badge`: a count, or an empty string for just a dot.
    fn rail_button(&mut self, ui: &mut Ui, icon: &str, label: &str, selected: bool, badge: Option<String>) -> Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
        let (h, pr) = w::hover_press(ui, &resp);
        let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, T);
        let p = ui.painter();
        let pill = Rect::from_center_size(rect.center(), vec2(44.0, 40.0));
        if h > 0.0 || sel > 0.0 {
            p.rect_filled(pill, R_MD, lerp_color(with_alpha(RAISED, h), RAISED_2, sel));
        }
        let fg = if selected { ACCENT } else { lerp_color(TEXT_2, TEXT, h) };
        icons::glyph(p, rect.center(), icon, 20.0 * (1.0 - 0.06 * pr), fg);
        match badge.as_deref() {
            Some("") => {
                p.circle_filled(pos2(pill.right() - 10.0, pill.top() + 10.0), 4.0, ACCENT);
            }
            Some(n) => {
                let c = pos2(pill.right() - 9.0, pill.top() + 9.0);
                p.circle_filled(c, 8.0, ACCENT);
                w::text(p, c, Align2::CENTER_CENTER, n, FontId::new(10.0, semibold()), ON_ACCENT, f32::INFINITY);
            }
            None => {}
        }
        resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(label)
    }

    fn rail_playlist(&mut self, ui: &mut Ui, pid: Option<u64>, name: &str, count: usize, selected: bool) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
        let (h, _) = w::hover_press(ui, &resp);
        let dragging = egui::DragAndDrop::has_payload_of_type::<DragSongs>(ui.ctx());
        let drop_hover = dragging && resp.contains_pointer();
        let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, T);
        let tile = Rect::from_center_size(rect.center(), vec2(52.0, 52.0));
        let p = ui.painter();
        if h > 0.0 || sel > 0.0 {
            p.rect_filled(tile, R_MD, lerp_color(with_alpha(RAISED, h), RAISED_2, sel));
        }
        if drop_hover {
            p.rect_stroke(tile, R_MD, Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
        }
        let art = Rect::from_center_size(rect.center(), vec2(40.0, 40.0));
        match pid {
            None => {
                p.rect_filled(art, R_SM, ACCENT);
                icons::heart_filled(p, art.center(), 18.0, ON_ACCENT);
            }
            Some(id) => {
                let first = self
                    .playlist(id)
                    .and_then(|pl| pl.tracks.iter().find_map(|path| self.lib.by_path(path).and_then(|t| t.art)));
                let tex = self.art.cover(first, ArtSize::S);
                if first.is_some() {
                    w::art(ui.painter(), art, tex, R_SM, Color32::WHITE);
                } else {
                    ui.painter().rect_filled(art, R_SM, RAISED_2);
                    icons::glyph(ui.painter(), art.center(), icons::LIST_MUSIC, 18.0, TEXT_2);
                }
            }
        }
        let tip = format!("{name}\n{}", theme::plural(count, "song", "songs"));
        let resp = resp.on_hover_text(tip);
        self.playlist_item_events(resp, pid);
    }

    /// Drag handles in the gaps beside the sidebar and the queue. Double-click the sidebar
    /// handle to fold or unfold it.
    fn dividers(&mut self, ui: &mut Ui, full: Rect, side: Rect, main: Rect, q_t: f32) {
        let g = GUTTER;
        let grab = |x: f32| Rect::from_min_max(pos2(x - 3.0, main.top() + R_LG), pos2(x + g + 3.0, main.bottom() - R_LG));

        // Sidebar
        let resp = ui.interact(grab(side.right()), Id::new("side-divider"), Sense::click_and_drag());
        let x = side.right() + g / 2.0;
        self.paint_divider(ui, &resp, x, main);
        if resp.double_clicked() {
            self.actions.push(Action::FoldSidebar(!self.settings.sidebar_collapsed));
        } else if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let want = p.x - full.left() - g - g / 2.0;
                if want < SIDEBAR_FOLD {
                    self.settings.sidebar_collapsed = true;
                } else {
                    self.settings.sidebar_collapsed = false;
                    // Don't let the page go under its minimum.
                    let q = if self.queue_open { self.settings.queue_w.clamp(QUEUE_MIN, QUEUE_MAX) + g } else { 0.0 };
                    let max = (full.width() - 3.0 * g - q - MAIN_MIN).max(SIDEBAR_MIN);
                    self.settings.sidebar_w = want.clamp(SIDEBAR_MIN, SIDEBAR_MAX.min(max));
                }
            }
        }
        if resp.drag_stopped() {
            self.settings.save();
        }

        // Queue
        if q_t >= 1.0 {
            let resp = ui.interact(grab(main.right()), Id::new("queue-divider"), Sense::drag());
            let x = main.right() + g / 2.0;
            self.paint_divider(ui, &resp, x, main);
            if resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let want = full.right() - g - p.x - g / 2.0;
                    let max = (full.width() - 4.0 * g - side.width() - MAIN_MIN).max(QUEUE_MIN);
                    self.settings.queue_w = want.clamp(QUEUE_MIN, QUEUE_MAX.min(max));
                }
            }
            if resp.drag_stopped() {
                self.settings.save();
            }
        }
    }

    fn paint_divider(&self, ui: &Ui, resp: &Response, x: f32, main: Rect) {
        let on = resp.hovered() || resp.dragged();
        let t = ui.ctx().animate_bool_with_time(resp.id.with("h"), on, T_FAST);
        if on {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if t > 0.0 {
            let c = if resp.dragged() { TEXT_2 } else { TEXT_3 };
            let r = Rect::from_center_size(pos2(x, main.center().y), vec2(2.0, main.height() - 2.0 * R_LG));
            ui.painter().rect_filled(r, 1.0, with_alpha(c, t));
        }
    }

    fn playlist_nav_item(&mut self, ui: &mut Ui, pid: Option<u64>, name: &str, count: usize, selected: bool) {
        let wdt = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(wdt, 48.0), Sense::click());
        let (h, _) = w::hover_press(ui, &resp);
        // Drop target for dragged songs.
        let dragging = egui::DragAndDrop::has_payload_of_type::<DragSongs>(ui.ctx());
        let drop_hover = dragging && resp.contains_pointer();
        let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, T);
        let p = ui.painter();
        let bg = lerp_color(with_alpha(RAISED, h), RAISED_2, sel);
        if h > 0.0 || sel > 0.0 {
            p.rect_filled(rect, R_MD, bg);
        }
        if drop_hover {
            p.rect_stroke(rect, R_MD, egui::Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
        }
        let art = Rect::from_min_size(pos2(rect.left() + S2, rect.top() + 6.0), vec2(36.0, 36.0));
        match pid {
            None => {
                p.rect_filled(art, R_SM, ACCENT);
                icons::heart_filled(p, art.center(), 16.0, ON_ACCENT);
            }
            Some(id) => {
                let first = self
                    .playlist(id)
                    .and_then(|pl| pl.tracks.iter().find_map(|path| self.lib.by_path(path).and_then(|t| t.art)));
                let tex = self.art.cover(first, ArtSize::S);
                if first.is_some() {
                    w::art(ui.painter(), art, tex, R_SM, Color32::WHITE);
                } else {
                    ui.painter().rect_filled(art, R_SM, RAISED_2);
                    icons::glyph(ui.painter(), art.center(), icons::LIST_MUSIC, 16.0, TEXT_2);
                }
            }
        }
        let x = art.right() + S3;
        let tw = rect.right() - x - S2;
        let fg = if selected { TEXT } else { lerp_color(TEXT, TEXT, h) };
        w::text(ui.painter(), pos2(x, rect.center().y - 8.0), Align2::LEFT_CENTER, name, body(), fg, tw);
        let sub = if pid.is_none() { theme::plural(count, "song", "songs") } else { format!("Playlist · {}", theme::plural(count, "song", "songs")) };
        w::text(ui.painter(), pos2(x, rect.center().y + 9.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, tw);

        self.playlist_item_events(resp, pid);
    }

    /// Click, drop and right-click for a playlist in the sidebar (full or folded).
    fn playlist_item_events(&mut self, resp: Response, pid: Option<u64>) {
        if let Some(drag) = resp.dnd_release_payload::<DragSongs>() {
            match pid {
                Some(id) => self.actions.push(Action::AddToPlaylist { pid: id, ids: drag.ids.clone() }),
                None => {
                    for id in &drag.ids {
                        if !self.loved.contains(id) {
                            self.actions.push(Action::ToggleLove(*id));
                        }
                    }
                    self.actions.push(Action::Toast("Added to Loved songs".into()));
                }
            }
        }
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            self.actions.push(Action::Go(match pid {
                Some(id) => Route::Playlist(id),
                None => Route::Loved,
            }));
        }
        if let Some(id) = pid {
            resp.context_menu(|ui| {
                if crate::views::menu_item(ui, icons::PENCIL, "Rename").clicked() {
                    self.actions.push(Action::Go(Route::Playlist(id)));
                    self.actions.push(Action::StartRename(id));
                    ui.close();
                }
                if crate::views::menu_item(ui, icons::TRASH, "Delete playlist").clicked() {
                    self.actions.push(Action::AskDeletePlaylist(id));
                    ui.close();
                }
            });
        }
    }

    /// The bottom bar. Returns the seek bar width (used to pace repaints).
    fn player_bar(&mut self, ui: &mut Ui, rect: Rect) -> f32 {
        let cur = self.now_info();
        let has = self.ps.current().is_some();
        let pad = S4;
        let left_w = (rect.width() * 0.3).min(360.0);
        let right_w = left_w;
        let center = Rect::from_min_max(pos2(rect.left() + left_w + pad, rect.top()), pos2(rect.right() - right_w - pad, rect.bottom()));

        // Left: art + title + love (or download, for a song played from YouTube)
        let art_r = Rect::from_min_size(pos2(rect.left() + pad, rect.center().y - 28.0), vec2(56.0, 56.0));
        let art_resp = ui.interact(art_r, Id::new("bar-art"), Sense::click());
        let (ah, _) = w::hover_press(ui, &art_resp);
        if let Some(t) = &cur {
            let tex = self.art.cover(t.art, ArtSize::S);
            w::art(ui.painter(), art_r, tex, R_SM + 2.0, Color32::WHITE);
            if ah > 0.0 {
                ui.painter().rect_filled(art_r, R_SM + 2.0, with_alpha(Color32::BLACK, 0.45 * ah));
                icons::glyph(ui.painter(), art_r.center(), icons::MAXIMIZE, 18.0, with_alpha(TEXT, ah));
            }
            if art_resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Show Now Playing").clicked() {
                self.actions.push(if self.route == Route::NowPlaying { Action::Back } else { Action::Go(Route::NowPlaying) });
            }
            let tx = art_r.right() + S3;
            let tw = (rect.left() + left_w - tx - 40.0).max(40.0);
            let title_r = w::text(ui.painter(), pos2(tx, rect.center().y - 9.0), Align2::LEFT_CENTER, &t.title, body_strong(), TEXT, tw);
            let sub = if t.preview.is_some() { format!("{} · From YouTube", t.artist) } else { t.artist.clone() };
            let artist_r = w::text(ui.painter(), pos2(tx, rect.center().y + 10.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, tw);
            if let Some(ak) = t.artist_key {
                let ar = ui.interact(artist_r, Id::new("bar-artist"), Sense::click());
                if ar.hovered() {
                    ui.painter().line_segment([artist_r.left_bottom(), artist_r.right_bottom()], egui::Stroke::new(1.0, TEXT_2));
                }
                if ar.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    self.actions.push(Action::Go(Route::Artist(ak)));
                }
            }
            let lr = Rect::from_center_size(pos2(title_r.right().max(artist_r.right()) + 22.0, rect.center().y), Vec2::splat(32.0));
            let lresp = ui.interact(lr, Id::new("bar-love"), Sense::click());
            match &t.preview {
                None => {
                    if self.love_button(ui, lr, lresp, t.id).clicked() {
                        self.actions.push(Action::ToggleLove(t.id));
                    }
                }
                Some(item) => self.save_preview_button(ui, lr, lresp, item),
            }
        } else {
            ui.painter().rect_filled(art_r, R_SM + 2.0, RAISED);
            icons::glyph(ui.painter(), art_r.center(), icons::MUSIC, 20.0, TEXT_3);
            let hint = if self.lib.tracks.is_empty() { "Pick a song to start" } else { "Press play to start your library" };
            w::text(ui.painter(), pos2(art_r.right() + S3, rect.center().y), Align2::LEFT_CENTER, hint, body(), TEXT_3, 220.0);
        }

        // Center: controls + seek
        let ctrl_y = rect.center().y - 12.0;
        let cx = center.center().x;
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_center_size(pos2(cx, ctrl_y), vec2(260.0, 40.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        c.spacing_mut().item_spacing.x = S3;
        let shuffle_tip = if self.ps.shuffle { "Shuffle is on" } else { "Shuffle" };
        if w::icon_button(&mut c, 32.0, icons::SHUFFLE, self.ps.shuffle, shuffle_tip).clicked() {
            self.actions.push(Action::ToggleShuffle);
        }
        if w::transport_button(&mut c, 36.0, w::Transport::Prev, "Previous (Ctrl+←)").clicked() {
            self.actions.push(Action::Prev);
        }
        if w::play_circle(&mut c, 40.0, self.ps.playing, true).clicked() {
            self.actions.push(Action::Toggle);
        }
        if w::transport_button(&mut c, 36.0, w::Transport::Next, "Next (Ctrl+→)").clicked() {
            self.actions.push(Action::Next);
        }
        let (rep_icon, rep_tip) = match self.ps.repeat {
            Repeat::Off => (icons::REPEAT, "Repeat"),
            Repeat::All => (icons::REPEAT, "Repeating all"),
            Repeat::One => (icons::REPEAT_1, "Repeating this song"),
        };
        if w::icon_button(&mut c, 32.0, rep_icon, self.ps.repeat != Repeat::Off, rep_tip).clicked() {
            self.actions.push(Action::CycleRepeat);
        }

        let dur = self.now_duration();
        let seek_w = center.width().min(620.0);
        let sy = rect.center().y + 20.0;
        let seek = Rect::from_center_size(pos2(cx, sy), vec2(seek_w - 104.0, 12.0));
        let pos = self.seek_preview.unwrap_or(self.pos);
        let frac = if dur > 0.0 { pos / dur } else { 0.0 };
        let out = w::slider(ui, seek, Id::new("seek"), frac, true);
        if let Some(v) = out.value {
            self.seek_preview = Some(v * dur);
        }
        if out.released {
            if let Some(p) = self.seek_preview.take() {
                if has {
                    self.actions.push(Action::Seek(p));
                    self.pos = p;
                }
            }
        }
        if !out.dragging && !out.released && out.value.is_none() {
            self.seek_preview = None;
        }
        let tcol = TEXT_2;
        w::text(ui.painter(), pos2(seek.left() - S3, sy), Align2::RIGHT_CENTER, &fmt_time(pos), small(), tcol, 60.0);
        w::text(ui.painter(), pos2(seek.right() + S3, sy), Align2::LEFT_CENTER, &fmt_time(dur), small(), tcol, 60.0);

        // Right: queue, mini, volume
        let mut rx = rect.right() - pad;
        let vol_w = 96.0;
        let vol_r = Rect::from_min_max(pos2(rx - vol_w, rect.center().y - 6.0), pos2(rx, rect.center().y + 6.0));
        let shown_vol = if self.ps.muted { 0.0 } else { self.ps.volume };
        let vo = w::slider(ui, vol_r, Id::new("volume"), shown_vol, false);
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
        rx = vol_r.left() - S1;
        let vicon = if self.ps.muted || self.ps.volume <= 0.001 {
            icons::VOLUME_X
        } else if self.ps.volume < 0.35 {
            icons::VOLUME_0
        } else if self.ps.volume < 0.7 {
            icons::VOLUME_1
        } else {
            icons::VOLUME_2
        };
        let vr = Rect::from_center_size(pos2(rx - 16.0, rect.center().y), Vec2::splat(32.0));
        let vresp = ui.interact(vr, Id::new("mute"), Sense::click());
        if w::icon_button_at(ui, vr, vresp, vicon, false, if self.ps.muted { "Unmute" } else { "Mute" }).clicked() {
            self.actions.push(Action::ToggleMute);
        }
        rx = vr.left() - S1;
        let mr = Rect::from_center_size(pos2(rx - 16.0, rect.center().y), Vec2::splat(32.0));
        let mresp = ui.interact(mr, Id::new("mini-btn"), Sense::click());
        if w::icon_button_at(ui, mr, mresp, icons::PIP, false, "Mini player (Ctrl+M)").clicked() {
            self.actions.push(Action::ToggleMini);
        }
        rx = mr.left() - S1;
        let qr = Rect::from_center_size(pos2(rx - 16.0, rect.center().y), Vec2::splat(32.0));
        let qresp = ui.interact(qr, Id::new("queue-btn"), Sense::click());
        if egui::DragAndDrop::has_payload_of_type::<DragSongs>(ui.ctx()) {
            // While a song is being dragged, the queue button invites the drop.
            let over = qresp.contains_pointer();
            let k = ui.ctx().animate_bool_with_time(Id::new("q-btn-drop"), over, T_FAST);
            ui.painter().circle_filled(qr.center(), 18.0 + 3.0 * k, with_alpha(ACCENT, 0.18 + 0.3 * k));
            ui.painter().circle_stroke(qr.center(), 18.0 + 3.0 * k, egui::Stroke::new(1.5, ACCENT));
        }
        if let Some(d) = qresp.dnd_release_payload::<DragSongs>() {
            if d.from_queue.is_none() {
                self.actions.push(Action::AddToQueue(d.ids.clone()));
            }
        }
        if w::icon_button_at(ui, qr, qresp, icons::LIST_MUSIC, self.queue_open, "Queue · drop songs here to add them").clicked() {
            self.actions.push(Action::ToggleQueue);
        }
        seek.width()
    }

    /// For a song played from YouTube: a download button (with progress) in place of the heart.
    pub fn save_preview_button(&mut self, ui: &Ui, rect: Rect, resp: egui::Response, item: &YtItem) {
        let in_lib = self.lib.yt_ids.contains(&item.id);
        let job = self.yt.shared.lk().jobs.iter().find(|j| j.item.id == item.id).map(|j| j.status.clone());
        let (h, pr) = w::hover_press(ui, &resp);
        let c = rect.center();
        let p = ui.painter();
        let job = job.filter(|s| !matches!(s, ytdlp::JobStatus::Done(path) if self.download_gone(path)));
        match (&job, in_lib) {
            (_, true) | (Some(ytdlp::JobStatus::Done(_)), _) => {
                icons::glyph(p, c, icons::CHECK, 16.0, ACCENT);
                let _ = resp.on_hover_text("Saved to your library");
            }
            (Some(ytdlp::JobStatus::Downloading { pct, .. }), _) => {
                w::progress_ring(p, c, 9.0, *pct, ACCENT);
                let _ = resp.on_hover_text("Saving to your library");
            }
            (Some(ytdlp::JobStatus::Waiting | ytdlp::JobStatus::Saving), _) => {
                w::progress_ring(p, c, 9.0, 1.0, ACCENT);
                let _ = resp.on_hover_text("Saving to your library");
            }
            _ => {
                icons::glyph(p, c, icons::ARROW_DOWN_TO_LINE, 16.0 * (1.0 - 0.08 * pr), lerp_color(TEXT_2, TEXT, h));
                let r = resp.on_hover_text("Download to your library").on_hover_cursor(CursorIcon::PointingHand);
                if r.clicked() {
                    self.actions.push(Action::YtDownload { items: vec![item.clone()], playlist_name: None });
                }
            }
        }
    }

    pub fn love_button(&self, ui: &Ui, rect: Rect, resp: egui::Response, id: u64) -> egui::Response {
        let loved = self.loved.contains(&id);
        let (h, pr) = w::hover_press(ui, &resp);
        // A small pop when it becomes loved.
        let pop = ui.ctx().animate_bool_with_time(resp.id.with("pop"), loved, T_SLOW);
        let bump = if loved { (pop * std::f32::consts::PI).sin() * 0.22 } else { 0.0 };
        let s = 16.0 * (1.0 + bump - 0.08 * pr);
        if loved {
            icons::heart_filled(ui.painter(), rect.center(), s, ACCENT);
        } else {
            icons::glyph(ui.painter(), rect.center(), icons::HEART, s, lerp_color(TEXT_2, TEXT, h));
        }
        resp.on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(if loved { "Remove from Loved songs" } else { "Love" })
    }

    fn queue_panel(&mut self, ui: &mut Ui, rect: Rect, t: f32) {
        let a = egui::emath::easing::cubic_out(t);
        let rect = rect.translate(vec2((1.0 - a) * 40.0, 0.0));
        ui.painter().rect_filled(rect, R_LG, with_alpha(PANEL, a));
        if a < 0.3 {
            return;
        }
        // The whole panel takes songs dragged in from anywhere (added to the end).
        let drop_resp = ui.interact(rect, Id::new("queue-drop"), Sense::hover());
        let dragging_in = egui::DragAndDrop::payload::<DragSongs>(ui.ctx())
            .map(|d| d.from_queue.is_none())
            .unwrap_or(false);
        let inner = rect.shrink2(vec2(S3, S4));
        let p = ui.painter().clone();
        w::text(&p, pos2(inner.left() + S1, inner.top() + 12.0), Align2::LEFT_CENTER, "Queue", h2(), TEXT, 200.0);
        let cr = Rect::from_center_size(pos2(inner.right() - 14.0, inner.top() + 12.0), Vec2::splat(28.0));
        let cresp = ui.interact(cr, Id::new("q-close"), Sense::click());
        if w::icon_button_at(ui, cr, cresp, icons::X, false, "Close").clicked() {
            self.actions.push(Action::ToggleQueue);
        }
        let list_r = Rect::from_min_max(pos2(inner.left(), inner.top() + 36.0), pos2(inner.right(), inner.bottom() - if dragging_in { 56.0 } else { 0.0 }));
        {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_r).layout(egui::Layout::top_down(egui::Align::Min)));
            self.queue_list(&mut child, list_r);
        }
        if dragging_in {
            let hover = drop_resp.contains_pointer();
            let hr = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 48.0), inner.max);
            let hl = ui.ctx().animate_bool_with_time(Id::new("q-drop-h"), hover, T_FAST);
            ui.painter().rect_filled(hr, R_MD, lerp_color(RAISED, with_alpha(ACCENT, 0.22), hl));
            ui.painter().rect_stroke(hr, R_MD, egui::Stroke::new(1.0, with_alpha(ACCENT, 0.4 + 0.6 * hl)), egui::StrokeKind::Inside);
            icons::glyph(ui.painter(), pos2(hr.left() + 24.0, hr.center().y), icons::LIST_END, 16.0, ACCENT);
            w::text(ui.painter(), pos2(hr.left() + 44.0, hr.center().y), Align2::LEFT_CENTER, "Drop to add to the queue", body_strong(), TEXT, hr.width() - 56.0);
        }
        if let Some(d) = drop_resp.dnd_release_payload::<DragSongs>() {
            if d.from_queue.is_none() {
                self.actions.push(Action::AddToQueue(d.ids.clone()));
            }
        }
    }

    fn queue_list(&mut self, ui: &mut Ui, list_r: Rect) {
        let Some(cur) = self.ps.current().cloned() else {
            ui.add_space(S8);
            w::empty_state(ui, icons::LIST_MUSIC, "Nothing queued", "Play a song, or drag songs here to line them up.", None);
            return;
        };
        w::section_label(ui.painter(), pos2(list_r.left() + S1, list_r.top() + 10.0), "Now playing");
        ui.add_space(22.0);
        self.queue_row(ui, &cur, self.ps.index, true);
        ui.add_space(S3);
        let upcoming: Vec<(usize, QItem)> = self
            .ps
            .queue
            .iter()
            .enumerate()
            .skip(self.ps.index + 1)
            .map(|(i, q)| (i, q.clone()))
            .collect();
        let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        w::section_label(ui.painter(), pos2(hr.left() + S1, hr.center().y), "Next up");
        if !upcoming.is_empty() {
            let cb = Rect::from_min_size(pos2(hr.right() - 44.0, hr.top()), vec2(44.0, 24.0));
            let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(cb));
            if w::link_button(&mut cu, "Clear").clicked() {
                self.actions.push(Action::ClearQueue);
            }
        }
        if upcoming.is_empty() {
            // Show what plays next from the library, so "Next" is never a dead end.
            let auto = self.autoplay_preview(6);
            if auto.is_empty() {
                ui.add_space(S2);
                w::text(ui.painter(), pos2(list_r.left() + S1, ui.cursor().top() + 10.0), Align2::LEFT_CENTER, "That's the last song.", small(), TEXT_3, 260.0);
                return;
            }
            ui.add_space(S1);
            w::text(ui.painter(), pos2(list_r.left() + S1, ui.cursor().top() + 8.0), Align2::LEFT_CENTER, "From your library", small(), TEXT_3, 260.0);
            ui.add_space(18.0);
            for id in auto {
                self.autoplay_row(ui, id);
            }
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("queue-scroll")
            .auto_shrink([false, false])
            .show_rows(ui, 52.0, upcoming.len(), |ui, range| {
                for (i, q) in &upcoming[range] {
                    self.queue_row(ui, q, *i, false);
                }
            });
    }

    fn autoplay_row(&mut self, ui: &mut Ui, id: u64) {
        let wdt = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(wdt, 52.0), Sense::click());
        let (h, _) = w::hover_press(ui, &resp);
        let Some(info) = self.info_for(id) else { return };
        let p = ui.painter().clone();
        w::hover_surface(&p, rect, h);
        let art_r = Rect::from_min_size(pos2(rect.left() + S1 + 2.0, rect.top() + 6.0), vec2(40.0, 40.0));
        let tex = self.art.cover(info.art, ArtSize::S);
        w::art(&p, art_r, tex, R_SM, with_alpha(Color32::WHITE, 0.7));
        let tx = art_r.right() + S3;
        let tw = rect.right() - tx - 8.0;
        w::text(&p, pos2(tx, rect.center().y - 8.0), Align2::LEFT_CENTER, &info.title, body(), lerp_color(TEXT_2, TEXT, h), tw);
        w::text(&p, pos2(tx, rect.center().y + 9.0), Align2::LEFT_CENTER, &info.artist, small(), TEXT_3, tw);
        if resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Play now").clicked() {
            self.actions.push(Action::PlayFromAutoplay(id));
        }
    }

    fn queue_row(&mut self, ui: &mut Ui, q: &QItem, index: usize, current: bool) {
        let wdt = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(wdt, 52.0), Sense::click_and_drag());
        let (h, _) = w::hover_press(ui, &resp);
        let info = self.info_for(q.id);
        let p = ui.painter().clone();
        w::hover_surface(&p, rect, h);
        let art_r = Rect::from_min_size(pos2(rect.left() + S1 + 2.0, rect.top() + 6.0), vec2(40.0, 40.0));
        let tex = self.art.cover(info.as_ref().and_then(|t| t.art), ArtSize::S);
        w::art(&p, art_r, tex, R_SM, Color32::WHITE);
        let tx = art_r.right() + S3;
        let tw = rect.right() - tx - 36.0;
        let (title, artist) = match &info {
            Some(t) => (t.title.clone(), t.artist.clone()),
            None => (q.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), String::new()),
        };
        w::text(&p, pos2(tx, rect.center().y - 8.0), Align2::LEFT_CENTER, &title, body(), if current { ACCENT } else { TEXT }, tw);
        w::text(&p, pos2(tx, rect.center().y + 9.0), Align2::LEFT_CENTER, &artist, small(), TEXT_2, tw);
        if !current && h > 0.05 {
            let xr = Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), Vec2::splat(28.0));
            let xresp = ui.interact(xr, resp.id.with("x"), Sense::click());
            if w::icon_button_at(ui, xr, xresp, icons::X, false, "Remove from queue").clicked() {
                self.actions.push(Action::RemoveFromQueue(q.uid));
            }
        }
        if !current && resp.drag_started() {
            egui::DragAndDrop::set_payload(
                ui.ctx(),
                DragSongs {
                    ids: vec![q.id],
                    from_playlist: None,
                    from_queue: Some(index),
                    from_songs: None,
                },
            );
        }
        // Drop line: songs dragged from the queue move; songs from elsewhere are inserted.
        let above = ui.input(|i| i.pointer.hover_pos().map(|p| p.y < rect.center().y).unwrap_or(true)) && !current;
        if resp.dnd_hover_payload::<DragSongs>().is_some() {
            let y = if above { rect.top() } else { rect.bottom() };
            ui.painter().line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], egui::Stroke::new(2.0, ACCENT));
        }
        if let Some(d) = resp.dnd_release_payload::<DragSongs>() {
            let to = if above { index } else { index + 1 };
            match d.from_queue {
                Some(from) => {
                    let mut to = to;
                    if from < to {
                        to -= 1;
                    }
                    self.actions.push(Action::MoveQueue { from, to });
                }
                None => self.actions.push(Action::InsertInQueue { at: to, ids: d.ids.clone() }),
            }
        }
        if resp.double_clicked() || (resp.clicked() && !current) {
            if !current {
                self.actions.push(Action::JumpTo(index));
            }
        }
        if self.lib.get(q.id).is_some() {
            let id = q.id;
            resp.context_menu(|ui| {
                self.song_menu(ui, id, None);
            });
        }
    }

    fn overlays(&mut self, ui: &mut Ui, full: Rect) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        // Toasts: one at a time, above the player bar.
        let life_of = |t: &Toast| match t.undo {
            Some(Undo::Restart) => 10.0,
            Some(_) => 6.0,
            None => 3.2,
        };
        self.toasts.retain(|t| now - t.at < life_of(t));
        if let Some(t) = self.toasts.last() {
            let age = (now - t.at) as f32;
            let life = life_of(t) as f32;
            let act = if matches!(t.undo, Some(Undo::Restart)) { "Restart" } else { "Undo" };
            let a_in = egui::emath::easing::cubic_out((age / 0.22).min(1.0));
            let a_out = ((life - age) / 0.18).clamp(0.0, 1.0);
            let a = a_in.min(a_out);
            let font = body_strong();
            let tw = w::text_width(ui, &t.text, font.clone()).min(520.0);
            let undo_w = if t.undo.is_some() { w::text_width(ui, act, body_strong()) + 28.0 } else { 0.0 };
            let size = vec2(tw + S5 * 2.0 + undo_w, 44.0);
            let center = pos2(full.center().x, full.bottom() - BAR_H - 36.0 + (1.0 - a_in) * 12.0);
            let r = Rect::from_center_size(center, size);
            let layer = egui::LayerId::new(egui::Order::Foreground, Id::new("toast"));
            let p = ctx.layer_painter(layer);
            p.add(egui::epaint::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha((120.0 * a) as u8) }.as_shape(r, 22.0));
            p.rect_filled(r, 22.0, with_alpha(Color32::from_rgb(0xF2, 0xF2, 0xF4), a));
            w::text(&p, pos2(r.left() + S5, r.center().y), Align2::LEFT_CENTER, &t.text, font, with_alpha(BG, a), tw);
            if t.undo.is_some() {
                let ur = Rect::from_min_max(pos2(r.right() - undo_w - S2, r.top() + 6.0), pos2(r.right() - 6.0, r.bottom() - 6.0));
                let resp = ui.interact(ur, Id::new("toast-undo"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                let (h, _) = w::hover_press(ui, &resp);
                p.rect_filled(ur, 16.0, with_alpha(Color32::from_black_alpha(20), h));
                w::text(&p, ur.center(), Align2::CENTER_CENTER, act, body_strong(), with_alpha(ACCENT_LO, a), 80.0);
                if resp.clicked() {
                    self.actions.push(Action::Undo);
                }
            }
            if age < life {
                ctx.request_repaint_after(Duration::from_secs_f32(if a < 1.0 { 0.016 } else { (life - age - 0.18).max(0.016) }));
            }
        }

        // Drag ghost
        if let Some(d) = egui::DragAndDrop::payload::<DragSongs>(&ctx) {
            if let Some(pos) = ctx.pointer_interact_pos() {
                let label = match d.ids.first().and_then(|id| self.lib.get(*id)) {
                    Some(t) if d.ids.len() == 1 => t.title.clone(),
                    _ => format!("{} songs", d.ids.len()),
                };
                let layer = egui::LayerId::new(egui::Order::Tooltip, Id::new("drag-ghost"));
                let p = ctx.layer_painter(layer);
                let tw = w::text_width(ui, &label, body_strong()).min(260.0);
                let r = Rect::from_min_size(pos + vec2(14.0, 10.0), vec2(tw + 24.0, 32.0));
                p.rect_filled(r, 16.0, ACCENT);
                w::text(&p, r.center(), Align2::CENTER_CENTER, &label, body_strong(), ON_ACCENT, tw);
                ctx.set_cursor_icon(CursorIcon::Grabbing);
            }
        }

        if let Some(d) = &self.dialog {
            let (title, body_text, primary, secondary, danger): (String, String, &str, &str, bool) = match d {
                Dialog::DeleteFile(id) => {
                    let name = self.lib.get(*id).map(|t| t.title.clone()).unwrap_or_default();
                    (
                        format!("Delete “{name}”?"),
                        if cfg!(windows) {
                            "The file moves to the Recycle Bin, so you can get it back from there.".into()
                        } else {
                            "The file will be deleted from your music folder.".into()
                        },
                        "Delete song",
                        "Keep song",
                        true,
                    )
                }
                Dialog::DeletePlaylist(pid) => {
                    let name = self.playlist(*pid).map(|p| p.name.clone()).unwrap_or_default();
                    (
                        format!("Delete “{name}”?"),
                        "The playlist goes away. The songs stay in your library.".into(),
                        "Delete playlist",
                        "Keep playlist",
                        true,
                    )
                }
                Dialog::CloseChoice => (
                    "Keep the music going?".into(),
                    "Segno can keep playing from the tray, near the clock, when you close the window. Click the tray icon to come back.".into(),
                    "Keep playing in tray",
                    "Quit Segno",
                    false,
                ),
            };
            let mut close = false;
            let mut primary_hit = false;
            let mut secondary_hit = false;
            let modal = egui::Modal::new(Id::new("dialog"))
                .frame(egui::Frame::new().fill(OVERLAY).corner_radius(R_LG).inner_margin(S6).stroke(egui::Stroke::new(1.0, HAIRLINE)))
                .show(&ctx, |ui| {
                    ui.set_width(380.0);
                    ui.label(egui::RichText::new(title).font(h2()).color(TEXT));
                    ui.add_space(S2);
                    ui.label(egui::RichText::new(body_text).font(body()).color(TEXT_2));
                    ui.add_space(S5);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let kind = PillKind::Primary;
                        let pr = w::pill(ui, primary, None, kind);
                        if danger {
                            // Destructive: the primary is still the action the person came for.
                        }
                        if pr.clicked() {
                            primary_hit = true;
                        }
                        if w::pill(ui, secondary, None, PillKind::Ghost).clicked() {
                            secondary_hit = true;
                        }
                    });
                });
            if modal.should_close() {
                close = true;
            }
            let d = self.dialog.as_ref().unwrap();
            if primary_hit {
                let a = match d {
                    Dialog::DeleteFile(id) => Action::DeleteFile(*id),
                    Dialog::DeletePlaylist(pid) => Action::DeletePlaylist(*pid),
                    Dialog::CloseChoice => Action::HideToTray,
                };
                self.actions.push(a);
            } else if secondary_hit {
                match d {
                    Dialog::CloseChoice => {
                        self.settings.close_to_tray = false;
                        self.actions.push(Action::QuitApp);
                    }
                    _ => self.actions.push(Action::CloseDialog),
                }
            } else if close {
                self.actions.push(Action::CloseDialog);
            }
        }
    }
}

use egui::FontId;
use egui::Vec2;
