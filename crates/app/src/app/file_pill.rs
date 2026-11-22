//! A window's file name as a small pill over its text (`[layout] winbar =
//! "pill"` or `"pill_left"`): the file's icon, its name, and a dot while it
//! has unsaved changes; on hover the Markdown file's preview buttons and
//! "×". It is drawn over the window, not in a row of Neovim's grid, so it
//! has no height of its own to keep in line with. It fades out while the
//! cursor is under it, and then the mouse is Neovim's there.

use super::*;
use super::tabs::{paint_winbar_button, WinbarButton};
use crate::config::WinbarMode;

/// From the window's edges to the pill; on the right, the scroll bar's room too.
const INSET: f32 = 6.0;
const INSET_RIGHT: f32 = 14.0;
/// How much of it is left while faded, and how long the fading takes.
const FADED: f32 = 0.15;
const FADE_SECS: f32 = 0.12;
/// A name's middle above its baseline, in font sizes: capitals reach ~0.7,
/// lowercase ~0.52.
const LETTERS_MIDDLE: f32 = 0.3;
/// The cursor this many cells from the pill (along the row) fades it too.
const REACH_CELLS: f32 = 2.0;

/// The pill of `size` in a window's text `area`: in its first row (centered
/// in it; one taller than the row starts at the area's top), `INSET` from the
/// left or the right edge.
pub(super) fn pill_rect(area: Rect, row_h: f32, size: Vec2, left: bool) -> Rect {
    let y = area.min.y + ((row_h - size.y) / 2.0).max(0.0);
    let x = if left { area.min.x + INSET } else { area.max.x - INSET_RIGHT - size.x };
    Rect::from_min_size(Pos2::new(x, y), size)
}

/// Whether the cursor (its cell) is under the pill or within a couple of
/// cells of it along the row. In a visual selection the selection may run
/// under it from anywhere on the row, so the row's enough.
pub(super) fn pill_faded(pill: Rect, cursor: Option<Rect>, cell_w: f32, visual: bool) -> bool {
    let Some(cell) = cursor else { return false };
    if visual {
        return cell.max.y > pill.min.y && cell.min.y < pill.max.y;
    }
    pill.expand2(Vec2::new(REACH_CELLS * cell_w, 0.0)).intersects(cell)
}

/// What a pill says about its window.
struct Pill {
    name: String,
    /// For the icon (and the Markdown buttons).
    path: std::path::PathBuf,
    modified: bool,
    markdown: bool,
}

impl VimApp {
    /// What window `w`'s pill shows, if it has one: a file's window, or a
    /// side preview ("Preview name").
    fn pill_for(&self, w: &grid::Window) -> Option<Pill> {
        let file_name = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(p) = self.preview.win(w.win).filter(|p| p.side) {
            let path = std::path::PathBuf::from(&p.path);
            return Some(Pill { name: format!("Preview {}", file_name(&path)), path, modified: false, markdown: false });
        }
        let path = self.win_paths.get(&w.win)?.clone();
        let markdown = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        Some(Pill { name: file_name(&path), modified: self.win_modified.contains(&w.win), markdown, path })
    }

