//! External command line (`ext_cmdline`) drawn as a floating box at the bottom
//! (above the status bar) or the top (`[cmdline] position`).

use std::sync::Arc;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Galley, Id, Order, Pos2, Rect, Sense, Stroke, Vec2};
use grid::{Cmdline, Editor, StyledChunk};

use crate::config::CmdlinePosition;
use crate::fonts::GridFonts;
use crate::popup::PopupTheme;
use crate::renderer::{rgb, CellMetrics};

/// Where the cmdline text was drawn, so the wildmenu (popupmenu with
/// grid -1) can be anchored under a given byte column.
#[derive(Clone)]
pub struct CmdlineLayout {
    pub rect: Rect,
    /// Whether the box is at the bottom: its menus open above it.
    pub at_bottom: bool,
    text_origin: Pos2,
    galley: Arc<Galley>,
    /// Characters drawn before the content (firstc, prompt, indent).
    prefix_chars: usize,
    content: String,
}

impl CmdlineLayout {
    /// Top-left of the character at screen column `col` of Neovim's own
    /// command line (its row: firstc, prompt, indent, content).
    pub fn pos_of_screen_col(&self, col: usize) -> Pos2 {
        self.text_origin + self.galley.pos_from_cursor(CCursor::new(col)).min.to_vec2()
    }

    /// Top-left of the character at byte offset `col` of the content.
    pub fn pos_of_byte(&self, col: usize) -> Pos2 {
        let col = col.min(self.content.len());
        let chars = self.content.get(..col).map(|s| s.chars().count()).unwrap_or(0);
        self.text_origin + self.galley.pos_from_cursor(CCursor::new(self.prefix_chars + chars)).min.to_vec2()
    }
}

/// Space between the box and the edge it keeps to (the screen's top, or
/// what is below it at the bottom: the status bar).
const EDGE_GAP: f32 = 8.0;

/// Where the box goes and which of its corners sits there: at the editor
/// area's left edge (`EDGE_GAP` in), not centered, so it doesn't move when the
/// side bar or a panel changes the area's width (the user's call). `bottom` is
/// the y of what the box sits on at the bottom (the status bar's top; the
/// screen's bottom when there is no bar). The box keeps its width and grows
/// away from the edge (a command-line block adds lines).
fn anchor(screen: Rect, position: CmdlinePosition, bottom: Option<f32>) -> (Pos2, egui::Align2) {
    let x = screen.min.x + EDGE_GAP;
    match position {
        CmdlinePosition::Top => (Pos2::new(x, screen.min.y + EDGE_GAP), egui::Align2::LEFT_TOP),
        CmdlinePosition::Bottom => {
            let base = bottom.unwrap_or(screen.max.y).min(screen.max.y);
            (Pos2::new(x, base - EDGE_GAP), egui::Align2::LEFT_BOTTOM)
        }
    }
}

/// The box's width in a window of this size.
fn box_width(screen: Rect) -> f32 {
    (screen.width() * 0.6).clamp(320.0_f32.min(screen.width() - 16.0), 760.0)
}

fn chunk_job(job: &mut LayoutJob, editor: &Editor, chunks: &[StyledChunk], base: &TextFormat, fonts: &GridFonts) {
    for chunk in chunks {
        let attr = editor.chunk_attr(&chunk.style);
        let mut fmt = base.clone();
        if let Some(fg) = attr.foreground {
            fmt.color = rgb(fg);
        }
        let (font_id, fake_italic) = fonts.font_id(attr.bold, attr.italic);
        fmt.font_id = font_id;
        fmt.italics = fake_italic;
        job.append(&chunk.text, 0.0, fmt);
    }
}

