//! The sleep timer: a moon in the player bar opens a small menu to stop the music later.
//! The timing itself lives in the audio engine (`player::Sleep`), so it works with the window
//! hidden in the tray.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Id, Rect, Sense, Ui};

use crate::app::App;
use crate::icons;
use crate::player::{Cmd, Sleep};
use crate::theme::*;
use crate::widgets as w;

const MENU_W: f32 = 252.0;
const GAP: f32 = S2;
/// The choices, in minutes.
const CHOICES: [(&str, u64); 4] = [("15 min", 15), ("30 min", 30), ("45 min", 45), ("1 hour", 60)];

impl App {
    /// The moon in the player bar. While the timer is on it turns accent-coloured, with the
    /// minutes left under it.
    pub(crate) fn sleep_button(&mut self, ui: &mut Ui, rect: Rect) {
        let sleep = self.ps.sleep;
        let left = secs_left(sleep);
        let resp = ui.interact(rect, Id::new("sleep-btn"), Sense::click());
        let tip = match sleep {
            Sleep::Off => "Sleep timer".to_string(),
            Sleep::At(_) => format!("Music stops in {}", in_words(left)),
            Sleep::EndOfSong => "Music stops after this song".to_string(),
        };
        let (h, pr) = w::hover_press(ui, &resp);
        let p = ui.painter();
        if h > 0.0 {
            p.circle_filled(rect.center(), rect.width() / 2.0, with_alpha(Color32::WHITE, 0.08 * h));
        }
        let on = sleep != Sleep::Off;
        let color = if on { ACCENT } else { lerp_color(TEXT_2, TEXT, h) };
        let s = rect.width() * 0.5 * (1.0 + 0.08 * h - 0.1 * pr);
        let c = rect.center();
        match sleep {
            Sleep::At(_) => {
                // Room for the minutes under the moon.
                icons::glyph(p, c - vec2(0.0, 5.0), icons::MOON, s * 0.9, color);
                w::text(p, c + vec2(0.0, 10.0), Align2::CENTER_CENTER, &badge(left), FontId::new(10.0, semibold()), ACCENT, rect.width() + 12.0);
                // Wake up when the minutes change, even while paused.
                let to_next = if left > 60 { left % 60 + 1 } else { 1 };
                ui.ctx().request_repaint_after(Duration::from_secs(to_next));
            }
            Sleep::EndOfSong => {
                icons::glyph(p, c, icons::MOON, s, color);
                p.circle_filled(c + vec2(0.0, rect.height() * 0.42), 1.8, ACCENT);
            }
            Sleep::Off => icons::glyph(p, c, icons::MOON, s, color),
        }
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip);
        egui::Popup::menu(&resp)
            .align(egui::RectAlign::TOP_END)
            .gap(S2)
            .width(MENU_W)
            .show(|ui| self.sleep_menu(ui));
    }

    fn sleep_menu(&mut self, ui: &mut Ui) {
        ui.set_width(MENU_W);
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        let third = (MENU_W - 2.0 * GAP) / 3.0;
        let half = (MENU_W - GAP) / 2.0;
        match self.ps.sleep {
            Sleep::Off => {
                heading(ui, "Sleep timer", "Fades the music out, then stops it");
                let mut pick: Option<(Sleep, String)> = None;
                ui.horizontal(|ui| {
                    for (label, mins) in &CHOICES[..3] {
                        if chip(ui, third, label, false).clicked() {
                            pick = Some((after(*mins), format!("Music stops in {label}")));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let (label, mins) = CHOICES[3];
                    if chip(ui, third, label, false).clicked() {
                        pick = Some((after(mins), format!("Music stops in {label}")));
                    }
                    if chip(ui, third * 2.0 + GAP, "End of song", false).clicked() {
                        pick = Some((Sleep::EndOfSong, "Music stops after this song".to_string()));
                    }
                });
                if let Some((sleep, note)) = pick {
                    self.player.send(Cmd::SetSleep(sleep));
                    self.toast(note);
                    ui.close();
                }
            }
            Sleep::At(t) => {
                let left = secs_left(Sleep::At(t));
                let (r, _) = ui.allocate_exact_size(vec2(MENU_W, 62.0), Sense::hover());
                let p = ui.painter();
                w::text(p, pos2(r.left() + 4.0, r.top() + 12.0), Align2::LEFT_CENTER, "Music stops in", small(), TEXT_2, MENU_W);
                w::text(p, pos2(r.left() + 4.0, r.top() + 42.0), Align2::LEFT_CENTER, &clock(left), h1(), TEXT, MENU_W);
                ui.horizontal(|ui| {
                    if chip(ui, half, "+10 min", false).clicked() {
                        self.player.send(Cmd::SetSleep(Sleep::At(t.max(Instant::now()) + Duration::from_secs(600))));
                    }
                    if chip(ui, half, "Turn off", true).clicked() {
                        self.player.send(Cmd::SetSleep(Sleep::Off));
                        self.toast("Sleep timer off");
                        ui.close();
                    }
                });
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            }
            Sleep::EndOfSong => {
                heading(ui, "Sleep timer", "Music stops after this song");
                if chip(ui, MENU_W, "Turn off", true).clicked() {
                    self.player.send(Cmd::SetSleep(Sleep::Off));
                    self.toast("Sleep timer off");
                    ui.close();
                }
            }
        }
    }
}

fn after(mins: u64) -> Sleep {
    Sleep::At(Instant::now() + Duration::from_secs(mins * 60))
}

/// Whole seconds until the timer is up (rounded up, so it never shows 0:00 while music plays).
fn secs_left(sleep: Sleep) -> u64 {
    match sleep {
        Sleep::At(t) => t.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u64,
        _ => 0,
    }
}

/// "23:41", or "1:05:00" past an hour.
fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Under the moon: "23m", or "45s" in the last minute.
fn badge(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m", secs.div_ceil(60))
    } else {
        format!("{secs}s")
    }
}

