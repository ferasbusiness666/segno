//! Icons: line icons come from a tiny subset of the Lucide icon font (ISC license); the solid
//! transport controls (play, pause, skip) and the filled heart are drawn as shapes so they stay
//! crisp and weighty at any size.

use egui::{epaint::PathShape, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Shape, Stroke};

pub const ARROW_DOWN_UP: &str = "\u{E046}";
pub const CHECK: &str = "\u{E06C}";
pub const CHEVRON_LEFT: &str = "\u{E06E}";
pub const CHEVRON_RIGHT: &str = "\u{E06F}";
pub const CIRCLE_ALERT: &str = "\u{E077}";
pub const CLOCK: &str = "\u{E250}";
pub const DISC: &str = "\u{E494}";
pub const DOWNLOAD: &str = "\u{E0B2}";
pub const ARROW_DOWN_TO_LINE: &str = "\u{E455}";
pub const ELLIPSIS: &str = "\u{E0B6}";
pub const FOLDER_OPEN: &str = "\u{E247}";
pub const HEART: &str = "\u{E0F2}";
pub const LINK: &str = "\u{E102}";
pub const LIST_END: &str = "\u{E2DF}";
pub const LIST_MUSIC: &str = "\u{E2E0}";
pub const LIST_PLUS: &str = "\u{E23F}";
pub const LIST_START: &str = "\u{E2E1}";
pub const MAXIMIZE: &str = "\u{E113}";
pub const MUSIC: &str = "\u{E122}";
pub const PENCIL: &str = "\u{E1F9}";
pub const PIP: &str = "\u{E3AF}";
pub const PLUS: &str = "\u{E13D}";
pub const REFRESH: &str = "\u{E145}";
pub const REPEAT: &str = "\u{E146}";
pub const REPEAT_1: &str = "\u{E1FD}";
pub const SEARCH: &str = "\u{E151}";
pub const SETTINGS: &str = "\u{E154}";
pub const SHUFFLE: &str = "\u{E15E}";
pub const TRASH: &str = "\u{E18E}";
pub const USER: &str = "\u{E468}";
pub const VOLUME_0: &str = "\u{E1A9}";
pub const VOLUME_1: &str = "\u{E1AA}";
pub const VOLUME_2: &str = "\u{E1AB}";
pub const VOLUME_X: &str = "\u{E1AC}";
pub const X: &str = "\u{E1B2}";
pub const INFO: &str = "\u{E0F9}";

/// Paint a line icon centered at `c`, `size` points tall.
pub fn glyph(p: &Painter, c: Pos2, icon: &str, size: f32, color: Color32) {
    p.text(c, Align2::CENTER_CENTER, icon, FontId::new(size, crate::theme::icon_family()), color);
}

/// Solid play triangle with softly rounded corners, optically centered.
pub fn play(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let h = size;
    let w = size * 0.88;
    // Optical center: a triangle looks left-heavy, nudge it right.
    let cx = c.x + size * 0.08;
    let a = pos2(cx - w / 2.0, c.y - h / 2.0);
    let b = pos2(cx + w / 2.0, c.y);
    let d = pos2(cx - w / 2.0, c.y + h / 2.0);
    rounded_poly(p, &[a, b, d], size * 0.12, color);
}

pub fn pause(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let bw = size * 0.3;
    let gap = size * 0.26;
    let r = (bw * 0.3) as u8;
    for x in [c.x - gap / 2.0 - bw, c.x + gap / 2.0] {
        p.rect_filled(
            egui::Rect::from_min_size(pos2(x, c.y - size / 2.0), vec2(bw, size)),
            r,
            color,
        );
    }
}

/// Skip forward (`dir` = 1) or back (`dir` = -1): a triangle and a bar.
pub fn skip(p: &Painter, c: Pos2, size: f32, color: Color32, dir: f32) {
    let h = size;
    let w = size * 0.72;
    let bar = size * 0.16;
    let total = w + bar;
    let left = c.x - total / 2.0;
    if dir > 0.0 {
        let a = pos2(left, c.y - h / 2.0);
        let b = pos2(left + w, c.y);
        let d = pos2(left, c.y + h / 2.0);
        rounded_poly(p, &[a, b, d], size * 0.1, color);
        p.rect_filled(
            egui::Rect::from_min_size(pos2(left + w - bar * 0.1, c.y - h / 2.0), vec2(bar, h)),
            (bar * 0.35) as u8,
            color,
        );
    } else {
        p.rect_filled(
            egui::Rect::from_min_size(pos2(left + bar * 0.1, c.y - h / 2.0), vec2(bar, h)),
            (bar * 0.35) as u8,
            color,
        );
        let x0 = left + bar;
        let a = pos2(x0 + w, c.y - h / 2.0);
        let b = pos2(x0, c.y);
        let d = pos2(x0 + w, c.y + h / 2.0);
        rounded_poly(p, &[a, d, b], size * 0.1, color);
    }
}

