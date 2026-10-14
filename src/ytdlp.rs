//! YouTube search and downloads through yt-dlp.
//!
//! - yt-dlp (and a JavaScript runtime it needs for YouTube) is fetched on first use and kept in
//!   the app's local folder. If Node, Deno or Bun is already installed, that is used instead.
//! - Every process runs hidden (no console windows) with UTF-8 output.
//! - Downloads go to a hidden temp folder inside the music folder, get proper tags and cover art,
//!   then are moved in as "Artist - Title.m4a" in one rename, so the library never sees half files.

use crate::store::LockExt;
use crate::store::{bin_dir, now_secs};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct YtItem {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration: f32,
}

impl YtItem {
    pub fn thumb_url(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", self.id)
    }
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
}

#[derive(Clone, Debug)]
pub struct PlaylistInfo {
    pub title: String,
    pub channel: String,
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

#[derive(Clone, Debug, PartialEq)]
pub enum JobStatus {
    Waiting,
    Downloading { pct: f32, speed: String },
    Saving,
    Done(PathBuf),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Job {
    pub uid: u64,
    pub item: YtItem,
    pub status: JobStatus,
    pub playlist: Option<u64>,
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
    pub jobs: Vec<Job>,
    pub version: Option<String>,
    /// Songs fetched to play before downloading, by video id.
    pub previews: std::collections::HashMap<String, PreviewState>,
}

pub enum YtEvent {
    Downloaded { path: PathBuf, playlist: Option<u64> },
    PreviewReady { item: YtItem, path: PathBuf, art: Option<(u64, [u8; 3])> },
    PreviewFailed { msg: String },
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
    preview_child: Arc<Mutex<Option<(u64, Child)>>>,
    preview_latest: Arc<Mutex<String>>,
    ev_tx: Sender<YtEvent>,
}

static JOB_UID: AtomicU64 = AtomicU64::new(1);
static UPDATED_THIS_SESSION: AtomicBool = AtomicBool::new(false);

impl Yt {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            setup: Setup::Unknown,
            search: Search::Idle,
            jobs: Vec::new(),
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
            preview_child: Arc::new(Mutex::new(None)),
            preview_latest: Arc::new(Mutex::new(String::new())),
            ev_tx: ev_tx2,
        };
        let ctx = yt.worker_ctx();
        std::thread::Builder::new()
            .name("downloads".into())
            .spawn(move || download_worker(ctx, wake_rx, ev_tx))
            .expect("download thread");
        yt
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

