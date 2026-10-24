//! The music library: scanning the folder, reading tags, grouping into albums and artists,
//! and watching the folder for changes (Windows change notifications, no polling).

use crate::art;
use crate::store::{cache_dir, hash_bytes, load_json, now_secs, path_id, save_json, SortBy};
use lofty::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const AUDIO_EXT: &[&str] = &["mp3", "m4a", "aac", "flac", "ogg", "oga", "wav", "aif", "aiff"];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Track {
    pub id: u64,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_no: u32,
    pub disc_no: u32,
    pub year: u32,
    pub duration: f32,
    pub mtime: u64,
    pub size: u64,
    pub added: u64,
    pub art: Option<u64>,
    pub color: Option<[u8; 3]>,
    #[serde(default)]
    pub yt_id: Option<String>,
    /// The art is the picture in the song's folder (cover.jpg…), not art inside the file.
    #[serde(default)]
    pub folder_art: bool,
    /// For songs without art of their own: a fingerprint of the folder picture they were last
    /// given (`NO_COVER` if the folder had none), so a new or changed picture is noticed.
    /// For songs with art of their own, anything but 0 means "nothing to check".
    /// 0: saved before 1.1.10, not checked yet.
    #[serde(default)]
    pub cover_sig: u64,
}

/// `cover_sig` of a folder with no cover picture.
const NO_COVER: u64 = 1;

impl Track {
    fn with_path(mut self, p: &Path) -> Self {
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let (a, t) = split_name(&stem);
        self.id = path_id(p);
        self.path = p.to_path_buf();
        self.title = t;
        self.artist = a;
        self
    }

    /// The artist used for grouping: album artist if set, else the first credited artist.
    pub fn primary_artist(&self) -> &str {
        let a = if !self.album_artist.trim().is_empty() {
            self.album_artist.as_str()
        } else {
            self.artist.as_str()
        };
        first_artist(a)
    }

    /// The artist the song is listed under on the Artists page ("Unknown artist" if none).
    pub fn artist_name(&self) -> &str {
        let n = self.primary_artist();
        if n.is_empty() {
            UNKNOWN_ARTIST
        } else {
            n
        }
    }

    /// The key of that artist's page. Everything that links to an artist uses this, so the link
    /// always finds the page (songs with no artist used to link to a page that didn't exist).
    pub fn artist_key(&self) -> u64 {
        artist_key(self.artist_name())
    }
}

pub const UNKNOWN_ARTIST: &str = "Unknown artist";

/// The first name in a credit like "A, B" or "A feat. B".
///
/// The separators are matched on the name itself, ignoring case for plain ASCII letters only.
/// (Lowercasing the whole name first and cutting at the place found there was wrong: some
/// letters change length when lowercased, like the Kelvin sign "K" becoming "k", so the cut
/// could land inside a letter and crash, or in the wrong place and group songs wrongly.)
/// Every separator starts with an ASCII byte, which is always the start of a letter in UTF-8,
/// so the cut is always valid.
pub fn first_artist(a: &str) -> &str {
    const SEPS: [&str; 7] = [", ", "; ", " feat. ", " feat ", " ft. ", " featuring ", " / "];
    let b = a.as_bytes();
    let cut = (0..b.len())
        .find(|&i| {
            SEPS.iter()
                .any(|s| b.len() - i >= s.len() && b[i..i + s.len()].eq_ignore_ascii_case(s.as_bytes()))
        })
        .unwrap_or(b.len());
    a[..cut].trim()
}

pub fn album_key(t: &Track) -> Option<u64> {
    if t.album.trim().is_empty() {
        return None;
    }
    let s = format!("{}\0{}", t.primary_artist().to_lowercase(), t.album.trim().to_lowercase());
    Some(hash_bytes(s.as_bytes()))
}

pub fn artist_key(name: &str) -> u64 {
    hash_bytes(name.trim().to_lowercase().as_bytes())
}

pub struct Album {
    pub key: u64,
    pub title: String,
    pub artist: String,
    pub year: u32,
    pub tracks: Vec<u64>,
    pub art: Option<u64>,
    pub color: Option<[u8; 3]>,
    pub added: u64,
}

