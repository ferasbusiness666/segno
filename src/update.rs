//! Updates from GitHub Releases.
//!
//! Segno asks GitHub for the newest release (every few hours when "Update automatically" is on, or
//! when you press "Check for updates"). If it's newer, the new Segno.exe is downloaded into the
//! cache in the background and checked: size, and SHA-256 when GitHub lists one. It replaces the
//! running exe when Segno quits, or right away with "Restart now".
//!
//! Windows won't let a running exe be overwritten, but it can be renamed. So the running exe is
//! moved aside to "Segno.exe.old", the new one is copied into its place, and the old one is
//! deleted the next time Segno starts.

use crate::store::LockExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The GitHub repository that publishes releases (owner/name).
pub const REPO: &str = "ferasbusiness666/segno";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Installed from the Microsoft Store, which handles updates itself.
pub const STORE: bool = cfg!(feature = "store");
/// Passed to the new exe after "Restart now" so it waits for this one to close.
pub const AFTER_UPDATE_ARG: &str = "--after-update";
/// How often to look for a new version. A check is one small request to GitHub (a few KB).
const CHECK_EVERY: u64 = 3 * 60 * 60;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Downloading { version: String, pct: f32 },
    /// Downloaded and checked. It takes over when Segno quits.
    Ready { version: String },
    /// A newer version is out, but Segno can't replace itself where it's installed.
    Available { version: String },
    Failed(String),
}

struct Release {
    version: String,
    page: String,
    asset_url: String,
    size: u64,
    sha256: Option<String>,
}

pub struct Updater {
    state: Arc<Mutex<State>>,
    /// Release page of the newest version found ("What's new").
    page: Arc<Mutex<Option<String>>>,
    staged: Arc<Mutex<Option<PathBuf>>>,
    busy: Arc<AtomicBool>,
    pub auto: Arc<AtomicBool>,
    /// Unix time of the last automatic check (saved in settings).
    pub last_checked: Arc<AtomicU64>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Updater {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>, auto: bool, last_checked: u64) -> Self {
        let u = Self {
            state: Arc::new(Mutex::new(State::Idle)),
            page: Arc::new(Mutex::new(None)),
            staged: Arc::new(Mutex::new(None)),
            busy: Arc::new(AtomicBool::new(false)),
            auto: Arc::new(AtomicBool::new(auto)),
            last_checked: Arc::new(AtomicU64::new(last_checked)),
            notify,
        };
        if !STORE {
            u.start_daily();
            std::thread::spawn(cleanup);
        }
        u
    }

    pub fn state(&self) -> State {
        self.state.lk().clone()
    }

    pub fn page(&self) -> String {
        self.page
            .lk()
            .clone()
            .unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest"))
    }

    /// Check now (the Settings button). Downloads the update if there is one.
    pub fn check_now(&self) {
        if STORE {
            return;
        }
        self.spawn_check(true);
    }

    /// A sleeping thread that checks every few hours while "Update automatically" is on.
    fn start_daily(&self) {
        let me = self.handle();
        std::thread::Builder::new()
            .name("updates".into())
            .spawn(move || loop {
                let last = me.last_checked.load(Ordering::Relaxed);
                let now = crate::store::now_secs();
                // Never right at startup: give the app a moment to settle.
                let wait = (last + CHECK_EVERY).saturating_sub(now).max(20);
                std::thread::sleep(Duration::from_secs(wait));
                if !me.auto.load(Ordering::Relaxed) {
                    // Look again in an hour in case it gets turned back on.
                    me.last_checked.store(crate::store::now_secs().saturating_sub(CHECK_EVERY - 3600), Ordering::Relaxed);
                    continue;
                }
                if matches!(*me.state.lk(), State::Ready { .. }) {
                    me.last_checked.store(crate::store::now_secs(), Ordering::Relaxed);
                    continue;
                }
                me.run_check(false);
                me.last_checked.store(crate::store::now_secs(), Ordering::Relaxed);
            })
            .ok();
    }

    fn spawn_check(&self, manual: bool) {
        let me = self.handle();
        std::thread::spawn(move || me.run_check(manual));
    }

