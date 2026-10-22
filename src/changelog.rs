//! "What's new": CHANGELOG.md, built into the app. Shown once after an update, and in Settings.

use crate::app::App;
use crate::icons;
use crate::theme::*;
use crate::update::{is_newer, VERSION};
use crate::widgets::{self as w, PillKind};
use egui::{pos2, vec2, Align2, Color32, Id, Sense};

const CHANGELOG: &str = include_str!("../CHANGELOG.md");

pub struct Release {
    pub version: String,
    /// ("New" / "Better" / "Fixed", lines)
    pub sections: Vec<(String, Vec<String>)>,
}

/// Every version in the changelog, newest first.
pub fn releases() -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    let mut in_comment = false;
    for line in CHANGELOG.lines() {
        let l = line.trim();
        if l.starts_with("<!--") {
            in_comment = !l.ends_with("-->");
            continue;
        }
        if in_comment {
            in_comment = !l.ends_with("-->");
            continue;
        }
        if let Some(v) = l.strip_prefix("## ") {
            out.push(Release { version: v.trim().to_string(), sections: Vec::new() });
        } else if let Some(s) = l.strip_prefix("### ") {
            if let Some(r) = out.last_mut() {
                r.sections.push((s.trim().to_string(), Vec::new()));
            }
        } else if let Some(item) = l.strip_prefix("- ") {
            if let Some(r) = out.last_mut() {
                if r.sections.is_empty() {
                    r.sections.push((String::new(), Vec::new()));
                }
                if let Some(sec) = r.sections.last_mut() {
                    sec.1.push(item.replace("**", "").trim().to_string());
                }
            }
        }
    }
    out
}

/// What the "What's new" window shows.
#[derive(Clone, PartialEq)]
pub enum WhatsNew {
    /// Right after an update: the versions newer than the one you had.
    Since(String),
    /// From Settings: everything.
    All,
}

fn section_color(name: &str) -> Color32 {
    match name.to_lowercase().as_str() {
        "new" => ACCENT,
        "fixed" => Color32::from_rgb(0x5A, 0xD1, 0x9A),
        _ => Color32::from_rgb(0x7A, 0xA7, 0xFF),
    }
}

impl App {
    pub(crate) fn whats_new_window(&mut self, ctx: &egui::Context) {
        let Some(which) = self.whats_new.clone() else { return };
        let list: Vec<Release> = releases()
            .into_iter()
            .filter(|r| match &which {
                WhatsNew::Since(old) => is_newer(&r.version, old) && !is_newer(&r.version, VERSION),
                WhatsNew::All => true,
            })
            .collect();
        if list.is_empty() {
            self.whats_new = None;
            return;
        }
        let screen = ctx.content_rect();
        let max_h = (screen.height() * 0.72).clamp(260.0, 620.0);
        let mut close = false;
        let modal = egui::Modal::new(Id::new("whats-new"))
            .frame(egui::Frame::new().fill(OVERLAY).corner_radius(R_LG).inner_margin(S6).stroke(egui::Stroke::new(1.0, HAIRLINE)))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                // Header
                let (hr, _) = ui.allocate_exact_size(vec2(440.0, 52.0), Sense::hover());
                let p = ui.painter();
                let c = pos2(hr.left() + 20.0, hr.center().y);
                p.circle_filled(c, 20.0, with_alpha(ACCENT, 0.16));
                icons::glyph(p, c, icons::SPARKLES, 20.0, ACCENT);
                let title = match &which {
                    WhatsNew::Since(_) => format!("Segno {VERSION} is here"),
                    WhatsNew::All => "What's new".to_string(),
                };
                let sub = match &which {
                    WhatsNew::Since(_) => "Here's what changed.",
                    WhatsNew::All => "Everything that changed, newest first.",
                };
                w::text(p, pos2(hr.left() + 54.0, hr.center().y - 10.0), Align2::LEFT_CENTER, &title, h2(), TEXT, 380.0);
                w::text(p, pos2(hr.left() + 54.0, hr.center().y + 12.0), Align2::LEFT_CENTER, sub, small(), TEXT_2, 380.0);
                ui.add_space(S4);
                egui::ScrollArea::vertical().max_height(max_h - 140.0).auto_shrink([false, true]).show(ui, |ui| {
                    // Leave room for the scroll bar so it never sits on the text.
                    ui.set_width(ui.available_width() - 14.0);
                    let many = list.len() > 1;
                    for (i, r) in list.iter().enumerate() {
                        if many {
                            if i > 0 {
                                ui.add_space(S4);
                            }
                            ui.label(egui::RichText::new(format!("Segno {}", r.version)).font(body_strong()).color(TEXT));
                            ui.add_space(S1);
                        }
                        for (name, lines) in &r.sections {
                            if !name.is_empty() {
                                ui.add_space(S2);
                                let col = section_color(name);
                                let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
                                let tw = w::text_width(ui, name, small_strong()) + 16.0;
                                let pill = egui::Rect::from_min_size(lr.min, vec2(tw, 18.0));
                                ui.painter().rect_filled(pill, 9.0, with_alpha(col, 0.16));
                                w::text(ui.painter(), pill.center(), Align2::CENTER_CENTER, name, small_strong(), col, tw);
                                ui.add_space(S1);
                            }
                            for line in lines {
                                ui.horizontal_top(|ui| {
                                    ui.add_space(S1);
                                    let (dr, _) = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover());
                                    ui.painter().circle_filled(pos2(dr.center().x, dr.top() + 10.0), 2.5, TEXT_3);
                                    ui.add(egui::Label::new(egui::RichText::new(line).font(body()).color(TEXT_2)).wrap());
                                });
                                ui.add_space(2.0);
                            }
                        }
                    }
                });
                ui.add_space(S5);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w::pill(ui, "Got it", None, PillKind::Primary).clicked() {
                        close = true;
                    }
                });
            });
        if close || modal.should_close() {
            self.whats_new = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_has_this_version_first() {
        let all = releases();
        assert_eq!(all[0].version, VERSION, "add a '## {VERSION}' section at the top of CHANGELOG.md");
        assert!(all.iter().all(|r| !r.sections.is_empty()));
        // Newest first.
        for pair in all.windows(2) {
            assert!(is_newer(&pair[0].version, &pair[1].version));
        }
    }
}
