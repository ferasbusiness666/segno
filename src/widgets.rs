//! Reusable controls. One component per job, used everywhere.

use crate::icons;
use crate::theme::{self, *};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Painter, Pos2, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, TextureId, Ui, Vec2,
};

/// Hover and press amounts (0..1), animated.
pub fn hover_press(ui: &Ui, resp: &Response) -> (f32, f32) {
    let ctx = ui.ctx();
    let h = ctx.animate_bool_with_time(resp.id.with("h"), resp.hovered(), T_FAST);
    let p = ctx.animate_bool_with_time(resp.id.with("p"), resp.is_pointer_button_down_on(), 0.07);
    (h, p)
}

/// Like `hover_press`, but the pointer counts anywhere over `rect`, buttons on top included.
/// Use it for cards whose buttons only show on hover: pointing at the button must not count
/// as leaving the card, or the button hides, the card is hovered again, and it flickers.
pub fn group_hover_press(ui: &Ui, resp: &Response, rect: Rect) -> (f32, f32) {
    let ctx = ui.ctx();
    let over = ui.rect_contains_pointer(rect) && !egui::DragAndDrop::has_any_payload(ctx);
    let h = ctx.animate_bool_with_time(resp.id.with("h"), over, T_FAST);
    let p = ctx.animate_bool_with_time(resp.id.with("p"), resp.is_pointer_button_down_on(), 0.07);
    (h, p)
}

pub fn scale_rect(r: Rect, s: f32) -> Rect {
    Rect::from_center_size(r.center(), r.size() * s)
}

/// Draw a single line of text (elided to `max_w`). Returns the drawn rect.
pub fn text(p: &Painter, pos: Pos2, align: Align2, s: &str, font: FontId, color: Color32, max_w: f32) -> Rect {
    let job = theme::line_job(s, font, color, max_w);
    let galley = p.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, color);
    rect
}

pub fn text_width(ui: &Ui, s: &str, font: FontId) -> f32 {
    ui.painter()
        .layout_job(theme::line_job(s, font, TEXT, f32::INFINITY))
        .size()
        .x
}

/// Cover art with rounded corners, or a quiet placeholder while it loads / when there is none.
pub fn art(p: &Painter, rect: Rect, tex: Option<TextureId>, radius: f32, tint: Color32) {
    match tex {
        Some(t) => {
            p.add(
                egui::epaint::RectShape::filled(rect, radius, tint)
                    .with_texture(t, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
            );
        }
        None => {
            p.rect_filled(rect, radius, RAISED_2);
            let s = (rect.width() * 0.36).clamp(12.0, 64.0);
            icons::glyph(p, rect.center(), icons::MUSIC, s, TEXT_3);
        }
    }
}

/// Image with a known aspect (YouTube thumbnails).
pub fn thumb(p: &Painter, rect: Rect, tex: Option<TextureId>, radius: f32) {
    match tex {
        Some(t) => {
            p.add(
                egui::epaint::RectShape::filled(rect, radius, Color32::WHITE)
                    .with_texture(t, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
            );
        }
        None => {
            p.rect_filled(rect, radius, RAISED_2);
        }
    }
}

pub fn soft_shadow(p: &Painter, rect: Rect, radius: f32, strength: f32) {
    let sh = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 36,
        spread: 0,
        color: Color32::from_black_alpha((150.0 * strength) as u8),
    };
    p.add(sh.as_shape(rect, radius));
}

/// Round icon button with a hover circle. `active` tints the icon with the accent.
pub fn icon_button(ui: &mut Ui, size: f32, icon: &str, active: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    icon_button_at(ui, rect, resp, icon, active, tip)
}

pub fn icon_button_at(ui: &Ui, rect: Rect, resp: Response, icon: &str, active: bool, tip: &str) -> Response {
    let (h, pr) = hover_press(ui, &resp);
    let p = ui.painter();
    if h > 0.0 {
        p.circle_filled(rect.center(), rect.width() / 2.0, with_alpha(Color32::WHITE, 0.08 * h));
    }
    let base = if active { ACCENT } else { lerp_color(TEXT_2, TEXT, h) };
    let s = rect.width() * 0.5 * (1.0 - 0.08 * pr);
    icons::glyph(p, rect.center(), icon, s, base);
    if active {
        p.circle_filled(rect.center() + vec2(0.0, rect.height() * 0.42), 1.8, ACCENT);
    }
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if tip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tip)
    }
}