pub struct Artist {
    pub key: u64,
    pub name: String,
    pub tracks: Vec<u64>,
    pub albums: Vec<u64>,
    pub art: Option<u64>,
    pub color: Option<[u8; 3]>,
}

#[derive(Default)]
pub struct Library {
    pub tracks: Vec<Track>,
    by_id: HashMap<u64, usize>,
    pub albums: Vec<Album>,
    pub album_by_key: HashMap<u64, usize>,
    pub artists: Vec<Artist>,
    pub artist_by_key: HashMap<u64, usize>,
    pub yt_ids: HashSet<String>,
    /// Song key (from the file's tags) → track id.
    pub by_key: HashMap<String, u64>,
    hay: Vec<String>,
    /// Bumped whenever the track list changes (for cached sorts in the UI).
    pub version: u64,
    pub scanning: bool,
    pub loaded_once: bool,
}

impl Library {
    pub fn get(&self, id: u64) -> Option<&Track> {
        self.by_id.get(&id).map(|&i| &self.tracks[i])
    }

    pub fn by_path(&self, p: &Path) -> Option<&Track> {
        self.get(path_id(p))
    }

    pub fn set_tracks(&mut self, mut tracks: Vec<Track>) {
        tracks.sort_by(|a, b| b.added.cmp(&a.added).then(a.title.cmp(&b.title)));
        self.by_id = tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        self.hay = tracks
            .iter()
            .map(|t| format!("{} {} {}", t.title, t.artist, t.album).to_lowercase())
            .collect();
        self.yt_ids = tracks.iter().filter_map(|t| t.yt_id.clone()).collect();
        self.by_key = tracks.iter().filter_map(|t| t.yt_id.clone().map(|k| (k, t.id))).collect();

        let mut albums: Vec<Album> = Vec::new();
        let mut album_by_key: HashMap<u64, usize> = HashMap::new();
        let mut artists: Vec<Artist> = Vec::new();
        let mut artist_by_key: HashMap<u64, usize> = HashMap::new();
        for t in &tracks {
            if let Some(k) = album_key(t) {
                let i = *album_by_key.entry(k).or_insert_with(|| {
                    albums.push(Album {
                        key: k,
                        title: t.album.trim().to_string(),
                        artist: t.artist_name().to_string(),
                        year: t.year,
                        tracks: Vec::new(),
                        art: t.art,
                        color: t.color,
                        added: t.added,
                    });
                    albums.len() - 1
                });
                let a = &mut albums[i];
                a.tracks.push(t.id);
                if a.art.is_none() && t.art.is_some() {
                    a.art = t.art;
                    a.color = t.color;
                }
                if a.year == 0 {
                    a.year = t.year;
                }
                a.added = a.added.max(t.added);
            }
            let name = t.artist_name();
            let k = t.artist_key();
            let i = *artist_by_key.entry(k).or_insert_with(|| {
                artists.push(Artist {
                    key: k,
                    name: name.to_string(),
                    tracks: Vec::new(),
                    albums: Vec::new(),
                    art: t.art,
                    color: t.color,
                });
                artists.len() - 1
            });
            let ar = &mut artists[i];
            ar.tracks.push(t.id);
            if ar.art.is_none() && t.art.is_some() {
                ar.art = t.art;
                ar.color = t.color;
            }
            if let Some(ak) = album_key(t) {
                if !ar.albums.contains(&ak) {
                    ar.albums.push(ak);
                }
            }
        }
        // Album tracks in disc/track order.
        let idx: HashMap<u64, &Track> = tracks.iter().map(|t| (t.id, t)).collect();
        for a in &mut albums {
            a.tracks.sort_by(|x, y| {
                let (x, y) = (idx[x], idx[y]);
                (x.disc_no, x.track_no, x.title.to_lowercase()).cmp(&(y.disc_no, y.track_no, y.title.to_lowercase()))
            });
        }
        albums.sort_by_key(|a| a.title.to_lowercase());
        artists.sort_by_key(|a| a.name.to_lowercase());
        self.album_by_key = albums.iter().enumerate().map(|(i, a)| (a.key, i)).collect();
        self.artist_by_key = artists.iter().enumerate().map(|(i, a)| (a.key, i)).collect();
        for ar in &mut artists {
            ar.tracks.sort_by(|x, y| {
                let (x, y) = (idx[x], idx[y]);
                (x.album.to_lowercase(), x.disc_no, x.track_no).cmp(&(y.album.to_lowercase(), y.disc_no, y.track_no))
            });
        }
        drop(idx);
        self.albums = albums;
        self.artists = artists;
        self.tracks = tracks;
        self.version += 1;
    }

