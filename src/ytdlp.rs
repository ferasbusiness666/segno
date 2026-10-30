//! Search and downloads through yt-dlp: YouTube by default, and links from any site yt-dlp
//! supports (SoundCloud, Bandcamp, …). Spotify links are read from Spotify and matched on
//! YouTube (see `spotify.rs`), because Spotify's audio can't be downloaded.
//!
//! - yt-dlp (and a JavaScript runtime it needs for YouTube) is fetched on first use and kept in
//!   the app's local folder. If Node, Deno or Bun is already installed, that is used instead.
//! - Every process runs hidden (no console windows) with UTF-8 output.
//! - Downloads go to a hidden temp folder inside the music folder, get proper tags and cover art,
//!   then are moved in as "Artist - Title.m4a" in one rename, so the library never sees half files.

use crate::store::LockExt;
use crate::store::{bin_dir, now_secs};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct YtItem {
    /// What identifies the song: an 11-character YouTube id, or "site:id" for other sites
    /// ("soundcloud:123", "spotify:4uLU6…"). Written into the file's tags, so the app knows
    /// which results are already in the library.
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration: f32,
    /// The page to download from. Empty for YouTube (built from the id).
    #[serde(default)]
    pub url: String,
    /// Thumbnail or cover picture. Empty for YouTube (built from the id).
    #[serde(default)]
    pub thumb: String,
    /// Where it's from, for other sites ("SoundCloud"). Empty for YouTube.
    #[serde(default)]
    pub site: String,
    /// Songs from Spotify links: what to look up on YouTube to get the audio.
    #[serde(default)]
    pub find: String,
    /// Album name, when the site says (Spotify albums).
    #[serde(default)]
    pub album: String,
}

impl YtItem {
    pub fn is_youtube(&self) -> bool {
        self.id.len() == 11 && !self.id.contains(':')
    }
    pub fn thumb_url(&self) -> Option<String> {
        if !self.thumb.is_empty() {
            Some(self.thumb.clone())
        } else if self.is_youtube() {
            Some(format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", self.id))
        } else {
            None
        }
    }
    /// What yt-dlp is given to fetch the audio.
    pub fn url(&self) -> String {
        if !self.find.is_empty() {
            format!("ytsearch5:{}", self.find)
        } else if !self.url.is_empty() {
            self.url.clone()
        } else {
            format!("https://www.youtube.com/watch?v={}", self.id)
        }
    }
    /// The page to open in a browser.
    pub fn page(&self) -> String {
        if !self.url.is_empty() {
            self.url.clone()
        } else {
            format!("https://www.youtube.com/watch?v={}", self.id)
        }
    }
    /// A file-name-safe version of the id, for temp and preview files.
    ///
    /// YouTube ids are used as they are (only letters, digits, "-" and "_"). Other ids
    /// ("soundcloud:a.b") get their symbols replaced and a hash of the full id added, so two
    /// different songs never share a name ("soundcloud:a.b" and "soundcloud:a_b" used to). A
    /// stem never has a "." in it, so "<stem>." only ever matches this song's own files.
    pub fn stem(&self) -> String {
        let safe = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if !self.id.is_empty() && self.id.chars().all(safe) {
            return self.id.clone();
        }
        let readable: String = self.id.chars().take(60).map(|c| if safe(c) { c } else { '_' }).collect();
        format!("{readable}-{:016x}", crate::store::hash_bytes(self.id.as_bytes()))
    }
    /// "YouTube", "SoundCloud", "Spotify"…
    pub fn source(&self) -> &str {
        if self.site.is_empty() { "YouTube" } else { &self.site }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlaylistInfo {
    pub title: String,
    pub channel: String,
    /// The link it came from (to know whether it's already saved).
    pub url: String,
    /// Its cover picture, if the site has one.
    pub cover: String,
    pub site: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Setup {
    Unknown,
    Working { label: String, progress: Option<f32> },
    Ready,
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum Search {
    Idle,
    Loading { query: String },
    Done { query: String, items: Vec<YtItem>, playlist: Option<PlaylistInfo> },
    #[allow(dead_code)]
    Failed { query: String, msg: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum JobStatus {
    Waiting,
    Downloading { pct: f32, speed: String },
    Saving,
    Done(PathBuf),
    Failed(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub uid: u64,
    pub item: YtItem,
    pub status: JobStatus,
    pub playlist: Option<u64>,
}

/// The Downloads list, kept between runs.
fn jobs_path() -> PathBuf {
    crate::store::data_dir().join("downloads.json")
}

fn save_jobs(jobs: &[Job]) {
    crate::store::save_json(&jobs_path(), &jobs_to_keep(jobs));
}

/// How many finished downloads (done or failed) the list keeps.
const HISTORY: usize = 300;

/// What's saved: every download that hasn't finished yet, however many there are, and the
/// newest finished ones up to `HISTORY`, in their order. (Only keeping the newest 300 of
/// everything used to drop songs still waiting when a big playlist was queued.)
fn jobs_to_keep(jobs: &[Job]) -> Vec<Job> {
    let finished = |j: &Job| matches!(j.status, JobStatus::Done(_) | JobStatus::Failed(_));
    let mut drop = jobs.iter().filter(|j| finished(j)).count().saturating_sub(HISTORY);
    jobs.iter()
        .filter(|j| {
            if drop > 0 && finished(j) {
                drop -= 1;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect()
}

/// Load the saved list. Downloads that were still going when Segno closed start again.
fn load_jobs() -> Vec<Job> {
    let mut jobs: Vec<Job> = crate::store::load_json(&jobs_path()).unwrap_or_default();
    for j in &mut jobs {
        if matches!(j.status, JobStatus::Downloading { .. } | JobStatus::Saving) {
            j.status = JobStatus::Waiting;
        }
    }
    let max = jobs.iter().map(|j| j.uid).max().unwrap_or(0);
    JOB_UID.fetch_max(max + 1, Ordering::SeqCst);
    jobs
}

/// A song to find more like it: its YouTube id if known, otherwise words to search for.
#[derive(Clone, Debug)]
pub struct Seed {
    pub id: Option<String>,
    pub find: String,
    /// Shown as "Songs like …".
    pub artist: String,
}

#[derive(Clone, Debug)]
pub enum Suggest {
    Idle,
    Loading { had: Vec<YtItem>, from: Vec<String> },
    Ready { items: Vec<YtItem>, from: Vec<String>, at: u64 },
    Failed(String),
}

#[derive(Serialize, Deserialize, Default)]
struct SuggestCache {
    items: Vec<YtItem>,
    from: Vec<String>,
    at: u64,
}

fn suggest_path() -> PathBuf {
    crate::store::cache_dir().join("suggestions.json")
}

fn load_suggestions() -> Suggest {
    match crate::store::load_json::<SuggestCache>(&suggest_path()) {
        Some(c) if !c.items.is_empty() => Suggest::Ready { items: c.items, from: c.from, at: c.at },
        _ => Suggest::Idle,
    }
}

/// "artist|title", lowercase letters and digits only, to spot the same song twice.
pub fn song_key(artist: &str, title: &str) -> String {
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    format!("{}|{}", norm(artist), norm(title))
}

/// Best guess at (artist, title) for a YouTube result: "Artist - Song (Official Video)" or the channel.
pub fn guess_artist_title(item: &YtItem) -> (String, String) {
    let t = remove_junk(&item.title);
    match t.split_once(" - ") {
        Some((a, b)) if !a.trim().is_empty() && !b.trim().is_empty() => (a.trim().to_string(), b.trim().to_string()),
        _ => (clean_channel(&item.channel), t),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PreviewState {
    Loading(f32),
    Ready(PathBuf),
    Failed(String),
}

pub struct Shared {
    pub setup: Setup,
    pub search: Search,
    pub suggest: Suggest,
    pub jobs: Vec<Job>,
    pub version: Option<String>,
    /// Songs fetched to play before downloading, by video id.
    pub previews: std::collections::HashMap<String, PreviewState>,
}

pub enum YtEvent {
    Downloaded { path: PathBuf, playlist: Option<u64>, key: String },
    /// A song that plays from its site is ready to play from this file.
    Fetched { item: YtItem, path: PathBuf, art: Option<(u64, [u8; 3])> },
    FetchFailed { item: YtItem, msg: String },
}

/// Where songs played from YouTube (before downloading) are kept for a few days.
pub fn preview_dir() -> PathBuf {
    let d = crate::store::cache_dir().join("preview");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Delete preview files older than three days (called once at startup).
pub fn clean_previews() {
    let Ok(rd) = std::fs::read_dir(preview_dir()) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|a| a > Duration::from_secs(3 * 24 * 3600))
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[derive(Clone)]
struct Tools {
    ytdlp: PathBuf,
    js: Option<(String, PathBuf)>,
    managed: bool,
    /// Whether this yt-dlp understands `--js-runtimes` / `--remote-components` (older copies don't).
    js_flag: bool,
    remote_flag: bool,
}

pub struct Yt {
    pub shared: Arc<Mutex<Shared>>,
    music_dir: Arc<Mutex<Option<PathBuf>>>,
    wake: Sender<()>,
    pub events: Receiver<YtEvent>,
    notify: Arc<dyn Fn() + Send + Sync>,
    search_gen: Arc<AtomicU64>,
    current: Arc<Mutex<Option<(u64, Child)>>>,
    tools: Arc<Mutex<Option<Tools>>>,
    ev_tx: Sender<YtEvent>,
}

static JOB_UID: AtomicU64 = AtomicU64::new(1);
static UPDATED_THIS_SESSION: AtomicBool = AtomicBool::new(false);

impl Yt {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let jobs = load_jobs();
        let resume = jobs.iter().any(|j| j.status == JobStatus::Waiting);
        let shared = Arc::new(Mutex::new(Shared {
            setup: Setup::Unknown,
            search: Search::Idle,
            suggest: load_suggestions(),
            jobs,
            version: None,
            previews: std::collections::HashMap::new(),
        }));
        let (wake, wake_rx) = channel();
        let (ev_tx, events) = channel();
        let ev_tx2 = ev_tx.clone();
        let yt = Yt {
            shared,
            music_dir: Arc::new(Mutex::new(None)),
            wake,
            events,
            notify,
            search_gen: Arc::new(AtomicU64::new(0)),
            current: Arc::new(Mutex::new(None)),
            tools: Arc::new(Mutex::new(None)),
            ev_tx: ev_tx2,
        };
        let ctx = yt.worker_ctx();
        std::thread::Builder::new()
            .name("downloads".into())
            .spawn(move || download_worker(ctx, wake_rx, ev_tx))
            .expect("download thread");
        if resume {
            let _ = yt.wake.send(());
        }
        yt
    }

    fn save(&self) {
        save_jobs(&self.shared.lk().jobs);
    }

    fn worker_ctx(&self) -> WorkerCtx {
        WorkerCtx {
            shared: self.shared.clone(),
            music_dir: self.music_dir.clone(),
            notify: self.notify.clone(),
            current: self.current.clone(),
            tools: self.tools.clone(),
        }
    }

    pub fn set_music_dir(&self, dir: PathBuf) {
        *self.music_dir.lk() = Some(dir);
    }

    /// Get the tools ready in the background (called when the person opens search).
    pub fn prepare(&self) {
        let ready = matches!(self.shared.lk().setup, Setup::Ready | Setup::Working { .. });
        if ready || recent_fail(Duration::from_secs(60)).is_some() {
            return;
        }
        let ctx = self.worker_ctx();
        std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ensure_tools(&ctx)));
        });
    }

    pub fn search(&self, query: String) {
        clear_fail();
        let gen = self.search_gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared.lk().search = Search::Loading { query: query.clone() };
        (self.notify)();
        let ctx = self.worker_ctx();
        let gen_ref = self.search_gen.clone();
        std::thread::spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_search(&ctx, &query)))
                .unwrap_or_else(|_| Err("Something went wrong while searching. Try again.".into()));
            if gen_ref.load(Ordering::SeqCst) != gen {
                return; // a newer search replaced this one
            }
            ctx.shared.lk().search = match res {
                Ok((items, playlist)) => Search::Done {
                    query,
                    items,
                    playlist,
                },
                Err(msg) => Search::Failed { query, msg },
            };
            (ctx.notify)();
        });
    }

    /// A handle the import page uses to read links off the main thread.
    pub fn resolver(&self) -> Resolver {
        Resolver(self.worker_ctx())
    }

    /// A handle the audio thread can use to fetch streamed songs on its own.
    pub fn fetcher(&self) -> StreamFetcher {
        StreamFetcher { ctx: self.worker_ctx(), ev_tx: self.ev_tx.clone() }
    }

    /// Find songs like the seeds (YouTube's mixes for them), leaving out what you already have.
    pub fn suggest(&self, seeds: Vec<Seed>, have_ids: std::collections::HashSet<String>, have_keys: std::collections::HashSet<String>) {
        let from: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for s in &seeds {
                if !s.artist.is_empty() && !v.contains(&s.artist) {
                    v.push(s.artist.clone());
                }
            }
            v
        };
        {
            let mut s = self.shared.lk();
            if matches!(s.suggest, Suggest::Loading { .. }) {
                return;
            }
            let had = match &s.suggest {
                Suggest::Ready { items, .. } => items.clone(),
                _ => Vec::new(),
            };
            s.suggest = Suggest::Loading { had, from: from.clone() };
        }
        clear_fail();
        (self.notify)();
        let ctx = self.worker_ctx();
        std::thread::spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_suggest(&ctx, &seeds, &have_ids, &have_keys)))
                .unwrap_or_else(|_| Err("Something went wrong. Try again.".into()));
            let next = match res {
                Ok(items) => {
                    let at = now_secs();
                    crate::store::save_json(&suggest_path(), &SuggestCache { items: items.clone(), from: from.clone(), at });
                    Suggest::Ready { items, from, at }
                }
                Err(msg) => Suggest::Failed(msg),
            };
            ctx.shared.lk().suggest = next;
            (ctx.notify)();
        });
    }