pub enum Transport {
    Prev,
    Next,
}

/// Solid skip buttons (player bar and full screen).
pub fn transport_button_c(ui: &mut Ui, size: f32, which: Transport, tip: &str, base: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let (h, pr) = hover_press(ui, &resp);
    let p = ui.painter();
    let c = lerp_color(base, TEXT, h);
    let s = size * 0.42 * (1.0 - 0.1 * pr);
    match which {
        Transport::Prev => icons::skip(p, rect.center(), s, c, -1.0),
        Transport::Next => icons::skip(p, rect.center(), s, c, 1.0),
    }
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip)
}

/// The big play/pause circle. `light` = white circle with dark icon (player bar);
/// otherwise the accent circle with a white icon (page headers).
pub fn play_circle(ui: &mut Ui, size: f32, playing: bool, light: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    paint_play_circle(ui, rect, &resp, playing, light, 1.0);
    resp.on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(if playing { "Pause (Space)" } else { "Play (Space)" })
}

pub fn paint_play_circle(ui: &Ui, rect: Rect, resp: &Response, playing: bool, light: bool, alpha: f32) {
    let (h, pr) = hover_press(ui, resp);
    let morph = ui.ctx().animate_bool_with_time(resp.id.with("pp"), playing, T_FAST);
    let p = ui.painter();
    let scale = 1.0 + 0.04 * h - 0.07 * pr;
    let r = rect.width() / 2.0 * scale;
    let (bg, fg) = if light {
        (lerp_color(Color32::from_rgb(0xEE, 0xEE, 0xF0), Color32::WHITE, h), BG)
    } else {
        (lerp_color(ACCENT, ACCENT_HI, h), ON_ACCENT)
    };
    if !light {
        p.add(
            egui::epaint::Shadow {
                offset: [0, 6],
                blur: 18,
                spread: 0,
                color: Color32::from_black_alpha((90.0 * alpha) as u8),
            }
            .as_shape(Rect::from_center_size(rect.center(), Vec2::splat(r * 2.0)), r),
        );
    }
    p.circle_filled(rect.center(), r, with_alpha(bg, alpha));
    let icon_s = r * 0.78;
    // Cross-fade between the two glyphs instead of swapping them.
    if morph < 1.0 {
        icons::play(p, rect.center(), icon_s, with_alpha(fg, (1.0 - morph) * alpha));
    }
    if morph > 0.0 {
        icons::pause(p, rect.center(), icon_s * 0.92, with_alpha(fg, morph * alpha));
    }
}

#[derive(PartialEq, Clone, Copy)]
pub enum PillKind {
    Primary,
    Secondary,
    Ghost,
}

