//! Windows-only bits: title bar colors, tray icon, media keys / Windows media overlay,
//! Recycle Bin and Explorer. Other platforms get quiet stand-ins so the app still runs there.

use std::path::Path;
use std::sync::mpsc::Sender;

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub enum TrayAction {
    Show,
    Toggle,
    Next,
    Prev,
    Quit,
}

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub enum MediaAction {
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    Stop,
    SeekTo(f32),
    Raise,
}

#[cfg_attr(not(windows), allow(dead_code))]
pub struct NowPlayingInfo<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: f32,
    pub cover: Option<std::path::PathBuf>,
}

#[cfg(windows)]
static SHOW_EVENT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Only one Segno at a time. A second launch asks the first to show itself and exits.
/// Right after an update the new exe starts while the old one is still closing, so it waits.
pub fn claim_single_instance() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::{CreateEventW, CreateMutexW, OpenEventW, SetEvent, EVENT_MODIFY_STATE};
        let after_update = std::env::args().any(|a| a == crate::update::AFTER_UPDATE_ARG);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let m = wide("Local\\SegnoMusicPlayer.Instance");
        let e = wide("Local\\SegnoMusicPlayer.Show");
        loop {
            let mutex = CreateMutexW(std::ptr::null(), 0, m.as_ptr());
            let already = GetLastError() == ERROR_ALREADY_EXISTS;
            if already && after_update && std::time::Instant::now() < deadline {
                if !mutex.is_null() {
                    CloseHandle(mutex);
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
                continue;
            }
            if already {
                let ev = OpenEventW(EVENT_MODIFY_STATE, 0, e.as_ptr());
                if !ev.is_null() {
                    SetEvent(ev);
                }
                return false;
            }
            let _ = mutex; // keep it for the life of the process
            let ev = CreateEventW(std::ptr::null(), 0, 0, e.as_ptr());
            if !ev.is_null() {
                let _ = SHOW_EVENT.set(ev as usize);
            }
            return true;
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Wait (on a sleeping thread, no polling) for a second launch to ask us to show up.
#[allow(unused_variables)]
pub fn listen_for_second_launch(tx: Sender<TrayAction>, wake: impl Fn() + Send + 'static) {
    #[cfg(windows)]
    if let Some(&ev) = SHOW_EVENT.get() {
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || unsafe {
                use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};
                loop {
                    if WaitForSingleObject(ev as windows_sys::Win32::Foundation::HANDLE, INFINITE) != 0 {
                        return;
                    }
                    let _ = tx.send(TrayAction::Show);
                    wake();
                }
            })
            .ok();
    }
}

/// Open a folder in Explorer.
pub fn open_folder(p: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(p).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    }
}

/// Open a web page in the default browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(url).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// Open Explorer with the file selected.
pub fn reveal(p: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut arg = std::ffi::OsString::from("/select,\"");
        arg.push(p.as_os_str());
        arg.push("\"");
        let _ = std::process::Command::new("explorer").raw_arg(arg).spawn();
    }
    #[cfg(not(windows))]
    {
        if let Some(parent) = p.parent() {
            open_folder(parent);
        }
    }
}

/// Mark a folder hidden so it doesn't clutter the music folder in Explorer.
pub fn hide_path(_p: &Path) {
    #[cfg(windows)]
    unsafe {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
        let w: Vec<u16> = _p.as_os_str().encode_wide().chain(Some(0)).collect();
        SetFileAttributesW(w.as_ptr(), FILE_ATTRIBUTE_HIDDEN);
    }
}

/// Move a file to the Recycle Bin (so a mistaken delete can be undone in Explorer).
pub fn recycle(p: &Path) -> bool {
    #[cfg(windows)]
    unsafe {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
            SHFILEOPSTRUCTW,
        };
        // Double-NUL-terminated list.
        let w: Vec<u16> = p.as_os_str().encode_wide().chain([0, 0]).collect();
        let mut op: SHFILEOPSTRUCTW = std::mem::zeroed();
        op.wFunc = FO_DELETE as u32;
        op.pFrom = w.as_ptr();
        op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;
        SHFileOperationW(&mut op) == 0 && !p.exists()
    }
    #[cfg(not(windows))]
    {
        std::fs::remove_file(p).is_ok()
    }
}

#[cfg(windows)]
pub fn hwnd_of(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let h = frame.window_handle().ok()?;
    match h.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn hwnd_of(_frame: &eframe::Frame) -> Option<isize> {
    None
}

/// Dark title bar that matches the app background (Windows 11 colors it exactly; Windows 10
/// gets the dark variant).
pub fn style_title_bar(_hwnd: isize, _bg: [u8; 3], _text: [u8; 3]) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        let hwnd = _hwnd as windows_sys::Win32::Foundation::HWND;
        let on: i32 = 1;
        // DWMWA_USE_IMMERSIVE_DARK_MODE (20; 19 on early Windows 10 builds)
        for attr in [20u32, 19] {
            if DwmSetWindowAttribute(hwnd, attr, &on as *const _ as _, 4) == 0 {
                break;
            }
        }
        let colorref = |c: [u8; 3]| -> u32 { c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 };
        let caption = colorref(_bg);
        let text = colorref(_text);
        DwmSetWindowAttribute(hwnd, 35, &caption as *const _ as _, 4); // DWMWA_CAPTION_COLOR
        DwmSetWindowAttribute(hwnd, 34, &caption as *const _ as _, 4); // DWMWA_BORDER_COLOR
        DwmSetWindowAttribute(hwnd, 36, &text as *const _ as _, 4); // DWMWA_TEXT_COLOR
        let round: u32 = 2; // DWMWCP_ROUND
        DwmSetWindowAttribute(hwnd, 33, &round as *const _ as _, 4); // DWMWA_WINDOW_CORNER_PREFERENCE
    }
}

