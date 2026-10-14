//! Design tokens. Every color, size and radius in the app comes from here.
//!
//! Design language: near-black canvas with soft rounded panels; the music's own artwork color
//! washes the top of each page; white type in three weights; one rose accent kept for the
//! primary action, the current song and progress.

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId};
use std::sync::Arc;

// Colors ---------------------------------------------------------------------------------------
pub const BG: Color32 = Color32::from_rgb(0x08, 0x08, 0x0A);
pub const PANEL: Color32 = Color32::from_rgb(0x12, 0x12, 0x15);
pub const RAISED: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x21);
pub const RAISED_2: Color32 = Color32::from_rgb(0x27, 0x27, 0x2E);
pub const OVERLAY: Color32 = Color32::from_rgb(0x22, 0x22, 0x28);
pub const HAIRLINE: Color32 = Color32::from_rgb(0x26, 0x26, 0x2C);
pub const TEXT: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF6);
pub const TEXT_2: Color32 = Color32::from_rgb(0xA4, 0xA4, 0xAE);
pub const TEXT_3: Color32 = Color32::from_rgb(0x6C, 0x6C, 0x76);
pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x4D, 0x6D);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x86);
pub const ACCENT_LO: Color32 = Color32::from_rgb(0xE0, 0x3E, 0x5C);
pub const ON_ACCENT: Color32 = Color32::WHITE;
pub const WARN: Color32 = Color32::from_rgb(0xFF, 0x9F, 0x5A);

// Spacing (4-pt grid) -------------------------------------------------------------------------
pub const S1: f32 = 4.0;
pub const S2: f32 = 8.0;
pub const S3: f32 = 12.0;
pub const S4: f32 = 16.0;
pub const S5: f32 = 20.0;
pub const S6: f32 = 24.0;
pub const S8: f32 = 32.0;
pub const S12: f32 = 48.0;

// Radii -------------------------------------------------------------------------------------------
pub const R_SM: f32 = 4.0;
pub const R_MD: f32 = 8.0;
pub const R_LG: f32 = 12.0;

// Layout ------------------------------------------------------------------------------------------
pub const GUTTER: f32 = 8.0;
pub const SIDEBAR_MIN: f32 = 200.0;
pub const SIDEBAR_MAX: f32 = 380.0;
/// Below this while dragging, the sidebar folds into the icon rail.
pub const SIDEBAR_FOLD: f32 = 150.0;
pub const RAIL_W: f32 = 72.0;
pub const QUEUE_MIN: f32 = 260.0;
pub const QUEUE_MAX: f32 = 480.0;
/// The page in the middle keeps at least this much room before the side panels give way.
pub const MAIN_MIN: f32 = 420.0;
pub const BAR_H: f32 = 80.0;
pub const ROW_H: f32 = 56.0;
pub const TOPBAR_H: f32 = 56.0;
pub const PAD_X: f32 = 24.0;

// Motion (seconds) --------------------------------------------------------------------------------
pub const T_FAST: f32 = 0.12;
pub const T: f32 = 0.18;
pub const T_SLOW: f32 = 0.28;

// Type ------------------------------------------------------------------------------------------
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}
pub fn icon_family() -> FontFamily {
    FontFamily::Name("icons".into())
}

pub fn body() -> FontId {
    FontId::new(14.0, FontFamily::Proportional)
}
pub fn body_strong() -> FontId {
    FontId::new(14.0, semibold())
}
pub fn small() -> FontId {
    FontId::new(12.0, FontFamily::Proportional)
}
pub fn small_strong() -> FontId {
    FontId::new(12.0, semibold())
}
pub fn h2() -> FontId {
    FontId::new(18.0, bold())
}
pub fn h1() -> FontId {
    FontId::new(28.0, bold())
}
pub fn display() -> FontId {
    FontId::new(40.0, bold())
}