    pub fn album(&self, key: u64) -> Option<&Album> {
        self.album_by_key.get(&key).map(|&i| &self.albums[i])
    }

    pub fn artist(&self, key: u64) -> Option<&Artist> {
        self.artist_by_key.get(&key).map(|&i| &self.artists[i])
    }

    pub fn sorted_ids(&self, sort: SortBy) -> Vec<u64> {
        let mut v: Vec<&Track> = self.tracks.iter().collect();
        match sort {
            SortBy::Added | SortBy::Custom => {}
            SortBy::Title => v.sort_by_cached_key(|t| t.title.to_lowercase()),
            SortBy::Artist => v.sort_by_cached_key(|t| {
                (t.primary_artist().to_lowercase(), t.album.to_lowercase(), t.disc_no, t.track_no)
            }),
            SortBy::Album => v.sort_by_cached_key(|t| (t.album.to_lowercase(), t.disc_no, t.track_no)),
        }
        v.into_iter().map(|t| t.id).collect()
    }

    /// Every word in the query must appear somewhere in title, artist or album.
    pub fn search(&self, query: &str, limit: usize) -> Vec<u64> {
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u32, u64)> = Vec::new();
        for (i, hay) in self.hay.iter().enumerate() {
            if words.iter().all(|w| hay.contains(w.as_str())) {
                let t = &self.tracks[i];
                let tl = t.title.to_lowercase();
                let q = query.trim().to_lowercase();
                let score = if tl == q {
                    0
                } else if tl.starts_with(&q) {
                    1
                } else if tl.contains(&q) {
                    2
                } else {
                    3
                };
                scored.push((score, t.id));
            }
        }
        scored.sort_by_key(|s| s.0);
        scored.into_iter().take(limit).map(|s| s.1).collect()
    }

    pub fn total_duration(&self, ids: &[u64]) -> f32 {
        ids.iter().filter_map(|id| self.get(*id)).map(|t| t.duration).sum()
    }
}

// ---------------------------------------------------------------------------------------------
// Scanner thread

pub enum ScanCmd {
    SetDir(PathBuf),
    Rescan,
}

