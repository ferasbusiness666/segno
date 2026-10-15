//! Cover art: shrink once to small JPEGs on disk, then load only what is on screen.

use crate::store::LockExt;
use crate::store::{art_dir, hash_bytes};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use image::{imageops::FilterType, DynamicImage, GenericImageView, RgbImage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ArtSize {
    /// List rows and the player bar.
    S,
    /// Grid cards.
    M,
    /// Headers and the Now Playing view.
    L,
}

impl ArtSize {
    pub fn px(self) -> u32 {
        match self {
            ArtSize::S => 112,
            ArtSize::M => 288,
            ArtSize::L => 640,
        }
    }
    fn suffix(self) -> &'static str {
        match self {
            ArtSize::S => "s",
            ArtSize::M => "m",
            ArtSize::L => "l",
        }
    }
}

pub fn art_file(key: u64, size: ArtSize) -> PathBuf {
    art_dir().join(format!("{key:016x}_{}.jpg", size.suffix()))
}

/// Trim flat dark bars (letterboxing on video thumbnails), then crop the center square.
pub fn square_crop(img: &DynamicImage) -> DynamicImage {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    if w < 8 || h < 8 {
        return img.clone();
    }
    let dark_row = |y: u32| -> bool {
        let mut sum = 0u32;
        let mut max = 0u8;
        for x in (0..w).step_by(4) {
            let p = rgb.get_pixel(x, y);
            let l = ((p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10) as u8;
            sum += l as u32;
            max = max.max(l);
        }
        let n = w.div_ceil(4);
        sum / n < 14 && max < 40
    };
    let dark_col = |x: u32| -> bool {
        let mut sum = 0u32;
        let mut max = 0u8;
        for y in (0..h).step_by(4) {
            let p = rgb.get_pixel(x, y);
            let l = ((p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10) as u8;
            sum += l as u32;
            max = max.max(l);
        }
        let n = h.div_ceil(4);
        sum / n < 14 && max < 40
    };
    let mut top = 0;
    while top < h / 3 && dark_row(top) {
        top += 1;
    }
    let mut bottom = h;
    while bottom > h * 2 / 3 && dark_row(bottom - 1) {
        bottom -= 1;
    }
    let mut left = 0;
    while left < w / 3 && dark_col(left) {
        left += 1;
    }
    let mut right = w;
    while right > w * 2 / 3 && dark_col(right - 1) {
        right -= 1;
    }
    let (cw, ch) = (right - left, bottom - top);
    let side = cw.min(ch);
    let x = left + (cw - side) / 2;
    let y = top + (ch - side) / 2;
    img.crop_imm(x, y, side, side)
}

/// The color the art "feels" like: the strongest saturated hue, weighted by how vivid it is.
pub fn dominant_color(img: &DynamicImage) -> [u8; 3] {
    let small = img.resize_exact(24, 24, FilterType::Triangle).to_rgb8();
    let mut bins = [[0f32; 4]; 12];
    let mut avg = [0f32; 3];
    let n = (small.width() * small.height()) as f32;
    for p in small.pixels() {
        let (r, g, b) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
        avg[0] += r;
        avg[1] += g;
        avg[2] += b;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        if d < 0.08 || max < 0.12 {
            continue;
        }
        let s = d / max;
        let hue = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } / 6.0;
        let i = ((hue * 12.0) as usize).min(11);
        let w = s * s * max;
        bins[i][0] += r * w;
        bins[i][1] += g * w;
        bins[i][2] += b * w;
        bins[i][3] += w;
    }
    let best = bins
        .iter()
        .max_by(|a, b| a[3].partial_cmp(&b[3]).unwrap())
        .copied()
        .unwrap_or([0.0; 4]);
    if best[3] > 1.5 {
        [
            (best[0] / best[3] * 255.0) as u8,
            (best[1] / best[3] * 255.0) as u8,
            (best[2] / best[3] * 255.0) as u8,
        ]
    } else {
        [
            (avg[0] / n * 255.0) as u8,
            (avg[1] / n * 255.0) as u8,
            (avg[2] / n * 255.0) as u8,
        ]
    }
}

fn save_jpeg(img: &RgbImage, path: &PathBuf) {
    let tmp = path.with_extension("tmp");
    if let Ok(f) = std::fs::File::create(&tmp) {
        let mut w = std::io::BufWriter::new(f);
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 86);
        if img.write_with_encoder(enc).is_ok() {
            drop(w);
            let _ = std::fs::rename(&tmp, path);
            return;
        }
    }
    let _ = std::fs::remove_file(&tmp);
}

/// Make the three cached sizes for a cover. Returns (key, color). Cheap if already cached.
pub fn store_cover(bytes: &[u8], known_color: Option<[u8; 3]>) -> Option<(u64, [u8; 3])> {
    let key = hash_bytes(bytes);
    let all_exist = [ArtSize::S, ArtSize::M, ArtSize::L]
        .iter()
        .all(|s| art_file(key, *s).exists());
    if all_exist {
        if let Some(c) = known_color {
            return Some((key, c));
        }
        let img = image::open(art_file(key, ArtSize::S)).ok()?;
        return Some((key, dominant_color(&img)));
    }
    let img = image::load_from_memory(bytes).ok()?;
    let img = square_crop(&img);
    for size in [ArtSize::L, ArtSize::M, ArtSize::S] {
        let px = size.px();
        let out = if img.width() > px {
            img.resize_exact(px, px, FilterType::CatmullRom)
        } else {
            img.clone()
        };
        save_jpeg(&out.to_rgb8(), &art_file(key, size));
    }
    Some((key, dominant_color(&img)))
}