    pub fn clear_search(&self) {
        self.search_gen.fetch_add(1, Ordering::SeqCst);
        self.shared.lk().search = Search::Idle;
    }

    pub fn download(&self, items: Vec<YtItem>, playlist: Option<u64>) {
        clear_fail();
        {
            let mut s = self.shared.lk();
            for item in items {
                let active = s.jobs.iter().any(|j| {
                    j.item.id == item.id
                        && matches!(j.status, JobStatus::Waiting | JobStatus::Downloading { .. } | JobStatus::Saving)
                });
                if active {
                    continue;
                }
                // Replace a previous failed/finished entry for the same video.
                s.jobs.retain(|j| j.item.id != item.id);
                s.jobs.push(Job {
                    uid: JOB_UID.fetch_add(1, Ordering::Relaxed),
                    item,
                    status: JobStatus::Waiting,
                    playlist,
                });
            }
        }
        self.save();
        let _ = self.wake.send(());
        (self.notify)();
    }

    pub fn retry(&self, uid: u64) {
        clear_fail();
        {
            let mut s = self.shared.lk();
            if let Some(j) = s.jobs.iter_mut().find(|j| j.uid == uid) {
                j.status = JobStatus::Waiting;
            }
            if let Setup::Failed(_) = s.setup {
                s.setup = Setup::Unknown;
            }
        }
        self.save();
        let _ = self.wake.send(());
        (self.notify)();
    }

    /// Try every failed download of one playlist again.
    pub fn retry_playlist(&self, pid: u64) {
        clear_fail();
        {
            let mut s = self.shared.lk();
            for j in s.jobs.iter_mut().filter(|j| j.playlist == Some(pid)) {
                if matches!(j.status, JobStatus::Failed(_)) {
                    j.status = JobStatus::Waiting;
                }
            }
        }
        self.save();
        let _ = self.wake.send(());
        (self.notify)();
    }

    pub fn cancel(&self, uid: u64) {
        if let Some((cur, child)) = self.current.lk().as_mut() {
            if *cur == uid {
                let _ = child.kill();
            }
        }
        self.shared.lk().jobs.retain(|j| j.uid != uid);
        self.save();
        (self.notify)();
    }

    pub fn clear_finished(&self) {
        self.shared
            .lock()
            .unwrap()
            .jobs
            .retain(|j| !matches!(j.status, JobStatus::Done(_) | JobStatus::Failed(_)));
        self.save();
        (self.notify)();
    }

    pub fn update_now(&self) {
        let ctx = self.worker_ctx();
        std::thread::spawn(move || {
            UPDATED_THIS_SESSION.store(false, Ordering::SeqCst);
            if let Ok(t) = ensure_tools(&ctx) {
                self_update(&ctx, &t);
            }
        });
    }

    pub fn active_count(&self) -> usize {
        self.shared
            .lock()
            .unwrap()
            .jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Waiting | JobStatus::Downloading { .. } | JobStatus::Saving))
            .count()
    }
}

/// Fetches songs that play from their site into the preview cache. Several can be on their
/// way at once (the one playing and the next in line).
#[derive(Clone)]
pub struct StreamFetcher {
    ctx: WorkerCtx,
    ev_tx: Sender<YtEvent>,
}

impl StreamFetcher {
    /// `done` gets the file (or why it failed). Asking again while it's on its way does nothing.
    pub fn fetch(&self, item: YtItem, done: impl FnOnce(Result<PathBuf, String>) + Send + 'static) {
        {
            let mut sh = self.ctx.shared.lk();
            if let Some(PreviewState::Ready(p)) = sh.previews.get(&item.id) {
                if p.is_file() {
                    let path = p.clone();
                    drop(sh);
                    done(Ok(path));
                    return;
                }
            }
            if matches!(sh.previews.get(&item.id), Some(PreviewState::Loading(_))) {
                return;
            }
            sh.previews.insert(item.id.clone(), PreviewState::Loading(0.0));
        }
        clear_fail();
        (self.ctx.notify)();
        let ctx = self.ctx.clone();
        let ev_tx = self.ev_tx.clone();
        std::thread::spawn(move || {
            let slot = Mutex::new(None);
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_preview(&ctx, &item, &slot)))
                .unwrap_or_else(|_| Err("Something went wrong. Try again.".into()));
            match res {
                Ok(path) => {
                    let art = preview_cover(&item);
                    ctx.shared.lk().previews.insert(item.id.clone(), PreviewState::Ready(path.clone()));
                    done(Ok(path.clone()));
                    let _ = ev_tx.send(YtEvent::Fetched { item, path, art });
                }
                Err(msg) => {
                    ctx.shared.lk().previews.insert(item.id.clone(), PreviewState::Failed(msg.clone()));
                    done(Err(msg.clone()));
                    let _ = ev_tx.send(YtEvent::FetchFailed { item, msg });
                }
            }
            (ctx.notify)();
        });
    }
}

#[derive(Clone)]
struct WorkerCtx {
    shared: Arc<Mutex<Shared>>,
    music_dir: Arc<Mutex<Option<PathBuf>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    current: Arc<Mutex<Option<(u64, Child)>>>,
    tools: Arc<Mutex<Option<Tools>>>,
}

impl WorkerCtx {
    fn set_setup(&self, s: Setup) {
        self.shared.lk().setup = s;
        (self.notify)();
    }
}

// ---------------------------------------------------------------------------------------------
// Tools

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.stdin(Stdio::null())
}