/// Load fonts: Inter (bundled) for Latin/Greek/Cyrillic, the system UI font as fallback for
/// Arabic, Hebrew and others, and the icon font.
pub fn install_fonts(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let mut add = |name: &str, data: FontData| {
        defs.font_data.insert(name.to_string(), Arc::new(data));
    };
    add("inter", FontData::from_static(include_bytes!("../assets/Inter-Regular.otf")));
    add("inter-sb", FontData::from_static(include_bytes!("../assets/Inter-SemiBold.otf")));
    add("inter-bold", FontData::from_static(include_bytes!("../assets/InterDisplay-Bold.otf")));
    add("icons", FontData::from_static(include_bytes!("../assets/icons.ttf")));

    // System fallbacks for scripts Inter doesn't cover (e.g. Arabic song titles).
    let mut fallback_regular = Vec::new();
    let mut fallback_bold = Vec::new();
    for (name, file, bold_face) in system_fallbacks() {
        if let Ok(bytes) = std::fs::read(&file) {
            add(&name, FontData::from_owned(bytes));
            if bold_face {
                fallback_bold.push(name);
            } else {
                fallback_regular.push(name);
            }
        }
    }
    if fallback_bold.is_empty() {
        fallback_bold = fallback_regular.clone();
    }

    let egui_defaults: Vec<String> = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let chain = |first: &str, fb: &Vec<String>| -> Vec<String> {
        let mut v = vec![first.to_string()];
        v.extend(fb.iter().cloned());
        v.extend(egui_defaults.iter().cloned());
        v
    };
    defs.families.insert(FontFamily::Proportional, chain("inter", &fallback_regular));
    defs.families.insert(semibold(), chain("inter-sb", &fallback_bold));
    defs.families.insert(bold(), chain("inter-bold", &fallback_bold));
    defs.families.insert(icon_family(), vec!["icons".into()]);
    ctx.set_fonts(defs);
}

fn system_fallbacks() -> Vec<(String, std::path::PathBuf, bool)> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        let dir = std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "C:\\Windows".into())
            .join("Fonts");
        v.push(("segoe".to_string(), dir.join("segoeui.ttf"), false));
        v.push(("segoe-b".to_string(), dir.join("seguisb.ttf"), true));
        v.push(("segoe-sym".to_string(), dir.join("seguisym.ttf"), false));
    }
    #[cfg(not(windows))]
    {
        for (n, p) in [
            ("noto-ar", "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf"),
            ("noto-he", "/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf"),
            ("dejavu", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
        ] {
            v.push((n.to_string(), p.into(), false));
        }
        v.push((
            "noto-ar-b".to_string(),
            "/usr/share/fonts/truetype/noto/NotoSansArabic-Bold.ttf".into(),
            true,
        ));
    }
    v
}

/// East-Asian fonts are large, so they load only when a title actually needs them.
pub fn add_cjk_fallback(ctx: &egui::Context) {
    #[cfg(windows)]
    {
        let dir = std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "C:\\Windows".into())
            .join("Fonts");
        let mut extra = Vec::new();
        for (n, f) in [("yahei", "msyh.ttc"), ("yugothic", "YuGothM.ttc"), ("malgun", "malgun.ttf")] {
            if let Ok(b) = std::fs::read(dir.join(f)) {
                extra.push((n, b));
            }
        }
        if extra.is_empty() {
            return;
        }
        ctx.add_font(egui::epaint::text::FontInsert::new(
            extra[0].0,
            FontData::from_owned(extra[0].1.clone()),
            vec![
                egui::epaint::text::InsertFontFamily {
                    family: FontFamily::Proportional,
                    priority: egui::epaint::text::FontPriority::Lowest,
                },
                egui::epaint::text::InsertFontFamily {
                    family: semibold(),
                    priority: egui::epaint::text::FontPriority::Lowest,
                },
                egui::epaint::text::InsertFontFamily {
                    family: bold(),
                    priority: egui::epaint::text::FontPriority::Lowest,
                },
            ],
        ));
        for (n, b) in extra.into_iter().skip(1) {
            ctx.add_font(egui::epaint::text::FontInsert::new(
                n,
                FontData::from_owned(b),
                vec![
                    egui::epaint::text::InsertFontFamily {
                        family: FontFamily::Proportional,
                        priority: egui::epaint::text::FontPriority::Lowest,
                    },
                    egui::epaint::text::InsertFontFamily {
                        family: semibold(),
                        priority: egui::epaint::text::FontPriority::Lowest,
                    },
                    egui::epaint::text::InsertFontFamily {
                        family: bold(),
                        priority: egui::epaint::text::FontPriority::Lowest,
                    },
                ],
            ));
        }
    }
    #[cfg(not(windows))]
    let _ = ctx;
}