// ---------------------------------------------------------------------------------------------
// Texture cache (UI side)

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ArtKey {
    Cover(u64, ArtSize),
    /// Remote thumbnail (YouTube search results), keyed by url.
    Url(String),
    /// A tiny, heavily blurred copy of a cover, stretched behind the full-screen view.
    Blur(u64),
}

enum Loaded {
    Ok(ArtKey, ColorImage),
    Failed(ArtKey),
}

struct Entry {
    tex: Option<TextureHandle>,
    last_used: u64,
    failed: bool,
}

pub struct ArtCache {
    entries: HashMap<ArtKey, Entry>,
    req_tx: Sender<ArtKey>,
    res_rx: Receiver<Loaded>,
    frame: u64,
}

impl ArtCache {
    pub fn new(ctx: Context) -> Self {
        let (req_tx, req_rx) = channel::<ArtKey>();
        let (res_tx, res_rx) = channel();
        let req_rx = Arc::new(Mutex::new(req_rx));
        // Two decoder threads keep scrolling smooth without hogging the CPU.
        for i in 0..2 {
            let req_rx = req_rx.clone();
            let res_tx = res_tx.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name(format!("art-{i}"))
                .spawn(move || loop {
                    let key = {
                        let rx = req_rx.lk();
                        match rx.recv() {
                            Ok(k) => k,
                            Err(_) => return,
                        }
                    };
                    let img = std::panic::catch_unwind(|| load(&key)).ok().flatten();
                    let msg = match img {
                        Some(i) => Loaded::Ok(key, i),
                        None => Loaded::Failed(key),
                    };
                    if res_tx.send(msg).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                })
                .ok();
        }
        Self {
            entries: HashMap::new(),
            req_tx,
            res_rx,
            frame: 0,
        }
    }

    /// Call once per frame before drawing.
    pub fn begin_frame(&mut self, ctx: &Context) {
        self.frame += 1;
        while let Ok(msg) = self.res_rx.try_recv() {
            match msg {
                Loaded::Ok(key, img) => {
                    let name = format!("{key:?}");
                    let tex = ctx.load_texture(name, img, TextureOptions::LINEAR);
                    if let Some(e) = self.entries.get_mut(&key) {
                        e.tex = Some(tex);
                    }
                }
                Loaded::Failed(key) => {
                    if let Some(e) = self.entries.get_mut(&key) {
                        e.failed = true;
                    }
                }
            }
        }
        // Keep memory flat: drop textures that have not been on screen for a while.
        if self.entries.len() > 260 {
            let cutoff = self.frame.saturating_sub(120);
            self.entries.retain(|_, e| e.last_used >= cutoff || e.tex.is_none() && !e.failed);
            if self.entries.len() > 400 {
                let mut v: Vec<_> = self.entries.iter().map(|(k, e)| (k.clone(), e.last_used)).collect();
                v.sort_by_key(|(_, t)| *t);
                for (k, _) in v.into_iter().take(self.entries.len() - 300) {
                    self.entries.remove(&k);
                }
            }
        }
    }

    pub fn get(&mut self, key: ArtKey) -> Option<egui::TextureId> {
        let frame = self.frame;
        match self.entries.get_mut(&key) {
            Some(e) => {
                e.last_used = frame;
                e.tex.as_ref().map(|t| t.id())
            }
            None => {
                self.entries.insert(
                    key.clone(),
                    Entry {
                        tex: None,
                        last_used: frame,
                        failed: false,
                    },
                );
                let _ = self.req_tx.send(key);
                None
            }
        }
    }

    pub fn cover(&mut self, key: Option<u64>, size: ArtSize) -> Option<egui::TextureId> {
        let k = key?;
        // Use a smaller size that is already loaded while the sharper one arrives.
        let want = self.get(ArtKey::Cover(k, size));
        if want.is_some() {
            return want;
        }
        for smaller in [ArtSize::M, ArtSize::S] {
            if smaller != size {
                if let Some(e) = self.entries.get(&ArtKey::Cover(k, smaller)) {
                    if let Some(t) = &e.tex {
                        return Some(t.id());
                    }
                }
            }
        }
        None
    }

}

fn to_color_image(img: DynamicImage) -> ColorImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw())
}

fn load(key: &ArtKey) -> Option<ColorImage> {
    match key {
        ArtKey::Cover(k, size) => {
            let img = image::open(art_file(*k, *size)).ok()?;
            Some(to_color_image(img))
        }
        ArtKey::Blur(k) => {
            let img = image::open(art_file(*k, ArtSize::S)).ok()?;
            let img = img.resize_exact(40, 40, FilterType::Triangle).blur(3.0);
            Some(to_color_image(img))
        }
        ArtKey::Url(url) => {
            let bytes = crate::net::get_bytes(url, 4 * 1024 * 1024).ok()?;
            let img = image::load_from_memory(&bytes).ok()?;
            // 16:9 thumbnails: keep the shape, just make them small.
            let (w, h) = img.dimensions();
            let target_w = 240u32.min(w);
            let img = if w > target_w {
                img.resize(target_w, (h * target_w / w).max(1), FilterType::Triangle)
            } else {
                img
            };
            Some(to_color_image(img))
        }
    }
}