pub struct LibEvent {
    pub tracks: Vec<Track>,
    pub scanning: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct Index {
    dir: PathBuf,
    tracks: Vec<Track>,
}

fn index_path() -> PathBuf {
    cache_dir().join("library.json")
}

pub fn spawn_scanner(notify: Arc<dyn Fn() + Send + Sync>) -> (Sender<ScanCmd>, Receiver<LibEvent>) {
    let (tx, rx) = channel::<ScanCmd>();
    let (ev_tx, ev_rx) = channel::<LibEvent>();
    let self_tx = tx.clone();
    std::thread::Builder::new()
        .name("library".into())
        .spawn(move || scanner_main(rx, self_tx, ev_tx, notify))
        .expect("library thread");
    (tx, ev_rx)
}

fn scanner_main(
    rx: Receiver<ScanCmd>,
    self_tx: Sender<ScanCmd>,
    ev_tx: Sender<LibEvent>,
    notify: Arc<dyn Fn() + Send + Sync>,
) {
    let mut dir: Option<PathBuf> = None;
    let mut watcher: Option<notify::RecommendedWatcher> = None;
    let mut cache: HashMap<PathBuf, Track> = HashMap::new();
    let mut colors: HashMap<u64, [u8; 3]> = HashMap::new();
    loop {
        let Ok(first) = rx.recv() else { return };
        let mut new_dir = None;
        let mut rescan = false;
        let apply = |c: ScanCmd, new_dir: &mut Option<PathBuf>, rescan: &mut bool| match c {
            ScanCmd::SetDir(d) => *new_dir = Some(d),
            ScanCmd::Rescan => *rescan = true,
        };
        apply(first, &mut new_dir, &mut rescan);
        // Debounce bursts of file events (a download writes, renames and tags in quick succession).
        let deadline = Instant::now() + Duration::from_millis(if new_dir.is_some() { 50 } else { 900 });
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(c) => apply(c, &mut new_dir, &mut rescan),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if let Some(d) = new_dir {
            if dir.as_ref() != Some(&d) {
                dir = Some(d.clone());
                cache.clear();
                if let Some(idx) = load_json::<Index>(&index_path()) {
                    if idx.dir == d {
                        for t in idx.tracks {
                            if let (Some(a), Some(c)) = (t.art, t.color) {
                                colors.insert(a, c);
                            }
                            cache.insert(t.path.clone(), t);
                        }
                    }
                }
                // Show what we knew last time right away; the scan below catches up.
                if !cache.is_empty() {
                    let _ = ev_tx.send(LibEvent {
                        tracks: cache.values().cloned().collect(),
                        scanning: true,
                    });
                    notify();
                }
                watcher = make_watcher(&d, self_tx.clone());
            }
            rescan = true;
        }
        let _ = &watcher;
        if rescan {
            if let Some(d) = dir.clone() {
                scan(&d, &mut cache, &mut colors, &ev_tx, &notify);
                save_json(
                    &index_path(),
                    &Index {
                        dir: d,
                        tracks: cache.values().cloned().collect(),
                    },
                );
            }
        }
    }
}

fn make_watcher(dir: &Path, tx: Sender<ScanCmd>) -> Option<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if matches!(ev.kind, EventKind::Access(_)) {
                return;
            }
            let relevant = ev.paths.iter().any(|p| {
                let s = p.to_string_lossy();
                if s.contains(".segno-tmp") || s.contains(".encore-tmp") {
                    return false;
                }
                // Folder events have no extension; audio files do. Cover pictures (cover.jpg…)
                // count too, so adding, changing or removing one updates the songs next to it.
                match p.extension() {
                    Some(e) => {
                        AUDIO_EXT.contains(&e.to_string_lossy().to_lowercase().as_str())
                            || p.file_name().is_some_and(|n| is_cover_name(&n.to_string_lossy()))
                    }
                    None => true,
                }
            });
            if relevant {
                let _ = tx.send(ScanCmd::Rescan);
            }
        }
    })
    .ok()?;
    w.watch(dir, RecursiveMode::Recursive).ok()?;
    Some(w)
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, std::fs::Metadata)>, depth: u32) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.starts_with('$') {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            walk(&p, out, depth + 1);
        } else if ft.is_file() {
            let ext = p
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if AUDIO_EXT.contains(&ext.as_str()) {
                if let Ok(m) = e.metadata() {
                    out.push((p, m));
                }
            }
        }
    }
}

fn is_cloud_only(_m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const OFFLINE: u32 = 0x1000;
        const RECALL_ON_OPEN: u32 = 0x40000;
        const RECALL_ON_DATA_ACCESS: u32 = 0x400000;
        _m.file_attributes() & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn mtime_secs(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn scan(
    dir: &Path,
    cache: &mut HashMap<PathBuf, Track>,
    colors: &mut HashMap<u64, [u8; 3]>,
    ev_tx: &Sender<LibEvent>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) {
    let mut files = Vec::new();
    walk(dir, &mut files, 0);
    let present: HashSet<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
    let before = cache.len();
    cache.retain(|p, _| present.contains(p));
    let mut changed = cache.len() != before;
    let mut covers = Covers::default();
    let mut last_send = Instant::now();
    let mut since_send = 0;
    for (path, meta) in files {
        let mtime = mtime_secs(&meta);
        let size = meta.len();
        if let Some(t) = cache.get_mut(&path) {
            if t.mtime == mtime && t.size == size {
                // The song didn't change, but the cover picture next to it may have.
                changed |= update_folder_art(t, &mut covers, colors);
                continue;
            }
        }
        let added = cache
            .get(&path)
            .map(|t| t.added)
            .unwrap_or_else(|| {
                meta.created()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(mtime)
                    .min(now_secs())
            });
        // Files that live only in the cloud (OneDrive "online-only") would be downloaded just to
        // read their tags. Use the file name for those instead.
        let mut t = if is_cloud_only(&meta) {
            let mut t = read_track(Path::new(""), mtime, size, added, colors);
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let (a, ti) = split_name(&stem);
            t.id = path_id(&path);
            t.path = path.clone();
            t.title = ti;
            t.artist = a;
            t
        } else {
            // A damaged file must never take the scanner down with it.
            let mut c2 = std::mem::take(colors);
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let t = read_track(&path, mtime, size, added, &mut c2);
                (t, c2)
            }));
            match res {
                Ok((t, c)) => {
                    *colors = c;
                    t
                }
                Err(_) => read_track(Path::new(""), mtime, size, added, colors).with_path(&path),
            }
        };
        if t.art.is_some() {
            // Art of its own: the folder picture never matters for this song.
            t.cover_sig = NO_COVER;
        } else {
            update_folder_art(&mut t, &mut covers, colors);
        }
        cache.insert(path, t);
        changed = true;
        since_send += 1;
        if since_send >= 200 || last_send.elapsed() > Duration::from_millis(1200) {
            since_send = 0;
            last_send = Instant::now();
            let _ = ev_tx.send(LibEvent {
                tracks: cache.values().cloned().collect(),
                scanning: true,
            });
            notify();
        }
    }
    let _ = changed;
    let _ = ev_tx.send(LibEvent {
        tracks: cache.values().cloned().collect(),
        scanning: false,
    });
    notify();
}