pub fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
    })
}

/// egui style: dark, quiet, rounded, with thin floating scrollbars.
pub fn install_style(ctx: &egui::Context) {
    use egui::{style::*, CornerRadius, Stroke, Vec2};
    ctx.all_styles_mut(|s| {
        s.visuals = Visuals::dark();
        let v = &mut s.visuals;
        v.panel_fill = PANEL;
        v.window_fill = OVERLAY;
        v.extreme_bg_color = RAISED;
        v.faint_bg_color = RAISED;
        v.override_text_color = None;
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.linear_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.window_corner_radius = CornerRadius::same(R_LG as u8);
        v.menu_corner_radius = CornerRadius::same(R_MD as u8);
        v.window_stroke = Stroke::new(1.0, HAIRLINE);
        v.window_shadow = egui::epaint::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        v.popup_shadow = egui::epaint::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(130),
        };
        v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
        let w = &mut v.widgets;
        let r = CornerRadius::same(R_SM as u8 + 2);
        w.noninteractive.bg_fill = OVERLAY;
        w.noninteractive.weak_bg_fill = OVERLAY;
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_2);
        w.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
        w.inactive.bg_fill = RAISED;
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = RAISED_2;
        w.hovered.weak_bg_fill = RAISED_2;
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_stroke = Stroke::NONE;
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = RAISED_2;
        w.active.weak_bg_fill = RAISED_2;
        w.active.fg_stroke = Stroke::new(1.0, TEXT);
        w.active.bg_stroke = Stroke::NONE;
        w.active.corner_radius = r;
        w.active.expansion = 0.0;
        w.open = w.hovered;
        s.spacing.item_spacing = Vec2::new(S2, S1);
        s.spacing.button_padding = Vec2::new(S3, 6.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.interact_size.y = 30.0;
        s.spacing.scroll = ScrollStyle {
            bar_width: 8.0,
            floating_width: 3.0,
            floating_allocated_width: 0.0,
            bar_inner_margin: 4.0,
            bar_outer_margin: 2.0,
            ..ScrollStyle::floating()
        };
        s.interaction.tooltip_delay = 0.6;
        s.animation_time = T;
        s.text_styles.insert(TextStyle::Body, body());
        s.text_styles.insert(TextStyle::Button, body());
        s.text_styles.insert(TextStyle::Small, small());
        s.text_styles.insert(TextStyle::Heading, h1());
    });
}

// Color helpers ---------------------------------------------------------------------------------

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// A rich but calm background tint from album art: keep the hue, cap the brightness so white
/// text always reads on top of it.
pub fn wash_from_art(rgb: Option<[u8; 3]>) -> Color32 {
    let [r, g, b] = rgb.unwrap_or([0x4A, 0x3A, 0x5E]);
    let (h, s, l) = rgb_to_hsl(r, g, b);
    let s = (s * 0.9).min(0.62);
    let l = l.clamp(0.22, 0.34);
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Color32::from_rgb(r, g, b)
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-5 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h * 6.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (f(r1), f(g1), f(b1))
}

// Text helpers ------------------------------------------------------------------------------------