    fn handle(&self) -> Handle {
        Handle {
            state: self.state.clone(),
            page: self.page.clone(),
            staged: self.staged.clone(),
            busy: self.busy.clone(),
            auto: self.auto.clone(),
            last_checked: self.last_checked.clone(),
            notify: self.notify.clone(),
        }
    }

    /// Put the downloaded version in place. Called when Segno quits.
    pub fn apply_on_exit(&self) {
        if !matches!(*self.state.lk(), State::Ready { .. }) {
            return;
        }
        if let Some(new) = self.staged.lk().take() {
            let _ = swap_in(&new);
        }
        *self.state.lk() = State::Idle;
    }

    /// Put the new version in place and start it. The caller then closes this window.
    pub fn restart(&self) -> Result<(), String> {
        let new = self.staged.lk().clone().ok_or("Nothing to install yet.")?;
        let exe = swap_in(&new)?;
        *self.staged.lk() = None;
        *self.state.lk() = State::Idle;
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg(AFTER_UPDATE_ARG);
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        cmd.spawn().map_err(|_| "Couldn't start the new version. Open Segno again to finish updating.".to_string())?;
        Ok(())
    }
}

#[derive(Clone)]
struct Handle {
    state: Arc<Mutex<State>>,
    page: Arc<Mutex<Option<String>>>,
    staged: Arc<Mutex<Option<PathBuf>>>,
    busy: Arc<AtomicBool>,
    auto: Arc<AtomicBool>,
    last_checked: Arc<AtomicU64>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Handle {
    fn set(&self, s: State) {
        *self.state.lk() = s;
        (self.notify)();
    }

    fn run_check(&self, manual: bool) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.check(manual)));
        if res.is_err() && manual {
            self.set(State::Failed("Something went wrong. Try again.".into()));
        }
        self.busy.store(false, Ordering::SeqCst);
    }

    fn check(&self, manual: bool) {
        let before = self.state.lk().clone();
        if matches!(before, State::Ready { .. }) {
            // Already downloaded; nothing more to do until Segno restarts.
            (self.notify)();
            return;
        }
        if manual {
            self.set(State::Checking);
        }
        let rel = match latest() {
            Ok(Some(r)) => r,
            Ok(None) => {
                self.set(State::UpToDate);
                return;
            }
            Err(msg) => {
                // Automatic checks fail quietly; you only hear about it if you asked.
                self.set(if manual { State::Failed(msg) } else { before });
                return;
            }
        };
        *self.page.lk() = Some(rel.page.clone());
        if !is_newer(&rel.version, VERSION) {
            self.set(State::UpToDate);
            return;
        }
        if !can_replace_self() {
            self.set(State::Available { version: rel.version });
            return;
        }
        match self.download(&rel) {
            Ok(path) => {
                *self.staged.lk() = Some(path);
                self.set(State::Ready { version: rel.version });
            }
            Err(msg) => self.set(if manual { State::Failed(msg) } else { State::Available { version: rel.version } }),
        }
    }

    fn download(&self, rel: &Release) -> Result<PathBuf, String> {
        let dir = update_dir();
        let dest = dir.join(format!("Segno-{}.exe", rel.version));
        // Downloaded before (Segno closed before installing it)? Reuse it if it still checks out.
        if dest.is_file() && verify(&dest, rel).is_ok() {
            return Ok(dest);
        }
        self.set(State::Downloading { version: rel.version.clone(), pct: 0.0 });
        let total = rel.size.max(1) as f32;
        let mut last_pct = -1.0f32;
        crate::net::download(&rel.asset_url, &dest, |done, len| {
            let pct = (done as f32 / len.map(|l| l as f32).unwrap_or(total)).min(1.0);
            if pct - last_pct >= 0.02 {
                last_pct = pct;
                self.set(State::Downloading { version: rel.version.clone(), pct });
            }
        })
        .map_err(|_| "The update didn't finish downloading. Try again later.".to_string())?;
        if let Err(e) = verify(&dest, rel) {
            let _ = std::fs::remove_file(&dest);
            return Err(e);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
        }
        Ok(dest)
    }
}