/// Bring the window to the front (used when the tray icon is clicked).
pub fn bring_to_front(_hwnd: isize) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
        let hwnd = _hwnd as windows_sys::Win32::Foundation::HWND;
        ShowWindow(hwnd, SW_SHOW);
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
}

// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
pub struct Tray {
    _icon: tray_icon::TrayIcon,
}

#[cfg(not(windows))]
pub struct Tray;

impl Tray {
    #[allow(unused_variables)]
    pub fn set_tooltip(&self, text: &str) {
        #[cfg(windows)]
        {
            // Tooltips are limited to 127 characters.
            let t: String = text.chars().take(120).collect();
            let _ = self._icon.set_tooltip(Some(t));
        }
    }

    #[allow(unused_variables)]
    pub fn new(rgba: Vec<u8>, size: u32, tx: Sender<TrayAction>, wake: impl Fn() + Send + Sync + Clone + 'static) -> Option<Self> {
        #[cfg(windows)]
        {
            use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
            use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
            let menu = Menu::new();
            let show = MenuItem::with_id("show", "Show Segno", true, None);
            let toggle = MenuItem::with_id("toggle", "Play / Pause", true, None);
            let next = MenuItem::with_id("next", "Next song", true, None);
            let prev = MenuItem::with_id("prev", "Previous song", true, None);
            let quit = MenuItem::with_id("quit", "Quit Segno", true, None);
            menu.append_items(&[
                &show,
                &PredefinedMenuItem::separator(),
                &toggle,
                &next,
                &prev,
                &PredefinedMenuItem::separator(),
                &quit,
            ])
            .ok()?;
            let icon = tray_icon::Icon::from_rgba(rgba, size, size).ok()?;
            let tray = TrayIconBuilder::new()
                .with_icon(icon)
                .with_tooltip("Segno")
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()
                .ok()?;
            let tx2 = tx.clone();
            let wake2 = wake.clone();
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                let show = matches!(
                    e,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick { .. }
                );
                if show {
                    let _ = tx2.send(TrayAction::Show);
                    wake2();
                }
            }));
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let a = match e.id.0.as_str() {
                    "show" => TrayAction::Show,
                    "toggle" => TrayAction::Toggle,
                    "next" => TrayAction::Next,
                    "prev" => TrayAction::Prev,
                    "quit" => TrayAction::Quit,
                    _ => return,
                };
                let _ = tx.send(a);
                wake();
            }));
            Some(Tray { _icon: tray })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
pub struct Media {
    controls: souvlaki::MediaControls,
}

#[cfg(not(windows))]
pub struct Media;

impl Media {
    #[allow(unused_variables)]
    pub fn new(hwnd: isize, tx: Sender<MediaAction>, wake: impl Fn() + Send + Sync + 'static) -> Option<Self> {
        #[cfg(windows)]
        {
            use souvlaki::{MediaControlEvent, MediaControls, PlatformConfig, SeekDirection};
            let mut controls = MediaControls::new(PlatformConfig {
                display_name: "Segno",
                dbus_name: "segno",
                hwnd: Some(hwnd as *mut std::ffi::c_void),
            })
            .ok()?;
            controls
                .attach(move |e: MediaControlEvent| {
                    let a = match e {
                        MediaControlEvent::Play => MediaAction::Play,
                        MediaControlEvent::Pause => MediaAction::Pause,
                        MediaControlEvent::Toggle => MediaAction::Toggle,
                        MediaControlEvent::Next => MediaAction::Next,
                        MediaControlEvent::Previous => MediaAction::Prev,
                        MediaControlEvent::Stop => MediaAction::Stop,
                        MediaControlEvent::SetPosition(p) => MediaAction::SeekTo(p.0.as_secs_f32()),
                        MediaControlEvent::Seek(SeekDirection::Forward) => MediaAction::Next,
                        MediaControlEvent::Seek(SeekDirection::Backward) => MediaAction::Prev,
                        MediaControlEvent::Raise => MediaAction::Raise,
                        _ => return,
                    };
                    let _ = tx.send(a);
                    wake();
                })
                .ok()?;
            Some(Media { controls })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    #[allow(unused_variables)]
    pub fn set_track(&mut self, info: Option<NowPlayingInfo>) {
        #[cfg(windows)]
        {
            use souvlaki::MediaMetadata;
            match info {
                Some(i) => {
                    let cover = i.cover.map(|p| format!("file://{}", p.display()));
                    let _ = self.controls.set_metadata(MediaMetadata {
                        title: Some(i.title),
                        artist: Some(i.artist),
                        album: if i.album.is_empty() { None } else { Some(i.album) },
                        cover_url: cover.as_deref(),
                        duration: (i.duration > 0.0).then(|| std::time::Duration::from_secs_f32(i.duration)),
                    });
                }
                None => {
                    let _ = self.controls.set_metadata(MediaMetadata::default());
                }
            }
        }
    }

    #[allow(unused_variables)]
    pub fn set_state(&mut self, playing: Option<bool>, position: f32) {
        #[cfg(windows)]
        {
            use souvlaki::{MediaPlayback, MediaPosition};
            let progress = Some(MediaPosition(std::time::Duration::from_secs_f32(position.max(0.0))));
            let pb = match playing {
                Some(true) => MediaPlayback::Playing { progress },
                Some(false) => MediaPlayback::Paused { progress },
                None => MediaPlayback::Stopped,
            };
            let _ = self.controls.set_playback(pb);
        }
    }
}