fn is_rtl_char(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

pub fn has_rtl(s: &str) -> bool {
    s.chars().any(is_rtl_char)
}

/// A one-line layout job that elides with "…" and puts mixed right-to-left text (Arabic,
/// Hebrew) in visual order, so titles read correctly.
pub fn line_job(text: &str, font: FontId, color: Color32, max_width: f32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat, TextWrapping};
    let mut job = LayoutJob::default();
    let fmt = TextFormat {
        font_id: font,
        color,
        ..Default::default()
    };
    if has_rtl(text) {
        // Lay out right-to-left text ourselves: runs in visual order, and inside an RTL run the
        // words in reverse order (each Arabic/Hebrew word is still shaped right-to-left by the
        // font engine), with brackets mirrored. This gives correct results for mixed titles like
        // "Artist - أغنية (Live)".
        let bidi = unicode_bidi::BidiInfo::new(text, None);
        for para in &bidi.paragraphs {
            let line = para.range.clone();
            let (levels, runs) = bidi.visual_runs(para, line);
            for run in runs {
                let seg = &text[run.clone()];
                if seg.is_empty() {
                    continue;
                }
                if levels[run.start].is_rtl() {
                    let mut tokens: Vec<String> = Vec::new();
                    let mut cur = String::new();
                    for ch in seg.chars() {
                        let is_word = ch.is_alphanumeric() || (0x0600..=0x06FF).contains(&(ch as u32));
                        if is_word {
                            cur.push(ch);
                        } else {
                            if !cur.is_empty() {
                                tokens.push(std::mem::take(&mut cur));
                            }
                            tokens.push(mirror(ch).to_string());
                        }
                    }
                    if !cur.is_empty() {
                        tokens.push(cur);
                    }
                    // One section per token, so the font engine shapes each word on its own and
                    // egui doesn't merge them back into one run.
                    for t in tokens.iter().rev() {
                        let start = job.text.len();
                        job.text.push_str(t);
                        job.sections.push(egui::text::LayoutSection {
                            leading_space: 0.0,
                            byte_range: egui::epaint::text::ByteIndex(start)..egui::epaint::text::ByteIndex(job.text.len()),
                            format: fmt.clone(),
                        });
                    }
                } else {
                    let start = job.text.len();
                    job.text.push_str(seg);
                    job.sections.push(egui::text::LayoutSection {
                        leading_space: 0.0,
                        byte_range: egui::epaint::text::ByteIndex(start)..egui::epaint::text::ByteIndex(job.text.len()),
                        format: fmt.clone(),
                    });
                }
            }
        }
    } else {
        job.append(text, 0.0, fmt);
    }
    job.wrap = TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    job
}

fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        c => c,
    }
}

pub fn fmt_time(secs: f32) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "12 songs, 48 min" / "1 song, 3 min" / "120 songs, 7 hr 5 min"
pub fn songs_and_length(n: usize, secs: f32) -> String {
    let songs = if n == 1 { "1 song".to_string() } else { format!("{} songs", group_digits(n)) };
    if secs <= 0.0 {
        return songs;
    }
    let mins = (secs / 60.0).round() as u64;
    let len = if mins >= 60 {
        format!("{} hr {} min", mins / 60, mins % 60)
    } else {
        format!("{} min", mins.max(1))
    };
    format!("{songs}, {len}")
}

pub fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", group_digits(n))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bidi_sections() {
        for s in ["أغنية المساء", "فرقة القمر - ليل طويل (فيديو كليب)", "فرقة القمر VEVO"] {
            let job = super::line_job(s, egui::FontId::default(), egui::Color32::WHITE, 100.0);
            let parts: Vec<String> = job.sections.iter().map(|sec| format!("[{}]", &job.text[sec.byte_range.start.0..sec.byte_range.end.0])).collect();
            println!("{s} => {}", parts.join(""));
        }
    }
}