    /// Play a song before downloading it: fetch it into the preview cache, then tell the app.
    pub fn preview(&self, item: YtItem) {
        clear_fail();
        *self.preview_latest.lk() = item.id.clone();
        // Only one preview at a time: stop the one still loading.
        if let Some((_, child)) = self.preview_child.lk().as_mut() {
            let _ = child.kill();
        }
        let ready = match self.shared.lk().previews.get(&item.id) {
            Some(PreviewState::Ready(p)) if p.is_file() => Some(p.clone()),
            _ => None,
        };
        if let Some(path) = ready {
            let art = preview_cover(&item.id);
            let _ = self.ev_tx.send(YtEvent::PreviewReady { item, path, art });
            (self.notify)();
            return;
        }
        self.shared.lk().previews.insert(item.id.clone(), PreviewState::Loading(0.0));
        (self.notify)();
        let ctx = self.worker_ctx();
        let slot = self.preview_child.clone();
        let latest = self.preview_latest.clone();
        let ev_tx = self.ev_tx.clone();
        std::thread::spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_preview(&ctx, &item, &slot)))
                .unwrap_or_else(|_| Err("Something went wrong. Try again.".into()));
            let is_latest = *latest.lk() == item.id;
            match res {
                Ok(path) => {
                    let art = preview_cover(&item.id);
                    ctx.shared.lk().previews.insert(item.id.clone(), PreviewState::Ready(path.clone()));
                    if is_latest {
                        let _ = ev_tx.send(YtEvent::PreviewReady { item, path, art });
                    }
                }
                Err(msg) => {
                    ctx.shared.lk().previews.insert(item.id.clone(), PreviewState::Failed(msg.clone()));
                    if is_latest && msg != "Cancelled" {
                        let _ = ev_tx.send(YtEvent::PreviewFailed { msg });
                    }
                }
            }
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
        (self.notify)();
    }

    pub fn clear_finished(&self) {
        self.shared
            .lock()
            .unwrap()
            .jobs
            .retain(|j| !matches!(j.status, JobStatus::Done(_) | JobStatus::Failed(_)));
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
    let low = stderr.to_lowercase();
    if low.contains("not a bot") || low.contains("http error 429") {
        "YouTube is limiting downloads right now. Try again in a few minutes.".into()
    } else if low.contains("private video") || low.contains("video unavailable") || low.contains("has been removed") {
        "This video isn't available.".into()
    } else if low.contains("confirm your age") || low.contains("age-restricted") {
        "This video is age-restricted, so YouTube won't allow the download.".into()
    } else if low.contains("requested format is not available") {
        "YouTube didn't offer an M4A version of this one.".into()
    } else if low.contains("getaddrinfo")
        || low.contains("unable to download webpage")
        || low.contains("connection")
        || low.contains("timed out")
    {
        "Couldn't reach YouTube. Check your internet connection.".into()
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

pub fn looks_like_link(s: &str) -> bool {
    let s = s.trim().to_lowercase();
    (s.starts_with("http://") || s.starts_with("https://") || s.starts_with("www.") || s.starts_with("youtu"))
        && (s.contains("youtube.com/") || s.contains("youtu.be/"))
}

fn run_search(ctx: &WorkerCtx, query: &str) -> Result<(Vec<YtItem>, Option<PlaylistInfo>), String> {
    let tools = ensure_tools(ctx)?;
    let q = query.trim();
    let mut attempt = 0;
    loop {
        let mut cmd = base_cmd(&tools);
        cmd.args(["--flat-playlist", "-J", "--no-warnings"]);
        if looks_like_link(q) {
            let mut url = q.to_string();
            if !url.starts_with("http") {
                url = format!("https://{url}");
            }
            // A song link that happens to sit in a playlist: take just the song.
            if url.contains("watch?") && url.contains("v=") {
                cmd.arg("--no-playlist");
            }
            cmd.arg(url);
        } else {
            cmd.arg(format!("ytsearch20:{q}"));
        }
        let out = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("The downloader didn't start ({e})."))?;
        if out.status.success() {
            let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| "YouTube sent something unexpected.".to_string())?;
            return Ok(parse_results(&v));
        }
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        if attempt == 0 && should_update_and_retry(&err) && self_update(ctx, &tools) {
            attempt += 1;
            continue;
        }
        return Err(friendly_error(&err));
    }
}

fn item_from(v: &Value, fallback_channel: &str) -> Option<YtItem> {
    let id = v.get("id")?.as_str()?.to_string();
    if id.len() != 11 {
        return None; // channels, playlists, shorts shelves
    }
    let title = v.get("title").and_then(Value::as_str).unwrap_or("").to_string();
    if title.is_empty() || title == "[Private video]" || title == "[Deleted video]" {
        return None;
    }
    let channel = ["channel", "uploader", "creator"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .unwrap_or(fallback_channel)
        .to_string();
    let duration = v.get("duration").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let live = v.get("live_status").and_then(Value::as_str).map(|s| s == "is_live").unwrap_or(false);
    if live {
        return None;
    }
    Some(YtItem {
        id,
        title,
        channel,
        duration,
    })
}

fn parse_results(v: &Value) -> (Vec<YtItem>, Option<PlaylistInfo>) {
    if let Some(entries) = v.get("entries").and_then(Value::as_array) {
        let channel = v
            .get("channel")
            .or_else(|| v.get("uploader"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let items: Vec<YtItem> = entries.iter().filter_map(|e| item_from(e, &channel)).collect();
        let is_search = v
            .get("extractor_key")
            .and_then(Value::as_str)
            .map(|s| s.contains("Search"))
            .unwrap_or(false);
        let playlist = (!is_search).then(|| PlaylistInfo {
            title: v.get("title").and_then(Value::as_str).unwrap_or("YouTube playlist").to_string(),
            channel,
        });
        (items, playlist)
    } else {
        (item_from(v, "").into_iter().collect(), None)
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
                cleanup_tmp(&tmp, &job.item.id);
                return Err(friendly_error(&stderr));
            }
        }
    };
    set_status(ctx, job.uid, JobStatus::Saving);
    (ctx.notify)();

    let audio = meta
        .get("filepath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            let p = tmp.join(format!("{}.m4a", job.item.id));
            p.is_file().then_some(p)
        })
        .ok_or("The download finished but the file is missing.")?;

    let tags = clean_meta(&meta, &job.item);
    let thumb = ["jpg", "webp", "png", "jpeg"]
        .iter()
        .map(|e| tmp.join(format!("{}.{e}", job.item.id)))
        .find(|p| p.is_file());
    let cover = thumb.as_ref().and_then(|p| cover_jpeg(p));
    write_tags(&audio, &tags, &job.item.id, cover);

    let name = sanitize(&if tags.artist.is_empty() {
        tags.title.clone()
    } else {
        format!("{} - {}", tags.artist, tags.title)
    });
    let mut dest = music.join(format!("{name}.m4a"));
    let mut n = 2;
    while dest.exists() {
        dest = music.join(format!("{name} ({n}).m4a"));
        n += 1;
    }
    std::fs::rename(&audio, &dest)
        .or_else(|_| std::fs::copy(&audio, &dest).map(|_| ()).and_then(|_| std::fs::remove_file(&audio)))
        .map_err(|_| "Couldn't move the song into your music folder.".to_string())?;
    if let Some(t) = thumb {
        let _ = std::fs::remove_file(t);
    }
    cleanup_tmp(&tmp, &job.item.id);
    Ok(dest)
}