fn update_dir() -> PathBuf {
    let d = crate::store::cache_dir().join("update");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Ask GitHub for the newest release. `None` means there are no releases yet.
fn latest() -> Result<Option<Release>, String> {
    let url = std::env::var("SEGNO_UPDATE_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    let body = match crate::net::get_bytes(&url, 4 << 20) {
        Ok(b) => b,
        Err(e) if e.contains("404") => return Ok(None),
        Err(_) => return Err("Couldn't reach GitHub. Check your internet connection and try again.".into()),
    };
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|_| "GitHub sent something unexpected. Try again later.".to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or("").trim();
    if tag.is_empty() || v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let page = v["html_url"].as_str().unwrap_or("").to_string();
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let name = |a: &serde_json::Value| a["name"].as_str().unwrap_or("").to_ascii_lowercase();
    // The portable exe: "Segno.exe" if it's there, otherwise any .exe that isn't an installer.
    let asset = assets
        .iter()
        .find(|a| name(a) == "segno.exe")
        .or_else(|| assets.iter().find(|a| {
            let n = name(a);
            n.ends_with(".exe") && !n.contains("setup") && !n.contains("install")
        }));
    let Some(asset) = asset else {
        // A release without the exe yet (still uploading): treat as nothing new for now.
        return Ok(None);
    };
    let sha256 = asset["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(|s| s.to_ascii_lowercase());
    Ok(Some(Release {
        version,
        page,
        asset_url: asset["browser_download_url"].as_str().unwrap_or("").to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
        sha256,
    }))
}

fn verify(path: &Path, rel: &Release) -> Result<(), String> {
    let bad = || "The update didn't download correctly. Try again later.".to_string();
    let data = std::fs::read(path).map_err(|_| bad())?;
    if rel.size > 0 && data.len() as u64 != rel.size {
        return Err(bad());
    }
    if let Some(want) = &rel.sha256 {
        let got = Sha256::digest(&data);
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        if &hex != want {
            return Err(bad());
        }
    }
    #[cfg(windows)]
    if !data.starts_with(b"MZ") {
        return Err(bad());
    }
    Ok(())
}

/// Compare versions like "1.10.0" and "1.9.2" (a leading "v" and "-beta" style suffixes are ignored).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.trim().parse().unwrap_or(0))
            .collect()
    }
    let (mut a, mut b) = (parts(candidate), parts(current));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

/// Can Segno write next to its own exe? (Not the case under Program Files, for example.)
fn can_replace_self() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(dir) = exe.parent() else { return false };
    let probe = dir.join(".segno-update-check");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".old");
    exe.with_file_name(name)
}

/// Move the running exe aside and copy the new one into its place. Returns the exe path.
fn swap_in(new: &Path) -> Result<PathBuf, String> {
    let fail = || "Couldn't install the update. Try again, or download it from the website.".to_string();
    let exe = std::env::current_exe().map_err(|_| fail())?;
    if !new.is_file() {
        return Err(fail());
    }
    let old = old_path(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|_| fail())?;
    if std::fs::copy(new, &exe).is_err() {
        // Put the working version back.
        let _ = std::fs::remove_file(&exe);
        let _ = std::fs::rename(&old, &exe);
        return Err(fail());
    }
    let _ = std::fs::remove_file(new);
    Ok(exe)
}

/// At startup: delete the exe left over from the last update, and downloads that are no
/// longer newer than this version.
fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let old = old_path(&exe);
        // The previous Segno may still be closing for a moment after "Restart now".
        for _ in 0..30 {
            if !old.exists() || std::fs::remove_file(&old).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    if let Ok(rd) = std::fs::read_dir(update_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let ver = name.trim_start_matches("Segno-").trim_end_matches(".exe").trim_end_matches(".part");
            if !is_newer(ver, VERSION) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("v1.10.0", "1.9.9"));
        assert!(is_newer("2", "1.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("v1.0", "1.0.0"));
        assert!(!is_newer("1.0.0-beta", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
    }
}