/// Whether a file name is a folder cover picture: cover, folder, front, album or albumart, as
/// .jpg, .jpeg, .png or .webp, in any letter case.
fn is_cover_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.rsplit_once('.').is_some_and(|(stem, ext)| {
        matches!(ext, "jpg" | "jpeg" | "png" | "webp") && matches!(stem, "cover" | "folder" | "front" | "album" | "albumart")
    })
}

/// A folder's cover picture and a fingerprint of it (name, size and change time), from the
/// folder listing alone, without reading the picture. None if the folder has none.
fn folder_cover_file(dir: &Path) -> Option<(PathBuf, u64)> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(PathBuf, std::fs::Metadata)> = None;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !is_cover_name(&name) {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        best = Some((e.path(), meta));
        if name.starts_with("cover.") || name.starts_with("folder.") {
            break;
        }
    }
    let (p, m) = best?;
    let when = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let sig = hash_bytes(format!("{}\0{}\0{}", p.to_string_lossy(), m.len(), when).as_bytes());
    Some((p, sig.max(NO_COVER + 1)))
}

/// Read a cover picture into the art cache.
fn load_cover(p: &Path, colors: &mut HashMap<u64, [u8; 3]>) -> Option<(u64, [u8; 3])> {
    let bytes = std::fs::read(p).ok()?;
    let known = colors.get(&hash_bytes(&bytes)).copied();
    let (k, c) = art::store_cover(&bytes, known)?;
    colors.insert(k, c);
    Some((k, c))
}

/// Folder pictures seen during one scan: each folder is listed once and each picture read once.
#[derive(Default)]
struct Covers {
    files: HashMap<PathBuf, Option<(PathBuf, u64)>>,
    loaded: HashMap<u64, Option<(u64, [u8; 3])>>,
}

impl Covers {
    fn file(&mut self, dir: &Path) -> Option<(PathBuf, u64)> {
        self.files.entry(dir.to_path_buf()).or_insert_with(|| folder_cover_file(dir)).clone()
    }
    fn sig(&mut self, dir: &Path) -> u64 {
        self.file(dir).map(|(_, s)| s).unwrap_or(NO_COVER)
    }
    fn art(&mut self, dir: &Path, colors: &mut HashMap<u64, [u8; 3]>) -> Option<(u64, [u8; 3])> {
        let (p, sig) = self.file(dir)?;
        *self.loaded.entry(sig).or_insert_with(|| load_cover(&p, colors))
    }
}