fn run_preview(ctx: &WorkerCtx, item: &YtItem, slot: &Mutex<Option<(u64, Child)>>) -> Result<PathBuf, String> {
    let dir = preview_dir();
    let audio = dir.join(format!("{}.m4a", item.id));
    let meta_path = dir.join(format!("{}.json", item.id));
    if audio.is_file() && meta_path.is_file() {
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
                return Err(friendly_error(&stderr));
            }
        }
    };
    let path = meta
        .get("filepath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .unwrap_or(audio);
    if !path.is_file() {
        return Err("The song didn't arrive. Try again.".into());
    }
    let _ = std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap_or_default());
    Ok(path)
}

/// Cover art for a previewed song, from the thumbnail yt-dlp saved next to it.
fn preview_cover(id: &str) -> Option<(u64, [u8; 3])> {
    let dir = preview_dir();
    let thumb = ["jpg", "webp", "png", "jpeg"]
        .iter()
        .map(|e| dir.join(format!("{id}.{e}")))
        .find(|p| p.is_file())?;
    let bytes = std::fs::read(thumb).ok()?;
    crate::art::store_cover(&bytes, None)
}

/// If the song was already played from YouTube, copy that file instead of downloading again.
fn take_from_preview(item: &YtItem, tmp: &Path) -> Option<Value> {
    let dir = preview_dir();
    let audio = dir.join(format!("{}.m4a", item.id));
    let meta_path = dir.join(format!("{}.json", item.id));
    if !audio.is_file() || !meta_path.is_file() {
        return None;
    }
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&meta_path).ok()?).ok()?;
    let dest = tmp.join(format!("{}.m4a", item.id));
    std::fs::copy(&audio, &dest).ok()?;
    for ext in ["jpg", "webp", "png", "jpeg"] {
        let t = dir.join(format!("{}.{ext}", item.id));
        if t.is_file() {
            let _ = std::fs::copy(&t, tmp.join(format!("{}.{ext}", item.id)));
            break;
        }
    }
    if let Value::Object(m) = &mut meta {
        m.insert("filepath".into(), Value::String(dest.to_string_lossy().to_string()));
    }
    Some(meta)
}

fn cleanup_tmp(tmp: &Path, id: &str) {
    if let Ok(rd) = std::fs::read_dir(tmp) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(id) {
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
    let mut cmd = base_cmd(tools);
    let out_tpl = tmp.join("%(id)s.%(ext)s");
    cmd.args([
        "-f",
        "bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]",
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
    match (status, meta) {
        (Some(s), Some(m)) if s.success() => Ok(m),
        (Some(s), None) if s.success() => Ok(Value::Null),
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
    tag.set_comment(format!("youtube:{yt_id}"));
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
    let upper = s.to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "LPT1", "LPT2"];
    if s.is_empty() || reserved.contains(&upper.as_str()) {
        s = format!("Track {}", now_secs());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