fn version_of(exe: &Path, arg: &str) -> Option<String> {
    let out = hidden(&mut Command::new(exe))
        .arg(arg)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for cand in [format!("{name}{EXE}"), format!("{name}.cmd")] {
            let p = dir.join(&cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

static SETUP_LOCK: Mutex<()> = Mutex::new(());
/// When the last setup attempt failed (so a background "prepare" doesn't retry in a loop, and a
/// second thread waiting on the lock doesn't immediately repeat a failure that just happened).
static LAST_FAIL: Mutex<Option<(Instant, String)>> = Mutex::new(None);

fn note_fail(msg: &str) {
    *LAST_FAIL.lk() = Some((Instant::now(), msg.to_string()));
}

fn recent_fail(within: Duration) -> Option<String> {
    LAST_FAIL
        .lk()
        .as_ref()
        .filter(|(t, _)| t.elapsed() < within)
        .map(|(_, m)| m.clone())
}

fn clear_fail() {
    *LAST_FAIL.lk() = None;
}

fn ensure_tools(ctx: &WorkerCtx) -> Result<Tools, String> {
    if let Some(t) = ctx.tools.lk().clone() {
        return Ok(t);
    }
    let _guard = SETUP_LOCK.lk();
    if let Some(t) = ctx.tools.lk().clone() {
        return Ok(t);
    }
    if let Some(m) = recent_fail(Duration::from_secs(3)) {
        return Err(m);
    }
    let bin = bin_dir();

    // 1. yt-dlp
    let managed = bin.join(format!("yt-dlp{EXE}"));
    let mut is_managed = true;
    if !managed.is_file() {
        ctx.set_setup(Setup::Working {
            label: "Setting up downloads".into(),
            progress: Some(0.0),
        });
        let asset = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp_linux" };
        let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{asset}");
        let res = crate::net::download(&url, &managed, |done, total| {
            let p = total.map(|t| done as f32 / t.max(1) as f32 * 0.5);
            ctx.set_setup(Setup::Working {
                label: "Setting up downloads".into(),
                progress: p,
            });
        });
        if let Err(e) = res {
            // Fall back to a copy the person already has.
            if let Some(p) = which("yt-dlp") {
                is_managed = false;
                let _ = e;
                return finish_tools(ctx, p, is_managed, &bin);
            }
            let _ = e;
            let msg = "Couldn't get the downloader. Check your internet connection, then try again.".to_string();
            note_fail(&msg);
            ctx.set_setup(Setup::Failed(msg.clone()));
            return Err(msg);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&managed, std::fs::Permissions::from_mode(0o755));
        }
    }
    finish_tools(ctx, managed, is_managed, &bin)
}

fn finish_tools(ctx: &WorkerCtx, ytdlp: PathBuf, managed: bool, bin: &Path) -> Result<Tools, String> {
    // 2. A JavaScript runtime (YouTube needs one to unlock full-quality audio).
    let mut js: Option<(String, PathBuf)> = None;
    let own_deno = bin.join(format!("deno{EXE}"));
    if own_deno.is_file() {
        js = Some(("deno".into(), own_deno.clone()));
    } else {
        for name in ["deno", "node", "bun"] {
            if let Some(p) = which(name) {
                if version_of(&p, "--version").is_some() {
                    js = Some((name.into(), p));
                    break;
                }
            }
        }
    }
    if js.is_none() {
        ctx.set_setup(Setup::Working {
            label: "Setting up downloads".into(),
            progress: Some(0.5),
        });
        let asset = if cfg!(windows) {
            "deno-x86_64-pc-windows-msvc.zip"
        } else {
            "deno-x86_64-unknown-linux-gnu.zip"
        };
        let url = format!("https://github.com/denoland/deno/releases/latest/download/{asset}");
        let zip_path = bin.join("deno.zip");
        let res = crate::net::download(&url, &zip_path, |done, total| {
            let p = total.map(|t| 0.5 + done as f32 / t.max(1) as f32 * 0.5);
            ctx.set_setup(Setup::Working {
                label: "Setting up downloads".into(),
                progress: p,
            });
        });
        if res.is_ok() && unzip_one(&zip_path, &format!("deno{EXE}"), &own_deno).is_ok() {
            js = Some(("deno".into(), own_deno));
        }
        let _ = std::fs::remove_file(&zip_path);
        // Without a runtime yt-dlp still works for many videos, so carry on either way.
    }
    let help = hidden(&mut Command::new(&ytdlp))
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let tools = Tools {
        ytdlp,
        js,
        managed,
        js_flag: help.contains("--js-runtimes"),
        remote_flag: help.contains("--remote-components"),
    };
    let version = version_of(&tools.ytdlp, "--version");
    if version.is_none() {
        let msg = "The downloader didn't start. Try again in a moment.".to_string();
        note_fail(&msg);
        // A broken copy (e.g. half-written) gets fetched again next time.
        if managed {
            let _ = std::fs::remove_file(&tools.ytdlp);
        }
        ctx.set_setup(Setup::Failed(msg.clone()));
        return Err(msg);
    }
    {
        let mut s = ctx.shared.lk();
        s.version = version;
        s.setup = Setup::Ready;
    }
    *ctx.tools.lk() = Some(tools.clone());
    (ctx.notify)();
    // Keep the managed copy fresh: YouTube changes often and old versions stop working.
    if tools.managed {
        let old = std::fs::metadata(&tools.ytdlp)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|e| e > Duration::from_secs(10 * 24 * 3600))
            .unwrap_or(false);
        if old {
            let ctx2 = ctx.clone();
            let t2 = tools.clone();
            std::thread::spawn(move || self_update(&ctx2, &t2));
        }
    }
    Ok(tools)
}

fn unzip_one(zip_path: &Path, name: &str, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
        let matches = entry.name().map(|n| n.rsplit('/').next() == Some(name)).unwrap_or(false);
        if matches {
            let tmp = dest.with_extension("part");
            let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            drop(out);
            std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755));
            }
            return Ok(());
        }
    }
    Err("not found in archive".into())
}

/// `yt-dlp -U` on our own copy. Returns true if it ran.
fn self_update(ctx: &WorkerCtx, tools: &Tools) -> bool {
    if !tools.managed || UPDATED_THIS_SESSION.swap(true, Ordering::SeqCst) {
        return false;
    }
    let prev = ctx.shared.lk().setup.clone();
    ctx.set_setup(Setup::Working {
        label: "Updating the downloader".into(),
        progress: None,
    });
    let _ = hidden(&mut Command::new(&tools.ytdlp))
        .arg("-U")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // -U replaces the file; touch it so we don't try again for a while.
    let version = version_of(&tools.ytdlp, "--version");
    let mut s = ctx.shared.lk();
    if version.is_some() {
        s.version = version;
    }
    s.setup = if matches!(prev, Setup::Failed(_)) { Setup::Ready } else { prev };
    drop(s);
    (ctx.notify)();
    true
}

fn base_cmd(tools: &Tools) -> Command {
    let mut c = Command::new(&tools.ytdlp);
    hidden(&mut c);
    c.env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .args(["--encoding", "utf-8", "--ignore-config", "--no-cache-dir"]);
    if tools.js_flag {
        if let Some((name, path)) = &tools.js {
            c.arg("--js-runtimes").arg(format!("{name}:{}", path.display()));
        }
    }
    if tools.remote_flag {
        // Lets yt-dlp fetch YouTube's challenge solver if its copy doesn't include one.
        c.args(["--remote-components", "ejs:github"]);
    }
    c
}

/// Turn yt-dlp's error output into one short, human sentence.
fn friendly_error(stderr: &str) -> String {
    friendly_error_for(stderr, "YouTube")
}

fn friendly_error_for(stderr: &str, site: &str) -> String {
    let low = stderr.to_lowercase();
    if low.contains("unsupported url") {
        "Segno can't open links from this site. Try a link to a song or playlist page.".into()
    } else if low.contains("drm") {
        format!("{site} protects this one, so it can't be downloaded.")
    } else if low.contains("not a bot") || low.contains("http error 429") {
        format!("{site} is limiting downloads right now. Try again in a few minutes.")
    } else if low.contains("private video") || low.contains("video unavailable") || low.contains("has been removed") || low.contains("http error 404") {
        "This one isn't available anymore.".into()
    } else if low.contains("confirm your age") || low.contains("age-restricted") {
        format!("This one is age-restricted, so {site} won't allow the download.")
    } else if low.contains("requested format is not available") {
        format!("{site} doesn't offer this one in a format Segno can play.")
    } else if low.contains("getaddrinfo")
        || low.contains("unable to download webpage")
        || low.contains("connection")
        || low.contains("timed out")
    {
        format!("Couldn't reach {site}. Check your internet connection.")
    } else if low.contains("no space left") {
        "Your drive is full. Free some space, then try again.".into()
    } else {
        let line = stderr
            .lines()
            .rev()
            .find(|l| l.starts_with("ERROR"))
            .unwrap_or_else(|| stderr.lines().last().unwrap_or("Something went wrong."));
        let line = line.trim_start_matches("ERROR:").trim();
        let line = match line.find("] ") {
            Some(i) if line.starts_with('[') => &line[i + 2..],
            _ => line,
        };
        line.chars().take(160).collect()
    }
}

fn should_update_and_retry(stderr: &str) -> bool {
    let low = stderr.to_lowercase();
    !(low.contains("private video")
        || low.contains("video unavailable")
        || low.contains("confirm your age")
        || low.contains("getaddrinfo")
        || low.contains("no space left"))
}

// ---------------------------------------------------------------------------------------------
// Search

