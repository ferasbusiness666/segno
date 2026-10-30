//! App state, the frame layout (sidebar, page, queue, player bar), and everything that reacts to
//! the player, the library and downloads.

use crate::store::LockExt;
use crate::art::{ArtCache, ArtSize};
use crate::library::{self, Library, ScanCmd};
use crate::platform::{self, MediaAction, TrayAction};
use crate::player::{self, Cmd, PlayerHandle, QItem};
use crate::store::{self, now_secs, path_id, Playlist, Settings};
use crate::theme::{self, *};
use crate::widgets::{self as w, PillKind};
use crate::update::{self, Updater};
use crate::ytdlp::{self, Yt, YtItem};
use crate::icons;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Key, Rect, Response, Sense, Stroke, Ui, ViewportCommand};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// "For you" (suggestions from YouTube mixes) is hidden until it's good enough to show.
pub const FOR_YOU: bool = false;

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
    ForYou,
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
    /// Look the lyrics up again, even if they weren't found last time.
    RetryLyrics,
    Volume(f32),
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    PlayNext(Vec<u64>),
    AddToQueue(Vec<u64>),
    InsertInQueue { at: usize, ids: Vec<u64> },
    PlayFromAutoplay(u64),
    /// Play a song from its site without downloading it.
    Preview(YtItem),
    /// Download a playlist's songs that still play from their site.
    DownloadPlaylist(u64),
    /// Download one song that plays from its site (by its track id).
    DownloadStream(u64),
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
    /// Dropped into a playlist at a spot (`at` is the entry it goes before; `None` = the end).
    InsertInPlaylist { pid: u64, at: Option<usize>, ids: Vec<u64> },
    RemoveFromPlaylist { pid: u64, index: usize },
    MoveInPlaylist { pid: u64, from: usize, to: usize },
    /// Change the order of the playlists in the sidebar.
    MovePlaylist { from: usize, to: usize },
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
    /// Keep a playlist from a link in the sidebar and download its songs into it.
    SavePlaylist { info: ytdlp::PlaylistInfo, items: Vec<YtItem> },
    /// Pick a picture for a playlist from your computer.
    PickPlaylistCover(u64),
    RemovePlaylistCover(u64),
    RetryPlaylist(u64),
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
    ToggleImmersive,
    SetCloseToTray(bool),
    Toast(String),
    Undo,
    CloseDialog,
    AcceptDownloadRights,
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
    /// Shown once, before the first download: only download what you're allowed to.
    DownloadRights,
}

/// Drag payload: the songs being dragged. Where they came from and how the row moves is in drag.rs.
#[derive(Clone)]
pub struct DragSongs {
    pub ids: Vec<u64>,
    pub from_queue: Option<usize>,
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
    /// Its file in the preview cache, once fetched.
    pub path: Option<PathBuf>,
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
    /// Songs that play from their site without being downloaded, by `store::stream_id`.
    pub transient: std::collections::HashMap<u64, Transient>,
    /// The same songs, shared with the audio thread so it can fetch them while the window sleeps.
    pub stream_reg: Arc<std::sync::Mutex<std::collections::HashMap<u64, YtItem>>>,
    /// Which song key each downloaded file is (until the library has scanned it).
    path_keys: std::collections::HashMap<PathBuf, String>,
    /// Playlist covers fetched in the background: (playlist, art key, color).
    cover_tx: std::sync::mpsc::Sender<(u64, u64, [u8; 3])>,
    cover_rx: Receiver<(u64, u64, [u8; 3])>,
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
    /// The song list's column titles are pinned under the top bar this frame.
    pub sticky_cols: bool,
    pub search: String,
    pub search_id: Id,
    pub focus_search: bool,
    pub yt_last_link: String,
    pub yt_requested: bool,
    link_pending: Option<(String, f64)>,
    pub queue_open: bool,
    /// Full-screen Now Playing.
    pub immersive: bool,
    /// When full screen was last turned on or off (for the fade between the two views).
    pub imm_since: f64,
    /// The window as it was before filling the screen (Windows: borderless, no mode switch).
    fs_saved: Option<platform::SavedWindow>,
    /// Up next beside the record, on the Now Playing page and in full screen.
    pub np_queue: bool,
    /// Lyrics of the song `lyrics_for`, looked up by the lyrics worker.
    pub lyrics: crate::lyrics::State,
    pub lyrics_for: Option<u64>,
    lyrics_asked: f64,
    lyrics_tx: std::sync::mpsc::Sender<crate::lyrics::Query>,
    lyrics_rx: Receiver<(u64, crate::lyrics::State)>,
    pub lyr: crate::nowplaying::LyricsView,
    pub imm_queue: bool,
    pub vinyl_angle: f32,
    pub backdrop_angle: f32,
    pub vinyl_last: f64,
    /// When the mouse last moved in full screen (the controls fade after a few quiet seconds).
    pub imm_active: f64,
    /// Songs you played, for suggestions.
    pub history: Vec<crate::foryou::Played>,
    pub played_noted: Option<u64>,
    pub suggest_turn: u64,
    pub suggest_auto_tried: bool,
    /// "Add songs" box next to the queue.
    pub add_query: String,
    /// The search box in an empty playlist.
    pub empty_query: String,
    /// The part of Settings that's open, and when it was picked (for the fade).
    pub settings_section: crate::views::SettingsSection,
    pub settings_section_t: f64,
    pub add_focus: bool,
    pub mini: bool,
    normal_size: Option<egui::Vec2>,
    normal_pos: Option<egui::Pos2>,
    was_maximized: bool,
    quitting: bool,
    pub toasts: Vec<Toast>,
    pub dialog: Option<Dialog>,
    /// The "What's new" window (after an update, or opened from Settings).
    pub whats_new: Option<crate::changelog::WhatsNew>,
    /// Where the sidebar's page highlight last was.
    nav_hl: Option<Rect>,
    /// A playlist being dragged to a new place in the sidebar: (its id, where it was, where the
    /// pointer holds it).
    pl_drag: Option<(u64, usize, f32)>,
    /// The song that was playing last frame, and the one before it (with when it changed)
    /// while the views cross-fade between them.
    swap_last: Option<u64>,
    swap_prev: Option<(NowInfo, f64)>,
    /// When a song was last dropped (the queue holds still a moment while it settles).
    pub flip_hold: f64,
    /// Popups and menus on screen, when each opened and where it grows from.
    pop_seen: HashMap<Id, (f64, egui::Pos2)>,
    /// What you pressed play on, shown above the queue ("From Road Trip Mix").
    pub play_from: Option<String>,
    /// The song being dragged, and the animations around it (see drag.rs).
    pub lift: Option<crate::drag::Lift>,
    pub drag_aim: Option<Rect>,
    pub settle: Option<crate::drag::Settle>,
    pub fly: Option<crate::drag::Fly>,
    pub row_shift: HashMap<(Id, usize), f32>,
    /// A download waiting for the "only download what you're allowed to" answer.
    pending_download: Option<Action>,
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

