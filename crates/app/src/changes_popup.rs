//! The Changes popup, from the agent panel's Changes tab: the files the
//! agents changed since they started (or since "Accept all"), in the UI
//! font, with +/− lines; a click opens a file's diff, "Accept all" counts
//! from here (the changes stay on disk).

use egui::{Align2, Color32, FontId, Id, Order, Pos2, Rect, Sense, Stroke, Vec2};
use grid::Editor;

use crate::changes::Changes;
use crate::decorations::DecoColors;
use crate::popup::{mix, PopupTheme};
use crate::style::{self, Chrome};

pub enum Action {
    /// Show this file's diff against the snapshot.
    Diff(String),
    /// Keep this file's changes.
    Accept(String),
    /// Undo this file's latest turn: the first click asks, the second does.
    Reject(String),
    AcceptAll,
    /// All the changed files in the diff view, stacked.
    ReviewAll,
}

/// Show the popup below `anchor` (the agent panel's tab strip). Returns
/// its rect and what was clicked.
pub fn show(ctx: &egui::Context, editor: &Editor, changes: &Changes, anchor: Rect, font_size: f32) -> (Rect, Option<Action>) {
    let git = changes.current().is_some();
    let since = changes.current().map_or("", |s| s.since.as_str());
    let count = changes.files.len();
    let (added, removed) = changes.totals();
    let theme = PopupTheme::from_editor(editor);
    let chrome = Chrome { fg: theme.fg, ..Chrome::from_editor(editor) };
    let deco = DecoColors::from_editor(editor);
    let screen = ctx.content_rect();
    let width = (anchor.width() - 16.0).clamp(280.0_f32.min(screen.width() - 16.0), 560.0);
    let font = FontId::proportional(font_size);
    let small = crate::style::scaled(&font, crate::style::SMALL);
    let mut action = None;
    let resp = egui::Area::new(Id::unique("eshvi-changes"))
        .order(Order::Foreground)
        .fixed_pos(Pos2::new((anchor.max.x - 8.0 - width).max(screen.min.x + 8.0), anchor.max.y + 4.0))
        .show(ctx, |ui| {
            theme.frame().inner_margin(style::DIALOG_PAD).show(ui, |ui| {
                ui.set_width(width);
                let painter = ui.painter().clone();
                // Header: how much changed since when, and the totals.
                let title = if !git {
                    "Not a git repository".to_owned()
                } else if changes.files.is_empty() {
                    "No changes".to_owned()
                } else {
                    format!("{count} file{} changed", if count == 1 { "" } else { "s" })
                };
                let (rect, _) = ui.allocate_exact_size(Vec2::new(width, font_size * 1.5), Sense::hover());
                let title = painter.layout_no_wrap(title, FontId::proportional(font_size + 1.0), theme.fg);
                let y = rect.center().y - title.size().y / 2.0;
                let title_w = title.size().x;
                painter.galley(Pos2::new(rect.min.x, y), title, theme.fg);
                let since = if git { format!("since {since}") } else { "changes are tracked with git".into() };
                let since = painter.layout_no_wrap(since, small.clone(), theme.dim_fg);
                painter.galley(Pos2::new(rect.min.x + title_w + 8.0, rect.center().y - since.size().y / 2.0), since, theme.dim_fg);
                if !changes.files.is_empty() {
                    counts(&painter, rect.right_center() - Vec2::new(8.0, 0.0), added, removed, &font, &deco, false);
                }
                ui.add_space(6.0);

                if !changes.files.is_empty() {
                    let line = ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover()).0;
                    painter.hline(line.x_range(), line.center().y, Stroke::new(style::HAIRLINE, theme.border));
                    ui.add_space(4.0);
                    let row_h = (font_size * 1.9).round();
                    egui::ScrollArea::vertical().max_height(screen.height() * 0.55).show(ui, |ui| {
                        for f in &changes.files {
                            let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, row_h), Sense::click());
                            let confirming = changes.confirm_reject.as_deref() == Some(f.path.as_str());
                            let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
                            let painter = ui.painter().clone();
                            let painter = &painter;
                            if hovered {
                                painter.rect_filled(rect, style::RADIUS, theme.sel_bg);
                                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            // Status badge.
                            let color = match f.status {
                                'A' => deco.added,
                                'D' => deco.deleted,
                                _ => deco.modified,
                            };
                            let badge = Rect::from_center_size(Pos2::new(rect.min.x + 6.0 + 9.0, rect.center().y), Vec2::splat(18.0));
                            painter.rect_filled(badge, style::RADIUS, mix(theme.bg, color, style::WASH));
                            painter.text(badge.center(), Align2::CENTER_CENTER, f.status.to_string(), small.clone(), color);
                            // File name, then its folder, dimmed.
                            let (dir, name) = match f.path.rsplit_once('/') {
                                Some((dir, name)) => (dir, name),
                                None => ("", f.path.as_str()),
                            };
                            let name_color = if f.status == 'D' { theme.dim_fg } else { theme.fg };
                            let x = badge.max.x + 10.0;
                            let name_g = painter.layout_no_wrap(name.to_owned(), font.clone(), name_color);
                            let name_w = name_g.size().x;
                            painter.galley(Pos2::new(x, rect.center().y - name_g.size().y / 2.0), name_g, name_color);
                            // Hovered (or asking to confirm): "Accept" and
                            // "Reject" in place of the counts.
                            let mut on_button = false;
                            let counts_w = if hovered || confirming {
                                let mut right = rect.max.x - 6.0;
                                let mut button = |label: &str, color: Color32, fill: &dyn Fn(bool) -> Color32, id: (&str, &str)| -> (f32, bool) {
                                    let g = painter.layout_no_wrap(label.to_owned(), small.clone(), color);
                                    let r = Rect::from_min_max(
                                        Pos2::new(right - g.size().x - 16.0, rect.min.y + 4.0),
                                        Pos2::new(right, rect.max.y - 4.0),
                                    );
                                    let b = ui.interact(r, Id::unique(id), Sense::click());
                                    let fill = fill(b.hovered());
                                    painter.rect_filled(r, style::RADIUS, fill);
                                    painter.galley(r.center() - g.size() / 2.0, g, color);
                                    on_button |= b.hovered();
                                    right = r.min.x - 6.0;
                                    (rect.max.x - r.min.x, b.clicked())
                                };
                                let (_, reject) = if confirming {
                                    button("Undo latest changes?", Color32::WHITE, &|h| style::primary_fill(deco.deleted, h), ("eshvi-changes-reject", f.path.as_str()))
                                } else {
                                    button("Reject", theme.fg, &|h| style::text_button_fill(&chrome, theme.bg, h), ("eshvi-changes-reject", f.path.as_str()))
                                };
                                let (w, accept) = button("Accept", theme.fg, &|h| style::text_button_fill(&chrome, theme.bg, h), ("eshvi-changes-accept", f.path.as_str()));
                                if reject {
                                    action = Some(Action::Reject(f.path.clone()));
                                } else if accept {
                                    action = Some(Action::Accept(f.path.clone()));
                                }
                                w
                            } else {
                                counts(painter, rect.right_center() - Vec2::new(8.0, 0.0), f.added, f.removed, &font, &deco, f.binary)
                            };
                            if !dir.is_empty() {
                                let room = (rect.max.x - 8.0 - counts_w - 12.0) - (x + name_w + 8.0);
                                let mut job = egui::text::LayoutJob::single_section(
                                    dir.to_owned(),
                                    egui::TextFormat::simple(small.clone(), theme.dim_fg),
                                );
                                job.wrap = egui::text::TextWrapping::truncate_at_width(room.max(0.0));
                                let g = painter.layout_job(job);
                                painter.galley(Pos2::new(x + name_w + 8.0, rect.center().y - g.size().y / 2.0), g, theme.dim_fg);
                            }
                            if resp.clicked() && !on_button && action.is_none() {
                                action = Some(Action::Diff(f.path.clone()));
                            }
                        }
                    });
                    ui.add_space(8.0);
                    // Footer: a hint, and "Accept all".
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, font_size * 2.2), Sense::hover());
                    let label = ui.painter().layout_no_wrap("Accept all".into(), font.clone(), Color32::WHITE);
                    let button = Rect::from_min_max(
                        Pos2::new(rect.max.x - label.size().x - 28.0, rect.min.y),
                        rect.max,
                    );
                    let resp = ui.interact(button, Id::unique("eshvi-changes-accept"), Sense::click());
                    let fill = style::primary_fill(theme.accent, resp.hovered());
                    ui.painter().rect_filled(button, style::RADIUS, fill);
                    ui.painter().galley(button.center() - label.size() / 2.0, label, Color32::WHITE);
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        action = Some(Action::AcceptAll);
                    }
                    // "Review All" beside it: every file, stacked, in the diff view.
                    let review = ui.painter().layout_no_wrap("Review All".into(), font.clone(), theme.fg);
                    let review_button = Rect::from_min_max(
                        Pos2::new(button.min.x - review.size().x - 28.0 - 6.0, rect.min.y),
                        Pos2::new(button.min.x - 6.0, rect.max.y),
                    );
                    let resp = ui.interact(review_button, Id::unique("eshvi-changes-review"), Sense::click());
                    ui.painter().rect_filled(review_button, style::RADIUS, style::text_button_fill(&chrome, theme.bg, resp.hovered()));
                    ui.painter().galley(review_button.center() - review.size() / 2.0, review, theme.fg);
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        action = Some(Action::ReviewAll);
                    }
                    let hint = ui.painter().layout_no_wrap("Click a file to see its diff".into(), small.clone(), theme.dim_fg);
                    ui.painter().galley(Pos2::new(rect.min.x, rect.center().y - hint.size().y / 2.0), hint, theme.dim_fg);
                }
            });
        })
        .response;
    (resp.rect, action)
}

/// "+12 −3" ending at `right` (or "binary"); returns its width.
fn counts(painter: &egui::Painter, right: Pos2, added: usize, removed: usize, font: &FontId, deco: &DecoColors, binary: bool) -> f32 {
    if binary {
        let g = painter.layout_no_wrap("binary".into(), font.clone(), deco.modified);
        let w = g.size().x;
        painter.galley(Pos2::new(right.x - w, right.y - g.size().y / 2.0), g, deco.modified);
        return w;
    }
    let minus = painter.layout_no_wrap(format!("−{removed}"), font.clone(), deco.deleted);
    let plus = painter.layout_no_wrap(format!("+{added}"), font.clone(), deco.added);
    let (mw, pw) = (minus.size().x, plus.size().x);
    painter.galley(Pos2::new(right.x - mw, right.y - minus.size().y / 2.0), minus, deco.deleted);
    painter.galley(Pos2::new(right.x - mw - 8.0 - pw, right.y - plus.size().y / 2.0), plus, deco.added);
    mw + 8.0 + pw
}