/// A pasted link (from any site) rather than words to search YouTube for.
pub fn looks_like_link(s: &str) -> bool {
    let s = s.trim().to_lowercase();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false;
    }
    if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("www.") || s.starts_with("spotify:") {
        return true;
    }
    // A bare "site.com/something".
    match s.split_once('/') {
        Some((host, _)) => {
            let parts: Vec<&str> = host.split('.').collect();
            parts.len() >= 2
                && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
                && parts.last().map(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic())).unwrap_or(false)
        }
        None => false,
    }
}

pub fn full_url(s: &str) -> String {
    let s = s.trim();
    if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("spotify:") {
        s.to_string()
    } else {
        format!("https://{s}")
    }
}

fn is_youtube_url(u: &str) -> bool {
    let u = u.to_lowercase();
    u.contains("youtube.com/") || u.contains("youtu.be/")
}

/// Reads links for the import page. Every call blocks, so use it from a thread.
#[derive(Clone)]
pub struct Resolver(WorkerCtx);

/// One of a channel's public playlists, before its songs are read.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelPlaylist {
    pub url: String,
    pub title: String,
    pub cover: String,
}

impl Resolver {
    /// A playlist, album or song link (any site, Spotify too).
    pub fn link(&self, url: &str) -> Result<(Vec<YtItem>, Option<PlaylistInfo>), String> {
        run_search(&self.0, url)
    }

    /// The public playlists on a YouTube channel's Playlists tab: (channel name, playlists).
    pub fn channel_playlists(&self, url: &str) -> Result<(String, Vec<ChannelPlaylist>), String> {
        let tools = ensure_tools(&self.0)?;
        let out = base_cmd(&tools)
            .args(["--flat-playlist", "-J", "--no-warnings"])
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("The downloader didn't start ({e})."))?;
        if !out.status.success() {
            return Err(friendly_error_for(&String::from_utf8_lossy(&out.stderr), "YouTube"));
        }
        let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| "YouTube sent something unexpected.".to_string())?;
        Ok(channel_playlists_from(&v))
    }
}

/// Read yt-dlp's answer for a channel's Playlists tab.
fn channel_playlists_from(v: &Value) -> (String, Vec<ChannelPlaylist>) {
    let channel = str_of(v, &["channel", "uploader"])
        .or_else(|| str_of(v, &["title"]).map(|t| t.trim_end_matches(" - Playlists").to_string()))
        .unwrap_or_default();
    let mut lists = Vec::new();
    for e in v.get("entries").and_then(Value::as_array).into_iter().flatten() {
        // Playlists only: a video's id is 11 characters, a playlist's is longer.
        let Some(id) = str_of(e, &["id"]).filter(|id| id.len() > 11) else { continue };
        let url = str_of(e, &["url"]).filter(|u| u.contains("list=")).unwrap_or_else(|| format!("https://www.youtube.com/playlist?list={id}"));
        lists.push(ChannelPlaylist {
            url,
            title: str_of(e, &["title"]).unwrap_or_else(|| "Playlist".into()),
            cover: best_thumb(e).unwrap_or_default(),
        });
    }
    (channel, lists)
}

fn run_search(ctx: &WorkerCtx, query: &str) -> Result<(Vec<YtItem>, Option<PlaylistInfo>), String> {
    let q = query.trim();
    let link = looks_like_link(q).then(|| full_url(q));
    if let Some(url) = &link {
        if crate::spotify::is_spotify(url) {
            return crate::spotify::resolve(url);
        }
    }
    let tools = ensure_tools(ctx)?;
    let site = link.as_deref().map(site_from_url).unwrap_or_else(|| "YouTube".into());
    let mut attempt = 0;
    loop {
        let mut cmd = base_cmd(&tools);
        cmd.args(["--flat-playlist", "-J", "--no-warnings"]);
        match &link {
            Some(url) => {
                // A song link that happens to sit in a playlist: take just the song.
                if is_youtube_url(url) && url.contains("watch?") && url.contains("v=") {
                    cmd.arg("--no-playlist");
                }
                cmd.arg(url);
            }
            None => {
                cmd.arg(format!("ytsearch20:{q}"));
            }
        }
        let out = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("The downloader didn't start ({e})."))?;
        if out.status.success() {
            let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| format!("{site} sent something unexpected."))?;
            let (items, playlist) = parse_results(&v, link.as_deref().unwrap_or(""));
            if link.is_some() && items.is_empty() && playlist.is_none() {
                return Err(format!("Couldn't find any songs at this {site} link."));
            }
            return Ok((items, playlist));
        }
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        if attempt == 0 && should_update_and_retry(&err) && self_update(ctx, &tools) {
            attempt += 1;
            continue;
        }
        return Err(friendly_error_for(&err, &site));
    }
}

// ---------------------------------------------------------------------------------------------
// Suggestions