/// Give a song the picture in its folder, or take it away, when the picture was added,
/// changed or removed. Songs with art of their own keep it. Returns whether the song changed.
fn update_folder_art(t: &mut Track, covers: &mut Covers, colors: &mut HashMap<u64, [u8; 3]>) -> bool {
    let own_art = t.art.is_some() && !t.folder_art;
    if own_art && t.cover_sig != 0 {
        return false;
    }
    let dir = t.path.parent().map(Path::to_path_buf).unwrap_or_default();
    let sig = covers.sig(&dir);
    if own_art {
        // Saved before 1.1.10, when it wasn't noted where art came from: if it's the folder's
        // picture, mark it, so it follows that picture from now on.
        t.folder_art = sig != NO_COVER && covers.art(&dir, colors).map(|(k, _)| k) == t.art;
        t.cover_sig = if t.folder_art { sig } else { NO_COVER };
        return true;
    }
    if sig == t.cover_sig {
        return false;
    }
    t.cover_sig = sig;
    match covers.art(&dir, colors) {
        Some((k, c)) => {
            t.art = Some(k);
            t.color = Some(c);
            t.folder_art = true;
        }
        None => {
            t.art = None;
            t.color = None;
            t.folder_art = false;
        }
    }
    true
}

/// "Artist - Title" from a file name, when a file has no tags.
fn split_name(stem: &str) -> (String, String) {
    for sep in [" - ", " – ", " — "] {
        if let Some((a, t)) = stem.split_once(sep) {
            if !a.trim().is_empty() && !t.trim().is_empty() {
                return (a.trim().to_string(), t.trim().to_string());
            }
        }
    }
    (String::new(), stem.trim().to_string())
}

pub fn yt_id_from_comment(c: &str) -> Option<String> {
    let c = c.trim();
    // Songs from other sites: "segno:soundcloud:123", "segno:spotify:4uLU6…".
    if let Some(rest) = c.strip_prefix("segno:") {
        let key = rest.split_whitespace().next().unwrap_or("");
        return (!key.is_empty()).then(|| key.to_string());
    }
    let id = if let Some(rest) = c.strip_prefix("youtube:") {
        rest.trim()
    } else if let Some(i) = c.find("watch?v=") {
        &c[i + 8..]
    } else if let Some(i) = c.find("youtu.be/") {
        &c[i + 9..]
    } else {
        return None;
    };
    let id: String = id
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect();
    (id.len() == 11).then_some(id)
}