pub fn show(
    ctx: &egui::Context,
    editor: &Editor,
    fonts: &GridFonts,
    m: CellMetrics,
    screen: Rect,
    focused: bool,
    position: CmdlinePosition,
    bottom: Option<f32>,
) -> Option<CmdlineLayout> {
    let line: &Cmdline = editor.cmdline.last()?;
    let theme = PopupTheme::from_editor(editor);
    let accent = theme.accent;
    let (mono, _) = fonts.font_id(false, false);
    let base = TextFormat { font_id: mono.clone(), color: theme.fg, ..Default::default() };

    let width = box_width(screen);
    let pad = 10.0;

    // Content line: firstc + prompt + indent + content.
    let mut job = LayoutJob::default();
    job.wrap.max_width = width - 2.0 * pad;
    let mut prefix_chars = 0;
    if !line.firstc.is_empty() {
        job.append(&line.firstc, 0.0, TextFormat { color: accent, ..base.clone() });
        prefix_chars += line.firstc.chars().count();
    }
    if !line.prompt.is_empty() {
        job.append(&line.prompt, 0.0, TextFormat { color: theme.dim_fg, ..base.clone() });
        prefix_chars += line.prompt.chars().count();
    }
    if line.indent > 0 {
        job.append(&" ".repeat(line.indent), 0.0, base.clone());
        prefix_chars += line.indent;
    }
    chunk_job(&mut job, editor, &line.content, &base, fonts);
    // Keep a cell of room for the cursor at the end.
    job.append(" ", 0.0, base.clone());

    let mut block_jobs = Vec::new();
    for l in &editor.cmdline_block {
        let mut j = LayoutJob::default();
        j.wrap.max_width = width - 2.0 * pad;
        chunk_job(&mut j, editor, l, &TextFormat { color: theme.dim_fg, ..base.clone() }, fonts);
        block_jobs.push(j);
    }

    let (pos, pivot) = anchor(screen, position, bottom);
    let mut layout = None;
    let rect = egui::Area::new(Id::unique("eshvi-cmdline"))
        .order(Order::Foreground)
        .pivot(pivot)
        .fixed_pos(pos)
        .constrain(true)
        .show(ctx, |ui| {
            theme.frame().inner_margin(egui::Margin::symmetric(pad as i8, 6)).show(ui, |ui| {
                ui.set_width(width - 2.0 * pad);
                for j in block_jobs {
                    ui.label(j);
                }
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(width - 2.0 * pad, galley.size().y.max(m.height)),
                    Sense::hover(),
                );
                let p = ui.painter();
                p.galley(rect.min, galley.clone(), theme.fg);

                // Cursor bar (or the pending special char) at `pos`.
                let content = line.text();
                let col = line.pos.min(content.len());
                let chars = content.get(..col).map(|s| s.chars().count()).unwrap_or(0);
                let cpos = rect.min + galley.pos_from_cursor(CCursor::new(prefix_chars + chars)).min.to_vec2();
                if let Some((c, _)) = &line.special {
                    p.text(cpos, egui::Align2::LEFT_TOP, c, mono.clone(), accent);
                } else if focused {
                    p.rect_filled(Rect::from_min_size(cpos, Vec2::new(2.0, m.height)), 0.0, theme.fg);
                } else {
                    p.rect_stroke(
                        Rect::from_min_size(cpos, Vec2::new(m.width, m.height)),
                        0.0,
                        Stroke::new(crate::style::HAIRLINE, theme.fg),
                        egui::StrokeKind::Inside,
                    );
                }
                layout = Some((rect.min, galley));
            });
        })
        .response
        .rect;
    let (text_origin, galley) = layout?;
    Some(CmdlineLayout { rect, at_bottom: position == CmdlinePosition::Bottom, text_origin, galley, prefix_chars, content: line.text() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 40.0), Vec2::new(1000.0, 700.0))
    }

    /// The box's rect for a given height, as egui places it by the pivot.
    fn rect_of(position: CmdlinePosition, bottom: Option<f32>, height: f32) -> Rect {
        let width = box_width(screen());
        let (pos, pivot) = anchor(screen(), position, bottom);
        pivot.anchor_size(pos, Vec2::new(width, height))
    }

    #[test]
    fn top_keeps_to_the_top_and_left_edges() {
        let r = rect_of(CmdlinePosition::Top, Some(500.0), 30.0);
        assert_eq!(r.min.y, screen().min.y + EDGE_GAP);
        assert_eq!(r.min.x, screen().min.x + EDGE_GAP);
        assert_eq!(r.width(), 600.0);
    }

    #[test]
    fn bottom_sits_above_the_status_bar_or_the_screens_bottom() {
        let r = rect_of(CmdlinePosition::Bottom, Some(700.0), 30.0);
        assert_eq!(r.max.y, 700.0 - EDGE_GAP);
        assert_eq!(r.min.x, screen().min.x + EDGE_GAP, "at the editor's left edge, not centered");
        // No bar: the bottom of the editor area.
        assert_eq!(rect_of(CmdlinePosition::Bottom, None, 30.0).max.y, screen().max.y - EDGE_GAP);
        // A taller box (a block) grows upward, its bottom staying.
        assert_eq!(rect_of(CmdlinePosition::Bottom, Some(700.0), 90.0).max.y, r.max.y);
        // A bar reported below the area doesn't push the box off it.
        assert_eq!(rect_of(CmdlinePosition::Bottom, Some(5000.0), 30.0).max.y, screen().max.y - EDGE_GAP);
    }

    #[test]
    fn width_is_the_same_in_both_positions() {
        assert_eq!(rect_of(CmdlinePosition::Top, None, 30.0).width(), rect_of(CmdlinePosition::Bottom, None, 30.0).width());
    }
}
