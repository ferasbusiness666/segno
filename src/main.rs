#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod art;
mod icons;
mod import;
mod library;
mod lyrics;
mod net;
mod platform;
mod player;
mod spotify;
mod queue;
mod nowplaying;
mod foryou;
mod changelog;
mod drag;
mod stream;
mod store;
mod theme;
mod update;
mod views;
mod widgets;
mod ytdlp;

use std::sync::OnceLock;

static LOGO: OnceLock<egui::TextureHandle> = OnceLock::new();
const ICON_PNG: &[u8] = include_bytes!("../assets/icon_256.png");
/// The small cut of the logo (no shine, bigger play mark) for the tray, where it's 16–24 px.
const ICON_SMALL_PNG: &[u8] = include_bytes!("../assets/icon_32.png");

/// The app icon as RGBA at `size` px (window and tray icons).
pub fn app_icon(size: u32) -> (Vec<u8>, u32) {
    let img = image::load_from_memory(if size <= 32 { ICON_SMALL_PNG } else { ICON_PNG }).expect("icon");
    let img = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3).to_rgba8();
    (img.into_raw(), size)
}

pub fn paint_logo(p: &egui::Painter, rect: egui::Rect) {
    if let Some(t) = LOGO.get() {
        p.image(
            t.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
}

/// Write panics to %LOCALAPPDATA%\\Segno\\crash.log (there's no console to show them).
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = store::cache_dir().join("crash.log");
        let thread = std::thread::current().name().unwrap_or("main").to_string();
        let msg = format!(
            "[{}] thread '{}': {}\n{}\n\n",
            store::now_secs(),
            thread,
            info,
            std::backtrace::Backtrace::force_capture()
        );
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = f.write_all(msg.as_bytes());
        }
        default(info);
    }));
}

fn main() -> eframe::Result {
    install_crash_log();
    if !platform::claim_single_instance() {
        // Another Segno is running: it has been asked to come to the front.
        return Ok(());
    }
    // An update downloaded last time but not installed yet (closed without "Restart now",
    // or the computer shut down): install it and open the new version instead.
    if update::install_pending() {
        return Ok(());
    }
    let (rgba, size) = app_icon(64);
    let icon = egui::IconData {
        rgba,
        width: size,
        height: size,
    };
    let settings = store::Settings::load();
    let inner = settings.window_size.unwrap_or([1180.0, 760.0]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Segno")
            .with_app_id("segno")
            .with_inner_size(inner)
            .with_min_inner_size([880.0, 580.0])
            .with_maximized(settings.maximized)
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Segno",
        options,
        Box::new(|cc| {
            let img = image::load_from_memory(ICON_PNG).expect("icon").to_rgba8();
            let (w, h) = img.dimensions();
            let tex = cc.egui_ctx.load_texture(
                "logo",
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw()),
                egui::TextureOptions {
                    mipmap_mode: Some(egui::TextureFilter::Linear),
                    ..egui::TextureOptions::LINEAR
                },
            );
            let _ = LOGO.set(tex);
            Ok(Box::new(app::App::new(cc)))
        }),
    )
}
