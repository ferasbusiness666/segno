//! Paths, settings, playlists and saved playback state. Everything is plain JSON.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "Segno";
/// The app's name before it became Segno; its folders are moved over once.
const OLD_NAME: &str = "Encore";

/// `base/Segno`, moving `base/Encore` there first if this is the first run after the rename.
fn app_dir(base: PathBuf) -> PathBuf {
    let d = base.join(APP_NAME);
    if !d.exists() {
        let old = base.join(OLD_NAME);
        if old.is_dir() {
            let _ = std::fs::rename(&old, &d);
        }
    }
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Roaming data: settings and playlists (small, worth keeping across machines).
pub fn data_dir() -> PathBuf {
    app_dir(dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")))
}

/// Local data: library index, cover cache, downloader binaries.
pub fn cache_dir() -> PathBuf {
    app_dir(dirs::cache_dir().or_else(dirs::data_local_dir).unwrap_or_else(|| PathBuf::from(".")))
}

pub fn art_dir() -> PathBuf {
    let d = cache_dir().join("art");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn bin_dir() -> PathBuf {
    let d = cache_dir().join("bin");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn default_music_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Music")))
        .unwrap_or_else(|| PathBuf::from("Music"))
}

/// Write JSON atomically (temp file + rename) so a crash never leaves a half file.
pub fn save_json<T: Serialize>(path: &Path, value: &T) {
    let tmp = path.with_extension("tmp");
    if let Ok(bytes) = serde_json::to_vec(value) {
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

pub fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortBy {
    #[default]
    Added,
    Title,
    Artist,
    Album,
    /// The order you dragged songs into.
    Custom,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Resume {
    /// Play order (paths), the original order before shuffle, and where we were.
    pub queue: Vec<PathBuf>,
    pub original: Vec<PathBuf>,
    pub index: usize,
    pub position: f32,
    /// Songs in the queue that play from their site (their entries above are `stream_entry`s).
    #[serde(default)]
    pub streams: Vec<crate::ytdlp::YtItem>,
    /// How each queue entry got there (see `player::Origin`), in queue order.
    #[serde(default)]
    pub origins: Vec<u8>,
    /// What you pressed play on ("Road Trip Mix", "Songs"…).
    #[serde(default)]
    pub from: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub music_dir: Option<PathBuf>,
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub close_to_tray: bool,
    pub autoplay: bool,
    pub sidebar_w: f32,
    pub sidebar_collapsed: bool,
    pub queue_w: f32,
    pub sort: SortBy,
    pub window_size: Option<[f32; 2]>,
    pub maximized: bool,
    pub mini_pos: Option<[f32; 2]>,
    pub resume: Resume,
    pub loved: Vec<PathBuf>,
    pub ytdlp_checked: u64,
    pub tray_hint_shown: bool,
    pub auto_update: bool,
    pub update_checked: u64,
    /// The version that last ran, to say "Updated to …" once after an update.
    pub last_version: String,
    /// The record turns on the Now Playing screens (redraws ~30 times a second while shown).
    pub spin_record: bool,
    /// Show synced lyrics (found on LRCLIB by song and artist name).
    pub show_lyrics: bool,
    /// The side panel on Now Playing shows lyrics instead of Up next.
    pub lyrics_panel: bool,
    /// Agreed to only download music they're allowed to (asked before the first download).
    pub download_rights_ok: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            music_dir: None,
            volume: 0.8,
            muted: false,
            shuffle: false,
            repeat: Repeat::Off,
            close_to_tray: true,
            autoplay: true,
            sidebar_w: 248.0,
            sidebar_collapsed: false,
            queue_w: 320.0,
            sort: SortBy::Added,
            window_size: None,
            maximized: false,
            mini_pos: None,
            resume: Resume::default(),
            loved: Vec::new(),
            ytdlp_checked: 0,
            tray_hint_shown: false,
            auto_update: true,
            update_checked: 0,
            last_version: String::new(),
            spin_record: true,
            show_lyrics: true,
            lyrics_panel: false,
            download_rights_ok: false,
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        data_dir().join("settings.json")
    }
    pub fn load() -> Self {
        load_json(&Self::path()).unwrap_or_default()
    }
    pub fn save(&self) {
        save_json(&Self::path(), self);
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub tracks: Vec<PathBuf>,
    pub created: u64,
    /// Your own picture, or the one from the site it was imported from (art cache key).
    #[serde(default)]
    pub cover: Option<u64>,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// Where an imported playlist came from, and its songs in their original order.
    #[serde(default)]
    pub source: Option<PlaylistSource>,
    /// Songs in this playlist that play from their site because they aren't downloaded,
    /// by song key. Their place in `tracks` is a `stream_entry`.
    #[serde(default)]
    pub streams: std::collections::HashMap<String, crate::ytdlp::YtItem>,
}

const STREAM: &str = "segno-stream:";

/// A playlist entry for a song that plays from its site (not a file).
pub fn stream_entry(key: &str) -> PathBuf {
    PathBuf::from(format!("{STREAM}{key}"))
}

/// The song key of a `stream_entry`, or None for a real file.
pub fn stream_key(p: &Path) -> Option<&str> {
    p.to_str()?.strip_prefix(STREAM)
}

/// The track id used for a song that plays from its site.
pub fn stream_id(key: &str) -> u64 {
    hash_bytes(format!("stream:{key}").as_bytes())
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PlaylistSource {
    pub url: String,
    pub site: String,
    /// Song keys (the same ones written into the files' tags), in the playlist's order.
    pub keys: Vec<String>,
}

pub fn load_playlists() -> Vec<Playlist> {
    load_json(&data_dir().join("playlists.json")).unwrap_or_default()
}

pub fn save_playlists(p: &[Playlist]) {
    save_json(&data_dir().join("playlists.json"), &p.to_vec());
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stable 64-bit hash (FNV-1a). Used for track ids and cover keys, so it must not
/// change between runs or Rust versions.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn path_id(p: &Path) -> u64 {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        hash_bytes(s.to_lowercase().as_bytes())
    } else {
        hash_bytes(s.as_bytes())
    }
}

/// Lock a mutex even if another thread panicked while holding it (the data is still usable for
/// us, and a music player should keep playing rather than cascade-crash).
pub trait LockExt<T> {
    fn lk(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn lk(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Your own order for the Songs list (paths, top to bottom).
pub fn load_song_order() -> Vec<PathBuf> {
    load_json(&data_dir().join("song_order.json")).unwrap_or_default()
}

pub fn save_song_order(v: &[PathBuf]) {
    save_json(&data_dir().join("song_order.json"), &v.to_vec());
}