    /// Draw window `grid`'s pill, and record where its "×" and buttons are
    /// (`winbar_tabs`, `winbar_buttons`) unless it's faded.
    pub(super) fn paint_file_pill(&mut self, ctx: &egui::Context, painter: &egui::Painter, grid: u64) {
        let mode = self.config.layout.winbar;
        if !mode.is_pill() {
            return;
        }
        let Some(w) = self.editor.windows.get(&grid).filter(|w| w.kind == WindowKind::Normal && !w.hidden).cloned() else { return };
        if self.sidebar_wins.contains(&w.win) {
            return;
        }
        let Some(pill) = self.pill_for(&w) else { return };
        let m = self.renderer.metrics();
        let chrome = Chrome::from_editor(&self.editor);
        let focused = self.editor.cursor.grid == grid;
        // The text area (a winbar of the user's own is above it).
        let mut area = self.window_rect(&w);
        area.min.y += w.margins.top as f32 * m.height;
        area.max.y -= w.margins.bottom as f32 * m.height;
        // Below another window: from the separator's line (the middle of the
        // row above), as close under it as the top window's is under its edge.
        if self.separator_above(&w) {
            area.min.y -= (m.height / 2.0).round();
        }
        let h = crate::config::pill_height(self.config.layout.pill_height, m.height);

        let font = {
            let mut f = self.grid_fonts.sidebar_font_id(false);
            // (A set height brings its text along.)
            f.size = match self.config.layout.pill_height {
                Some(_) => (h * 0.65).round(),
                None => self.status_font_size + 1.0,
            };
            f
        };
        let deco_colors = DecoColors::from_editor(&self.editor);
        let name_color = if self.preview.win(w.win).is_some_and(|p| p.side) {
            chrome.text(focused)
        } else {
            let deco = self.decos.get(&pill.path, false);
            let c = deco_colors.name(&deco).unwrap_or(chrome.fg);
            if focused { c } else { chrome.dim(c) }
        };
        let text_color = chrome.text(focused);
        let pad = style::GAP + 2.0;
        let icon_size = style::file_icon_size(&font).min(h - 2.0);
        let close_w = style::button_size(h);
        let hover = ctx.input(|i| i.pointer.hover_pos());
        let hover_id = egui::Id::unique(("eshvi-file-pill-hover", w.win));
        // Under the pointer as of the last frame: the buttons and "×" make
        // it wider, so the pointer is tested against what it was.
        let was_hovered: bool = ctx.data(|d| d.get_temp(hover_id)).unwrap_or(false);
        let slot = pill.modified || was_hovered;
        let buttons = pill.markdown && was_hovered;
        let btn = Vec2::new((h * 1.1).round(), close_w);
        let buttons_w = if buttons { 2.0 * btn.x + 2.0 + 4.0 } else { 0.0 };
        let slot_w = if slot { style::GAP + close_w } else { 0.0 };
        let fixed = pad + icon_size + style::GAP + buttons_w + slot_w + if slot { pad / 2.0 } else { pad };
        // At most half the window, the name ending in "…" where it doesn't fit.
        let room = (area.width() / 2.0 - fixed).max(font.size * 3.0);
        let mut job = egui::text::LayoutJob::single_section(pill.name.clone(), egui::TextFormat::simple(font.clone(), name_color));
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let name = painter.layout_job(job);
        let size = Vec2::new((fixed + name.size().x).round(), h);
        let rect = pill_rect(area, m.height, size, mode == WinbarMode::PillLeft);

        let cursor = (focused && self.editor.cmdline.is_empty()).then(|| {
            let (row, col) = self.editor.cursor_screen_pos();
            Rect::from_min_size(self.origin + Vec2::new(col as f32 * m.width, row as f32 * m.height), Vec2::new(m.width, m.height))
        });
        let visual = self.editor.mode.starts_with("visual") || self.editor.mode.starts_with("select");
        let dragging = self.drag.is_some() && hover.is_some_and(|p| area.contains(p));
        let faded = dragging || pill_faded(rect, cursor, m.width, visual);
        let fade = ctx.animate_bool_with_time(egui::Id::unique(("eshvi-file-pill-fade", w.win)), faded, FADE_SECS);
        // Not under a float or popup drawn over it.
        let over = |p: Pos2| self.top_window_at(p).is_some_and(|t| t != w.win) || self.over_popup(ctx, p);
        let hovered = !faded && hover.is_some_and(|p| rect.contains(p) && !over(p));
        if hovered != was_hovered {
            ctx.data_mut(|d| d.insert_temp(hover_id, hovered));
            ctx.request_repaint();
        }

        let mut painter = painter.with_clip_rect(area.intersect(painter.clip_rect()).expand2(Vec2::new(INSET_RIGHT, 0.0)));
        painter.set_opacity(1.0 - (1.0 - FADED) * fade);
        painter.rect(rect, style::round(h), chrome.panel, egui::Stroke::new(style::HAIRLINE, chrome.line()), egui::StrokeKind::Inside);
        let icon = Rect::from_center_size(Pos2::new(rect.min.x + pad + icon_size / 2.0, rect.center().y), Vec2::splat(icon_size));
        if focused {
            self.paint_file_icon(&painter, &pill.path, icon);
        } else {
            self.paint_file_icon_mono(&painter, &pill.path, icon, text_color);
        }
        // On the letters' middle, as the icon and buttons are (the line's box
        // has room for descenders: centering it puts the text low). Between
        // the capitals' middle and the lowercase's, as names are mostly lowercase.
        // (`digits_middle` is 0.35 above the baseline.)
        let middle = super::tabs::digits_middle(&name, font.size) + (0.35 - LETTERS_MIDDLE) * font.size;
        let name_pos = Pos2::new(icon.max.x + style::GAP, rect.center().y - middle);
        let name_w = name.size().x;
        painter.galley(name_pos, name, name_color);
        if faded {
            return;
        }
        // Buttons and "×" after the name; the dot where the "×" goes.
        let mut x = name_pos.x + name_w + 4.0;
        for button in [WinbarButton::PreviewSide, WinbarButton::Preview].into_iter().filter(|_| buttons) {
            let r = Rect::from_min_size(Pos2::new(x, rect.center().y - btn.y / 2.0), btn);
            x = r.max.x + 2.0;
            let on = button == WinbarButton::Preview && self.preview.win(w.win).is_some_and(|p| !p.side);
            let lit = style::hovered(&painter, r);
            style::paint_button_fill(&painter, r, &chrome, chrome.panel, lit, on);
            paint_winbar_button(&painter, button, r.center(), (h * 0.5).round(), chrome.text(on || lit));
            self.winbar_buttons.push((r, w.win, button));
        }
        let close = Rect::from_center_size(Pos2::new(rect.max.x - pad / 2.0 - close_w / 2.0, rect.center().y), Vec2::splat(close_w));
        if slot {
            if pill.modified && !hovered {
                painter.circle_filled(close.center(), style::STROKE * 2.5, name_color);
            } else {
                let lit = style::hovered(&painter, close);
                style::glyph_button(&painter, Glyph::Cross, close.center(), h, &chrome, chrome.panel, lit);
            }
        }
        // A press on the name focuses the window; on "×" (or a middle click) closes it.
        self.winbar_tabs.push((rect, if slot { close } else { Rect::NOTHING }, w.win, true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 40.0), Vec2::new(400.0, 300.0))
    }

