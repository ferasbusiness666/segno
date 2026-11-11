//! The equalizer's pop-up: a button beside the moon in the player bar opens it. An on/off
//! switch, the presets, and ten sliders with the curve they make drawn behind them. Every
//! change is heard at once (the sound itself is in `eq.rs`).

use egui::epaint::PathShape;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Id, Rect, Sense, Stroke, Ui};

use crate::app::App;
use crate::eq::{self, BANDS, FLAT, LABELS, MAX_DB, PRESETS};
use crate::icons;
use crate::theme::*;
use crate::widgets as w;

const MENU_W: f32 = 436.0;
const SLIDERS_H: f32 = 172.0;
/// Slider values move in half-dB steps, and stick to 0 when close.
const STEP: f32 = 0.5;
const STICK: f32 = 0.75;

impl App {
    /// The equalizer button: accent-coloured while it's changing the sound.
    pub(crate) fn eq_button(&mut self, ui: &mut Ui, rect: Rect) {
        let shaping = self.settings.eq_on && self.settings.eq_gains != FLAT;
        let resp = ui.interact(rect, Id::new("eq-btn"), Sense::click());
        let (h, pr) = w::hover_press(ui, &resp);
        let p = ui.painter();
        if h > 0.0 {
            p.circle_filled(rect.center(), rect.width() / 2.0, with_alpha(Color32::WHITE, 0.08 * h));
        }
        let color = if shaping { ACCENT } else { lerp_color(TEXT_2, TEXT, h) };
        icons::glyph(p, rect.center(), icons::SLIDERS, rect.width() * 0.5 * (1.0 + 0.08 * h - 0.1 * pr), color);
        let tip = match (self.settings.eq_on, eq::preset_of(&self.settings.eq_gains)) {
            (false, _) => "Equalizer".to_string(),
            (true, Some(name)) => format!("Equalizer: {name}"),
            (true, None) => "Equalizer: Custom".to_string(),
        };
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip);
        egui::Popup::menu(&resp)
            .align(egui::RectAlign::TOP_END)
            .gap(S2)
            .width(MENU_W)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.eq_menu(ui));
    }

    fn eq_menu(&mut self, ui: &mut Ui) {
        ui.set_width(MENU_W);
        ui.spacing_mut().item_spacing = vec2(S2, S2);
        let mut changed = false;
        let mut save = false;

        // Title, what's playing through it, and the switch.
        let (r, resp) = ui.allocate_exact_size(vec2(MENU_W, 44.0), Sense::click());
        let on = self.settings.eq_on;
        let name = eq::preset_of(&self.settings.eq_gains).unwrap_or("Custom");
        let p = ui.painter();
        w::text(p, pos2(r.left() + 4.0, r.top() + 13.0), Align2::LEFT_CENTER, "Equalizer", body_strong(), TEXT, MENU_W - 80.0);
        let sub = if on { name.to_string() } else { "Off".to_string() };
        w::text(p, pos2(r.left() + 4.0, r.top() + 32.0), Align2::LEFT_CENTER, &sub, small(), TEXT_2, MENU_W - 80.0);
        let t = ui.ctx().animate_bool_with_time(Id::new("eq-switch"), on, T);
        let sw = Rect::from_center_size(pos2(r.right() - 26.0, r.center().y), vec2(44.0, 24.0));
        p.rect_filled(sw, 12, lerp_color(RAISED_2, ACCENT, t));
        p.circle_filled(pos2(sw.left() + 12.0 + 20.0 * egui::emath::easing::cubic_out(t), sw.center().y), 9.5, Color32::WHITE);
        if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            self.settings.eq_on = !on;
            (changed, save) = (true, true);
        }

        // Presets, and the user's own curve when there is one.
        let custom = self.settings.eq_custom;
        let has_custom = custom != FLAT && eq::preset_of(&custom).is_none();
        let current = if on { eq::preset_of(&self.settings.eq_gains).or(Some("Custom")) } else { None };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let chips = PRESETS.iter().map(|(n, g)| (*n, *g)).chain(has_custom.then_some(("Custom", custom)));
            for (label, gains) in chips {
                if preset_chip(ui, label, current == Some(label)).clicked() {
                    self.settings.eq_gains = gains;
                    self.settings.eq_on = true;
                    (changed, save) = (true, true);
                }
            }
        });
        ui.add_space(S1);

        // The sliders, with the curve they make behind them.
        let (area, _) = ui.allocate_exact_size(vec2(MENU_W, SLIDERS_H + 22.0), Sense::hover());
        let track = Rect::from_min_max(pos2(area.left(), area.top() + 14.0), pos2(area.right(), area.top() + 14.0 + SLIDERS_H - 28.0));
        let col_w = area.width() / 10.0;
        let x_of = |i: f32| area.left() + col_w * (i + 0.5);
        let y_of = |db: f32| track.center().y - db.clamp(-MAX_DB, MAX_DB) / MAX_DB * track.height() / 2.0;
        let dim = if on { 1.0 } else { 0.45 };
        let p = ui.painter().clone();
        // 0 dB, and ±6 dB faintly.
        p.line_segment([pos2(area.left() + 6.0, y_of(0.0)), pos2(area.right() - 6.0, y_of(0.0))], Stroke::new(1.0, with_alpha(Color32::WHITE, 0.12)));
        for db in [-6.0, 6.0] {
            p.line_segment([pos2(area.left() + 6.0, y_of(db)), pos2(area.right() - 6.0, y_of(db))], Stroke::new(1.0, with_alpha(Color32::WHITE, 0.04)));
        }
        // The curve: what the sliders do together, worked out from the filters themselves.
        let curve = eq::Curve::new(&self.settings.eq_gains);
        let pts: Vec<egui::Pos2> = (0..=90)
            .map(|k| {
                let band = -0.4 + 9.8 * k as f32 / 90.0;
                let f = BANDS[0] * 2f64.powf(band as f64);
                pos2(x_of(band), y_of(curve.db_at(f) as f32))
            })
            .collect();
        if on {
            for pair in pts.windows(2) {
                // Filled in thin slices so the shape doesn't have to be convex.
                let (a, b) = (pair[0], pair[1]);
                let quad = vec![a, b, pos2(b.x, y_of(0.0)), pos2(a.x, y_of(0.0))];
                p.add(egui::Shape::convex_polygon(quad, with_alpha(ACCENT, 0.10), Stroke::NONE));
            }
        }
        p.add(PathShape::line(pts, Stroke::new(2.0, with_alpha(ACCENT, 0.9 * dim))));

        for (i, label) in LABELS.iter().enumerate() {
            let x = x_of(i as f32);
            let hit = Rect::from_center_size(pos2(x, track.center().y), vec2(col_w, track.height() + 24.0));
            let resp = ui.interact(hit, Id::new(("eq-band", i)), Sense::click_and_drag());
            let v = self.settings.eq_gains[i];
            if resp.double_clicked() {
                self.settings.eq_gains[i] = 0.0;
                self.settings.eq_custom = self.settings.eq_gains;
                self.settings.eq_on = true;
                (changed, save) = (true, true);
            } else if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
                let raw = (track.center().y - pos.y) / (track.height() / 2.0) * MAX_DB;
                let mut nv = ((raw / STEP).round() * STEP).clamp(-MAX_DB, MAX_DB);
                if nv.abs() < STICK {
                    nv = 0.0;
                }
                if nv != v || !self.settings.eq_on {
                    self.settings.eq_gains[i] = nv;
                    self.settings.eq_custom = self.settings.eq_gains;
                    self.settings.eq_on = true;
                    changed = true;
                }
            }
            if resp.drag_stopped() || resp.clicked() {
                save = true;
            }
            let v = self.settings.eq_gains[i];
            let (h, _) = w::hover_press(ui, &resp);
            let active = resp.dragged() || resp.hovered();
            // The track, filled from 0 dB to the value.
            p.line_segment([pos2(x, track.top()), pos2(x, track.bottom())], Stroke::new(4.0, with_alpha(Color32::WHITE, 0.08 + 0.04 * h)));
            if v != 0.0 {
                p.line_segment([pos2(x, y_of(0.0)), pos2(x, y_of(v))], Stroke::new(4.0, with_alpha(ACCENT, dim)));
            }
            let knob = pos2(x, y_of(v));
            p.circle_filled(knob, 7.0 + 1.5 * h, with_alpha(Color32::WHITE, 0.6 + 0.4 * dim));
            // The value while it's being looked at, the band's frequency always.
            if active {
                let label = if v == 0.0 { "0".to_string() } else { format!("{v:+.1}").replace(".0", "") };
                w::text(&p, pos2(x, track.top() - 9.0), Align2::CENTER_CENTER, &label, FontId::new(11.0, semibold()), TEXT, col_w);
            }
            w::text(&p, pos2(x, area.bottom() - 7.0), Align2::CENTER_CENTER, label, FontId::new(11.0, semibold()), if active { TEXT } else { TEXT_3 }, col_w);
            let _ = resp.on_hover_cursor(CursorIcon::ResizeVertical).on_hover_text("Drag to change · double-click for 0");
        }

        if changed {
            self.player.eq.set(self.settings.eq_on, &self.settings.eq_gains);
        }
        if save {
            self.settings.save();
        }
    }
}

/// A preset to pick: filled while it's the one playing.
fn preset_chip(ui: &mut Ui, label: &str, on: bool) -> egui::Response {
    let tw = w::text_width(ui, label, small_strong());
    let (r, resp) = ui.allocate_exact_size(vec2(tw + 24.0, 30.0), Sense::click());
    let (h, pr) = w::hover_press(ui, &resp);
    let r = w::scale_rect(r, 1.0 - 0.03 * pr);
    let (bg, fg) = if on { (TEXT, BG) } else { (lerp_color(RAISED_2, Color32::from_rgb(0x33, 0x33, 0x3B), h), TEXT) };
    let p = ui.painter();
    p.rect_filled(r, 15, bg);
    w::text(p, r.center(), Align2::CENTER_CENTER, label, small_strong(), fg, f32::INFINITY);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}