fn read_track(path: &Path, mtime: u64, size: u64, added: u64, colors: &mut HashMap<u64, [u8; 3]>) -> Track {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (fa, ft) = split_name(&stem);
    let mut t = Track {
        id: path_id(path),
        path: path.to_path_buf(),
        title: ft,
        artist: fa,
        album: String::new(),
        album_artist: String::new(),
        track_no: 0,
        disc_no: 0,
        year: 0,
        duration: 0.0,
        mtime,
        size,
        added,
        art: None,
        color: None,
        yt_id: None,
        folder_art: false,
        cover_sig: 0,
    };
    let Ok(tf) = lofty::read_from_path(path) else {
        return t;
    };
    t.duration = tf.properties().duration().as_secs_f32();
    let Some(tag) = tf.primary_tag().or_else(|| tf.first_tag()) else {
        return t;
    };
    if let Some(v) = tag.title() {
        if !v.trim().is_empty() {
            t.title = v.trim().to_string();
        }
    }
    if let Some(v) = tag.artist() {
        if !v.trim().is_empty() {
            t.artist = v.trim().to_string();
        }
    }
    if let Some(v) = tag.album() {
        t.album = v.trim().to_string();
    }
    if let Some(v) = tag.get_string(ItemKey::AlbumArtist) {
        t.album_artist = v.trim().to_string();
    }
    t.track_no = tag.track().unwrap_or(0);
    t.disc_no = tag.disk().unwrap_or(0);
    t.year = tag.date().map(|d| d.year as u32).unwrap_or(0);
    if let Some(c) = tag.comment() {
        t.yt_id = yt_id_from_comment(&c);
    }
    let pics = tag.pictures();
    let pic = pics
        .iter()
        .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
        .or_else(|| pics.first());
    if let Some(p) = pic {
        let bytes = p.data();
        let known = colors.get(&hash_bytes(bytes)).copied();
        if let Some((k, c)) = art::store_cover(bytes, known) {
            colors.insert(k, c);
            t.art = Some(k);
            t.color = Some(c);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_artist_cuts_between_letters() {
        assert_eq!(first_artist("A feat. B"), "A");
        assert_eq!(first_artist("A FEAT. B"), "A");
        assert_eq!(first_artist("A Ft. B"), "A");
        assert_eq!(first_artist("A, B & C"), "A");
        assert_eq!(first_artist("A; B"), "A");
        assert_eq!(first_artist("AC/DC"), "AC/DC");
        assert_eq!(first_artist("A / B"), "A");
        assert_eq!(first_artist("Solo"), "Solo");
        assert_eq!(first_artist(""), "");
        assert_eq!(first_artist("Beyoncé feat. JAY-Z"), "Beyoncé");
        // Letters that change length when lowercased: these crashed or grouped wrongly.
        assert_eq!(first_artist("\u{212A} feat. Guest"), "\u{212A}"); // Kelvin sign: crashed
        assert_eq!(first_artist("İlkay, Someone"), "İlkay"); // was "İlkay,"
        assert_eq!(first_artist("Ⱥ ft. B"), "Ⱥ");
        assert_eq!(first_artist("ẞ, X"), "ẞ");
    }

    fn song(path: &Path, artist: &str) -> Track {
        let mut t = read_track(Path::new(""), 1, 1, 1, &mut HashMap::new()).with_path(path);
        t.artist = artist.into();
        t
    }

    #[test]
    fn unknown_artist_links_find_the_page() {
        let mut lib = Library::default();
        let nobody = song(Path::new("/m/Untitled.mp3"), "");
        let someone = song(Path::new("/m/Someone - Song.mp3"), "Someone feat. X");
        let (n, s) = (nobody.clone(), someone.clone());
        lib.set_tracks(vec![nobody, someone]);
        // The key every link uses (player bar, "Go to artist", rows) finds the artist page.
        let page = lib.artist(n.artist_key()).expect("Unknown artist page");
        assert_eq!(page.name, UNKNOWN_ARTIST);
        assert!(page.tracks.contains(&n.id));
        assert_eq!(lib.artist(s.artist_key()).map(|a| a.name.as_str()), Some("Someone"));
    }

    #[test]
    fn folder_covers_follow_the_picture() {
        let root = std::env::temp_dir().join(format!("segno-cover-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // Keep the art cache inside the test folder.
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        let album = root.join("music").join("Album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("01 Song.mp3"), b"not really audio").unwrap();
        let picture = |path: &Path, size: u32, rgb: [u8; 3]| {
            image::RgbImage::from_pixel(size, size, image::Rgb(rgb)).save(path).unwrap();
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let (mut cache, mut colors) = (HashMap::new(), HashMap::new());
        let music = root.join("music");
        let mut rescan = |cache: &mut HashMap<PathBuf, Track>| {
            scan(&music, cache, &mut colors, &tx, &notify);
            cache.values().next().cloned().unwrap()
        };

        let t = rescan(&mut cache);
        assert_eq!(t.art, None, "no picture yet");

        // Added after the first scan: the song wasn't touched, the picture still shows up.
        picture(&album.join("cover.png"), 8, [200, 30, 60]);
        let t = rescan(&mut cache);
        let first = t.art.expect("picked up the new cover");
        assert!(t.folder_art);
        assert_eq!(rescan(&mut cache).art, Some(first), "nothing changes when nothing changed");

        // Replaced with another picture.
        picture(&album.join("cover.png"), 12, [20, 160, 90]);
        let second = rescan(&mut cache).art.expect("still has a cover");
        assert_ne!(second, first, "follows the new picture");

        // A song saved before 1.1.10 (no record of where its art came from) gets sorted out.
        for t in cache.values_mut() {
            t.folder_art = false;
            t.cover_sig = 0;
        }
        let t = rescan(&mut cache);
        assert!(t.folder_art && t.art == Some(second));

        // Removed.
        std::fs::remove_file(album.join("cover.png")).unwrap();
        let t = rescan(&mut cache);
        assert_eq!(t.art, None, "the picture is gone");
        assert!(!t.folder_art);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cover_names() {
        for n in ["cover.jpg", "Cover.PNG", "folder.jpeg", "front.webp", "AlbumArt.jpg", "album.png"] {
            assert!(is_cover_name(n), "{n}");
        }
        for n in ["cover.gif", "song.jpg", "cover", "my cover.jpg", "cover.jpg.part"] {
            assert!(!is_cover_name(n), "{n}");
        }
    }
}