/// Rounded button with optional icon. One Primary per screen.
pub fn pill(ui: &mut Ui, label: &str, icon: Option<&str>, kind: PillKind) -> Response {
    let font = body_strong();
    let tw = text_width(ui, label, font.clone());
    let icon_w = if icon.is_some() { 16.0 + S2 } else { 0.0 };
    let size = vec2(tw + icon_w + S4 * 2.0, 36.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    paint_pill(ui, rect, &resp, label, icon, kind);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn paint_pill(ui: &Ui, rect: Rect, resp: &Response, label: &str, icon: Option<&str>, kind: PillKind) {
    let (h, pr) = hover_press(ui, resp);
    let r = scale_rect(rect, 1.0 - 0.03 * pr);
    let p = ui.painter();
    let (bg, fg) = match kind {
        PillKind::Primary => (lerp_color(ACCENT, ACCENT_HI, h), ON_ACCENT),
        PillKind::Secondary => (lerp_color(RAISED_2, Color32::from_rgb(0x33, 0x33, 0x3B), h), TEXT),
        PillKind::Ghost => (with_alpha(Color32::WHITE, 0.07 * h), lerp_color(TEXT_2, TEXT, h)),
    };
    p.rect_filled(r, CornerRadius::same((r.height() / 2.0) as u8), bg);
    let font = body_strong();
    let tw = text_width(ui, label, font.clone());
    let icon_w = if icon.is_some() { 16.0 + S2 } else { 0.0 };
    let mut x = r.center().x - (tw + icon_w) / 2.0;
    if let Some(ic) = icon {
        if ic == "play" {
            icons::play(p, pos2(x + 7.0, r.center().y), 12.0, fg);
        } else {
            icons::glyph(p, pos2(x + 8.0, r.center().y), ic, 16.0, fg);
        }
        x += icon_w;
    }
    text(p, pos2(x, r.center().y), Align2::LEFT_CENTER, label, font, fg, f32::INFINITY);
}

/// Text-only button ("Clear", "Show all").
pub fn link_button(ui: &mut Ui, label: &str) -> Response {
    let font = small_strong();
    let tw = text_width(ui, label, font.clone());
    let (rect, resp) = ui.allocate_exact_size(vec2(tw + S2, 24.0), Sense::click());
    let (h, _) = hover_press(ui, &resp);
    text(
        ui.painter(),
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font,
        lerp_color(TEXT_2, TEXT, h),
        f32::INFINITY,
    );
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// A thin slider (seek, volume). Returns the new value while dragging/clicking, and whether the
/// drag just ended. The track thickens and a knob appears on hover.
pub struct SliderOut {
    pub value: Option<f32>,
    pub released: bool,
    pub hovered: bool,
    pub dragging: bool,
}

pub fn slider(ui: &mut Ui, rect: Rect, id: Id, value: f32, accent_on_hover: bool) -> SliderOut {
    let hit = rect.expand2(vec2(0.0, 8.0));
    let resp = ui.interact(hit, id, Sense::click_and_drag());
    let active = resp.hovered() || resp.dragged();
    let h = ui.ctx().animate_bool_with_time(id.with("h"), active, T_FAST);
    let mut out = SliderOut {
        value: None,
        released: resp.drag_stopped() || resp.clicked(),
        hovered: resp.hovered(),
        dragging: resp.dragged(),
    };
    if resp.is_pointer_button_down_on() || resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            out.value = Some(((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        }
    }
    let v = out.value.unwrap_or(value).clamp(0.0, 1.0);
    let p = ui.painter();
    let thick = 4.0 + 2.0 * h;
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), thick));
    p.rect_filled(track, CornerRadius::same((thick / 2.0) as u8), Color32::from_rgb(0x3A, 0x3A, 0x42));
    let fill = Rect::from_min_max(track.min, pos2(track.left() + track.width() * v, track.bottom()));
    let fill_c = if accent_on_hover { lerp_color(TEXT, ACCENT, h) } else { TEXT };
    if v > 0.0 {
        p.rect_filled(fill, CornerRadius::same((thick / 2.0) as u8), fill_c);
    }
    if h > 0.01 {
        let knob = pos2(fill.right(), track.center().y);
        p.circle_filled(knob + vec2(0.0, 1.0), 7.0 * h, Color32::from_black_alpha((80.0 * h) as u8));
        p.circle_filled(knob, 6.5 * h, Color32::WHITE);
    }
    if active {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    out
}

/// Sidebar navigation row.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool, badge: Option<String>) -> Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
    let (h, pr) = hover_press(ui, &resp);
    let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, T);
    let p = ui.painter();
    let bg = lerp_color(with_alpha(RAISED, h), RAISED_2, sel);
    if h > 0.0 || sel > 0.0 {
        p.rect_filled(rect, R_MD, bg);
    }
    let fg = lerp_color(lerp_color(TEXT_2, TEXT, h), TEXT, sel);
    let ic = pos2(rect.left() + S3 + 9.0, rect.center().y);
    icons::glyph(p, ic, icon, 18.0 * (1.0 - 0.06 * pr), if selected { ACCENT } else { fg });
    let font = if selected { body_strong() } else { body() };
    text(
        p,
        pos2(rect.left() + S3 + 18.0 + S3, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        fg,
        rect.width() - 80.0,
    );
    if let Some(b) = badge {
        let tw = text_width(ui, &b, small_strong());
        let br = Rect::from_center_size(pos2(rect.right() - S3 - (tw + 12.0) / 2.0, rect.center().y), vec2(tw + 12.0, 20.0));
        ui.painter().rect_filled(br, 10, ACCENT);
        text(ui.painter(), br.center(), Align2::CENTER_CENTER, &b, small_strong(), ON_ACCENT, f32::INFINITY);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Section label ("Playlists").
pub fn section_label(p: &Painter, pos: Pos2, s: &str) {
    text(p, pos, Align2::LEFT_CENTER, s, small_strong(), TEXT_3, f32::INFINITY);
}

/// A rotating arc. Only drawn while something is actually loading.
pub fn spinner(ui: &Ui, center: Pos2, radius: f32, color: Color32) {
    let t = ui.input(|i| i.time) as f32;
    let start = t * 5.5;
    let n = 24;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = start + i as f32 / n as f32 * std::f32::consts::PI * 1.3;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    ui.painter().add(Shape::line(pts, Stroke::new(2.0, color)));
    // ~30 fps is plenty for a spinner and half the cost of 60.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// Circular progress (downloads).
pub fn progress_ring(p: &Painter, center: Pos2, radius: f32, frac: f32, color: Color32) {
    p.circle_stroke(center, radius, Stroke::new(2.5, Color32::from_rgb(0x3A, 0x3A, 0x42)));
    let frac = frac.clamp(0.0, 1.0);
    if frac > 0.0 {
        let n = (48.0 * frac).max(2.0) as usize;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * frac * std::f32::consts::TAU;
                center + vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        p.add(Shape::line(pts, Stroke::new(2.5, color)));
    }
}

/// Vertical gradient that follows a rounded-top rectangle (page color wash).
pub fn vgradient(p: &Painter, rect: Rect, radius: f32, stops: &[(f32, Color32)]) {
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    let color_at = |y: f32| -> Color32 {
        let t = ((y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let mut prev = stops[0];
        for s in stops {
            if t <= s.0 {
                let span = (s.0 - prev.0).max(1e-4);
                return lerp_color(prev.1, s.1, (t - prev.0) / span);
            }
            prev = *s;
        }
        stops.last().unwrap().1
    };
    let mut ys: Vec<f32> = Vec::new();
    for i in 0..=8 {
        ys.push(rect.top() + radius * (i as f32 / 8.0));
    }
    for i in 1..=24 {
        ys.push(rect.top() + radius + (rect.height() - radius) * (i as f32 / 24.0));
    }
    let inset = |y: f32| -> f32 {
        let dy = y - rect.top();
        if dy >= radius {
            0.0
        } else {
            let k = radius - dy;
            radius - (radius * radius - k * k).max(0.0).sqrt()
        }
    };
    let mut mesh = Mesh::default();
    for (i, &y) in ys.iter().enumerate() {
        let x = inset(y);
        let c = color_at(y);
        mesh.vertices.push(Vertex {
            pos: pos2(rect.left() + x, y),
            uv: WHITE_UV,
            color: c,
        });
        mesh.vertices.push(Vertex {
            pos: pos2(rect.right() - x, y),
            uv: WHITE_UV,
            color: c,
        });
        if i > 0 {
            let b = (i as u32 - 1) * 2;
            mesh.indices.extend_from_slice(&[b, b + 1, b + 2, b + 1, b + 3, b + 2]);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// Outline-free card surface on hover.
pub fn hover_surface(p: &Painter, rect: Rect, h: f32) {
    if h > 0.0 {
        p.rect_filled(rect, R_MD, with_alpha(RAISED, h));
    }
}

pub fn empty_state(ui: &mut Ui, icon: &str, title: &str, body_text: &str, button: Option<(&str, Option<&str>)>) -> bool {
    let w = ui.available_width().min(420.0);
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(S12);
        let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 32.0, RAISED);
        icons::glyph(ui.painter(), r.center(), icon, 26.0, TEXT_2);
        ui.add_space(S4);
        let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
        text(ui.painter(), r.center(), Align2::CENTER_CENTER, title, h2(), TEXT, w);
        ui.add_space(S1);
        let galley = ui.painter().layout(body_text.to_string(), body(), TEXT_2, w);
        let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
        let x = r.center().x - galley.size().x / 2.0;
        // Center each line.
        let job = {
            let mut j = egui::text::LayoutJob::simple(body_text.to_string(), body(), TEXT_2, w);
            j.halign = egui::Align::Center;
            j
        };
        let g = ui.painter().layout_job(job);
        let _ = x;
        ui.painter().galley(pos2(r.center().x, r.top()), g, TEXT_2);
        if let Some((label, icon)) = button {
            ui.add_space(S5);
            if pill(ui, label, icon, PillKind::Primary).clicked() {
                clicked = true;
            }
        }
    });
    clicked
}

/// Rounded search box with icon and clear button. Returns the text edit output.
pub fn search_box(ui: &mut Ui, value: &mut String, hint: &str, id: Id, width: f32) -> egui::text_edit::TextEditOutput {
    let height = 38.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    let f = ui.ctx().animate_bool_with_time(id.with("f"), focused, T_FAST);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same((height / 2.0) as u8), lerp_color(RAISED, RAISED_2, f));
    if f > 0.0 {
        p.rect_stroke(
            rect,
            CornerRadius::same((height / 2.0) as u8),
            Stroke::new(1.0, with_alpha(Color32::WHITE, 0.25 * f)),
            StrokeKind::Inside,
        );
    }
    icons::glyph(p, pos2(rect.left() + 20.0, rect.center().y), icons::SEARCH, 16.0, lerp_color(TEXT_3, TEXT, f));
    let edit_rect = Rect::from_min_max(pos2(rect.left() + 38.0, rect.top()), pos2(rect.right() - 34.0, rect.bottom()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let out = egui::TextEdit::singleline(value)
        .id(id)
        .hint_text(egui::RichText::new(hint).color(TEXT_3))
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::symmetric(0, 0))
        .font(body())
        .text_color(TEXT)
        .desired_width(edit_rect.width())
        .show(&mut child);
    if !value.is_empty() {
        let cr = Rect::from_center_size(pos2(rect.right() - 19.0, rect.center().y), Vec2::splat(26.0));
        let resp = ui.interact(cr, id.with("clear"), Sense::click());
        let (h, _) = hover_press(ui, &resp);
        ui.painter().circle_filled(cr.center(), 11.0, with_alpha(Color32::WHITE, 0.06 + 0.08 * h));
        icons::glyph(ui.painter(), cr.center(), icons::X, 13.0, TEXT_2);
        if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            value.clear();
            ui.memory_mut(|m| m.request_focus(id));
        }
    }
    out
}

/// A picture cut to a circle and turned by `angle` (radians). Used for the record's label.
pub fn art_disc(p: &Painter, center: Pos2, r: f32, tex: TextureId, angle: f32, tint: Color32) {
    let n = 72;
    let mut mesh = egui::Mesh::with_texture(tex);
    mesh.colored_vertex(center, tint);
    mesh.vertices[0].uv = pos2(0.5, 0.5);
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let pos = center + vec2(a.cos(), a.sin()) * r;
        // The picture turns with the record: sample it at the angle minus the rotation.
        let ua = a - angle;
        let uv = pos2(0.5 + 0.5 * ua.cos(), 0.5 + 0.5 * ua.sin());
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: tint });
    }
    for i in 1..=n as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    p.add(Shape::mesh(mesh));
}

/// A vinyl record: black disc, fine grooves, a light sheen that stays put while the record
/// turns, and the cover as the label in the middle.
pub fn vinyl(p: &Painter, center: Pos2, r: f32, label: Option<TextureId>, label_color: Color32, angle: f32) {
    // Disc and its edge.
    p.circle_filled(center + vec2(0.0, r * 0.03), r * 1.01, Color32::from_black_alpha(90));
    p.circle_filled(center, r, Color32::from_rgb(0x0C, 0x0C, 0x0E));
    // Grooves: thin rings, slightly uneven like the real thing.
    let rings = 26;
    for i in 0..rings {
        let t = i as f32 / rings as f32;
        let rr = r * (0.40 + 0.57 * t);
        let a = if i % 3 == 0 { 0.07 } else { 0.035 };
        p.circle_stroke(center, rr, Stroke::new(1.0, Color32::from_white_alpha((a * 255.0) as u8)));
    }
    // The gap between tracks.
    p.circle_stroke(center, r * 0.72, Stroke::new(2.0, Color32::from_rgb(0x07, 0x07, 0x08)));
    // Sheen: two soft highlights across from each other, fixed in place.
    let n = 120;
    let mut mesh = egui::Mesh::default();
    let (inner, outer) = (r * 0.36, r * 0.995);
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let d = (2.0 * (a - (-0.8))).cos().max(0.0).powf(8.0);
        let col = Color32::from_white_alpha((d * 34.0) as u8);
        let dir = vec2(a.cos(), a.sin());
        mesh.colored_vertex(center + dir * inner, Color32::from_white_alpha((d * 8.0) as u8));
        mesh.colored_vertex(center + dir * outer, col);
    }
    for i in 0..n as u32 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    p.add(Shape::mesh(mesh));
    p.circle_stroke(center, r, Stroke::new(1.0, Color32::from_white_alpha(18)));
    // Label.
    let lr = r * 0.34;
    match label {
        Some(t) => art_disc(p, center, lr, t, angle, Color32::WHITE),
        None => {
            p.circle_filled(center, lr, label_color);
            let mark = center + vec2(angle.cos(), angle.sin()) * lr * 0.6;
            p.circle_filled(mark, lr * 0.08, Color32::from_white_alpha(120));
        }
    }
    p.circle_stroke(center, lr, Stroke::new(1.0, Color32::from_black_alpha(80)));
    // Spindle hole.
    p.circle_filled(center, r * 0.022, Color32::from_rgb(0x0C, 0x0C, 0x0E));
    p.circle_stroke(center, r * 0.022, Stroke::new(1.0, Color32::from_white_alpha(40)));
}

/// A square picture of side `side`, turned by `angle` around `center`.
pub fn rotated_image(p: &Painter, center: Pos2, side: f32, tex: TextureId, angle: f32, tint: Color32) {
    let (s, c) = angle.sin_cos();
    let h = side / 2.0;
    let rot = |x: f32, y: f32| center + vec2(x * c - y * s, x * s + y * c);
    let mut mesh = egui::Mesh::with_texture(tex);
    for (x, y, u, v) in [(-h, -h, 0.0, 0.0), (h, -h, 1.0, 0.0), (h, h, 1.0, 1.0), (-h, h, 0.0, 1.0)] {
        mesh.vertices.push(egui::epaint::Vertex { pos: rot(x, y), uv: pos2(u, v), color: tint });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
}

/// A wide thumbnail cropped to the square in its middle (video covers on suggestion cards).
pub fn thumb_square(p: &Painter, rect: Rect, tex: Option<TextureId>, radius: f32) {
    match tex {
        Some(t) => {
            let uv = Rect::from_min_max(pos2(0.21875, 0.0), pos2(0.78125, 1.0));
            p.add(egui::epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(t, uv));
        }
        None => {
            p.rect_filled(rect, radius, RAISED_2);
            icons::glyph(p, rect.center(), icons::MUSIC, rect.width() * 0.22, TEXT_3);
        }
    }
}