/// A filled heart from cubic curves.
pub fn heart_filled(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let s = size / 24.0;
    let o = c - vec2(12.0 * s, 12.5 * s);
    let pt = |x: f32, y: f32| o + vec2(x * s, y * s);
    // Two lobes and a point, sampled along beziers into one convex-ish polygon.
    let mut pts = Vec::new();
    let segs = [
        [pt(12.0, 21.0), pt(4.5, 15.5), pt(2.0, 12.0), pt(2.0, 8.5)],
        [pt(2.0, 8.5), pt(2.0, 5.4), pt(4.4, 3.0), pt(7.5, 3.0)],
        [pt(7.5, 3.0), pt(9.4, 3.0), pt(11.0, 4.0), pt(12.0, 5.6)],
        [pt(12.0, 5.6), pt(13.0, 4.0), pt(14.6, 3.0), pt(16.5, 3.0)],
        [pt(16.5, 3.0), pt(19.6, 3.0), pt(22.0, 5.4), pt(22.0, 8.5)],
        [pt(22.0, 8.5), pt(22.0, 12.0), pt(19.5, 15.5), pt(12.0, 21.0)],
    ];
    for seg in segs {
        for i in 0..10 {
            let t = i as f32 / 10.0;
            let u = 1.0 - t;
            let q = seg[0].to_vec2() * (u * u * u)
                + seg[1].to_vec2() * (3.0 * u * u * t)
                + seg[2].to_vec2() * (3.0 * u * t * t)
                + seg[3].to_vec2() * (t * t * t);
            pts.push(q.to_pos2());
        }
    }
    // The notch makes the outline concave, so fill each half separately (both are convex).
    let mid = pts.len() / 2;
    let left: Vec<Pos2> = pts[..=mid].to_vec();
    let mut right: Vec<Pos2> = pts[mid..].to_vec();
    right.push(pts[0]);
    p.add(Shape::convex_polygon(left, color, Stroke::NONE));
    p.add(Shape::convex_polygon(right, color, Stroke::NONE));
}

fn rounded_poly(p: &Painter, pts: &[Pos2], r: f32, color: Color32) {
    let n = pts.len();
    let mut out = Vec::with_capacity(n * 6);
    for i in 0..n {
        let prev = pts[(i + n - 1) % n];
        let cur = pts[i];
        let next = pts[(i + 1) % n];
        let d1 = (prev - cur).normalized();
        let d2 = (next - cur).normalized();
        let a = cur + d1 * r;
        let b = cur + d2 * r;
        // Quadratic curve from a to b with control at the corner.
        for k in 0..=5 {
            let t = k as f32 / 5.0;
            let u = 1.0 - t;
            let q = a.to_vec2() * (u * u) + cur.to_vec2() * (2.0 * u * t) + b.to_vec2() * (t * t);
            out.push(q.to_pos2());
        }
    }
    p.add(Shape::Path(PathShape::convex_polygon(out, color, Stroke::NONE)));
}

/// A small three-bar "now playing" mark (static: no looping animation, to save battery).
/// The "now playing" bars. `motion` 0 = still (paused), 1 = bouncing (playing), in between
/// while they settle or start; `t` is the time in seconds.
pub fn now_playing_bars(p: &Painter, c: Pos2, size: f32, color: Color32, t: f64, motion: f32) {
    let bw = size * 0.18;
    let gap = size * 0.14;
    let still = [0.55, 1.0, 0.75];
    // Each bar bounces at its own pace, from two waves mixed, so it never looks like a loop.
    let speeds = [(7.3, 4.1), (5.9, 9.7), (8.6, 5.3)];
    let total = bw * 3.0 + gap * 2.0;
    for i in 0..3 {
        let (a, b) = speeds[i];
        let k = i as f64 * 1.7;
        let wave = 0.5 + 0.3 * (t * a + k).sin() as f32 + 0.2 * (t * b + k * 2.3).sin() as f32;
        let moving = 0.25 + 0.75 * wave.clamp(0.0, 1.0);
        let h = still[i] + (moving - still[i]) * motion;
        let x = c.x - total / 2.0 + i as f32 * (bw + gap);
        let hh = size * h;
        p.rect_filled(
            egui::Rect::from_min_size(pos2(x, c.y + size / 2.0 - hh), vec2(bw, hh)),
            (bw / 2.0) as u8,
            color,
        );
    }
}
pub const PANEL_CLOSE: &str = "\u{E21C}";
pub const PANEL_OPEN: &str = "\u{E21D}";
pub const SPARKLES: &str = "\u{E412}";
pub const IMAGE: &str = "\u{E0F6}";
pub const MINIMIZE: &str = "\u{E11B}";