/// For the tooltip: "23 min", "1 min", "less than a minute".
fn in_words(secs: u64) -> String {
    match secs.div_ceil(60) {
        _ if secs < 60 => "less than a minute".to_string(),
        m => format!("{m} min"),
    }
}

fn heading(ui: &mut Ui, title: &str, sub: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(MENU_W, 42.0), Sense::hover());
    let p = ui.painter();
    w::text(p, pos2(r.left() + 4.0, r.top() + 12.0), Align2::LEFT_CENTER, title, body_strong(), TEXT, MENU_W);
    w::text(p, pos2(r.left() + 4.0, r.top() + 30.0), Align2::LEFT_CENTER, sub, small(), TEXT_2, MENU_W);
}

/// A rounded choice. `quiet`: no fill until hovered (for "Turn off").
fn chip(ui: &mut Ui, wdt: f32, label: &str, quiet: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(wdt, 36.0), Sense::click());
    let (h, pr) = w::hover_press(ui, &resp);
    let r = w::scale_rect(r, 1.0 - 0.03 * pr);
    let bg = if quiet {
        with_alpha(Color32::WHITE, 0.07 * h)
    } else {
        lerp_color(RAISED_2, Color32::from_rgb(0x33, 0x33, 0x3B), h)
    };
    let p = ui.painter();
    p.rect_filled(r, R_MD, bg);
    let fg = if quiet { lerp_color(TEXT_2, TEXT, h) } else { TEXT };
    w::text(p, r.center(), Align2::CENTER_CENTER, label, body_strong(), fg, wdt);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_reads_well() {
        assert_eq!(clock(23 * 60 + 41), "23:41");
        assert_eq!(clock(65 * 60), "1:05:00");
        assert_eq!(clock(9), "0:09");
        assert_eq!(badge(23 * 60 + 1), "24m");
        assert_eq!(badge(60), "1m");
        assert_eq!(badge(45), "45s");
        assert_eq!(in_words(30 * 60), "30 min");
        assert_eq!(in_words(20), "less than a minute");
    }
}