    #[test]
    fn the_pill_sits_in_the_windows_first_row() {
        let size = Vec2::new(120.0, 19.0);
        let right = pill_rect(area(), 21.0, size, false);
        assert_eq!(right.max.x, 500.0 - INSET_RIGHT);
        assert_eq!(right.size(), size);
        assert!(right.min.y >= 40.0 && right.max.y <= 61.0, "inside the first row: {right:?}");
        let left = pill_rect(area(), 21.0, size, true);
        assert_eq!(left.min.x, 100.0 + INSET);
        assert_eq!(left.min.y, right.min.y);
        assert!(area().contains_rect(left) && area().contains_rect(right));
    }

    #[test]
    fn a_set_height_may_pass_the_row() {
        let tall = Vec2::new(120.0, 28.0);
        let r = pill_rect(area(), 21.0, tall, false);
        assert_eq!((r.min.y, r.height()), (40.0, 28.0));
        // It covers two rows: the cursor on the second fades it too.
        let cell = |y: f32| Some(Rect::from_min_size(Pos2::new(r.min.x + 10.0, y), Vec2::new(8.0, 21.0)));
        assert!(pill_faded(r, cell(40.0), 8.0, false) && pill_faded(r, cell(61.0), 8.0, false));
        assert!(!pill_faded(r, cell(82.0), 8.0, false));
    }

    #[test]
    fn the_cursor_under_the_pill_fades_it() {
        let pill = pill_rect(area(), 21.0, Vec2::new(120.0, 19.0), false);
        let cell = |x: f32, y: f32| Some(Rect::from_min_size(Pos2::new(x, y), Vec2::new(8.0, 21.0)));
        // On it, two cells to its left (reach), and a row below.
        assert!(pill_faded(pill, cell(pill.min.x + 20.0, 40.0), 8.0, false));
        assert!(pill_faded(pill, cell(pill.min.x - 8.0 * 2.0, 40.0), 8.0, false));
        assert!(!pill_faded(pill, cell(pill.min.x - 8.0 * 4.0, 40.0), 8.0, false));
        assert!(!pill_faded(pill, cell(pill.min.x + 20.0, 61.0), 8.0, false));
        assert!(!pill_faded(pill, None, 8.0, false));
        // In a visual selection the row is enough.
        assert!(pill_faded(pill, cell(100.0, 40.0), 8.0, true));
        assert!(!pill_faded(pill, cell(100.0, 61.0), 8.0, true));
    }
}