/// The first YouTube result for some words.
fn find_video(tools: &Tools, words: &str) -> Option<String> {
    let out = base_cmd(tools)
        .args(["--flat-playlist", "-J", "--no-warnings"])
        .arg(format!("ytsearch1:{words}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let (items, _) = parse_results(&v, "");
    items.into_iter().find(|i| i.is_youtube()).map(|i| i.id)
}

fn run_suggest(
    ctx: &WorkerCtx,
    seeds: &[Seed],
    have_ids: &std::collections::HashSet<String>,
    have_keys: &std::collections::HashSet<String>,
) -> Result<Vec<YtItem>, String> {
    let tools = ensure_tools(ctx)?;
    let mut lists: Vec<Vec<YtItem>> = Vec::new();
    let mut seed_ids = std::collections::HashSet::new();
    let mut last_err = String::new();
    for seed in seeds.iter().take(4) {
        let id = match &seed.id {
            Some(id) => id.clone(),
            None => match find_video(&tools, &seed.find) {
                Some(id) => id,
                None => continue,
            },
        };
        seed_ids.insert(id.clone());
        // YouTube's "Mix" for a song: songs that go with it.
        let url = format!("https://www.youtube.com/watch?v={id}&list=RD{id}");
        let out = base_cmd(&tools)
            .args(["--flat-playlist", "-J", "--no-warnings", "--playlist-end", "30"])
            .arg(&url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        match out {
            Ok(o) if o.status.success() => {
                if let Ok(v) = serde_json::from_slice::<Value>(&o.stdout) {
                    let (items, _) = parse_results(&v, &url);
                    if !items.is_empty() {
                        lists.push(items);
                    }
                }
            }
            Ok(o) => last_err = String::from_utf8_lossy(&o.stderr).to_string(),
            Err(e) => last_err = format!("The downloader didn't start ({e})."),
        }
    }
    if lists.is_empty() {
        return Err(if last_err.is_empty() {
            "Couldn't find suggestions right now. Try again in a bit.".into()
        } else {
            friendly_error(&last_err)
        });
    }
    // Take turns between the mixes so one artist doesn't fill the page.
    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_keys = std::collections::HashSet::new();
    let mut out = Vec::new();
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..longest {
        for l in &lists {
            let Some(it) = l.get(i) else { continue };
            let (artist, title) = guess_artist_title(it);
            let key = song_key(&artist, &title);
            let fits = it.duration == 0.0 || (60.0..=720.0).contains(&it.duration);
            if fits
                && !seed_ids.contains(&it.id)
                && !have_ids.contains(&it.id)
                && !have_keys.contains(&key)
                && seen_ids.insert(it.id.clone())
                && seen_keys.insert(key)
            {
                out.push(it.clone());
            }
        }
    }
    out.truncate(40);
    if out.is_empty() {
        return Err("Nothing new to suggest yet. Play a few more songs and try again.".into());
    }
    Ok(out)
}

/// "SoundCloud" from "https://soundcloud.com/…", "Bandcamp" from "artist.bandcamp.com".
pub fn site_from_url(url: &str) -> String {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let host = host.trim_start_matches("www.").trim_start_matches("m.").trim_start_matches("music.");
    let known = [
        ("youtube.", "YouTube"),
        ("youtu.be", "YouTube"),
        ("soundcloud.", "SoundCloud"),
        ("bandcamp.", "Bandcamp"),
        ("spotify.", "Spotify"),
        ("deezer.", "Deezer"),
        ("vimeo.", "Vimeo"),
        ("mixcloud.", "Mixcloud"),
        ("audiomack.", "Audiomack"),
        ("tiktok.", "TikTok"),
        ("instagram.", "Instagram"),
        ("twitter.", "X"),
        ("x.com", "X"),
        ("facebook.", "Facebook"),
        ("dailymotion.", "Dailymotion"),
        ("archive.org", "Internet Archive"),
    ];
    for (k, name) in known {
        if host.contains(k) {
            return name.to_string();
        }
    }
    // Otherwise the domain's main word, capitalized: "jamendo.com" → "Jamendo".
    let parts: Vec<&str> = host.split('.').collect();
    let main = if parts.len() >= 2 { parts[parts.len() - 2] } else { host };
    let mut c = main.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => "this site".into(),
    }
}

/// yt-dlp's extractor name to a site name: "SoundcloudSet" → "SoundCloud".
fn site_from_extractor(ie: &str) -> Option<String> {
    let low = ie.to_lowercase();
    let known = [
        ("youtube", "YouTube"),
        ("soundcloud", "SoundCloud"),
        ("bandcamp", "Bandcamp"),
        ("vimeo", "Vimeo"),
        ("mixcloud", "Mixcloud"),
        ("audiomack", "Audiomack"),
        ("deezer", "Deezer"),
        ("tiktok", "TikTok"),
        ("dailymotion", "Dailymotion"),
    ];
    known.iter().find(|(k, _)| low.starts_with(k)).map(|(_, n)| n.to_string())
}

fn str_of(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

/// The biggest picture in a yt-dlp "thumbnails" list, or its "thumbnail".
fn best_thumb(v: &Value) -> Option<String> {
    let list = v.get("thumbnails").and_then(Value::as_array);
    let best = list.and_then(|l| {
        l.iter()
            .filter(|t| t.get("url").and_then(Value::as_str).map(|u| u.starts_with("http")).unwrap_or(false))
            .max_by_key(|t| t.get("width").and_then(Value::as_u64).unwrap_or(0) * t.get("height").and_then(Value::as_u64).unwrap_or(1))
            .and_then(|t| t.get("url").and_then(Value::as_str))
            .map(str::to_string)
    });
    best.or_else(|| str_of(v, &["thumbnail"]).filter(|u| u.starts_with("http")))
}

/// A readable title from a link's last part: ".../artist/some-song-name" → "Some Song Name".
fn title_from_url(url: &str) -> String {
    let last = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("");
    last.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn item_from(v: &Value, fallback_channel: &str, root_ie: &str, link: &str) -> Option<YtItem> {
    let ie = str_of(v, &["ie_key", "extractor_key"]).unwrap_or_else(|| root_ie.to_string());
    let page = str_of(v, &["webpage_url", "url"]).filter(|u| u.starts_with("http")).unwrap_or_default();
    let youtube = ie.to_lowercase().starts_with("youtube") || (ie.is_empty() && is_youtube_url(&page)) || (page.is_empty() && is_youtube_url(link));
    let title = str_of(v, &["track", "title"]).unwrap_or_default();
    if title == "[Private video]" || title == "[Deleted video]" {
        return None;
    }
    let live = v.get("live_status").and_then(Value::as_str).map(|s| s == "is_live").unwrap_or(false);
    if live {
        return None;
    }
    let channel = str_of(v, &["artist", "channel", "uploader", "creator"]).unwrap_or_else(|| fallback_channel.to_string());
    let duration = v.get("duration").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    if youtube {
        let id = str_of(v, &["id"])?;
        if id.len() != 11 || title.is_empty() {
            return None; // channels, playlists, shorts shelves
        }
        return Some(YtItem { id, title, channel, duration, ..Default::default() });
    }
    // Any other site: keep its own link, picture and name.
    let page = if page.is_empty() { link.to_string() } else { page };
    if page.is_empty() {
        return None;
    }
    // yt-dlp's Generic extractor (direct file links, sites it doesn't know) names a song after
    // its file, so ".../album-a/song.mp3" and ".../album-b/song.mp3" both came out as "song"
    // and were treated as one song. Those are told apart by their whole link instead.
    let generic = ie.is_empty() || ie.eq_ignore_ascii_case("generic");
    let raw_id = match str_of(v, &["id"]) {
        Some(id) if !generic => id,
        _ => format!("{:x}", crate::store::hash_bytes(page.as_bytes())),
    };
    let site = site_from_extractor(&ie).unwrap_or_else(|| site_from_url(&page));
    let slug: String = site.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let title = if title.is_empty() { title_from_url(&page) } else { title };
    if title.is_empty() {
        return None;
    }
    Some(YtItem {
        id: format!("{slug}:{raw_id}"),
        title,
        channel,
        duration,
        url: page,
        thumb: best_thumb(v).unwrap_or_default(),
        site,
        ..Default::default()
    })
}

fn parse_results(v: &Value, link: &str) -> (Vec<YtItem>, Option<PlaylistInfo>) {
    let root_ie = str_of(v, &["extractor_key", "ie_key"]).unwrap_or_default();
    if let Some(entries) = v.get("entries").and_then(Value::as_array) {
        let channel = str_of(v, &["channel", "uploader", "artist"]).unwrap_or_default();
        // Child entries of a playlist inherit the playlist's site.
        let child_ie = root_ie.trim_end_matches("Tab").trim_end_matches("Set").trim_end_matches("Album").trim_end_matches("Playlist").to_string();
        let items: Vec<YtItem> = entries.iter().filter_map(|e| item_from(e, &channel, &child_ie, "")).collect();
        let is_search = root_ie.contains("Search");
        let site = site_from_extractor(&root_ie).unwrap_or_else(|| site_from_url(link));
        let playlist = (!is_search).then(|| PlaylistInfo {
            title: str_of(v, &["title"]).unwrap_or_else(|| format!("{site} playlist")),
            channel,
            url: link.to_string(),
            cover: best_thumb(v).unwrap_or_default(),
            site,
        });
        (items, playlist)
    } else {
        (item_from(v, "", &root_ie, link).into_iter().collect(), None)
    }
}

// ---------------------------------------------------------------------------------------------
// Downloads

fn download_worker(ctx: WorkerCtx, wake: Receiver<()>, events: Sender<YtEvent>) {
    loop {
        let next = {
            let mut s = ctx.shared.lk();
            s.jobs
                .iter_mut()
                .find(|j| j.status == JobStatus::Waiting)
                .map(|j| {
                    j.status = JobStatus::Downloading {
                        pct: 0.0,
                        speed: String::new(),
                    };
                    j.clone()
                })
        };
        let Some(job) = next else {
            if wake.recv().is_err() {
                return;
            }
            continue;
        };
        (ctx.notify)();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(&ctx, &job)))
            .unwrap_or_else(|_| Err("Something went wrong with this download. Try again.".into()));
        let status = match result {
            Ok(path) => {
                let _ = events.send(YtEvent::Downloaded {
                    path: path.clone(),
                    playlist: job.playlist,
                    key: job.item.id.clone(),
                });
                JobStatus::Done(path)
            }
            Err(msg) => JobStatus::Failed(msg),
        };
        {
            let mut s = ctx.shared.lk();
            if let Some(j) = s.jobs.iter_mut().find(|j| j.uid == job.uid) {
                j.status = status;
            }
            save_jobs(&s.jobs);
        }
        (ctx.notify)();
    }
}

fn set_status(ctx: &WorkerCtx, uid: u64, status: JobStatus) {
    let mut s = ctx.shared.lk();
    if let Some(j) = s.jobs.iter_mut().find(|j| j.uid == uid) {
        j.status = status;
    }
}

fn run_job(ctx: &WorkerCtx, job: &Job) -> Result<PathBuf, String> {
    let tools = ensure_tools(ctx)?;
    let music = ctx
        .music_dir
        .lock()
        .unwrap()
        .clone()
        .ok_or("Pick a music folder first.")?;
    // Left over from before the rename.
    let _ = std::fs::remove_dir_all(music.join(".encore-tmp"));
    let tmp = music.join(".segno-tmp");
    std::fs::create_dir_all(&tmp).map_err(|_| "Couldn't write to your music folder.".to_string())?;
    crate::platform::hide_path(&tmp);

    let uid = job.uid;
    let notify = ctx.notify.clone();
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut on_progress = |pct: f32, speed: String| {
        set_status(ctx, uid, JobStatus::Downloading { pct, speed });
        // Battery: at most four UI refreshes per second for progress.
        if last.elapsed() > Duration::from_millis(250) {
            last = Instant::now();
            notify();
        }
    };
    let mut attempt = 0;
    let reused = take_from_preview(&job.item, &tmp);
    let meta = loop {
        if let Some(m) = reused.clone() {
            break m;
        }
        match run_ytdlp(ctx, &tools, &job.item, job.uid, &tmp, &ctx.current, &mut on_progress) {
            Ok(meta) => break meta,
            Err(stderr) => {
                if !ctx.shared.lk().jobs.iter().any(|j| j.uid == job.uid) {
                    return Err("Cancelled".into());
                }
                if attempt == 0 && should_update_and_retry(&stderr) && self_update(ctx, &tools) {
                    attempt += 1;
                    set_status(
                        ctx,
                        job.uid,
                        JobStatus::Downloading {
                            pct: 0.0,
                            speed: String::new(),
                        },
                    );
                    continue;
                }
                cleanup_tmp(&tmp, &job.item.stem());
                return Err(friendly_error_for(&stderr, job.item.source()));
            }
        }
    };
    set_status(ctx, job.uid, JobStatus::Saving);
    (ctx.notify)();

    let stem = job.item.stem();
    let audio = meta
        .get("filepath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| find_audio(&tmp, &stem))
        .ok_or("The download finished but the file is missing.")?;
    let ext = audio
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .filter(|e| crate::library::AUDIO_EXT.contains(&e.as_str()))
        .ok_or_else(|| format!("{} only offered this one in a format Segno can't play.", job.item.source()))?;

    let tags = clean_meta(&meta, &job.item);
    let thumb = ["jpg", "webp", "png", "jpeg"]
        .iter()
        .map(|e| tmp.join(format!("{stem}.{e}")))
        .find(|p| p.is_file());
    // Songs from Spotify use the album cover from Spotify rather than the video's thumbnail.
    let cover = item_cover(&job.item).or_else(|| thumb.as_ref().and_then(|p| cover_jpeg(p)));
    write_tags(&audio, &tags, &job.item.id, cover);

    let name = sanitize(&if tags.artist.is_empty() {
        tags.title.clone()
    } else {
        format!("{} - {}", tags.artist, tags.title)
    });
    let mut dest = music.join(format!("{name}.{ext}"));
    let mut n = 2;
    while dest.exists() {
        dest = music.join(format!("{name} ({n}).{ext}"));
        n += 1;
    }
    std::fs::rename(&audio, &dest)
        .or_else(|_| std::fs::copy(&audio, &dest).map(|_| ()).and_then(|_| std::fs::remove_file(&audio)))
        .map_err(|_| "Couldn't move the song into your music folder.".to_string())?;
    if let Some(t) = thumb {
        let _ = std::fs::remove_file(t);
    }
    cleanup_tmp(&tmp, &stem);
    Ok(dest)
}

/// A song's file in the preview cache, if it was fetched before (and is complete).
pub fn cached(item: &YtItem) -> Option<PathBuf> {
    preview_entry(item).map(|(path, _)| path)
}

/// Saved next to each preview: the full id of the song it holds.
const PREVIEW_ID: &str = "segno_id";

/// Whether a preview's saved details belong to this song. Older previews (before 1.1.9) only
/// have yt-dlp's own id, which for YouTube is the song's id.
fn is_entry_for(meta: &Value, item: &YtItem) -> bool {
    let saved = meta.get(PREVIEW_ID).or_else(|| meta.get("id")).and_then(Value::as_str);
    saved == Some(item.id.as_str())
}

/// A song's preview: its audio file and saved details, only if the entry is complete and
/// really is this song (checked against the id saved with it, never just the file name).
fn preview_entry(item: &YtItem) -> Option<(PathBuf, Value)> {
    let dir = preview_dir();
    let stem = item.stem();
    let meta: Value = serde_json::from_slice(&std::fs::read(dir.join(format!("{stem}.json"))).ok()?).ok()?;
    if !is_entry_for(&meta, item) {
        return None;
    }
    Some((find_audio(&dir, &stem)?, meta))
}

/// The downloaded audio file for a stem, whatever its extension.
fn find_audio(dir: &Path, stem: &str) -> Option<PathBuf> {
    crate::library::AUDIO_EXT
        .iter()
        .map(|e| dir.join(format!("{stem}.{e}")))
        .find(|p| p.is_file())
}

/// For Spotify songs: the album cover Spotify shows, as a square JPEG.
fn item_cover(item: &YtItem) -> Option<Vec<u8>> {
    if item.find.is_empty() || item.thumb.is_empty() {
        return None;
    }
    let bytes = crate::net::get_bytes(&item.thumb, 8 << 20).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let sq = crate::art::square_crop(&img);
    let mut buf = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
    sq.to_rgb8().write_with_encoder(enc).ok()?;
    Some(buf)
}

fn run_preview(ctx: &WorkerCtx, item: &YtItem, slot: &Mutex<Option<(u64, Child)>>) -> Result<PathBuf, String> {
    let dir = preview_dir();
    let stem = item.stem();
    let meta_path = dir.join(format!("{stem}.json"));
    if let Some((audio, _)) = preview_entry(item) {
        return Ok(audio);
    }
    let tools = ensure_tools(ctx)?;
    let id = item.id.clone();
    let shared = ctx.shared.clone();
    let notify = ctx.notify.clone();
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut on_progress = |pct: f32, _speed: String| {
        shared.lk().previews.insert(id.clone(), PreviewState::Loading(pct));
        if last.elapsed() > Duration::from_millis(250) {
            last = Instant::now();
            notify();
        }
    };
    let mut attempt = 0;
    let meta = loop {
        match run_ytdlp(ctx, &tools, item, 0, &dir, slot, &mut on_progress) {
            Ok(m) => break m,
            Err(stderr) => {
                if stderr == "killed" {
                    return Err("Cancelled".into());
                }
                if attempt == 0 && should_update_and_retry(&stderr) && self_update(ctx, &tools) {
                    attempt += 1;
                    continue;
                }
                return Err(friendly_error_for(&stderr, item.source()));
            }
        }
    };
    let path = meta
        .get("filepath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| find_audio(&dir, &stem))
        .ok_or("The song didn't arrive. Try again.")?;
    // Save the details with the song's full id, so the entry is only ever used for this song.
    let mut saved = match meta {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    saved.insert(PREVIEW_ID.into(), Value::String(item.id.clone()));
    let _ = std::fs::write(&meta_path, serde_json::to_vec(&Value::Object(saved)).unwrap_or_default());
    Ok(path)
}

/// Cover art for a previewed song: the Spotify cover, or the thumbnail yt-dlp saved next to it.
fn preview_cover(item: &YtItem) -> Option<(u64, [u8; 3])> {
    if let Some(jpg) = item_cover(item) {
        return crate::art::store_cover(&jpg, None);
    }
    let dir = preview_dir();
    let stem = item.stem();
    let thumb = ["jpg", "webp", "png", "jpeg"]
        .iter()
        .map(|e| dir.join(format!("{stem}.{e}")))
        .find(|p| p.is_file())?;
    let bytes = std::fs::read(thumb).ok()?;
    crate::art::store_cover(&bytes, None)
}

/// If the song was already played before downloading, copy that file instead of downloading again.
fn take_from_preview(item: &YtItem, tmp: &Path) -> Option<Value> {
    let dir = preview_dir();
    let stem = item.stem();
    let (audio, mut meta) = preview_entry(item)?;
    let dest = tmp.join(audio.file_name()?);
    std::fs::copy(&audio, &dest).ok()?;
    for ext in ["jpg", "webp", "png", "jpeg"] {
        let t = dir.join(format!("{stem}.{ext}"));
        if t.is_file() {
            let _ = std::fs::copy(&t, tmp.join(format!("{stem}.{ext}")));
            break;
        }
    }
    if let Value::Object(m) = &mut meta {
        m.insert("filepath".into(), Value::String(dest.to_string_lossy().to_string()));
    }
    Some(meta)
}

/// Delete one song's leftover files from the temp folder ("<stem>.m4a", "<stem>.webp",
/// "<stem>.m4a.part"…). Only names that are the stem followed by a "." count, so cleaning up
/// "soundcloud_12" never touches "soundcloud_123.m4a" from another download.
fn cleanup_tmp(tmp: &Path, stem: &str) {
    let mine = format!("{stem}.");
    if let Ok(rd) = std::fs::read_dir(tmp) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == stem || name.starts_with(&mine) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let _ = std::fs::remove_dir(tmp); // only if empty
}

fn human_speed(bps: f64) -> String {
    if bps >= 1_000_000.0 {
        format!("{:.1} MB/s", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.0} KB/s", bps / 1_000.0)
    } else {
        String::new()
    }
}

/// Run one download. Ok = metadata JSON printed after the move; Err = stderr text.
fn run_ytdlp(
    _ctx: &WorkerCtx,
    tools: &Tools,
    item: &YtItem,
    uid: u64,
    tmp: &Path,
    slot: &Mutex<Option<(u64, Child)>>,
    on_progress: &mut dyn FnMut(f32, String),
) -> Result<Value, String> {
    let first = run_ytdlp_once(tools, item, uid, tmp, slot, on_progress, true);
    // A Spotify song with no YouTube result of about the right length: take the best match anyway.
    if !item.find.is_empty() && item.duration > 0.0 && matches!(&first, Err(e) if e == NO_MATCH) {
        return run_ytdlp_once(tools, item, uid, tmp, slot, on_progress, false);
    }
    first
}

const NO_MATCH: &str = "Couldn't find this song on YouTube.";

fn run_ytdlp_once(
    tools: &Tools,
    item: &YtItem,
    uid: u64,
    tmp: &Path,
    slot: &Mutex<Option<(u64, Child)>>,
    on_progress: &mut dyn FnMut(f32, String),
    match_length: bool,
) -> Result<Value, String> {
    let mut cmd = base_cmd(tools);
    // Our own file name (the item's stem), so any site and any search lands where we look.
    let out_tpl = tmp.join(format!("{}.%(ext)s", item.stem()));
    cmd.args([
        "-f",
        // Formats Segno can play without converting: M4A/AAC first (what YouTube offers),
        // then MP3, FLAC and Ogg Vorbis for other sites.
        "bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]/bestaudio[ext=mp3]/bestaudio[acodec=mp3]/bestaudio[ext=flac]/bestaudio[acodec=flac]/bestaudio[acodec=vorbis]/best[ext=mp3]/best[ext=m4a]",
        "--no-playlist",
        "--no-mtime",
        "--force-overwrites",
        "--write-thumbnail",
        "--newline",
        "--progress",
        "--progress-template",
        "download:ENCP|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s",
        "--print",
        "after_move:ENCM|%(.{id,title,uploader,channel,artist,artists,creator,track,album,release_year,upload_date,filepath})j",
        "-o",
    ]);
    cmd.arg(&out_tpl);
    cmd.arg("-o").arg(format!("thumbnail:{}", out_tpl.display()));
    if !item.find.is_empty() {
        // A Spotify song found on YouTube: take the first result whose length is close to
        // the real song's, so a 10-minute live version or a 30-second clip doesn't win.
        cmd.args(["--max-downloads", "1"]);
        if match_length && item.duration > 0.0 {
            let lo = (item.duration * 0.85 - 5.0).max(0.0) as u32;
            let hi = (item.duration * 1.15 + 5.0) as u32;
            cmd.arg("--match-filter").arg(format!("duration >= {lo} & duration <= {hi}"));
        }
    }
    cmd.arg(item.url());
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("The downloader didn't start ({e})."))?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    *slot.lk() = Some((uid, child));

    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let mut meta: Option<Value> = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("ENCP|") {
            let parts: Vec<&str> = rest.split('|').collect();
            let num = |i: usize| parts.get(i).and_then(|s| s.trim().parse::<f64>().ok());
            let done = num(0).unwrap_or(0.0);
            let total = num(1).or(num(2)).unwrap_or(0.0);
            let pct = if total > 0.0 { (done / total).clamp(0.0, 1.0) as f32 } else { 0.0 };
            let speed = num(3).map(human_speed).unwrap_or_default();
            on_progress(pct, speed);
        } else if let Some(rest) = line.strip_prefix("ENCM|") {
            meta = serde_json::from_str(rest).ok();
        }
    }
    let status = {
        let mut cur = slot.lk();
        let st = cur.as_mut().and_then(|(_, c)| c.wait().ok());
        *cur = None;
        st
    };
    #[cfg(unix)]
    let killed = status.map(|s| {
        use std::os::unix::process::ExitStatusExt;
        s.signal().is_some()
    }).unwrap_or(false);
    // On Windows a killed process just exits; the caller treats it as a normal failure.
    #[cfg(not(unix))]
    let killed = false;
    let err = err_thread.join().unwrap_or_default();
    // 101: stopped on purpose after the first match (--max-downloads).
    let ok = |s: &std::process::ExitStatus| s.success() || s.code() == Some(101);
    match (status, meta) {
        (Some(s), Some(m)) if ok(&s) => Ok(m),
        (Some(s), None) if ok(&s) && find_audio(tmp, &item.stem()).is_some() => Ok(Value::Null),
        (Some(s), None) if ok(&s) && !item.find.is_empty() => Err(NO_MATCH.into()),
        (Some(s), None) if ok(&s) => Ok(Value::Null),
        _ if killed => Err("killed".into()),
        _ => Err(if err.trim().is_empty() { "The download stopped.".into() } else { err }),
    }
}

// ---------------------------------------------------------------------------------------------
// Tags

pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: u32,
}

const JUNK: &[&str] = &[
    "official", "video", "audio", "lyric", "visualizer", "visualiser", "hd", "4k", "hq", "mv", "m/v", "clip",
    "explicit", "music video", "full song", "فيديو", "كليب", "حصري", "اوديو", "أوديو", "الكلمات",
];

/// Strip "(Official Video)", "[Lyrics]", "| Official Audio" and friends.
pub fn remove_junk(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let close = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '【' => Some('】'),
            _ => None,
        };
        if let Some(close) = close {
            if let Some(j) = chars[i + 1..].iter().position(|&x| x == close) {
                let inner: String = chars[i + 1..i + 1 + j].iter().collect();
                let low = inner.to_lowercase();
                let junk = JUNK.iter().any(|w| {
                    low.split(|ch: char| !ch.is_alphanumeric() && ch != '/')
                        .any(|word| word == *w || (w.len() >= 5 && word.starts_with(*w)))
                        || (w.contains(' ') && low.contains(w))
                        || (!w.is_ascii() && low.contains(w))
                });
                if junk {
                    i += j + 2;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    // A trailing "| Official Video" style segment.
    if let Some(idx) = out.rfind(" | ") {
        let tail = out[idx + 3..].to_lowercase();
        if JUNK.iter().any(|w| tail.contains(w)) {
            out.truncate(idx);
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim_matches(|c: char| c == '-' || c == '|' || c == '–' || c.is_whitespace()).to_string()
}

fn clean_channel(ch: &str) -> String {
    let mut c = ch.trim().to_string();
    for suf in [" - Topic", "VEVO", " Official", "Official"] {
        if c.ends_with(suf) && c.len() > suf.len() {
            c.truncate(c.len() - suf.len());
            c = c.trim().to_string();
        }
    }
    c
}

pub fn clean_meta(meta: &Value, item: &YtItem) -> Tags {
    let s = |k: &str| meta.get(k).and_then(Value::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    // Found from a Spotify link: Spotify's own title and artist are the clean ones.
    if !item.find.is_empty() {
        return Tags {
            title: item.title.clone(),
            artist: item.channel.clone(),
            album: item.album.clone(),
            year: 0,
        };
    }
    let artists = meta
        .get("artists")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
        .filter(|x| !x.is_empty());
    let year = meta
        .get("release_year")
        .and_then(Value::as_u64)
        .map(|y| y as u32)
        .unwrap_or(0);
    let album = s("album").unwrap_or_default();
    let artist_field = artists.or_else(|| s("artist")).or_else(|| s("creator"));
    if let (Some(track), Some(artist)) = (s("track"), artist_field.clone()) {
        return Tags {
            title: remove_junk(&track),
            artist,
            album,
            year,
        };
    }
    let title = s("title").unwrap_or_else(|| item.title.clone());
    let channel = clean_channel(&s("channel").or_else(|| s("uploader")).unwrap_or_else(|| item.channel.clone()));
    let mut artist = artist_field.unwrap_or(channel);
    let mut song = title.clone();
    for sep in [" - ", " – ", " — "] {
        if let Some((a, t)) = title.split_once(sep) {
            if !a.trim().is_empty() && !t.trim().is_empty() {
                artist = a.trim().to_string();
                song = t.trim().to_string();
                break;
            }
        }
    }
    let song = remove_junk(&song);
    Tags {
        title: if song.is_empty() { title } else { song },
        artist: remove_junk(&artist),
        album,
        year,
    }
}

/// Square, 600 px JPEG cover from a video thumbnail.
fn cover_jpeg(path: &Path) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?;
    let sq = crate::art::square_crop(&img);
    let sq = if sq.width() > 600 {
        sq.resize_exact(600, 600, image::imageops::FilterType::CatmullRom)
    } else {
        sq
    };
    let mut buf = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
    sq.to_rgb8().write_with_encoder(enc).ok()?;
    Some(buf)
}

fn write_tags(path: &Path, t: &Tags, yt_id: &str, cover: Option<Vec<u8>>) {
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::*;
    use lofty::tag::Tag;
    let Ok(mut tf) = lofty::read_from_path(path) else { return };
    if tf.primary_tag().is_none() {
        let tt = tf.primary_tag_type();
        tf.insert_tag(Tag::new(tt));
    }
    let Some(tag) = tf.primary_tag_mut() else { return };
    tag.set_title(t.title.clone());
    if !t.artist.is_empty() {
        tag.set_artist(t.artist.clone());
    }
    if !t.album.is_empty() {
        tag.set_album(t.album.clone());
    }
    if t.year > 0 {
        tag.set_date(lofty::tag::items::Timestamp {
            year: t.year as u16,
            ..Default::default()
        });
    }
    // Which song this is, so search results show it's already in the library.
    if yt_id.len() == 11 && !yt_id.contains(':') {
        tag.set_comment(format!("youtube:{yt_id}"));
    } else {
        tag.set_comment(format!("segno:{yt_id}"));
    }
    if let Some(bytes) = cover {
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(
            Picture::unchecked(bytes)
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Jpeg)
                .build(),
        );
    }
    let _ = tf.save_to_path(path, WriteOptions::default());
}

pub fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s = s.trim_end_matches(['.', ' ']).to_string();
    if s.chars().count() > 120 {
        s = s.chars().take(120).collect::<String>().trim_end().to_string();
    }
    // Windows won't make a file named after a device (CON, COM4, LPT¹…), even with an
    // extension after it ("CON.txt", "CON .txt"). The name before the first "." decides, and
    // it gets a "_" added ("CON_.txt").
    let base = s[..s.find('.').unwrap_or(s.len())].trim_end();
    if is_windows_device_name(base) {
        s.insert(base.len(), '_');
    }
    if s.is_empty() {
        s = format!("Track {}", now_secs());
    }
    s
}

/// The names Windows keeps for devices, in any letter case: CON, PRN, AUX, NUL, CONIN$,
/// CONOUT$, and COM or LPT followed by one digit 0–9 or ¹ ² ³.
fn is_windows_device_name(base: &str) -> bool {
    let u = base.to_ascii_uppercase();
    if matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    ["COM", "LPT"].iter().any(|p| {
        let mut rest = u.strip_prefix(p).unwrap_or("").chars();
        matches!((rest.next(), rest.next()), (Some(c), None) if c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn channel_playlists_are_read() {
        let v = serde_json::json!({
            "id": "UCabc", "title": "Some Name - Playlists", "channel": "Some Name",
            "entries": [
                {"_type": "url", "id": "PLroadtrip0123456", "url": "https://www.youtube.com/playlist?list=PLroadtrip0123456", "title": "Road trip",
                 "thumbnails": [{"url": "https://i.ytimg.com/vi/x/hqdefault.jpg", "width": 480, "height": 360}]},
                {"_type": "url", "id": "OLAK5uy_album01234", "title": "An album"},
                {"_type": "url", "id": "dQw4w9WgXcQ", "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "title": "A video"}
            ]
        });
        let (channel, lists) = super::channel_playlists_from(&v);
        assert_eq!(channel, "Some Name");
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].title, "Road trip");
        assert_eq!(lists[0].cover, "https://i.ytimg.com/vi/x/hqdefault.jpg");
        assert_eq!(lists[1].url, "https://www.youtube.com/playlist?list=OLAK5uy_album01234");
    }

    use super::*;

    #[test]
    fn suggestion_keys() {
        let it = YtItem {
            id: "abcdefghijk".into(),
            title: "Nova Lane - Glass Houses (Official Video)".into(),
            channel: "NovaLaneVEVO".into(),
            ..Default::default()
        };
        let (a, t) = guess_artist_title(&it);
        assert_eq!((a.as_str(), t.as_str()), ("Nova Lane", "Glass Houses"));
        assert_eq!(song_key(&a, &t), song_key("nova lane", "Glass Houses!"));
        let topic = YtItem { title: "Glass Houses".into(), channel: "Nova Lane - Topic".into(), ..it };
        assert_eq!(guess_artist_title(&topic).0, "Nova Lane");
    }
    #[test]
    fn junk() {
        assert_eq!(remove_junk("Blinding Lights (Official Video)"), "Blinding Lights");
        assert_eq!(remove_junk("Song [Lyrics] (feat. X)"), "Song (feat. X)");
        assert_eq!(remove_junk("Song | Official Music Video"), "Song");
        assert_eq!(remove_junk("Hello (Remastered 2011)"), "Hello (Remastered 2011)");
        assert_eq!(remove_junk("Video Games"), "Video Games");
    }
    #[test]
    fn meta() {
        let item = YtItem {
            id: "abcdefghijk".into(),
            title: "The Weeknd - Blinding Lights (Official Video)".into(),
            channel: "TheWeekndVEVO".into(),
            duration: 200.0,
            ..Default::default()
        };
        let m = serde_json::json!({"title": item.title, "channel": "TheWeekndVEVO"});
        let t = clean_meta(&m, &item);
        assert_eq!(t.artist, "The Weeknd");
        assert_eq!(t.title, "Blinding Lights");
        let m = serde_json::json!({"title": "x", "track": "Blinding Lights", "artists": ["The Weeknd"], "album": "After Hours", "release_year": 2020});
        let t = clean_meta(&m, &item);
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str(), t.year), ("Blinding Lights", "The Weeknd", "After Hours", 2020));
        let m = serde_json::json!({"title": "Lose Yourself", "channel": "Eminem - Topic"});
        let t = clean_meta(&m, &item);
        assert_eq!(t.artist, "Eminem");
    }
    #[test]
    fn names() {
        assert_eq!(sanitize("AC/DC - Back: In Black?"), "AC DC - Back In Black");
        assert!(looks_like_link("https://youtu.be/dQw4w9WgXcQ"));
        assert!(looks_like_link("https://www.youtube.com/playlist?list=PL123"));
        assert!(!looks_like_link("lofi beats"));
        assert!(looks_like_link("https://soundcloud.com/artist/song"));
        assert!(looks_like_link("soundcloud.com/artist/song"));
        assert!(looks_like_link("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"));
        assert!(!looks_like_link("Mr. Brightside"));
        assert!(!looks_like_link("ac/dc"));
        assert_eq!(site_from_url("https://soundcloud.com/x/y"), "SoundCloud");
        assert_eq!(site_from_url("https://someartist.bandcamp.com/track/x"), "Bandcamp");
        assert_eq!(site_from_url("https://www.jamendo.com/track/1"), "Jamendo");
        assert_eq!(title_from_url("https://soundcloud.com/artist/my-cool-song?si=1"), "My Cool Song");
    }

    #[test]
    fn other_sites() {
        // A SoundCloud set, flat: entries carry their own links.
        let v = serde_json::json!({
            "_type": "playlist", "extractor_key": "SoundcloudSet", "title": "Summer", "uploader": "DJ X",
            "thumbnails": [{"url": "https://i1.sndcdn.com/a-t500x500.jpg", "width": 500, "height": 500}],
            "entries": [
                {"ie_key": "Soundcloud", "id": "111", "url": "https://soundcloud.com/djx/first-track", "title": "First Track", "duration": 200.5},
                {"ie_key": "Soundcloud", "id": "222", "url": "https://soundcloud.com/djx/second-track"}
            ]
        });
        let (items, pl) = parse_results(&v, "https://soundcloud.com/djx/sets/summer");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "soundcloud:111");
        assert_eq!(items[0].site, "SoundCloud");
        assert_eq!(items[1].title, "Second Track");
        assert_eq!(items[0].url(), "https://soundcloud.com/djx/first-track");
        let pl = pl.unwrap();
        assert_eq!(pl.title, "Summer");
        assert_eq!(pl.cover, "https://i1.sndcdn.com/a-t500x500.jpg");
        // A single page on some other site.
        let v = serde_json::json!({"extractor_key": "Bandcamp", "id": 42, "title": "Song", "artist": "Band",
            "webpage_url": "https://band.bandcamp.com/track/song", "thumbnail": "https://f4.bcbits.com/img/a1.jpg"});
        let (items, pl) = parse_results(&v, "https://band.bandcamp.com/track/song");
        assert!(pl.is_none());
        assert_eq!(items[0].id, "bandcamp:42");
        assert_eq!(items[0].channel, "Band");
        assert!(items[0].stem().starts_with("bandcamp_42-"), "{}", items[0].stem());
        // YouTube results keep plain ids.
        let v = serde_json::json!({"_type": "playlist", "extractor_key": "YoutubeSearch", "entries": [
            {"ie_key": "Youtube", "id": "dQw4w9WgXcQ", "title": "Song", "channel": "Ch", "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ"}]});
        let (items, pl) = parse_results(&v, "");
        assert!(pl.is_none());
        assert_eq!(items[0].id, "dQw4w9WgXcQ");
        assert!(items[0].url.is_empty());
    }

    fn item(id: &str) -> YtItem {
        YtItem { id: id.into(), ..Default::default() }
    }

    #[test]
    fn generic_links_are_told_apart_by_their_link() {
        let song = |url: &str| serde_json::json!({ "extractor_key": "Generic", "id": "song", "title": "song", "webpage_url": url });
        let (a, _) = parse_results(&song("https://example.test/album-a/song.mp3"), "https://example.test/album-a/song.mp3");
        let (b, _) = parse_results(&song("https://example.test/album-b/song.mp3"), "https://example.test/album-b/song.mp3");
        let (a2, _) = parse_results(&song("https://example.test/album-a/song.mp3"), "https://example.test/album-a/song.mp3");
        assert_eq!((a.len(), b.len()), (1, 1));
        assert_ne!(a[0].id, b[0].id);
        assert_ne!(a[0].stem(), b[0].stem());
        assert_eq!(a[0].id, a2[0].id, "the same link is the same song every time");
        // A preview saved for one isn't used for the other.
        let saved = serde_json::json!({ PREVIEW_ID: a[0].id.clone() });
        assert!(is_entry_for(&saved, &a[0]) && !is_entry_for(&saved, &b[0]));
        // Sites with real ids keep them.
        let sc = serde_json::json!({ "extractor_key": "Soundcloud", "id": "111", "title": "T", "webpage_url": "https://soundcloud.com/x/t" });
        assert_eq!(parse_results(&sc, "https://soundcloud.com/x/t").0[0].id, "soundcloud:111");
    }

    #[test]
    fn unfinished_downloads_are_always_kept() {
        let job = |uid: u64, status: JobStatus| Job { uid, item: item(&format!("x:{uid}")), status, playlist: None };
        // 400 finished downloads, then 301 still waiting and one going.
        let mut jobs: Vec<Job> = (0..400).map(|i| job(i, JobStatus::Done(PathBuf::from("a.m4a")))).collect();
        jobs.extend((400..701).map(|i| job(i, JobStatus::Waiting)));
        jobs.push(job(701, JobStatus::Downloading { pct: 0.5, speed: String::new() }));
        let kept = jobs_to_keep(&jobs);
        let waiting = kept.iter().filter(|j| !matches!(j.status, JobStatus::Done(_) | JobStatus::Failed(_))).count();
        assert_eq!(waiting, 302, "every unfinished download is kept");
        let done: Vec<u64> = kept.iter().filter(|j| matches!(j.status, JobStatus::Done(_))).map(|j| j.uid).collect();
        assert_eq!(done.len(), HISTORY);
        assert_eq!(done[0], 100, "the oldest finished ones go first");
        // Order is kept.
        assert!(kept.windows(2).all(|w| w[0].uid < w[1].uid));
        // A short list is kept as it is.
        assert_eq!(jobs_to_keep(&jobs[..10]).len(), 10);
    }

    #[test]
    fn stems_never_clash() {
        // YouTube ids stay as they are, so older previews are still found.
        assert_eq!(item("dQw4w9WgXcQ").stem(), "dQw4w9WgXcQ");
        // These used to share "soundcloud_a_b".
        let ids = ["soundcloud:a.b", "soundcloud:a_b", "soundcloud:a/b", "soundcloud_a_b", "spotify:x", "spotify_x"];
        let stems: std::collections::HashSet<String> = ids.iter().map(|i| item(i).stem()).collect();
        assert_eq!(stems.len(), ids.len(), "{stems:?}");
        for s in &stems {
            assert!(!s.contains('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{s}");
        }
        // Same id, same name every time.
        assert_eq!(item("soundcloud:a.b").stem(), item("soundcloud:a.b").stem());
    }

    #[test]
    fn preview_belongs_to_its_song() {
        let a = item("soundcloud:a.b");
        let b = item("soundcloud:a_b");
        let saved = serde_json::json!({ PREVIEW_ID: "soundcloud:a.b", "id": "a.b" });
        assert!(is_entry_for(&saved, &a));
        assert!(!is_entry_for(&saved, &b));
        // A preview from before 1.1.9: yt-dlp's own id, which is the song's id on YouTube.
        let yt = item("dQw4w9WgXcQ");
        assert!(is_entry_for(&serde_json::json!({ "id": "dQw4w9WgXcQ" }), &yt));
        assert!(!is_entry_for(&serde_json::json!({ "id": "something else" }), &yt));
        assert!(!is_entry_for(&serde_json::Value::Null, &yt));
    }

    #[test]
    fn cleanup_only_takes_its_own_files() {
        let dir = std::env::temp_dir().join(format!("segno-cleanup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mine = ["soundcloud_12.m4a", "soundcloud_12.webp", "soundcloud_12.m4a.part", "soundcloud_12.json"];
        let others = ["soundcloud_123.m4a", "soundcloud_12x.webp", "soundcloud_1.m4a"];
        for f in mine.iter().chain(others.iter()) {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        cleanup_tmp(&dir, "soundcloud_12");
        for f in mine {
            assert!(!dir.join(f).exists(), "{f} should be gone");
        }
        for f in others {
            assert!(dir.join(f).exists(), "{f} belongs to another download");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_device_names() {
        for n in ["CON", "con", "PRN", "AUX", "NUL", "COM0", "COM4", "COM9", "LPT1", "LPT9", "COM¹", "lpt³", "CONIN$", "CONOUT$"] {
            assert_eq!(sanitize(n), format!("{n}_"), "{n}");
        }
        assert_eq!(sanitize("CON.txt"), "CON_.txt");
        assert_eq!(sanitize("nul.tar.gz"), "nul_.tar.gz");
        assert_eq!(sanitize("COM4 .mp3"), "COM4_ .mp3");
        // Real names that only look close stay as they are.
        for n in ["CONCERT", "Console", "COM10", "LPT", "Com Truise - Brokendate", "Nulbarich", "Con Funk Shun", "AUX - Song"] {
            assert_eq!(sanitize(n), n, "{n}");
        }
    }
}