        let (cover_tx, cover_rx) = channel();
        let (lyrics_reply, lyrics_rx) = channel();
        let lyrics_tx = {
            let ctx2 = ctx.clone();
            crate::lyrics::worker(move |id, st| {
                let _ = lyrics_reply.send((id, st));
                ctx2.request_repaint();
            })
        };
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
            stream_reg: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            path_keys: std::collections::HashMap::new(),
            cover_tx,
            cover_rx,
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
            sticky_cols: false,
            search: String::new(),
            search_id: Id::new("search-box"),
            focus_search: false,
            yt_last_link: String::new(),
            yt_requested: false,
            link_pending: None,
            queue_open: false,
            immersive: false,
            imm_since: -10.0,
            fs_saved: None,
            np_queue: true,
            lyrics: crate::lyrics::State::Missing,
            lyrics_for: None,
            lyrics_asked: 0.0,
            lyrics_tx,
            lyrics_rx,
            lyr: Default::default(),
            imm_queue: false,
            vinyl_angle: 0.0,
            backdrop_angle: 0.0,
            vinyl_last: 0.0,
            imm_active: 0.0,
            history: crate::foryou::load_history(),
            played_noted: None,
            suggest_turn: 0,
            suggest_auto_tried: false,
            add_query: String::new(),
            empty_query: String::new(),
            settings_section: Default::default(),
            settings_section_t: 0.0,
            add_focus: false,
            mini: false,
            normal_size: None,
            normal_pos: None,
            was_maximized: false,
            quitting: false,
            toasts: Vec::new(),
            dialog: None,
            whats_new: None,
            play_from: None,
            nav_hl: None,
            pl_drag: None,
            swap_last: None,
            swap_prev: None,
            flip_hold: 0.0,
            pop_seen: HashMap::new(),
            lift: None,
            drag_aim: None,
            settle: None,
            fly: None,
            row_shift: HashMap::new(),
            pending_download: None,
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
        // The audio thread asks for streamed songs itself and hears back directly.
        {
            let reg = app.stream_reg.clone();
            let fetcher = app.yt.fetcher();
            let ph = app.player.clone();
            *app.player.fetcher.lk() = Some(Arc::new(move |id: u64| {
                let item = reg.lk().get(&id).cloned();
                let Some(item) = item else {
                    ph.send(Cmd::FetchFailed { id });
                    return;
                };
                let ph2 = ph.clone();
                fetcher.fetch(item, move |res| {
                    ph2.send(match res {
                        Ok(path) => Cmd::Fetched { id, path },
                        Err(_) => Cmd::FetchFailed { id },
                    })
                });
            }));
        }
        app.register_playlist_streams();
        let r = app.settings.resume.clone();
        for it in r.streams.iter().cloned() {
            app.register_stream(it);
        }
        if !r.queue.is_empty() {
            let mk = |v: &Vec<PathBuf>| {
                v.iter()
                    .map(|p| match store::stream_key(p) {
                        Some(k) => app.stream_qitem(store::stream_id(k)).unwrap_or_else(|| QItem::stream(store::stream_id(k), p.clone())),
                        None => QItem::new(path_id(p), p.clone()),
                    })
                    .collect::<Vec<_>>()
            };
            let queue: Vec<QItem> = mk(&r.queue)
                .into_iter()
                .enumerate()
                .map(|(i, q)| q.with_origin(player::Origin::from_code(r.origins.get(i).copied().unwrap_or(0))))
                .collect();
            app.play_from = r.from.clone();
            // Keep uids shared between queue and original for the same entries.
            let mut original = Vec::new();
            let mut used = vec![false; queue.len()];
            for p in &r.original {
                let same = |q: &QItem| match store::stream_key(p) {
                    Some(k) => q.id == store::stream_id(k),
                    None => &q.path == p,
                };
                if let Some(i) = queue.iter().enumerate().position(|(i, q)| !used[i] && same(q)) {
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
                app.whats_new = Some(crate::changelog::WhatsNew::Since(app.settings.last_version.clone()));
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
            .filter_map(|id| match self.lib.get(*id) {
                Some(t) => Some(QItem::new(t.id, t.path.clone())),
                None => self.stream_qitem(*id),
            })
            .collect()
    }

    /// While the song is changing: the old song and how far the change is (0..1, eased) over
    /// `secs`.
    pub fn song_swap(&self, ctx: &egui::Context, secs: f32) -> Option<(NowInfo, f32)> {
        let (old, at) = self.swap_prev.as_ref()?;
        let t = ((ctx.input(|i| i.time) - at) as f32 / secs).clamp(0.0, 1.0);
        if t >= 1.0 {
            return None;
        }
        ctx.request_repaint();
        Some((old.clone(), egui::emath::easing::cubic_out(t)))
    }

    /// Queue entries for songs you add by hand (Play next, Add to queue, dragged in).
    pub fn added_items(&self, ids: &[u64]) -> Vec<QItem> {
        self.qitems(ids).into_iter().map(|q| q.with_origin(player::Origin::Added)).collect()
    }

    /// The name of what's being played from, for the queue ("Road Trip Mix", "Songs"…).
    fn source_label(&self, ids: &[u64]) -> Option<String> {
        let same = |v: &[u64]| v.len() == ids.len() && v == ids;
        match &self.route {
            Route::Songs => Some("Songs".into()),
            Route::Loved => Some("Loved songs".into()),
            Route::ForYou => Some("For you".into()),
            Route::Search => Some("your search".into()),
            Route::Playlist(pid) => self.playlist(*pid).map(|p| p.name.clone()),
            Route::Album(k) => self.lib.album(*k).map(|a| a.title.clone()),
            Route::Artist(k) => self.lib.artist(*k).map(|a| a.name.clone()),
            // The play button on an album or artist card.
            Route::Albums => self.lib.albums.iter().find(|a| same(&a.tracks)).map(|a| a.title.clone()),
            Route::Artists => self.lib.artists.iter().find(|a| same(&a.tracks)).map(|a| a.name.clone()),
            _ => None,
        }
    }

    /// A playlist's picture: its own cover, else the first song's.
    pub fn playlist_art(&self, pid: u64) -> (Option<u64>, Option<[u8; 3]>) {
        let Some(p) = self.playlist(pid) else { return (None, None) };
        if p.cover.is_some() {
            return (p.cover, p.color);
        }
        let first = p.tracks.iter().find_map(|path| self.lib.by_path(path).filter(|t| t.art.is_some()));
        (first.and_then(|t| t.art), first.and_then(|t| t.color))
    }

    pub fn current_id(&self) -> Option<u64> {
        self.ps.current().map(|q| q.id)
    }

    /// Put songs in a playlist: before entry `at`, or at the end. Songs already in it stay put.
    fn add_to_playlist(&mut self, pid: u64, at: Option<usize>, ids: Vec<u64>) {
        // Files by path; songs that play from their site as stream entries.
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut streams: Vec<YtItem> = Vec::new();
        for i in &ids {
            if let Some(t) = self.lib.get(*i) {
                paths.push(t.path.clone());
            } else if let Some(tr) = self.transient.get(i) {
                paths.push(store::stream_entry(&tr.item.id));
                streams.push(tr.item.clone());
            }
        }
        let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) else { return };
        let mut added = 0;
        for it in streams {
            p.streams.insert(it.id.clone(), it);
        }
        let mut at = at.map(|a| a.min(p.tracks.len()));
        for path in paths {
            if p.tracks.contains(&path) {
                continue;
            }
            match at.as_mut() {
                Some(a) => {
                    p.tracks.insert(*a, path);
                    *a += 1;
                }
                None => p.tracks.push(path),
            }
            added += 1;
        }
        let name = p.name.clone();
        store::save_playlists(&self.playlists);
        self.toast(if added == 0 { format!("Already in “{name}”") } else { format!("Added to “{name}”") });
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
                artist_key: Some(t.artist_key()),
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

    /// The side panel on Now Playing / full screen is showing lyrics.
    pub fn lyrics_mode(&self) -> bool {
        self.settings.show_lyrics && self.settings.lyrics_panel
    }

    /// Open, close or switch the side panel (`lyrics`: the lyrics button, else Up next).
    pub fn toggle_side(&mut self, full_screen: bool, lyrics: bool) {
        let mode = self.lyrics_mode();
        let open = if full_screen { &mut self.imm_queue } else { &mut self.np_queue };
        if *open && mode == lyrics {
            *open = false;
        } else {
            *open = true;
            if self.settings.lyrics_panel != lyrics {
                self.settings.lyrics_panel = lyrics;
                self.settings.save();
            }
        }
    }

    /// Look the lyrics up while the lyrics panel shows the song playing. Nothing is sent
    /// before it's opened.
    fn sync_lyrics(&mut self) {
        let open = if self.immersive { self.imm_queue } else { self.route == Route::NowPlaying && self.np_queue };
        let Some(id) = self.current_id().filter(|_| open && self.lyrics_mode()) else { return };
        let now = self.ctx.input(|i| i.time);
        // Couldn't reach the site last time: ask again every half a minute while it's open.
        let retry = matches!(self.lyrics, crate::lyrics::State::Failed) && now - self.lyrics_asked > 30.0;
        if self.lyrics_for == Some(id) && !retry {
            return;
        }
        self.ask_lyrics(id, false);
    }

    /// Send a song to the lyrics worker (`force`: even if it wasn't found last time).
    fn ask_lyrics(&mut self, id: u64, force: bool) {
        let Some(info) = self.info_for(id) else { return };
        if self.lyrics_for != Some(id) {
            self.lyr = Default::default();
        }
        self.lyrics_for = Some(id);
        self.lyrics = crate::lyrics::State::Loading;
        self.lyrics_asked = self.ctx.input(|i| i.time);
        // A song playing from a video: its own title and channel say more than the names
        // cleaned from them (Spotify imports already have clean names).
        let (title, artist, from_video) = match &info.preview {
            Some(item) if item.find.is_empty() => (item.title.clone(), item.channel.clone(), true),
            _ => (info.title, info.artist, false),
        };
        let artist = if artist == crate::library::UNKNOWN_ARTIST { String::new() } else { artist };
        let _ = self.lyrics_tx.send(crate::lyrics::Query {
            id,
            song: self.lib.get(id).map(|t| t.path.clone()),
            title,
            artist,
            from_video,
            duration: info.duration,
            force,
        });
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
        // Same as the player: songs not queued yet; if everything was, it goes around again.
        let pick = |skip_queued: bool| {
            let mut out = Vec::new();
            for k in 0..songs.len() {
                let id = songs[(start + k) % songs.len()];
                if Some(id) == cur || (skip_queued && queued.contains(&id)) {
                    continue;
                }
                out.push(id);
                if out.len() >= n {
                    break;
                }
            }
            out
        };
        let out = pick(true);
        if out.is_empty() {
            pick(false)
        } else {
            out
        }
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

    /// Track ids of a playlist (files and songs that play from their site), skipping files
    /// that are gone.
    pub fn playlist_ids(&self, pid: u64) -> Vec<(usize, u64)> {
        self.playlist(pid)
            .map(|p| p.tracks.iter().enumerate().filter_map(|(i, path)| self.entry_id(path).map(|id| (i, id))).collect())
            .unwrap_or_default()
    }

    /// The track id behind a playlist entry.
    pub fn entry_id(&self, path: &std::path::Path) -> Option<u64> {
        if let Some(t) = self.lib.by_path(path) {
            return Some(t.id);
        }
        let key = match store::stream_key(path) {
            Some(k) => k.to_string(),
            // Just downloaded and not scanned yet: keep showing it as the song it was.
            None => self.path_keys.get(path)?.clone(),
        };
        let id = store::stream_id(&key);
        self.transient.contains_key(&id).then_some(id)
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
            // Wake up to save even if nothing else happens on screen.
            self.ctx.request_repaint_after(Duration::from_millis(3200));
        }
    }

    fn save_resume(&mut self) {
        let s = self.player.snapshot();
        // Streamed songs are saved by key (their cached file may be cleaned up before next time).
        let mut streams: Vec<YtItem> = Vec::new();
        let mut entry = |q: &QItem| -> PathBuf {
            match self.transient.get(&q.id).filter(|_| q.stream) {
                Some(tr) => {
                    if !streams.iter().any(|i| i.id == tr.item.id) {
                        streams.push(tr.item.clone());
                    }
                    store::stream_entry(&tr.item.id)
                }
                None => q.path.clone(),
            }
        };
        let queue = s.queue.iter().map(&mut entry).collect();
        let original = s.original.iter().map(&mut entry).collect();
        self.settings.resume = store::Resume {
            queue,
            original,
            index: s.index,
            position: self.player.position(),
            streams,
            origins: s.queue.iter().map(|q| q.origin.code()).collect(),
            from: self.play_from.clone(),
        };
        self.settings.volume = s.volume;
        self.settings.muted = s.muted;
        self.settings.shuffle = s.shuffle;
        self.settings.repeat = s.repeat;
    }

    fn remember_window(&mut self) {
        if self.mini || self.immersive {
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
            self.resolve_streams();
        }
        while let Ok(ev) = self.yt.events.try_recv() {
            match ev {
                ytdlp::YtEvent::Downloaded { path, playlist, key } => {
                    self.path_keys.insert(path.clone(), key.clone());
                    self.stream_downloaded(&key, &path);
                    if let Some(pid) = playlist {
                        let lib = &self.lib;
                        let keys = &self.path_keys;
                        if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                            if !p.tracks.contains(&path) {
                                // Imported playlists keep their original order as songs arrive.
                                let pos = p.source.as_ref().and_then(|src| {
                                    let rank = |k: &str| src.keys.iter().position(|x| x == k);
                                    let mine = rank(&key)?;
                                    p.tracks.iter().position(|t| {
                                        let tk = keys.get(t).cloned().or_else(|| lib.by_path(t).and_then(|x| x.yt_id.clone()));
                                        tk.and_then(|k| rank(&k)).map(|r| r > mine).unwrap_or(false)
                                    })
                                });
                                let at = pos.unwrap_or(p.tracks.len());
                                p.tracks.insert(at, path.clone());
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
                ytdlp::YtEvent::Fetched { item, path, art } => self.on_fetched(item, path, art),
                ytdlp::YtEvent::FetchFailed { item, msg } => self.on_fetch_failed(item, msg),
            }
        }
        self.sync_autoplay();
        self.sync_updates();
        self.sync_lyrics();
        while let Ok((id, st)) = self.lyrics_rx.try_recv() {
            if self.lyrics_for == Some(id) {
                self.lyrics = st;
            }
        }
        while let Ok((pid, key, color)) = self.cover_rx.try_recv() {
            if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                if p.cover.is_none() {
                    p.cover = Some(key);
                    p.color = Some(color);
                }
            }
            store::save_playlists(&self.playlists);
        }
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
        self.note_played();
        // A new song: remember the old one so the views can cross-fade.
        let cur_id = self.current_id();
        if cur_id != self.swap_last {
            let now = ctx.input(|i| i.time);
            self.swap_prev = match (self.swap_last.and_then(|id| self.info_for(id)), cur_id) {
                (Some(old), Some(_)) => Some((old, now)),
                _ => None,
            };
            self.swap_last = cur_id;
        }

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
        self.set_immersive(ctx, false);
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
        // The first download asks you to confirm you'll only download what you're allowed to.
        if matches!(a, Action::YtDownload { .. } | Action::DownloadPlaylist(_)) && !self.settings.download_rights_ok {
            self.pending_download = Some(a);
            self.dialog = Some(Dialog::DownloadRights);
            return;
        }
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
                self.play_from = if ids.len() > 1 { self.source_label(&ids) } else { None };
                self.player.send(Cmd::PlayList { items, start, shuffle });
                self.mark_dirty();
            }
            Action::PlayPath(p) => {
                self.play_from = None;
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
                    self.play_from = Some("Songs".into());
                }
            }
            Action::Toggle => self.player.send(Cmd::Toggle),
            Action::Next => self.player.send(Cmd::Next),
            Action::InsertInQueue { at, ids } => {
                let n = ids.len();
                let items = self.added_items(&ids);
                let empty = self.ps.queue.is_empty();
                self.player.send(Cmd::InsertAt { at, items });
                if empty {
                    self.player.send(Cmd::Play);
                }
                self.toast(if n == 1 { "Added to queue".to_string() } else { format!("Added {n} songs to queue") });
            }
            Action::PlayFromAutoplay(id) => {
                let items: Vec<QItem> = self.qitems(&[id]).into_iter().map(|q| q.with_origin(player::Origin::Library)).collect();
                let at = self.ps.index + 1;
                self.player.send(Cmd::InsertAt { at, items });
                self.player.send(Cmd::JumpTo(at));
            }
            Action::Preview(item) => {
                self.yt_requested = true;
                self.play_stream(item);
            }
            Action::DownloadPlaylist(pid) => self.download_playlist(pid),
            Action::DownloadStream(id) => self.download_stream(id),
            Action::SetAutoplay(v) => {
                self.settings.autoplay = v;
                self.player.send(Cmd::SetAutoplay(v));
                self.mark_dirty();
            }
            Action::Prev => self.player.send(Cmd::Prev),
            Action::Seek(s) => self.player.send(Cmd::Seek(s)),
            Action::RetryLyrics => {
                if let Some(id) = self.current_id() {
                    self.ask_lyrics(id, true);
                }
            }
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
                let items = self.added_items(&ids);
                let empty = self.ps.queue.is_empty();
                self.player.send(Cmd::PlayNext(items));
                if empty {
                    self.player.send(Cmd::Play);
                }
                self.toast(if n == 1 { "Playing next".to_string() } else { format!("{n} songs playing next") });
            }
            Action::AddToQueue(ids) => {
                let n = ids.len();
                let items = self.added_items(&ids);
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
                let mut tracks = Vec::new();
                let mut streams = std::collections::HashMap::new();
                for i in &with {
                    if let Some(t) = self.lib.get(*i) {
                        tracks.push(t.path.clone());
                    } else if let Some(tr) = self.transient.get(i) {
                        tracks.push(store::stream_entry(&tr.item.id));
                        streams.insert(tr.item.id.clone(), tr.item.clone());
                    }
                }
                self.playlists.push(Playlist {
                    id,
                    name: name.clone(),
                    tracks,
                    created: now_secs(),
                    streams,
                    ..Default::default()
                });
                store::save_playlists(&self.playlists);
                self.go(Route::Playlist(id));
                if with.is_empty() {
                    self.rename = Some((id, name, true));
                } else {
                    self.toast(format!("Added to “{name}”"));
                }
            }
            Action::AddToPlaylist { pid, ids } => self.add_to_playlist(pid, None, ids),
            Action::InsertInPlaylist { pid, at, ids } => self.add_to_playlist(pid, at, ids),
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
            Action::MovePlaylist { from, to } => {
                if from < self.playlists.len() && to < self.playlists.len() && from != to {
                    let p = self.playlists.remove(from);
                    self.playlists.insert(to, p);
                    store::save_playlists(&self.playlists);
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
                            ..Default::default()
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
            Action::RetryPlaylist(pid) => self.yt.retry_playlist(pid),
            Action::SavePlaylist { info, items } => {
                // Saved, not downloaded: songs already in the library go in as files, the rest
                // play from the site until you download them.
                let keys: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
                // Saving the same link again updates that playlist instead of making a copy.
                let existing = self
                    .playlists
                    .iter()
                    .find(|p| p.source.as_ref().map(|s| s.url == info.url).unwrap_or(false))
                    .map(|p| p.id);
                let pid = existing.unwrap_or_else(|| store::hash_bytes(format!("{}{}", now_secs(), info.url).as_bytes()));
                let mut entries: Vec<PathBuf> = Vec::new();
                let mut streams = std::collections::HashMap::new();
                for it in &items {
                    match self.lib.by_key.get(&it.id).and_then(|t| self.lib.get(*t)) {
                        Some(t) => entries.push(t.path.clone()),
                        None => {
                            entries.push(store::stream_entry(&it.id));
                            streams.insert(it.id.clone(), it.clone());
                        }
                    }
                }
                let n_stream = streams.len();
                match self.playlists.iter_mut().find(|p| p.id == pid) {
                    Some(p) => {
                        if let Some(src) = p.source.as_mut() {
                            src.keys = keys;
                        }
                        // Add what's new on the site; keep your order and your removals as they are.
                        for (k, it) in streams {
                            p.streams.entry(k).or_insert(it);
                        }
                        for e in entries {
                            if !p.tracks.contains(&e) {
                                p.tracks.push(e);
                            }
                        }
                    }
                    None => self.playlists.push(Playlist {
                        id: pid,
                        name: info.title.clone(),
                        tracks: entries,
                        created: now_secs(),
                        source: Some(store::PlaylistSource { url: info.url.clone(), site: info.site.clone(), keys }),
                        streams,
                        ..Default::default()
                    }),
                }
                store::save_playlists(&self.playlists);
                self.register_playlist_streams();
                // The playlist's own picture, fetched in the background.
                let has_cover = self.playlist(pid).map(|p| p.cover.is_some()).unwrap_or(false);
                if !info.cover.is_empty() && !has_cover {
                    let (tx, url) = (self.cover_tx.clone(), info.cover.clone());
                    let ctx = self.ctx.clone();
                    std::thread::spawn(move || {
                        if let Some((key, color)) = crate::net::get_bytes(&url, 16 << 20).ok().and_then(|b| crate::art::store_cover(&b, None)) {
                            let _ = tx.send((pid, key, color));
                            ctx.request_repaint();
                        }
                    });
                }
                self.go(Route::Playlist(pid));
                self.toast(if n_stream == 0 {
                    format!("Saved “{}”", info.title)
                } else {
                    format!("Saved “{}” · it plays online until you download it", info.title)
                });
            }
            Action::PickPlaylistCover(pid) => {
                let picked = rfd::FileDialog::new()
                    .set_title("Choose a picture for this playlist")
                    .add_filter("Pictures", &["jpg", "jpeg", "png", "webp", "gif", "bmp"])
                    .pick_file();
                if let Some(file) = picked {
                    match std::fs::read(&file).ok().and_then(|b| crate::art::store_cover(&b, None)) {
                        Some((key, color)) => {
                            if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                                p.cover = Some(key);
                                p.color = Some(color);
                            }
                            store::save_playlists(&self.playlists);
                        }
                        None => self.toast("That picture couldn't be opened. Try a JPG or PNG."),
                    }
                }
            }
            Action::RemovePlaylistCover(pid) => {
                if let Some(p) = self.playlists.iter_mut().find(|p| p.id == pid) {
                    p.cover = None;
                    p.color = None;
                }
                store::save_playlists(&self.playlists);
            }
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
            Action::ToggleMini => {
                self.set_immersive(ctx, false);
                self.toggle_mini(ctx)
            }
            Action::ToggleQueue => {
                // On Now Playing the queue sits beside the record instead of in the side panel.
                if self.route == Route::NowPlaying {
                    self.toggle_side(false, false);
                } else {
                    self.queue_open = !self.queue_open;
                }
            }
            Action::ToggleImmersive => {
                if self.mini {
                    self.toggle_mini(ctx);
                }
                let on = !self.immersive;
                self.set_immersive(ctx, on);
            }
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
            Action::CloseDialog => {
                self.dialog = None;
                self.pending_download = None;
            }
            Action::AcceptDownloadRights => {
                self.dialog = None;
                self.settings.download_rights_ok = true;
                self.settings.save();
                if let Some(a) = self.pending_download.take() {
                    self.run(ctx, a);
                }
            }
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

    /// Full-screen Now Playing on or off. On Windows the window just grows to cover the
    /// screen without its frame, so the display never switches modes (no flash).
    fn set_immersive(&mut self, ctx: &egui::Context, on: bool) {
        if on == self.immersive {
            return;
        }
        self.immersive = on;
        let now = ctx.input(|i| i.time);
        self.imm_since = now;
        self.imm_active = now;
        match (on, self.hwnd) {
            (true, Some(h)) => {
                self.fs_saved = platform::enter_fullscreen(h);
                if self.fs_saved.is_none() {
                    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true));
                }
            }
            (false, Some(h)) => match self.fs_saved.take() {
                Some(saved) => {
                    platform::leave_fullscreen(h, &saved);
                    platform::style_title_bar(h, [BG.r(), BG.g(), BG.b()], [TEXT_2.r(), TEXT_2.g(), TEXT_2.b()]);
                }
                None => ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false)),
            },
            (on, None) => ctx.send_viewport_cmd(ViewportCommand::Fullscreen(on)),
        }
        ctx.request_repaint();
    }

    /// A quick fade from the background color right after switching full screen on or off,
    /// so the window growing or shrinking never shows a half-drawn frame.
    fn view_fade(&self, ui: &Ui, full: Rect) {
        let age = (ui.input(|i| i.time) - self.imm_since) as f32;
        const FADE: f32 = 0.22;
        if (0.0..FADE).contains(&age) {
            let a = 1.0 - egui::emath::easing::cubic_out(age / FADE);
            ui.painter().rect_filled(full, 0.0, with_alpha(BG, a));
            ui.ctx().request_repaint();
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
            let pos = platform::mini_position(self.hwnd, self.settings.mini_pos, 300.0, ctx.pixels_per_point()).or_else(|| {
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
            if i.consume_key(egui::Modifiers::NONE, Key::F11) {
                self.actions.push(Action::ToggleImmersive);
            }
            if !typing {
                if i.consume_key(egui::Modifiers::NONE, Key::Space) {
                    self.actions.push(Action::Toggle);
                }
                if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                    if self.immersive {
                        self.actions.push(Action::ToggleImmersive);
                    } else if self.mini {
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
        // The record turns on the Now Playing screens: ~30 fps, only while one is showing.
        let record_shown = self.immersive || (!self.mini && self.route == Route::NowPlaying);
        if record_shown && self.settings.spin_record && self.ps.current().is_some() {
            ctx.request_repaint_after(Duration::from_millis(33));
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

        if self.immersive {
            self.immersive_ui(ui);
            self.view_fade(ui, full);
            self.overlays(ui, full);
            self.run_actions(&ctx);
            self.schedule_repaint(&ctx, 0.0);
            return;
        }

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
        let q_t = ctx.animate_bool_with_time(Id::new("queue-open"), self.queue_open && self.route != Route::NowPlaying, T_SLOW);
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
        self.view_fade(ui, full);
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
        let out = w::search_box(ui, &mut q, "Search", self.search_id, width);
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
            (icons::SPARKLES, "For you", Route::ForYou),
        ];
        // One highlight that slides to the page you pick (drawn under the items).
        let hl = ui.painter().add(egui::Shape::Noop);
        let mut sel_rect = None;
        for (icon, label, route) in nav {
            if route == Route::ForYou && !FOR_YOU {
                continue;
            }
            let sel = r == route || matches!((&r, &route), (Route::Album(_), Route::Albums) | (Route::Artist(_), Route::Artists));
            let resp = w::nav_item_ex(ui, icon, label, sel, None, false);
            if sel {
                sel_rect = Some(resp.rect);
            }
            if resp.clicked() {
                self.actions.push(Action::Go(route));
            }
        }
        let active = self.yt.active_count();
        let badge = (active > 0).then(|| active.to_string());
        let resp = w::nav_item_ex(ui, icons::DOWNLOAD, "Downloads", r == Route::Downloads, badge, false);
        if r == Route::Downloads {
            sel_rect = Some(resp.rect);
        }
        if resp.clicked() {
            self.actions.push(Action::Go(Route::Downloads));
        }
        self.nav_highlight(ui, hl, sel_rect);

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
                let (slot, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
                self.playlist_nav_item(ui, slot, resp, None, "Loved songs", loved_n, r == Route::Loved);
                let pls: Vec<(u64, String, usize)> = self
                    .playlists
                    .iter()
                    .map(|p| (p.id, p.name.clone(), p.tracks.iter().filter(|t| self.entry_id(t).is_some()).count()))
                    .collect();
                self.sidebar_playlists(ui, &pls, &r);
            });
        let rest = ui.available_height() - 36.0;
        if rest > 0.0 {
            ui.add_space(rest);
        }
        let update_badge = matches!(self.updater.state(), update::State::Ready { .. } | update::State::Available { .. }).then(|| "Update".to_string());
        let has_update = update_badge.is_some();
        if w::nav_item(ui, icons::SETTINGS, "Settings", r == Route::Settings, update_badge).clicked() {
            if has_update {
                self.settings_section = crate::views::SettingsSection::Updates;
            }
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
            (icons::SPARKLES, "For you", Route::ForYou, 0),
            (icons::DOWNLOAD, "Downloads", Route::Downloads, active),
        ];
        let items = items.map(|(i, l, r, n)| (i, l, r, (n > 0).then(|| n.min(99).to_string())));
        for (icon, label, route, badge) in items {
            if route == Route::ForYou && !FOR_YOU {
                continue;
            }
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
                    .map(|p| (p.id, p.name.clone(), p.tracks.iter().filter(|t| self.entry_id(t).is_some()).count()))
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
                let first = self.playlist_art(id).0;
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

    /// The sidebar's selected-page highlight: it glides from one item to the next, and fades
    /// when the page isn't one of them.
    fn nav_highlight(&mut self, ui: &Ui, slot: egui::layers::ShapeIdx, sel: Option<Rect>) {
        let ctx = ui.ctx();
        let shown = ctx.animate_bool_with_time(Id::new("nav-hl-on"), sel.is_some(), T);
        // Coming back from another page: appear in place instead of sliding from the old spot.
        let snap = shown < 0.05;
        if let Some(r) = sel {
            self.nav_hl = Some(r);
        }
        let Some(r) = self.nav_hl else { return };
        let y = w::glide(ctx, Id::new("nav-hl-y"), r.top(), if snap { 0.0 } else { 0.16 });
        if shown <= 0.0 {
            return;
        }
        let hr = Rect::from_min_size(pos2(r.left(), y), r.size());
        ui.painter().set(slot, egui::Shape::rect_filled(hr, R_MD, with_alpha(RAISED_2, shown)));
    }

    /// The playlists in the sidebar. Drag one up or down to change the order: it lifts and
    /// follows the pointer while the others slide out of the way.
    fn sidebar_playlists(&mut self, ui: &mut Ui, pls: &[(u64, String, usize)], r: &Route) {
        const ROW: f32 = 48.0;
        let ctx = ui.ctx().clone();
        let wdt = ui.available_width();
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let released = ctx.input(|i| i.pointer.any_released());
        // Where the list starts (rows are laid out one under the other).
        let top = ui.cursor().top();
        // Drop target while dragging: where the lifted playlist would go.
        let drag = self.pl_drag.filter(|(id, from, _)| pls.get(*from).map(|p| p.0) == Some(*id));
        if self.pl_drag.is_some() && drag.is_none() {
            self.pl_drag = None;
        }
        let gap = match (drag, pointer) {
            (Some((_, _, grab)), Some(p)) => Some((((p.y - grab - top) / ROW).round().max(0.0) as usize).min(pls.len().saturating_sub(1))),
            _ => None,
        };
        let mut drop_to = None;
        for (i, (id, name, n)) in pls.iter().enumerate() {
            let (slot, resp) = ui.allocate_exact_size(vec2(wdt, ROW), Sense::click_and_drag());
            let mut target = slot.top() - top;
            if let (Some((_, from, _)), Some(t)) = (drag, gap) {
                if i != from {
                    let j = if i > from { i - 1 } else { i };
                    let k = if j >= t { j + 1 } else { j };
                    target = k as f32 * ROW;
                }
            }
            let y = w::glide(&ctx, Id::new(("pl-glide", *id)), target, 0.18);
            let draw = Rect::from_min_size(pos2(slot.left(), top + y), slot.size());
            if resp.drag_started() {
                let grab = pointer.map(|p| p.y - slot.top()).unwrap_or(ROW / 2.0);
                self.pl_drag = Some((*id, i, grab));
            }
            if drag.map(|d| d.0) == Some(*id) {
                if released {
                    drop_to = gap.map(|t| (i, t));
                }
                continue;
            }
            self.playlist_nav_item(ui, draw, resp, Some(*id), name, *n, *r == Route::Playlist(*id));
        }
        // The lifted playlist, under the pointer, in line with the list.
        if let (Some((id, from, grab)), Some(p)) = (drag, pointer) {
            if let Some((_, name, n)) = pls.get(from) {
                let y = (p.y - grab).clamp(top - ROW / 2.0, top + (pls.len() as f32 - 0.5) * ROW);
                let ghost = Rect::from_min_size(pos2(ui.max_rect().left(), y), vec2(wdt, ROW));
                let layer = egui::LayerId::new(egui::Order::Tooltip, Id::new("pl-ghost"));
                let gp = ctx.layer_painter(layer);
                gp.add(egui::epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(150) }.as_shape(ghost, R_MD));
                gp.rect_filled(ghost, R_MD, RAISED_2);
                self.paint_playlist_item(&gp, ghost, Some(id), name, *n, true, 0.0);
                ctx.set_cursor_icon(CursorIcon::Grabbing);
                // Let go: it lands where it was dropped (gliding from under the pointer).
                if released {
                    w::glide(&ctx, Id::new(("pl-glide", id)), y - top, 0.0);
                }
                // Near the top or bottom of the list: scroll.
                let clip = ui.clip_rect();
                let edge = 40.0;
                let up = ((clip.top() + edge - p.y) / edge).clamp(0.0, 1.0);
                let down = ((p.y - (clip.bottom() - edge)) / edge).clamp(0.0, 1.0);
                if up > 0.0 || down > 0.0 {
                    let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.05);
                    ui.scroll_with_delta(vec2(0.0, (up - down) * 600.0 * dt));
                }
                ctx.request_repaint();
            }
        }
        if released && drag.is_some() {
            if let Some((from, to)) = drop_to {
                if from != to {
                    self.actions.push(Action::MovePlaylist { from, to });
                }
            }
            self.pl_drag = None;
        }
    }

    fn playlist_nav_item(&mut self, ui: &mut Ui, rect: Rect, resp: Response, pid: Option<u64>, name: &str, count: usize, selected: bool) {
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
        let p = p.clone();
        self.paint_playlist_item(&p, rect, pid, name, count, selected, h);
        self.playlist_item_events(resp, pid);
    }

    /// A sidebar playlist's picture, name and song count.
    #[allow(clippy::too_many_arguments)]
    fn paint_playlist_item(&mut self, p: &egui::Painter, rect: Rect, pid: Option<u64>, name: &str, count: usize, _selected: bool, h: f32) {
        let art = Rect::from_min_size(pos2(rect.left() + S2, rect.top() + 6.0), vec2(36.0, 36.0));
        // Pictures lean in a touch on hover.
        let art = w::scale_rect(art, 1.0 + 0.05 * h);
        match pid {
            None => {
                p.rect_filled(art, R_SM, ACCENT);
                icons::heart_filled(p, art.center(), 16.0, ON_ACCENT);
            }
            Some(id) => {
                let first = self.playlist_art(id).0;
                let tex = self.art.cover(first, ArtSize::S);
                if first.is_some() {
                    w::art(p, art, tex, R_SM, Color32::WHITE);
                } else {
                    w::gradient_tile(p, art, R_SM, lerp_color(ACCENT_LO, RAISED_2, 0.55), lerp_color(ACCENT_LO, BG, 0.82));
                    icons::glyph(p, art.center(), icons::LIST_MUSIC, 16.0, with_alpha(TEXT, 0.75));
                }
            }
        }
        let x = rect.left() + S2 + 36.0 + S3;
        let tw = rect.right() - x - S2;
        w::text(p, pos2(x, rect.center().y - 8.0), Align2::LEFT_CENTER, name, body(), TEXT, tw);
        let sub = if pid.is_none() { theme::plural(count, "song", "songs") } else { format!("Playlist · {}", theme::plural(count, "song", "songs")) };
        w::text(p, pos2(x, rect.center().y + 9.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, tw);
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
            let has_cover = self.playlist(id).map(|p| p.cover.is_some()).unwrap_or(false);
            resp.context_menu(|ui| {
                if crate::views::menu_item(ui, icons::PENCIL, "Rename").clicked() {
                    self.actions.push(Action::Go(Route::Playlist(id)));
                    self.actions.push(Action::StartRename(id));
                    ui.close();
                }
                if crate::views::menu_item(ui, icons::IMAGE, "Choose picture…").clicked() {
                    self.actions.push(Action::PickPlaylistCover(id));
                    ui.close();
                }
                if has_cover && crate::views::menu_item(ui, icons::X, "Remove picture").clicked() {
                    self.actions.push(Action::RemovePlaylistCover(id));
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
        let pad = S4;
        let left_w = (rect.width() * 0.3).min(360.0);
        let right_w = left_w;
        let center = Rect::from_min_max(pos2(rect.left() + left_w + pad, rect.top()), pos2(rect.right() - right_w - pad, rect.bottom()));

        // Left: art + title + love (or download, for a song played from YouTube)
        let art_r = Rect::from_min_size(pos2(rect.left() + pad, rect.center().y - 28.0), vec2(56.0, 56.0));
        let art_resp = ui.interact(art_r, Id::new("bar-art"), Sense::click());
        let (ah, _) = w::hover_press(ui, &art_resp);
        // A new song: the old cover and title fade out and up as the new ones come in.
        let mut swap = 1.0;
        if let Some((old, e)) = self.song_swap(ui.ctx(), 0.34) {
            swap = e;
            let tex = self.art.cover(old.art, ArtSize::S);
            w::art(ui.painter(), art_r, tex, R_SM + 2.0, with_alpha(Color32::WHITE, 1.0 - swap));
            let tx = art_r.right() + S3;
            let tw = (rect.left() + left_w - tx - 40.0).max(40.0);
            let dy = -12.0 * swap;
            // Out of the way quickly, so the old and new titles never pile up.
            let a = (1.0 - swap * 3.0).max(0.0);
            w::text(ui.painter(), pos2(tx, rect.center().y - 9.0 + dy), Align2::LEFT_CENTER, &old.title, body_strong(), with_alpha(TEXT, a), tw);
            w::text(ui.painter(), pos2(tx, rect.center().y + 10.0 + dy), Align2::LEFT_CENTER, &old.artist, small(), with_alpha(TEXT_2, a), tw);
        }
        if let Some(t) = &cur {
            let tex = self.art.cover(t.art, ArtSize::S);
            w::art(ui.painter(), w::scale_rect(art_r, 0.94 + 0.06 * swap), tex, R_SM + 2.0, with_alpha(Color32::WHITE, swap));
            if ah > 0.0 {
                ui.painter().rect_filled(art_r, R_SM + 2.0, with_alpha(Color32::BLACK, 0.45 * ah));
                icons::glyph(ui.painter(), art_r.center(), icons::MAXIMIZE, 18.0, with_alpha(TEXT, ah));
            }
            if art_resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Show Now Playing").clicked() {
                self.actions.push(if self.route == Route::NowPlaying { Action::Back } else { Action::Go(Route::NowPlaying) });
            }
            let tx = art_r.right() + S3;
            let tw = (rect.left() + left_w - tx - 40.0).max(40.0);
            let dy = 12.0 * (1.0 - swap);
            let ta = ((swap - 0.2) / 0.8).clamp(0.0, 1.0);
            let title_r = w::text(ui.painter(), pos2(tx, rect.center().y - 9.0 + dy), Align2::LEFT_CENTER, &t.title, body_strong(), with_alpha(TEXT, ta), tw);
            let sub = match &t.preview {
                Some(i) => format!("{} · From {}", t.artist, i.source()),
                None => t.artist.clone(),
            };
            let artist_r = w::text(ui.painter(), pos2(tx, rect.center().y + 10.0 + dy), Align2::LEFT_CENTER, &sub, small(), with_alpha(TEXT_2, ta), tw);
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
        let cx = center.center().x;
        self.transport_row(ui, pos2(cx, rect.center().y - 12.0), false, "bar");
        let seek_w = center.width().min(620.0);
        let sy = rect.center().y + 20.0;
        let seek = Rect::from_center_size(pos2(cx, sy), vec2(seek_w - 104.0, 12.0));
        let (pos, dur) = self.seek_bar(ui, seek, Id::new("seek"));
        if self.ps.waiting {
            // A song that plays from its site is on its way.
            w::spinner(ui, pos2(seek.left() - S3 - 7.0, sy), 6.0, TEXT_2);
        } else {
            w::text(ui.painter(), pos2(seek.left() - S3, sy), Align2::RIGHT_CENTER, &fmt_time(pos), small(), TEXT_2, 60.0);
        }
        w::text(ui.painter(), pos2(seek.right() + S3, sy), Align2::LEFT_CENTER, &fmt_time(dur), small(), TEXT_2, 60.0);

        // Right: queue, mini, volume
        let mut rx = rect.right() - pad;
        let vol_w = 96.0;
        let vol_r = Rect::from_min_max(pos2(rx - vol_w, rect.center().y - 6.0), pos2(rx, rect.center().y + 6.0));
        rx = vol_r.left() - S1;
        let vr = Rect::from_center_size(pos2(rx - 16.0, rect.center().y), Vec2::splat(32.0));
        self.volume_control(ui, vol_r, vr.center(), "bar");
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
        let q_on = if self.route == Route::NowPlaying { self.np_queue && !self.lyrics_mode() } else { self.queue_open };
        if w::icon_button_at(ui, qr, qresp, icons::LIST_MUSIC, q_on, "Queue · drop songs here to add them").clicked() {
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

    /// The "now playing" bars: bouncing while the song plays, settling when it's paused. While
    /// they move the window redraws about 25 times a second (only while they're on screen).
    pub fn np_bars(&self, ui: &Ui, p: &egui::Painter, c: egui::Pos2, size: f32, color: Color32) {
        let playing = self.ps.playing && !self.ps.waiting;
        let ctx = ui.ctx();
        let motion = ctx.animate_bool_with_time(Id::new("np-bars-motion"), playing, 0.35);
        let t = ctx.input(|i| i.time);
        icons::now_playing_bars(p, c, size, color, t, motion);
        if motion > 0.0 {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    pub fn love_button(&self, ui: &Ui, rect: Rect, resp: egui::Response, id: u64) -> egui::Response {
        let loved = self.loved.contains(&id);
        let (h, pr) = w::hover_press(ui, &resp);
        // A small pop when it becomes loved.
        let pop = ui.ctx().animate_bool_with_time(resp.id.with("pop"), loved, T_SLOW);
        let bump = if loved { (pop * std::f32::consts::PI).sin() * 0.22 } else { 0.0 };
        let s = 16.0 * (1.0 + bump + 0.08 * h - 0.08 * pr);
        if loved {
            icons::heart_filled(ui.painter(), rect.center(), s, ACCENT);
        } else {
            icons::glyph(ui.painter(), rect.center(), icons::HEART, s, lerp_color(TEXT_2, TEXT, h));
        }
        resp.on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(if loved { "Remove from Loved songs" } else { "Love" })
    }

    /// The queue panel. It slides in from the right edge (the page makes room as it comes).
    fn queue_panel(&mut self, ui: &mut Ui, rect: Rect, t: f32) {
        let a = egui::emath::easing::cubic_out(t);
        let rect = rect.translate(vec2((1.0 - a) * rect.width() * 0.35, 0.0));
        let mut pu = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("queue-panel"));
        pu.multiply_opacity(egui::emath::easing::quadratic_out(t));
        let ui = &mut pu;
        ui.painter().rect_filled(rect, R_LG, PANEL);
        let inner = rect.shrink2(vec2(S3, S4));
        let p = ui.painter().clone();
        w::text(&p, pos2(inner.left() + S1, inner.top() + 12.0), Align2::LEFT_CENTER, "Queue", h2(), TEXT, 200.0);
        let cr = Rect::from_center_size(pos2(inner.right() - 14.0, inner.top() + 12.0), Vec2::splat(28.0));
        let cresp = ui.interact(cr, Id::new("q-close"), Sense::click());
        if w::icon_button_at(ui, cr, cresp, icons::X, false, "Close").clicked() {
            self.actions.push(Action::ToggleQueue);
        }
        let list_r = Rect::from_min_max(pos2(inner.left(), inner.top() + 36.0), inner.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_r).layout(egui::Layout::top_down(egui::Align::Min)));
        self.queue_list(&mut child, list_r, true, "panel");
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

        // The song being dragged (lifted row), and its landing.
        self.draw_drag(&ctx);

        if self.dialog.is_none() {
            self.whats_new_window(&ctx);
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
                Dialog::DownloadRights => (
                    "Only download what you're allowed to".into(),
                    "Segno can save music from YouTube and other sites. Only download songs you own, \
                     songs that are free to download, or songs the artist or rights holder lets you \
                     download. You're responsible for what you download, and for following each \
                     site's terms."
                        .into(),
                    "I understand",
                    "Cancel",
                    false,
                ),
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
                    Dialog::DownloadRights => Action::AcceptDownloadRights,
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
        self.pop_in(&ctx);
    }

    /// Menus and popups grow in from where you clicked (they already fade in).
    fn pop_in(&mut self, ctx: &egui::Context) {
        const POP: f32 = 0.16;
        let now = ctx.input(|i| i.time);
        let screen = ctx.content_rect();
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let layers: Vec<(egui::LayerId, Rect)> = ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|l| l.order == egui::Order::Foreground && l.id != Id::new("toast"))
                .filter_map(|l| m.area_rect(l.id).map(|r| (l, r)))
                // Full-screen layers are dialogs with a backdrop: those only fade.
                .filter(|(_, r)| r.width() < screen.width() - 2.0)
                .collect()
        });
        self.pop_seen.retain(|id, _| layers.iter().any(|(l, _)| l.id == *id));
        for (layer, r) in layers {
            let (since, from) = *self.pop_seen.entry(layer.id).or_insert_with(|| (now, r.clamp(pointer.unwrap_or(r.min))));
            let t = ((now - since) as f32 / POP).clamp(0.0, 1.0);
            if t < 1.0 {
                let s = 0.9 + 0.1 * egui::emath::easing::cubic_out(t);
                ctx.set_transform_layer(layer, egui::emath::TSTransform::new(from.to_vec2() * (1.0 - s), s));
                ctx.request_repaint();
            } else if ctx.layer_transform_to_global(layer).is_some() {
                ctx.set_transform_layer(layer, egui::emath::TSTransform::IDENTITY);
            }
        }
    }
}

use egui::FontId;
use egui::Vec2;
