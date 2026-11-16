//! Grid rendering. `GridRenderer` hides the text backend so the egui
//! painter implementation can later be swapped for cosmic-text + glyphon.

use std::collections::HashMap;
use std::sync::Arc;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Color32, Galley, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use grid::{Cell, CursorShape, Editor, Underline};

use crate::fonts::GridFonts;
use crate::images::{self, Images, Tile};
use crate::popup::{self, FloatPalette};

pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A background with a highlight's `blend` (0..=100, 'winblend' and
/// 'pumblend'): Neovim leaves blending floats over what's below to the UI.
/// 100 is fully see-through.
pub fn blended(color: Color32, blend: u8) -> Color32 {
    if blend == 0 {
        return color;
    }
    let alpha = (255.0 * (100 - blend.min(100)) as f32 / 100.0).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// `top` (premultiplied, maybe see-through) painted over opaque `bottom`.
pub fn over(top: Color32, bottom: Color32) -> Color32 {
    let keep = 1.0 - top.a() as f32 / 255.0;
    let c = |t: u8, b: u8| (t as f32 + b as f32 * keep).round().min(255.0) as u8;
    Color32::from_rgb(c(top.r(), bottom.r()), c(top.g(), bottom.g()), c(top.b(), bottom.b()))
}

/// The color a see-through float (a backdrop) lays over what's below it.
pub fn see_through_color(editor: &Editor, grid: &grid::Grid) -> Option<Color32> {
    let attr = editor.hl_attr(grid.cell(0, 0)?.hl_id);
    (attr.blend > 0).then(|| blended(rgb(attr.colors(&editor.default_colors).bg), attr.blend))
}

/// Whether float `grid` is see-through as a whole (a 'winblend' backdrop,
/// like lazy.nvim's): its top-left cell blends.
pub fn is_see_through(editor: &Editor, grid: &grid::Grid) -> bool {
    grid.cell(0, 0).is_some_and(|c| editor.hl_attr(c.hl_id).blend > 0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
}

/// How a grid layer is decorated.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerStyle {
    /// Corner radius in points; 0 for normal windows.
    pub corner_radius: f32,
    /// Draw a drop shadow under the layer.
    pub shadow: bool,
    /// Lay rows out in the proportional sidebar font instead of the cell
    /// grid (e.g. NvimTree). Rows still match Neovim's rows.
    pub proportional: bool,
    /// A floating window: its border is drawn natively, and the layer takes
    /// the background of its content.
    pub float: bool,
    /// How far (points, left and right) a float's border rules reach past
    /// the layer, to meet the frame of the float it lies in.
    pub rule_extend: (f32, f32),
    /// The color a float covers, for its border cells outside the frame:
    /// the editor's background, dimmed by any backdrop below.
    pub under: Option<Color32>,
}

impl LayerStyle {
    pub const SIDEBAR: Self = Self {
        corner_radius: 0.0,
        shadow: false,
        proportional: true,
        float: false,
        rule_extend: (0.0, 0.0),
        under: None,
    };
}

/// Whether each character of a one-line galley starts on its cell, as
/// monospace text without width-changing ligatures does. (Within a point:
/// glyph positions are rounded to pixels, so at 1x they drift up to a
/// pixel from fractional cells anyway.)
/// (Shaping keeps one glyph per character; a ligature's other characters
/// get zero-width glyphs.)
pub(crate) fn on_cells(galley: &Galley, cw: f32) -> bool {
    let Some(row) = galley.rows.first() else { return true };
    let mut x = None;
    for (i, g) in row.row.glyphs.iter().enumerate() {
        // Zero-width glyphs inside a ligature sit at its end; they draw nothing.
        if g.advance_width == 0.0 {
            continue;
        }
        let start = *x.get_or_insert(g.pos.x);
        if (g.pos.x - start - i as f32 * cw).abs() > 1.0 {
            return false;
        }
    }
    true
}

/// Text drawn for a cell. Symbols no installed text font has would come
/// from egui's emoji font, framed like buttons; a plain look-alike is drawn
/// instead (Claude Code marks its replies with ⏺).
pub(crate) fn text_glyph(text: &str) -> &str {
    match text {
        "⏺" => "●",
        _ => text,
    }
}

/// Left padding of proportional (sidebar) rows, in points.
const SIDEBAR_PAD: f32 = 8.0;

/// Look of a GUI statusline.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusLook {
    /// Fixed bar colors `(bg, fg)`; `None` keeps the statusline's own
    /// highlights (theme colors).
    pub uniform: Option<(Color32, Color32)>,
    /// Text size in points.
    pub font_size: f32,
    /// Pill color behind the mode name ("NORMAL", "INSERT"...), if any.
    pub mode_color: Option<Color32>,
    /// 1px line along the bar's top edge.
    pub top_border: Option<Color32>,
    /// Filetype of the current buffer: the icon glyph right before this
    /// word (e.g. lualine's filetype component) is located so the GUI can
    /// draw its own file icon there.
    pub filetype: Option<String>,
}

/// Index of an icon glyph (Nerd Font / private use, not a powerline
/// separator) followed by optional spaces and then `word`.
fn icon_before_word(cells: &[Cell], word: &str) -> Option<usize> {
    if word.is_empty() {
        return None;
    }
    (0..cells.len()).find(|&i| {
        let t = &cells[i].text;
        let pua = t.chars().next().is_some_and(|c| (0xE000..=0xF8FF).contains(&(c as u32)) || c as u32 >= 0xF0000);
        if !pua || is_powerline_separator(t) {
            return false;
        }
        let rest: String = cells[i + 1..].iter().map(|c| c.text.as_str()).collect();
        rest.trim_start().starts_with(word)
    })
}

/// Whether text looks like a statusline mode name: one short upper-case
/// word, possibly with dashes ("NORMAL", "V-LINE", "O-PENDING").
fn is_mode_label(text: &str) -> bool {
    let t = text.trim();
    (2..=12).contains(&t.len()) && t.chars().all(|c| c.is_ascii_uppercase() || c == '-')
}

/// Positions in a painted bar, in screen coordinates.
#[derive(Clone, Copy, Debug, Default)]
pub struct BarInfo {
    /// The file-type icon glyph (see `StatusLook::filetype`).
    pub icon_slot: Option<Rect>,
    /// Where the left part's text ends (the free gap starts).
    pub left_end: f32,
}

/// A laid-out bar with what was found in it.
struct BarLayout {
    look: StatusLook,
    layout: RowLayout,
    pill: Option<ModePill>,
    /// x range of the file-type icon glyph.
    icon: Option<(f32, f32)>,
}

/// Where to draw the mode pill: x range, and the vertical center of the
/// text's capital letters relative to the top of the first run.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ModePill {
    x0: f32,
    x1: f32,
    caps_center: f32,
}

/// Vertical center of capital letters in a galley (baseline minus half the
/// cap height), so a pill behind caps-only text looks centered.
fn caps_center(galley: &Galley, font_size: f32) -> f32 {
    galley
        .rows
        .iter()
        .flat_map(|r| r.row.glyphs.iter().map(move |g| r.pos.y + g.pos.y))
        .next()
        .map_or(galley.size().y / 2.0, |baseline| baseline - 0.35 * font_size)
}

/// x range of the first text run of a laid-out statusline if it is a mode
/// name, for drawing a pill behind it.
fn mode_span(cells: &[Cell], col_x: &[f32]) -> Option<(f32, f32)> {
    let start = cells.iter().position(|c| !c.text.trim().is_empty())?;
    let hl = cells[start].hl_id;
    let mut end = start;
    while end < cells.len() && cells[end].hl_id == hl && !cells[end].text.trim().is_empty() {
        end += 1;
    }
    let text: String = cells[start..end].iter().map(|c| c.text.as_str()).collect();
    is_mode_label(&text).then(|| (col_x[start], col_x[end]))
}

/// Options for proportional layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PropStyle {
    font_size: Option<f32>,
    /// Draw all text in one color and no backgrounds.
    uniform_fg: Option<Color32>,
    /// Extra space before text whose highlight differs from the previous
    /// text (separates statusline sections once colors are uniform).
    section_gap: f32,
}

/// Cursor `(fg, bg)`: the mode's cursor highlight if it has colors, else
/// the `Cursor` group (what GUI Vims use), else the default colors swapped.
/// Swapping the cell's own colors (Neovim's suggestion for `attr_id` 0) can
/// make the cursor vanish on highlighted text such as a search match.
fn cursor_colors(editor: &Editor, attr_id: u64) -> (u32, u32) {
    let d = &editor.default_colors;
    let has_colors = |a: &grid::HlAttr| a.foreground.is_some() || a.background.is_some();
    let attr = Some(editor.hl_attr(attr_id))
        .filter(|a| attr_id != 0 && has_colors(a))
        .or_else(|| editor.hl_groups.get("Cursor").map(|id| editor.hl_attr(*id)).filter(|a| has_colors(a)));
    match attr {
        Some(a) => {
            let c = a.colors(d);
            (c.fg, c.bg)
        }
        None => (d.bg, d.fg),
    }
}

/// Powerline separator glyphs (arrows, slants). Meaningless on a
/// single-color bar.
fn is_powerline_separator(text: &str) -> bool {
    text.chars().next().is_some_and(|c| (0xE0B0..=0xE0D7).contains(&(c as u32)))
}

const BORDER_CHARS: &str = "─│╭╮╰╯┌┐└┘═║╔╗╚╝━┃┏┓┗┛";
const HORIZONTAL_CHARS: &str = "─═━";

fn is_border(text: &str) -> bool {
    is_one_of(text, BORDER_CHARS)
}

fn is_one_of(text: &str, chars: &str) -> bool {
    let mut it = text.chars();
    it.next().is_some_and(|c| chars.contains(c)) && it.next().is_none()
}

/// A float's border drawn with box-drawing characters (or blank cells).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Border {
    border_hl: u64,
    /// Highlight of the content, used for the hidden border cells.
    inner_hl: u64,
    kind: BorderKind,
    /// The edge is blank cells in the border's highlight (Neovim's `solid`
    /// border), not line glyphs.
    blank: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BorderKind {
    /// All four sides: drawn as an outline along the layer's edge.
    Frame,
    /// Only a top and/or bottom edge (snacks' picker input and preview):
    /// drawn as a rule through the middle of that row.
    Rules { top: bool, bottom: bool },
    /// Neovim's `shadow` border: a see-through (blended) right column and
    /// bottom row. Left undrawn; the float gets the GUI's own shadow.
    Shadow,
}

/// Detect a float's border: a full box (all corners and both side edges)
/// of line glyphs; a shadow (`blended` tells which highlights are
/// see-through); a box of blank cells in a highlight of its own (`solid`);
/// else top/bottom rows that are all horizontal lines but for one title.
fn detect_border(grid: &grid::Grid, blended: &dyn Fn(u64) -> bool) -> Option<Border> {
    let (w, h) = (grid.width(), grid.height());
    if w < 3 || h < 2 {
        return None;
    }
    let border_at = |r: usize, c: usize| grid.cell(r, c).is_some_and(|cell| is_border(&cell.text));
    let corners = [(0, 0), (0, w - 1), (h - 1, 0), (h - 1, w - 1)];
    let sides = |at: &dyn Fn(usize, usize) -> bool| {
        h >= 3 && corners.iter().all(|&(r, c)| at(r, c)) && (1..h - 1).all(|r| at(r, 0) && at(r, w - 1))
    };
    if sides(&border_at) {
        let (border_hl, inner_hl) = (grid.cell(0, 0)?.hl_id, grid.cell(1, 1)?.hl_id);
        return Some(Border { border_hl, inner_hl, kind: BorderKind::Frame, blank: false });
    }
    let blank_at = |r: usize, c: usize, hl: Option<u64>| {
        grid.cell(r, c).is_some_and(|cell| cell.text.trim().is_empty() && hl.is_none_or(|hl| cell.hl_id == hl))
    };
    let see_through = |r: usize, c: usize| blank_at(r, c, None) && grid.cell(r, c).is_some_and(|cell| blended(cell.hl_id));
    // (Not a float that is see-through as a whole, 'winblend'.)
    let solid_content = grid.cell(0, 0).is_some_and(|cell| !blended(cell.hl_id));
    if solid_content && (0..h).all(|r| see_through(r, w - 1)) && (0..w).all(|c| see_through(h - 1, c)) {
        let (border_hl, inner_hl) = (grid.cell(h - 1, w - 1)?.hl_id, grid.cell(0, 0)?.hl_id);
        return Some(Border { border_hl, inner_hl, kind: BorderKind::Shadow, blank: true });
    }
    let (border_hl, inner_hl) = (grid.cell(0, 0)?.hl_id, grid.cell(h.min(2) - 1, 1)?.hl_id);
    let solid = Border { border_hl, inner_hl, kind: BorderKind::Frame, blank: true };
    // A border row: all border, or with a title on it, which never spans
    // all the way from one corner to the other. A row of content between
    // side borders alone (blink.cmp's documentation: `border = { " ", "",
    // "", " ", "", "", " ", " " }`, padding on the left and right) does.
    let edge_row = |r: usize| {
        let cells = grid.row(r);
        let changes = cells.windows(2).filter(|p| solid.is_edge(&p[0]) != solid.is_edge(&p[1])).count();
        changes == 0 || (changes == 2 && (solid.is_edge(&cells[1]) || solid.is_edge(&cells[w - 2])))
    };
    if inner_hl != border_hl && sides(&|r, c| blank_at(r, c, Some(border_hl))) && edge_row(0) && edge_row(h - 1) {
        return Some(solid);
    }
    let (top, bottom) = (is_rule_row(grid, 0), is_rule_row(grid, h - 1));
    let rule_row = match (top, bottom) {
        (true, _) => 0,
        (false, true) => h - 1,
        (false, false) => return None,
    };
    // A float that is nothing but rules has no content to match.
    let content_row = (0..h).find(|&r| !(r == 0 && top || r == h - 1 && bottom))?;
    Some(Border {
        border_hl: grid.cell(rule_row, 0)?.hl_id,
        inner_hl: grid.cell(content_row, 0)?.hl_id,
        kind: BorderKind::Rules { top, bottom },
        blank: false,
    })
}

/// Text in a plugin's float is kept readable: a color too close to the
/// background it ends up on (snacks' picker paths in `NonText`) is moved
/// toward the normal text color until it reaches `popup::MIN_CONTRAST`, so it
/// stays dimmer than normal text. The editor's own text is left alone.
struct ContrastFloor {
    /// The float's base color: the background of cells without one.
    base: Color32,
    palette: Option<FloatPalette>,
    fg: Color32,
}

impl ContrastFloor {
    fn apply(&self, fg: Color32, cell_bg: Option<Color32>) -> Color32 {
        let bg = cell_bg.map_or(self.base, |c| self.palette.map_or(c, |p| p.map_bg(c)));
        popup::with_contrast(fg, bg, self.fg, popup::MIN_CONTRAST)
    }
}

/// An input field's text is set in this far from its edge.
const FIELD_INSET: f32 = 6.0;
const FIELD_RADIUS: f32 = 4.0;

/// Space between a frame's header hairline and the first content row.
const HEADER_GAP: f32 = 3.0;

/// Padding between a frame and the content inside it: about the GUI
/// popups' own, within the border cell it replaces.
pub fn frame_margin(m: CellMetrics) -> Vec2 {
    Vec2::new(6.0f32.min(m.width), 6.0f32.min(m.height / 2.0))
}

/// Whether `grid` (a float) has a border of any kind.
pub fn has_border(grid: &grid::Grid) -> bool {
    detect_border(grid, &|_| false).is_some()
}

/// Whether `grid` (a float) has a full box border, drawn as a frame.
pub fn has_frame(grid: &grid::Grid) -> bool {
    detect_border(grid, &|_| false).is_some_and(|b| b.kind == BorderKind::Frame)
}

/// A row of horizontal line glyphs, from edge to edge, except for one run of
/// other text (a title).
fn is_rule_row(grid: &grid::Grid, row: usize) -> bool {
    let line = |c: &Cell| is_one_of(&c.text, HORIZONTAL_CHARS);
    let cells = grid.row(row);
    let (Some(first), Some(last)) = (cells.first(), cells.last()) else { return false };
    if !line(first) || !line(last) {
        return false;
    }
    // Line, then at most one stretch of text, then line again.
    let changes = cells.windows(2).filter(|p| line(&p[0]) != line(&p[1])).count();
    changes == 0 || changes == 2
}

impl Border {
    /// The rows drawn as border: the outline's top and bottom, or the rules.
    fn edge_rows(&self, height: usize) -> Vec<usize> {
        let last = height - 1;
        match self.kind {
            BorderKind::Frame => vec![0, last],
            BorderKind::Rules { top, bottom } => {
                [(top, 0), (bottom, last)].into_iter().filter_map(|(on, r)| on.then_some(r)).collect()
            }
            BorderKind::Shadow => Vec::new(),
        }
    }

    /// A cell of the edge itself: a line glyph, or for a blank border a
    /// blank cell in the border's highlight (not a title's padding).
    fn is_edge(&self, cell: &Cell) -> bool {
        if self.blank {
            cell.text.trim().is_empty() && cell.hl_id == self.border_hl
        } else {
            is_border(&cell.text)
        }
    }

    /// The columns `[start, end)` of text on a top/bottom border row (a
    /// float's title or footer), if any.
    fn label_span(&self, grid: &grid::Grid, row: usize) -> Option<(usize, usize)> {
        let w = grid.width();
        let cols: Vec<usize> = (1..w - 1).filter(|&c| grid.cell(row, c).is_some_and(|cell| !self.is_edge(cell))).collect();
        Some((*cols.first()?, cols.last()? + 1))
    }

    /// The label's columns without the blank cells padding it (`" Files "`).
    fn trimmed_label(&self, grid: &grid::Grid, row: usize, (c0, c1): (usize, usize)) -> Option<(usize, usize)> {
        let cells = &grid.row(row)[c0..c1];
        let blank = |c: &Cell| c.text.trim().is_empty();
        let start = cells.iter().position(|c| !blank(c))?;
        let end = cells.iter().rposition(|c| !blank(c))? + 1;
        Some((c0 + start, c0 + end))
    }

    /// Whether the title row is drawn as a header strip inside the frame
    /// (only a full frame's top row).
    fn is_header(&self, row: usize) -> bool {
        self.kind == BorderKind::Frame && row == 0
    }

    /// The column a header title starts at, lined up with the content: the
    /// first column with text on any row inside the frame (which-key indents
    /// its keys by one), at most a few columns in; 1 for an empty float
    /// (snacks' box, its input and list are floats of their own).
    fn content_col(&self, grid: &grid::Grid) -> usize {
        let (w, h) = (grid.width(), grid.height());
        let first_text = |r: usize| (1..w - 1).find(|&c| grid.cell(r, c).is_some_and(|cell| !cell.text.trim().is_empty()));
        (1..h - 1).filter_map(first_text).min().unwrap_or(1).min(4)
    }

    /// A row with border glyphs replaced by blank content cells. Other text
    /// on the border (titles) is kept.
    fn mask_row(&self, grid: &grid::Grid, row: usize) -> Vec<Cell> {
        let edge_row = self.edge_rows(grid.height()).contains(&row);
        let frame = self.kind == BorderKind::Frame;
        let last = grid.width() - 1;
        grid.row(row)
            .iter()
            .enumerate()
            .map(|(col, cell)| {
                if (edge_row || frame && (col == 0 || col == last)) && self.is_edge(cell) {
                    Cell { text: " ".into(), hl_id: self.inner_hl }
                } else {
                    cell.clone()
                }
            })
            .collect()
    }
}

/// The most frequent item (the first of equally frequent ones).
fn dominant<T: PartialEq + Copy>(items: impl Iterator<Item = T>) -> Option<T> {
    let mut counts: Vec<(T, usize)> = Vec::new();
    for item in items {
        match counts.iter_mut().find(|(t, _)| *t == item) {
            Some((_, n)) => *n += 1,
            None => counts.push((item, 1)),
        }
    }
    counts.iter().fold(None, |best: Option<(T, usize)>, &(t, n)| match best {
        Some((_, bn)) if bn >= n => best,
        _ => Some((t, n)),
    })
    .map(|(t, _)| t)
}

/// A label on a border line hugs the right end when it was placed there
/// (a footer's hints); otherwise it starts at the left, like a section label.
fn label_at_right(width: usize, (c0, c1): (usize, usize)) -> bool {
    let (before, after) = (c0.saturating_sub(1), width.saturating_sub(c1 + 1));
    after <= 2 && before > after
}

/// The longest run (at least 2) of blank cells with one highlight that is
/// neither at the start nor the end: a statusline's `%=` filler.
fn find_filler(cells: &[Cell]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let mut col = 0;
    while col < cells.len() {
        let start = col;
        while col < cells.len() && cells[col].text == " " && cells[col].hl_id == cells[start].hl_id {
            col += 1;
        }
        if col == start {
            col += 1;
            continue;
        }
        let interior = start > 0 && col < cells.len();
        if interior && col - start >= 2 && best.is_none_or(|(s, e)| col - start > e - s) {
            best = Some((start, col));
        }
    }
    best
}

/// Icon-like cell: Nerd Font / private-use glyphs, box drawing and other
/// non-alphanumeric symbols. In sidebar rows these keep monospace spacing.
fn is_icon(text: &str) -> bool {
    text.chars().next().is_some_and(|c| {
        let u = c as u32;
        (0xE000..=0xF8FF).contains(&u) || u >= 0xF0000 || (!c.is_ascii() && !c.is_alphanumeric())
    })
}

impl LayerStyle {
    /// Style for Neovim floating windows, from the popup settings.
    pub fn float() -> Self {
        let s = crate::popup::settings();
        Self { corner_radius: s.corner_radius, shadow: s.shadow, float: true, ..Default::default() }
    }
}

pub trait GridRenderer {
    fn metrics(&self) -> CellMetrics;

    /// Switch fonts (guifont, zoom) or extra line spacing; everything is
    /// re-measured and re-laid out on the next frame.
    fn set_fonts(&mut self, fonts: GridFonts, linespace: f32);

    /// Called once per frame before painting; drops caches that are stale.
    fn begin_frame(&mut self, ctx: &egui::Context, editor: &Editor);

    /// Paint one grid as a layer with its top-left corner at `origin`.
    fn paint_grid(&mut self, painter: &Painter, editor: &Editor, grid: u64, origin: Pos2, style: LayerStyle);

    /// Repaint cells `[col_start, col_end)` of a grid row (a statusline or
    /// winbar) as a GUI bar: proportional font, the part after the filler
    /// aligned right, filling `rect`, which may be larger than the cells.
    /// Returns where things were found in the bar (file-type icon glyph,
    /// end of the left part).
    fn paint_bar(
        &mut self,
        painter: &Painter,
        editor: &Editor,
        grid: u64,
        row: usize,
        cols: (usize, usize),
        rect: Rect,
        look: StatusLook,
    ) -> Option<BarInfo>;

    /// Paint the cursor. `grid_origin` is the top-left of the cursor's grid.
    fn paint_cursor(&mut self, painter: &Painter, editor: &Editor, grid_origin: Pos2, focused: bool);

    /// Rows of a window's grid scrolled (`grid_scroll`): draw them gliding
    /// from where they were. Call before painting the new state. Every row
    /// glides with its text, the cursor's too (its cursorline, its number),
    /// and the cursor with it, as in Neovide: the highlight never sits on
    /// another line while the text moves.
    fn begin_scroll(&mut self, scrolled: grid::Scrolled);

    /// Stop any glide of a grid: it's shown where it is.
    fn end_scroll(&mut self, grid: u64);

    /// Move scrolling along by `dt` seconds; true while any is still moving.
    fn animate_scroll(&mut self, dt: f32) -> bool;

    /// Column under `x` (relative to the grid's left edge) for grids drawn
    /// with a proportional layout; `None` means use `x / cell width`.
    fn col_at(&self, grid: u64, row: usize, x: f32) -> Option<usize>;

    /// Forget cached data for grids that no longer exist.
    fn retain_grids(&mut self, keep: &dyn Fn(u64) -> bool);

    /// Images for placeholder cells (the kitty graphics protocol).
    fn images(&mut self) -> &mut Images;
}

#[derive(Clone, Copy)]
pub(crate) enum DecoKind {
    Underline,
    Undercurl,
    Underdouble,
    Underdotted,
    Underdashed,
    Strikethrough,
    /// A filled dot centered in its cell, at this height in the row (the
    /// text's middle): `●` in proportional rows (the Problems list's
    /// severity marks), drawn rather than taken from whichever font has the
    /// glyph, which sits it off the text.
    Dot(f32),
}

/// A row's `●` is drawn (`DecoKind::Dot`); this holds its place.
const DOT_PLACEHOLDER: &str = "\u{2007}";

/// Decoration line within a row, in cell-relative x coordinates.
#[derive(Clone)]
pub(crate) struct Deco {
    pub kind: DecoKind,
    pub x0: f32,
    pub x1: f32,
    pub color: Color32,
}

/// Everything needed to repaint a row, positioned relative to the row's
/// top-left so moving a window doesn't invalidate it.
#[derive(Clone)]
struct RowLayout {
    version: u64,
    backgrounds: Vec<(f32, f32, Color32)>,
    /// Galleys and their offsets from the row's top-left.
    runs: Vec<(Vec2, Arc<Galley>)>,
    decos: Vec<Deco>,
    /// Proportional rows: x of each column's left edge (len = width + 1).
    col_x: Option<Vec<f32>>,
    /// Laid out with a float's border characters hidden.
    masked: bool,
    /// Laid out with the contrast floor for text on this float base.
    floor: Option<Color32>,
    /// Image placeholder cells (snacks.image): column and image slice.
    tiles: Vec<(usize, Tile)>,
}

impl RowLayout {
    /// Left edge and width of a cell.
    fn cell_span(&self, col: usize, width: usize, cw: f32) -> (f32, f32) {
        match &self.col_x {
            Some(xs) => {
                let x0 = xs[col.min(xs.len() - 1)];
                let x1 = xs[(col + width).min(xs.len() - 1)];
                // Trailing cells have no text width; keep the cursor visible.
                (x0, (x1 - x0).max(cw * 0.5))
            }
            None => (col as f32 * cw, width as f32 * cw),
        }
    }
}

pub struct EguiGridRenderer {
    fonts: GridFonts,
    linespace: f32,
    metrics: CellMetrics,
    rows: HashMap<u64, Vec<Option<RowLayout>>>,
    /// Bars keyed by (grid, row, col_start, col_end).
    status_rows: HashMap<(u64, usize, usize, usize), BarLayout>,
    /// The bars painted since the last `begin_frame`: the others are dropped
    /// there (each window layout has its own keys, and they added up).
    status_seen: std::collections::HashSet<(u64, usize, usize, usize)>,
    hl_generation: u64,
    /// `None` until the first frame, when egui's fonts become available.
    pixels_per_point: Option<f32>,
    atlas_fill: f32,
    /// Kitty graphics protocol images, drawn in their placeholder cells.
    images: Images,
    /// Grids whose text is drawn shifted from its cells (an input line
    /// drawn as a field); the cursor follows.
    text_offsets: HashMap<u64, Vec2>,
    /// Grids whose rows are gliding after a scroll.
    scrolls: HashMap<u64, ScrollAnim>,
}

/// A scroll being animated: rows `top..bot` are drawn `offset` rows from
/// where they are (easing to 0); rows that scrolled out of the region are
/// drawn from `old`, the layouts from before the latest scroll of `shift`
/// rows.
struct ScrollAnim {
    top: usize,
    bot: usize,
    offset: f32,
    shift: i64,
    old: Vec<Option<RowLayout>>,
}

/// How fast scrolling eases: the offset's time constant in seconds (≈95%
/// done in 150 ms).
const SCROLL_TAU: f32 = 0.05;

impl EguiGridRenderer {
    pub fn new(fonts: GridFonts) -> Self {
        // Estimate until the fonts can be measured on the first frame.
        let metrics = CellMetrics { width: fonts.size * 0.6, height: fonts.size * 1.3 };
        Self {
            fonts,
            linespace: 0.0,
            metrics,
            rows: HashMap::new(),
            status_rows: HashMap::new(),
            status_seen: Default::default(),
            hl_generation: u64::MAX,
            pixels_per_point: None,
            atlas_fill: 0.0,
            images: Images::default(),
            text_offsets: HashMap::new(),
            scrolls: HashMap::new(),
        }
    }

    /// A font's cell: the width of `M` and the row height (the terminals measure theirs the same way).
    pub(crate) fn measure(ctx: &egui::Context, fonts: &GridFonts, linespace: f32) -> CellMetrics {
        let (font_id, _) = fonts.font_id(false, false);
        ctx.fonts_mut(|f| CellMetrics {
            width: f.glyph_width(&font_id, 'M'),
            height: (f.row_height(&font_id) + linespace).max(1.0),
        })
    }

    fn invalidate_all(&mut self) {
        self.rows.clear();
        self.status_rows.clear();
    }

    /// Paint a laid-out row at `top_left`. Backgrounds go through `palette`
    /// (a restyled float) and are skipped where they match the layer's
    /// fill, `skip_bg`.
    fn paint_row(&self, painter: &Painter, layout: &RowLayout, top_left: Pos2, skip_bg: &dyn Fn(Color32) -> bool, palette: Option<&FloatPalette>) {
        let h = self.metrics.height;
        for &(x0, x1, color) in &layout.backgrounds {
            let color = palette.map_or(color, |p| p.map_bg(color));
            if skip_bg(color) {
                continue;
            }
            let rect = Rect::from_min_max(Pos2::new(top_left.x + x0, top_left.y), Pos2::new(top_left.x + x1, top_left.y + h));
            painter.rect_filled(rect, 0.0, color);
        }
        for &(col, tile) in &layout.tiles {
            self.paint_tile(painter, top_left + Vec2::new(col as f32 * self.metrics.width, 0.0), tile);
        }
        for (offset, galley) in &layout.runs {
            painter.galley(top_left + *offset, galley.clone(), Color32::WHITE);
        }
        self.paint_decos(painter, top_left, &layout.decos);
    }

    /// One placeholder cell's slice of its image. The image is fit into
    /// its placement's columns × rows, keeping its aspect ratio, centered.
    fn paint_tile(&self, painter: &Painter, cell_min: Pos2, tile: Tile) {
        let Some(placed) = self.images.placed(tile.image, tile.placement) else { return };
        let m = self.metrics;
        let boxed = Vec2::new(placed.cols as f32 * m.width, placed.rows as f32 * m.height);
        let [iw, ih] = placed.image.size.map(|v| v.max(1) as f32);
        let scale = (boxed.x / iw).min(boxed.y / ih);
        let drawn = Rect::from_center_size((boxed / 2.0).to_pos2(), Vec2::new(iw, ih) * scale);
        // In the placement's coordinates.
        let cell = Rect::from_min_size(Pos2::new(tile.col as f32 * m.width, tile.row as f32 * m.height), Vec2::new(m.width, m.height));
        let part = cell.intersect(drawn);
        if part.width() <= 0.0 || part.height() <= 0.0 {
            return;
        }
        let uv = Rect::from_min_max(
            ((part.min - drawn.min) / drawn.size()).to_pos2(),
            ((part.max - drawn.min) / drawn.size()).to_pos2(),
        );
        let screen = part.translate(cell_min - cell.min);
        painter.image(placed.image.texture.id(), screen, uv, Color32::WHITE);
    }

    fn text_format(&self, fg: Color32, bold: bool, italic: bool) -> TextFormat {
        let (font_id, fake_italic) = self.fonts.font_id(bold, italic);
        TextFormat {
            font_id,
            color: fg,
            italics: fake_italic,
            line_height: Some(self.metrics.height),
            ..Default::default()
        }
    }

    fn layout_row(&self, ctx: &egui::Context, editor: &Editor, cells: &[Cell], version: u64, floor: Option<&ContrastFloor>) -> RowLayout {
        let cw = self.metrics.width;
        let defaults = &editor.default_colors;
        let default_bg = rgb(defaults.bg);

        let mut backgrounds: Vec<(f32, f32, Color32)> = Vec::new();
        let mut decos: Vec<Deco> = Vec::new();
        let mut jobs: Vec<(f32, LayoutJob)> = Vec::new();
        let mut tiles: Vec<(usize, Tile)> = Vec::new();

        // Current text run: (start col, hl id, text).
        let mut run: Option<(usize, u64, String)> = None;
        let flush_run = |run: &mut Option<(usize, u64, String)>, jobs: &mut Vec<(f32, LayoutJob)>| {
            if let Some((col, hl_id, text)) = run.take() {
                if text.trim().is_empty() {
                    return;
                }
                let attr = editor.hl_attr(hl_id);
                let colors = attr.colors(defaults);
                let mut fg = rgb(colors.fg);
                if let Some(floor) = floor {
                    let bg = blended(rgb(colors.bg), attr.blend);
                    fg = floor.apply(fg, if bg == default_bg { None } else { Some(bg) });
                }
                let fmt = self.text_format(fg, attr.bold, attr.italic);
                jobs.push((col as f32 * cw, LayoutJob::single_section(text, fmt)));
            }
        };

        let mut col = 0;
        while col < cells.len() {
            let cell = &cells[col];
            let attr = editor.hl_attr(cell.hl_id);
            let colors = attr.colors(defaults);
            let width = if cells.get(col + 1).is_some_and(|c| c.text.is_empty()) { 2 } else { 1 };
            let x0 = col as f32 * cw;
            let x1 = (col + width) as f32 * cw;

            let bg = blended(rgb(colors.bg), attr.blend);
            if bg != default_bg {
                match backgrounds.last_mut() {
                    Some(last) if last.2 == bg && (last.1 - x0).abs() < 0.01 => last.1 = x1,
                    _ => backgrounds.push((x0, x1, bg)),
                }
            }

            // An image placeholder: drawn as its image, not as text.
            let left = tiles.last().filter(|(c, _)| c + 1 == col).map(|&(_, t)| t);
            if let Some(tile) = images::tile(&cell.text, attr.foreground.unwrap_or(0), attr.special.unwrap_or(0), left) {
                tiles.push((col, tile));
                flush_run(&mut run, &mut jobs);
                col += width;
                continue;
            }

            let kind = match attr.underline {
                Underline::None => None,
                Underline::Line => Some(DecoKind::Underline),
                Underline::Curl => Some(DecoKind::Undercurl),
                Underline::Double => Some(DecoKind::Underdouble),
                Underline::Dotted => Some(DecoKind::Underdotted),
                Underline::Dashed => Some(DecoKind::Underdashed),
            };
            for (kind, color) in kind
                .map(|k| (k, rgb(colors.sp)))
                .into_iter()
                .chain(attr.strikethrough.then(|| (DecoKind::Strikethrough, rgb(colors.fg))))
            {
                match decos.last_mut() {
                    Some(d)
                        if std::mem::discriminant(&d.kind) == std::mem::discriminant(&kind)
                            && d.color == color
                            && (d.x1 - x0).abs() < 0.01 =>
                    {
                        d.x1 = x1
                    }
                    _ => decos.push(Deco { kind, x0, x1, color }),
                }
            }

            // ASCII cells of the same highlight share a run. Anything else
            // gets its own run so fallback-font advances can't shift the grid.
            let simple = cell.text.len() == 1 && cell.text.is_ascii() && width == 1;
            match &mut run {
                Some((_, hl, text)) if simple && *hl == cell.hl_id && text.is_ascii() => text.push_str(&cell.text),
                _ => {
                    flush_run(&mut run, &mut jobs);
                    run = Some((col, cell.hl_id, text_glyph(&cell.text).to_owned()));
                    if !simple {
                        flush_run(&mut run, &mut jobs);
                    }
                }
            }
            col += width;
        }
        flush_run(&mut run, &mut jobs);

        let runs = ctx.fonts_mut(|f| {
            let mut runs = Vec::with_capacity(jobs.len());
            for (x, job) in jobs {
                let galley = f.layout_job(job.clone());
                if on_cells(&galley, cw) {
                    runs.push((Vec2::new(x, 0.0), galley));
                    continue;
                }
                // Shaping moved characters off their cells (a ligature with
                // its own width): one character per cell instead.
                let format = job.sections.first().map(|s| s.format.clone()).unwrap_or_default();
                for (i, ch) in job.text.chars().enumerate() {
                    let one = LayoutJob::single_section(ch.to_string(), format.clone());
                    runs.push((Vec2::new(x + i as f32 * cw, 0.0), f.layout_job(one)));
                }
            }
            runs
        });
        RowLayout { version, backgrounds, runs, decos, col_x: None, masked: false, floor: floor.map(|f| f.base), tiles }
    }

    /// Shape cells as proportional text. Icons (and the space after one)
    /// use the grid font so their spacing survives. Returns the galley and
    /// the character index at the start of each column (len = cells + 1).
    fn shape_proportional(
        &self,
        ctx: &egui::Context,
        editor: &Editor,
        cells: &[Cell],
        style: &PropStyle,
    ) -> (Arc<Galley>, Vec<usize>) {
        let defaults = &editor.default_colors;
        let format = |hl_id: u64, mono: bool| {
            let attr = editor.hl_attr(hl_id);
            let mut font_id =
                if mono { self.fonts.sidebar_icon_font_id() } else { self.fonts.sidebar_font_id(attr.bold) };
            if let Some(size) = style.font_size {
                font_id.size = size;
            }
            TextFormat {
                font_id,
                color: style.uniform_fg.unwrap_or_else(|| rgb(attr.colors(defaults).fg)),
                italics: attr.italic,
                ..Default::default()
            }
        };
        // Highlight of the last run that had visible text.
        let mut last_text_hl: Option<u64> = None;
        let mut append = |job: &mut LayoutJob, hl: u64, mono: bool, text: &str| {
            let visible = !text.trim().is_empty();
            let gap = match last_text_hl {
                Some(prev) if visible && prev != hl => style.section_gap,
                _ => 0.0,
            };
            if visible {
                last_text_hl = Some(hl);
            }
            job.append(text, gap, format(hl, mono));
        };
        let mut col_char = vec![0usize; cells.len() + 1];
        let mut job = LayoutJob::default();
        let mut run: Option<(u64, bool, String)> = None;
        let mut chars = 0;
        let mut after_icon = false;
        for (col, cell) in cells.iter().enumerate() {
            col_char[col] = chars;
            if cell.text.is_empty() {
                continue; // right half of a double-width char
            }
            let icon = is_icon(&cell.text);
            let mono = icon || (after_icon && cell.text == " ");
            after_icon = icon;
            let text = if cell.text == "●" { DOT_PLACEHOLDER } else { cell.text.as_str() };
            match &mut run {
                Some((hl, m, run_text)) if *hl == cell.hl_id && *m == mono => run_text.push_str(text),
                _ => {
                    if let Some((hl, m, text)) = run.take() {
                        append(&mut job, hl, m, &text);
                    }
                    run = Some((cell.hl_id, mono, text.to_owned()));
                }
            }
            chars += cell.text.chars().count();
        }
        col_char[cells.len()] = chars;
        if let Some((hl, m, text)) = run.take() {
            append(&mut job, hl, m, &text);
        }
        (ctx.fonts_mut(|f| f.layout_job(job)), col_char)
    }

    /// Lay cells out as proportional text (sidebar rows, statuslines).
    /// With `justify`, the longest run of blank cells in the middle (like a
    /// statusline's filler) absorbs the slack so the part after it ends at
    /// the right edge (`width_px`). Backgrounds touching the first or last
    /// column extend to the edges.
    fn layout_cells_proportional(
        &self,
        ctx: &egui::Context,
        editor: &Editor,
        cells: &[Cell],
        version: u64,
        justify: bool,
        width_px: f32,
        style: &PropStyle,
    ) -> RowLayout {
        let n = cells.len();
        let defaults = &editor.default_colors;
        let default_bg = rgb(defaults.bg);
        let pos = |g: &Galley, c: usize| g.pos_from_cursor(CCursor::new(c)).min.x;
        let y_of = |g: &Galley| ((self.metrics.height - g.size().y) / 2.0).round();

        let filler = if justify { find_filler(cells) } else { None };
        let (col_x, runs) = match filler {
            None => {
                let (g, cc) = self.shape_proportional(ctx, editor, cells, style);
                let xs = cc.iter().map(|&c| SIDEBAR_PAD + pos(&g, c)).collect();
                (xs, vec![(Vec2::new(SIDEBAR_PAD, y_of(&g)), g)])
            }
            Some((fs, fe)) => {
                let (lg, lc) = self.shape_proportional(ctx, editor, &cells[..fs], style);
                let (rg, rc) = self.shape_proportional(ctx, editor, &cells[fe..], style);
                let left_end = SIDEBAR_PAD + lg.size().x;
                let right_x = (width_px - SIDEBAR_PAD - rg.size().x).max(left_end);
                let mut xs = vec![0.0; n + 1];
                for c in 0..=fs {
                    xs[c] = SIDEBAR_PAD + pos(&lg, lc[c]);
                }
                for c in fe..=n {
                    xs[c] = right_x + pos(&rg, rc[c - fe]);
                }
                // Filler columns share the gap evenly.
                let gap = (xs[fe] - xs[fs]) / (fe - fs) as f32;
                for c in fs + 1..fe {
                    xs[c] = xs[fs] + gap * (c - fs) as f32;
                }
                let runs = vec![(Vec2::new(SIDEBAR_PAD, y_of(&lg)), lg), (Vec2::new(right_x, y_of(&rg)), rg)];
                (xs, runs)
            }
        };
        let x_of = |col: usize| if col == 0 { 0.0 } else { col_x[col] };
        // Past the right edge; the clip rect trims it.
        let x_end = |col: usize| if col >= n { 1.0e5 } else { col_x[col] };

        let mut backgrounds: Vec<(f32, f32, Color32)> = Vec::new();
        let mut decos: Vec<Deco> = Vec::new();
        let mut col = 0;
        while col < n {
            let start = col;
            let hl = cells[col].hl_id;
            while col < n && cells[col].hl_id == hl {
                col += 1;
            }
            let attr = editor.hl_attr(hl);
            let colors = attr.colors(defaults);
            let bg = rgb(colors.bg);
            if bg != default_bg && style.uniform_fg.is_none() {
                backgrounds.push((x_of(start), x_end(col), bg));
            }
            let (x0, x1) = (col_x[start], col_x[col]);
            if style.uniform_fg.is_some() {
                continue;
            }
            if attr.underline != Underline::None {
                decos.push(Deco { kind: DecoKind::Underline, x0, x1, color: rgb(colors.sp) });
            }
            if attr.strikethrough {
                decos.push(Deco { kind: DecoKind::Strikethrough, x0, x1, color: rgb(colors.fg) });
            }
        }
        // The text's middle: its baseline, less a bit under half a capital
        // (between lowercase and capital letters' middles).
        let font_size = style.font_size.unwrap_or_else(|| self.fonts.sidebar_font_id(false).size);
        let middle = runs.first().map_or(self.metrics.height / 2.0, |(offset, g)| {
            offset.y + caps_center(g, font_size) + 0.05 * font_size
        });
        for (col, cell) in cells.iter().enumerate().filter(|(_, c)| c.text == "●") {
            let color = style.uniform_fg.unwrap_or_else(|| rgb(editor.hl_attr(cell.hl_id).colors(defaults).fg));
            decos.push(Deco { kind: DecoKind::Dot(middle), x0: col_x[col], x1: col_x[col + 1], color });
        }
        RowLayout { version, backgrounds, runs, decos, col_x: Some(col_x), masked: false, floor: None, tiles: Vec::new() }
    }

    fn paint_decos(&self, painter: &Painter, top_left: Pos2, decos: &[Deco]) {
        paint_decos_at(painter, top_left, self.metrics.height, decos);
    }
}

/// Underlines, strikethroughs and dots for a row `h` tall at `top_left`.
pub(crate) fn paint_decos_at(painter: &Painter, top_left: Pos2, h: f32, decos: &[Deco]) {
    let underline_y = top_left.y + h - 1.5;
    for d in decos {
        let (x0, x1) = (top_left.x + d.x0, top_left.x + d.x1);
        let stroke = Stroke::new(1.0, d.color);
        match d.kind {
            DecoKind::Dot(y) => {
                let r = (h * 0.16).clamp(2.5, 4.0);
                painter.circle_filled(Pos2::new((x0 + x1) / 2.0, top_left.y + y), r, d.color);
            }
            DecoKind::Underline => {
                painter.hline(x0..=x1, underline_y, stroke);
            }
            DecoKind::Underdouble => {
                painter.hline(x0..=x1, underline_y, stroke);
                painter.hline(x0..=x1, underline_y - 2.0, stroke);
            }
            DecoKind::Undercurl => {
                let mut pts = Vec::new();
                let step = 2.0;
                let mut x = x0;
                let mut up = true;
                while x <= x1 {
                    pts.push(Pos2::new(x, underline_y + if up { -1.0 } else { 1.0 }));
                    up = !up;
                    x += step;
                }
                painter.add(Shape::line(pts, stroke));
            }
            DecoKind::Underdotted => {
                painter.extend(Shape::dotted_line(
                    &[Pos2::new(x0, underline_y), Pos2::new(x1, underline_y)],
                    d.color,
                    3.0,
                    0.6,
                ));
            }
            DecoKind::Underdashed => {
                painter.extend(Shape::dashed_line(
                    &[Pos2::new(x0, underline_y), Pos2::new(x1, underline_y)],
                    stroke,
                    4.0,
                    2.0,
                ));
            }
            DecoKind::Strikethrough => {
                painter.hline(x0..=x1, top_left.y + h * 0.5, stroke);
            }
        }
    }
}

impl GridRenderer for EguiGridRenderer {
    fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    fn set_fonts(&mut self, fonts: GridFonts, linespace: f32) {
        self.fonts = fonts;
        self.linespace = linespace;
        // Forces a re-measure and full re-layout in `begin_frame`.
        self.pixels_per_point = None;
        self.invalidate_all();
    }

    fn begin_frame(&mut self, ctx: &egui::Context, editor: &Editor) {
        let seen = std::mem::take(&mut self.status_seen);
        self.status_rows.retain(|key, _| seen.contains(key));
        let ppp = ctx.pixels_per_point();
        // egui empties its glyph atlas when it fills up; cached galleys
        // would then point at stale texture coordinates.
        let fill = ctx.fonts(|f| f.font_atlas_fill_ratio());
        if self.pixels_per_point != Some(ppp) || fill < self.atlas_fill {
            self.pixels_per_point = Some(ppp);
            self.metrics = Self::measure(ctx, &self.fonts, self.linespace);
            self.invalidate_all();
        }
        self.atlas_fill = fill;
        if editor.hl_generation != self.hl_generation {
            self.hl_generation = editor.hl_generation;
            self.invalidate_all();
        }
    }


    fn paint_bar(
        &mut self,
        painter: &Painter,
        editor: &Editor,
        grid_id: u64,
        row: usize,
        (col_start, col_end): (usize, usize),
        rect: Rect,
        look: StatusLook,
    ) -> Option<BarInfo> {
        let grid = editor.grids.get(&grid_id)?;
        if row >= grid.height() || col_end > grid.width() || col_start >= col_end {
            return None;
        }
        let m = self.metrics;
        let key = (grid_id, row, col_start, col_end);
        self.status_seen.insert(key);
        let version = grid.row_version(row);
        let stale = self.status_rows.get(&key).is_none_or(|b| b.look != look || b.layout.version != version);
        if stale {
            let mut cells = grid.row(row)[col_start..col_end].to_vec();
            if look.uniform.is_some() {
                for c in &mut cells {
                    if is_powerline_separator(&c.text) {
                        c.text = " ".into();
                    }
                }
            }
            // The GUI draws its own file icon there. Nerd Font glyphs often
            // spill past their cell, so the glyph isn't drawn at all: an
            // invisible placeholder that is still laid out as an icon (em
            // spaces) keeps the slot.
            let icon_index = look.filetype.as_deref().and_then(|ft| icon_before_word(&cells, ft));
            if let Some(i) = icon_index {
                // Two em spaces: room for a full-size icon plus a gap.
                cells[i].text = "\u{2003}\u{2003}".into();
            }
            let style = PropStyle {
                font_size: Some(look.font_size),
                uniform_fg: look.uniform.map(|(_, fg)| fg),
                section_gap: if look.uniform.is_some() { look.font_size } else { 0.0 },
            };
            let layout = self.layout_cells_proportional(painter.ctx(), editor, &cells, version, true, rect.width(), &style);
            let pill = look.mode_color.and(layout.col_x.as_deref()).and_then(|xs| mode_span(&cells, xs)).map(|(x0, x1)| {
                let caps = layout.runs.first().map_or(m.height / 2.0, |(o, g)| o.y + caps_center(g, look.font_size));
                ModePill { x0, x1, caps_center: caps }
            });
            // The slot includes the space after the glyph: the icon is wider
            // than one cell.
            let icon = icon_index.and_then(|i| {
                let end = if cells.get(i + 1).is_some_and(|c| c.text == " ") { i + 2 } else { i + 1 };
                layout.col_x.as_ref().map(|xs| (xs[i], xs[end]))
            });
            self.status_rows.insert(key, BarLayout { look: look.clone(), layout, pill, icon });
        }
        let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        // Cover the cell-grid drawing underneath (and the leftover pixels
        // when `rect` extends past the grid).
        let base = look.uniform.map_or(rgb(editor.default_colors.bg), |(bg, _)| bg);
        painter.rect_filled(rect, 0.0, base);
        let bar = &self.status_rows[&key];
        let (layout, pill) = (&bar.layout, &bar.pill);
        if look.uniform.is_none() {
            // Theme colors: stretch backgrounds over the full bar height.
            for &(x0, x1, color) in &layout.backgrounds {
                let r = Rect::from_min_max(Pos2::new(rect.min.x + x0, rect.min.y), Pos2::new(rect.min.x + x1, rect.max.y));
                painter.rect_filled(r, 0.0, color);
            }
        }
        // Text centered in the bar.
        let top_left = rect.min + Vec2::new(0.0, ((rect.height() - m.height) / 2.0).round());
        if let (Some(color), Some(p)) = (look.mode_color, pill) {
            let h = (look.font_size * 1.45).min(rect.height() - 2.0);
            let cy = top_left.y + p.caps_center;
            let pill_rect =
                Rect::from_min_max(Pos2::new(rect.min.x + p.x0 - 7.0, cy - h / 2.0), Pos2::new(rect.min.x + p.x1 + 7.0, cy + h / 2.0));
            painter.rect_filled(pill_rect, 4.0, color);
        }
        for (offset, galley) in &layout.runs {
            painter.galley(top_left + *offset, galley.clone(), Color32::WHITE);
        }
        if let Some(color) = look.top_border {
            painter.hline(rect.x_range(), rect.min.y + 0.5, Stroke::new(1.0, color));
        }
        let icon_slot = bar.icon.map(|(x0, x1)| {
            Rect::from_min_max(Pos2::new(rect.min.x + x0, rect.min.y), Pos2::new(rect.min.x + x1, rect.max.y))
        });
        let left_end = layout.runs.first().map_or(rect.min.x, |(o, g)| top_left.x + o.x + g.size().x);
        Some(BarInfo { icon_slot, left_end })
    }


    fn paint_grid(&mut self, painter: &Painter, editor: &Editor, grid_id: u64, origin: Pos2, style: LayerStyle) {
        self.paint_grid_here(painter, editor, grid_id, origin, style);
    }

    fn paint_cursor(&mut self, painter: &Painter, editor: &Editor, grid_origin: Pos2, focused: bool) {
        self.paint_cursor_here(painter, editor, grid_origin, focused);
    }

    fn begin_scroll(&mut self, s: grid::Scrolled) {
        let height = s.bot.saturating_sub(s.top) as i64;
        // A jump of a screen or more isn't a scroll to follow.
        if s.rows.abs() >= height || height == 0 {
            self.scrolls.remove(&s.grid);
            return;
        }
        let offset = self.scrolls.get(&s.grid).filter(|a| (a.top, a.bot) == (s.top, s.bot)).map_or(0.0, |a| a.offset);
        let old = self.rows.get(&s.grid).cloned().unwrap_or_default();
        let offset = (offset + s.rows as f32).clamp(-(height as f32), height as f32);
        self.scrolls.insert(s.grid, ScrollAnim { top: s.top, bot: s.bot, offset, shift: s.rows, old });
    }

    fn end_scroll(&mut self, grid: u64) {
        self.scrolls.remove(&grid);
    }

    fn animate_scroll(&mut self, dt: f32) -> bool {
        let keep = (-dt / SCROLL_TAU).exp();
        self.scrolls.retain(|_, a| {
            a.offset *= keep;
            a.offset.abs() > 0.01
        });
        !self.scrolls.is_empty()
    }

    fn col_at(&self, grid: u64, row: usize, x: f32) -> Option<usize> {
        // Column positions only exist for proportional rows.
        let xs = self.rows.get(&grid)?.get(row)?.as_ref()?.col_x.as_ref()?;
        // Last column whose left edge is at or before x.
        let n = xs.len().saturating_sub(1);
        Some(xs[..n].partition_point(|&left| left <= x).saturating_sub(1))
    }

    fn retain_grids(&mut self, keep: &dyn Fn(u64) -> bool) {
        self.rows.retain(|id, _| keep(*id));
        self.scrolls.retain(|id, _| keep(*id));
        self.text_offsets.retain(|id, _| keep(*id));
    }

    fn images(&mut self) -> &mut Images {
        &mut self.images
    }
}

impl EguiGridRenderer {
    fn paint_grid_here(&mut self, painter: &Painter, editor: &Editor, grid_id: u64, origin: Pos2, style: LayerStyle) {
        let Some(grid) = editor.grids.get(&grid_id) else { return };
        let m = self.metrics;
        let size = Vec2::new(grid.width() as f32 * m.width, grid.height() as f32 * m.height);
        let layer_rect = Rect::from_min_size(origin, size);

        // Floats whose edges are box-drawing characters get a real frame:
        // the border glyphs are hidden and a (rounded) outline is drawn, or,
        // for top/bottom edges alone, a rule through the row.
        let border = if style.float { detect_border(grid, &|hl| editor.hl_attr(hl).blend > 0) } else { None };
        let defaults = &editor.default_colors;
        let bg_of = |hl: u64| {
            let attr = editor.hl_attr(hl);
            blended(rgb(attr.colors(defaults).bg), attr.blend)
        };
        // The layer's base color: for floats, the most common background
        // down the first column (inside a border), so it can be drawn as one
        // rounded rect; not the top-left cell, which may be a list's
        // selected row. Border cells without a background of their own take
        // it too (Neovim gives them Normal's, a band in a darker float).
        let base_bg = if style.float {
            let (col, rows) = match border {
                Some(Border { kind: BorderKind::Frame, .. }) => (1, 1..grid.height() - 1),
                Some(Border { kind: BorderKind::Shadow, .. }) => (0, 0..grid.height() - 1),
                _ => (0, 0..grid.height()),
            };
            dominant(rows.filter_map(|r| grid.cell(r, col)).map(|c| bg_of(c.hl_id)))
        } else {
            None
        };
        // A plugin's float in the theme's float color is drawn in the popup
        // colors, like the GUI's own popups.
        let palette = base_bg
            .and_then(|_| FloatPalette::from_editor(editor))
            .filter(|p| base_bg.is_some_and(|b| p.applies_to(b)));
        let unmapped_base = base_bg;
        let base_bg = base_bg.map(|c| palette.map_or(c, |p| p.map_bg(c)));
        let base = base_bg.unwrap_or(rgb(defaults.bg));
        // Titles/footers on the border rows sit on the outline (below).
        let labels: Vec<(usize, (usize, usize))> = match border {
            Some(b) => b.edge_rows(grid.height()).into_iter().filter_map(|r| Some((r, b.label_span(grid, r)?))).collect(),
            None => Vec::new(),
        };
        let border_color = border.map(|b| palette.map_or_else(|| rgb(editor.hl_attr(b.border_hl).colors(defaults).fg), |p| p.border));
        // A plugin's float (in the theme's float color, not a terminal's or
        // a menu's own) keeps its dim text readable.
        let floor = (style.float && unmapped_base == Some(popup::normal_float_bg(editor)))
            .then(|| ContrastFloor { base, palette, fg: rgb(defaults.fg) });
        // One row with nothing but a rule under it is an input line (snacks'
        // picker prompt): drawn as a field, its text centered in the two
        // rows and set in from the field's edge.
        let input_field = style.float
            && grid.height() == 2
            && labels.is_empty()
            && matches!(border, Some(Border { kind: BorderKind::Rules { top: false, bottom: true }, .. }));
        let shift = if input_field { Vec2::new(FIELD_INSET, (m.height / 2.0).round()) } else { Vec2::ZERO };
        if input_field {
            self.text_offsets.insert(grid_id, shift);
        } else {
            self.text_offsets.remove(&grid_id);
        }
        // A frame hugs the content like the GUI's own popups, instead of
        // running around the outside of the border cells: those are cleared
        // to the editor's background (no half-covered text peeking out), and
        // the frame, its fill and shadow sit `frame_margin` around the inside.
        // A title makes the top row a header strip inside the frame, grown
        // upward by the usual margin (floats laid inside, like snacks'
        // picker input, start below it). A footer keeps its line through the
        // middle of the bottom row.
        let frame_rect = match border {
            Some(Border { kind: BorderKind::Frame, .. }) => {
                if base_bg.is_none_or(|c| c.is_opaque()) {
                    painter.rect_filled(layer_rect, 0.0, style.under.unwrap_or(rgb(defaults.bg)));
                }
                let margin = frame_margin(m);
                let labeled = |row: usize| labels.iter().any(|(r, _)| *r == row);
                let top = if labeled(0) { m.height + margin.y } else { margin.y };
                let bottom = if labeled(grid.height() - 1) { m.height / 2.0 } else { margin.y };
                let inner = layer_rect.shrink2(Vec2::new(m.width, m.height));
                let mut r = Rect::from_min_max(inner.min - Vec2::new(margin.x, top), inner.max + Vec2::new(margin.x, bottom));
                // Kept within the editor area, so a float on its first or
                // last row (a picker at the top) keeps its edge and corners;
                // the header just gets less room above the title. So is one
                // wider than the editor (a picker's minimum width in a narrow
                // window: Neovim leaves its border off the screen).
                let clip = painter.clip_rect();
                r.min.y = r.min.y.max(clip.min.y.min(layer_rect.min.y + m.height));
                r.max.y = r.max.y.min(clip.max.y.max(layer_rect.max.y - m.height));
                r.min.x = r.min.x.max(clip.min.x);
                r.max.x = r.max.x.min(clip.max.x);
                r
            }
            // The shadow cells are left out; what's below shows through.
            Some(Border { kind: BorderKind::Shadow, .. }) => Rect::from_min_size(origin, size - Vec2::new(m.width, m.height)),
            _ => layer_rect,
        };
        // Rounded only with a border all round (a frame): a float without
        // one, or with padding on its sides alone (blink.cmp's menu and
        // docs), keeps square corners, flush with the text as in VS Code.
        let radius = match border {
            Some(Border { kind: BorderKind::Frame | BorderKind::Shadow, .. }) => style.corner_radius,
            _ => 0.0,
        };
        // A see-through float (a backdrop) dims what's below; a shadow
        // would only darken it unevenly.
        if style.shadow && base.is_opaque() {
            let shadow = crate::style::shadow();
            painter.add(shadow.as_shape(frame_rect, radius));
        }
        painter.rect_filled(frame_rect, radius, base);
        let outer = painter;
        let painter = painter.with_clip_rect(frame_rect.intersect(painter.clip_rect()));
        let painter = &painter;
        if input_field {
            let pad = (m.height * 0.3).round();
            let y = origin.y + shift.y;
            let field = Rect::from_x_y_ranges(layer_rect.x_range(), y - pad..=y + m.height + pad);
            // Its edge in the accent color while typing in it.
            let edge = if editor.cursor.grid == grid_id {
                popup::PopupTheme::from_editor(editor).accent
            } else {
                border_color.unwrap_or(base)
            };
            painter.rect_filled(field, FIELD_RADIUS, popup::input_field_bg(base));
            painter.rect_stroke(field, FIELD_RADIUS, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        }

        let mut cache = self.rows.remove(&grid_id).unwrap_or_default();
        cache.resize_with(grid.height(), || None);
        cache.truncate(grid.height());

        let ctx = painter.ctx().clone();
        let masked = border.is_some();
        for (row, slot) in cache.iter_mut().enumerate() {
            let version = grid.row_version(row);
            let stale = slot.as_ref().is_none_or(|l| {
                l.version != version
                    || l.col_x.is_some() != style.proportional
                    || l.masked != masked
                    || l.floor != floor.as_ref().map(|f| f.base)
            });
            if stale {
                let cells: std::borrow::Cow<[Cell]> = match border {
                    Some(b) => b.mask_row(grid, row).into(),
                    None => grid.row(row).into(),
                };
                let mut layout = if style.proportional {
                    let width_px = grid.width() as f32 * m.width;
                    self.layout_cells_proportional(&ctx, editor, &cells, version, false, width_px, &PropStyle::default())
                } else {
                    self.layout_row(&ctx, editor, &cells, version, floor.as_ref())
                };
                layout.masked = masked;
                *slot = Some(layout);
            }
        }
        // An input field shows its own fill (not the prompt's cursorline).
        let skip_bg = |c: Color32| input_field || Some(c) == base_bg;
        // Scrolling: the region's rows glide from where they were, each
        // with its backgrounds (the cursorline too), clipped to the region;
        // what scrolled into it is drawn from before (`old`).
        let anim = self.scrolls.get(&grid_id);
        let gliding = |row: usize| anim.is_some_and(|a| (a.top..a.bot).contains(&row));
        for (row, slot) in cache.iter().enumerate() {
            if labels.iter().any(|(r, _)| *r == row) || input_field && row == 1 || gliding(row) {
                continue;
            }
            let top_left = origin + Vec2::new(0.0, row as f32 * m.height) + shift;
            self.paint_row(painter, slot.as_ref().unwrap(), top_left, &skip_bg, palette.as_ref());
        }
        if let Some(a) = anim {
            let y = |row: usize| origin.y + row as f32 * m.height;
            let r = Rect::from_x_y_ranges(layer_rect.x_range(), y(a.top)..=y(a.bot));
            let clips = [painter.with_clip_rect(r.intersect(painter.clip_rect()))];
            for row in (a.top..a.bot).filter(|&r| gliding(r)) {
                let Some(layout) = cache.get(row).and_then(Option::as_ref) else { continue };
                let top_left = origin + Vec2::new(0.0, (row as f32 + a.offset) * m.height) + shift;
                for p in &clips {
                    self.paint_row(p, layout, top_left, &skip_bg, palette.as_ref());
                }
            }
            // Old rows whose lines aren't on a row now: those that scrolled
            // out.
            for k in a.top..a.bot {
                let now = k as i64 - a.shift;
                let shown = now >= a.top as i64 && now < a.bot as i64;
                let Some(layout) = a.old.get(k).and_then(Option::as_ref).filter(|_| !shown) else { continue };
                let top_left = origin + Vec2::new(0.0, (now as f32 + a.offset) * m.height);
                for p in &clips {
                    self.paint_row(p, layout, top_left, &|c| Some(c) == base_bg, palette.as_ref());
                }
            }
        }
        self.rows.insert(grid_id, cache);

        if let Some(b) = border.filter(|_| !input_field) {
            let stroke = Stroke::new(1.0, border_color.unwrap_or(base));
            // Where a border row's line runs: the layer's edge for a frame,
            // the middle of the row for a rule.
            let line_y = |row: usize| match b.kind {
                BorderKind::Frame if row == 0 => frame_rect.min.y + 0.5,
                BorderKind::Frame => frame_rect.max.y - 0.5,
                BorderKind::Rules { .. } => origin.y + (row as f32 + 0.5) * m.height,
                BorderKind::Shadow => origin.y,
            };
            match b.kind {
                BorderKind::Frame => {
                    outer.rect_stroke(frame_rect, radius, stroke, egui::StrokeKind::Inside);
                }
                BorderKind::Rules { .. } => {
                    let (left, right) = style.rule_extend;
                    for row in b.edge_rows(grid.height()) {
                        outer.hline(layer_rect.min.x - left..=layer_rect.max.x + right, line_y(row).round() + 0.5, stroke);
                    }
                }
                BorderKind::Shadow => {}
            }
            // Labels in the UI font, text only (cell backgrounds would stick
            // out of the frame). A frame's title heads its header strip, in
            // line with the content, above a hairline that leaves a gap
            // before the first row; other labels sit on their line, which is
            // cut behind them like a legend.
            let text_left = origin.x + m.width;
            for &(row, span) in &labels {
                let Some((c0, c1)) = b.trimmed_label(grid, row, span) else { continue };
                let (galley, _) = self.shape_proportional(&ctx, editor, &grid.row(row)[c0..c1], &PropStyle::default());
                let size = galley.size();
                if b.is_header(row) {
                    let (top, line) = (frame_rect.min.y, origin.y + m.height - HEADER_GAP);
                    let x = origin.x + b.content_col(grid) as f32 * m.width;
                    outer.galley(Pos2::new(x, top + ((line - top - size.y) / 2.0).round()), galley, Color32::WHITE);
                    outer.hline(frame_rect.x_range(), line.round() - 0.5, stroke);
                    continue;
                }
                let line_y = line_y(row);
                let x = if label_at_right(grid.width(), (c0, c1)) {
                    origin.x + (grid.width() - 1) as f32 * m.width - size.x
                } else {
                    text_left
                };
                let gap = Rect::from_x_y_ranges(x - 4.0..=x + size.x + 4.0, line_y - 1.0..=line_y + 1.0);
                outer.rect_filled(gap.intersect(frame_rect), 0.0, base);
                outer.galley(Pos2::new(x, (line_y - size.y / 2.0).round()), galley, Color32::WHITE);
            }
        }
    }

    fn paint_cursor_here(&mut self, painter: &Painter, editor: &Editor, grid_origin: Pos2, focused: bool) {
        if editor.busy {
            return;
        }
        let cur = editor.cursor;
        let Some(grid) = editor.grids.get(&cur.grid) else { return };
        let Some(cell) = grid.cell(cur.row, cur.col) else { return };
        let m = self.metrics;
        let wide = grid.cell(cur.row, cur.col + 1).is_some_and(|c| c.text.is_empty());
        let layout = self.rows.get(&cur.grid).and_then(|rows| rows.get(cur.row)).and_then(Option::as_ref);
        let proportional = layout.is_some_and(|l| l.col_x.is_some());
        let (x, cell_w) = match layout {
            Some(l) => l.cell_span(cur.col, if wide { 2 } else { 1 }, m.width),
            None => (cur.col as f32 * m.width, if wide { 2.0 } else { 1.0 } * m.width),
        };
        let mut shift = self.text_offsets.get(&cur.grid).copied().unwrap_or_default();
        // The cursor glides with its row.
        if let Some(a) = self.scrolls.get(&cur.grid).filter(|a| (a.top..a.bot).contains(&cur.row)) {
            shift.y += a.offset * m.height;
        }
        let cell_origin = grid_origin + Vec2::new(x, cur.row as f32 * m.height) + shift;

        let cell_attr = editor.hl_attr(cell.hl_id);
        let mode = editor.current_mode();
        let (fg, bg) = cursor_colors(editor, mode.map_or(0, |m| m.attr_id));
        let (fg, bg) = (rgb(fg), rgb(bg));

        let shape = if editor.cursor_style_enabled {
            mode.map(|m| m.cursor_shape).unwrap_or_default()
        } else {
            CursorShape::Block
        };
        let pct = mode.and_then(|m| m.cell_percentage).unwrap_or(100).clamp(1, 100) as f32 / 100.0;
        let full = Rect::from_min_size(cell_origin, Vec2::new(cell_w, m.height));

        if !focused {
            painter.rect_stroke(full.shrink(0.5), 0.0, Stroke::new(1.0, bg), egui::StrokeKind::Inside);
            return;
        }
        match shape {
            CursorShape::Block => {
                painter.rect_filled(full, 0.0, bg);
                if !cell.text.trim().is_empty() {
                    let fmt = if proportional {
                        TextFormat { font_id: self.fonts.sidebar_font_id(cell_attr.bold), color: fg, ..Default::default() }
                    } else {
                        self.text_format(fg, cell_attr.bold, cell_attr.italic)
                    };
                    let galley = painter.layout_job(LayoutJob::single_section(cell.text.clone(), fmt));
                    let y = if proportional { ((m.height - galley.size().y) / 2.0).round() } else { 0.0 };
                    painter.galley(cell_origin + Vec2::new(0.0, y), galley, fg);
                }
            }
            CursorShape::Vertical => {
                let w = (m.width * pct).max(1.0).min(cell_w);
                painter.rect_filled(Rect::from_min_size(cell_origin, Vec2::new(w, m.height)), 0.0, bg);
            }
            CursorShape::Horizontal => {
                let h = (m.height * pct).max(1.0);
                let r = Rect::from_min_max(Pos2::new(full.min.x, full.max.y - h), full.max);
                painter.rect_filled(r, 0.0, bg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_icon;


    use super::{detect_border, find_filler, Border, BorderKind};
    use grid::{Cell, Grid, LineCell};

    fn grid_from(rows: &[&str], hl: u64) -> Grid {
        let w = rows[0].chars().count();
        let mut g = Grid::new(w, rows.len());
        for (r, s) in rows.iter().enumerate() {
            let cells: Vec<_> = s.chars().map(|c| LineCell { text: c.to_string(), hl_id: hl, repeat: 1 }).collect();
            g.put_line(r, 0, &cells);
        }
        g
    }

    #[test]
    fn detects_box_borders_and_keeps_titles() {
        let mut g = grid_from(&["╭ Hover ─╮", "│ text   │", "╰────────╯"], 7);
        g.put_line(1, 1, &[LineCell { text: " ".into(), hl_id: 9, repeat: 8 }]);
        let b = detect_border(&g, &|_| false).unwrap();
        assert_eq!(b, Border { border_hl: 7, inner_hl: 9, kind: BorderKind::Frame, blank: false });
        let top: String = b.mask_row(&g, 0).iter().map(|c| c.text.as_str()).collect();
        assert_eq!(top, "  Hover   ");
        // The title's columns, drawn on the outline; no footer.
        assert_eq!(b.label_span(&g, 0), Some((1, 8)));
        assert_eq!(b.label_span(&g, 2), None);
        // Drawn without its padding, as the frame's header.
        assert_eq!(b.trimmed_label(&g, 0, (1, 8)), Some((2, 7)));
        assert!(b.is_header(0) && !b.is_header(2));
        // The title lines up with the content's first text column.
        let keys = grid_from(&["╭ Space ─╮", "│ b  Bufs│", "│ e  Expl│", "╰────────╯"], 7);
        assert_eq!(detect_border(&keys, &|_| false).unwrap().content_col(&keys), 2);
        let empty = grid_from(&["╭ Box ─╮", "│      │", "│      │", "╰──────╯"], 7);
        assert_eq!(detect_border(&empty, &|_| false).unwrap().content_col(&empty), 1);
        assert!(detect_border(&grid_from(&["abc", "def", "ghi"], 0), &|_| false).is_none());
    }

    #[test]
    fn detects_solid_and_shadow_borders() {
        // `solid`: blank cells in the border's highlight (5) all round, with
        // a title in its own (6); content in 9.
        let mut g = grid_from(&["  Name  ", "        ", "        "], 5);
        g.put_line(0, 2, &[
            LineCell { text: "N".into(), hl_id: 6, repeat: 1 },
            LineCell { text: "a".into(), hl_id: 6, repeat: 1 },
            LineCell { text: "m".into(), hl_id: 6, repeat: 1 },
            LineCell { text: "e".into(), hl_id: 6, repeat: 1 },
        ]);
        g.put_line(1, 1, &[LineCell { text: "x".into(), hl_id: 9, repeat: 6 }]);
        let b = detect_border(&g, &|_| false).unwrap();
        assert_eq!(b, Border { border_hl: 5, inner_hl: 9, kind: BorderKind::Frame, blank: true });
        assert_eq!(b.label_span(&g, 0), Some((2, 6)));
        assert_eq!(b.label_span(&g, 2), None);
        let side: String = b.mask_row(&g, 1).iter().map(|c| c.text.as_str()).collect();
        assert_eq!(side, " xxxxxx ");
        // Padding in the content's own highlight is not a border.
        let plain = grid_from(&["    ", " ab ", "    "], 9);
        assert!(detect_border(&plain, &|_| false).is_none());
        // Padding on the sides alone (blink.cmp's docs): its first and last
        // rows are content, not a title and a footer.
        let mut sides = grid_from(&[" fn find() ", " Looks for ", " @return   "], 5);
        for r in 0..3 {
            sides.put_line(r, 1, &[LineCell { text: " ".into(), hl_id: 9, repeat: 9 }]);
        }
        assert!(detect_border(&sides, &|_| false).is_none());

        // `shadow`: a see-through right column and bottom row (highlight 8).
        let mut g = grid_from(&["ab ", "cd ", "   "], 9);
        g.put_line(0, 2, &[LineCell { text: " ".into(), hl_id: 8, repeat: 1 }]);
        g.put_line(1, 2, &[LineCell { text: " ".into(), hl_id: 8, repeat: 1 }]);
        g.put_line(2, 0, &[LineCell { text: " ".into(), hl_id: 8, repeat: 3 }]);
        let blended = |hl: u64| hl == 8;
        assert_eq!(detect_border(&g, &blended).unwrap().kind, BorderKind::Shadow);
        // Not without blending, nor for a float see-through as a whole.
        assert!(detect_border(&g, &|_| false).is_none());
        assert!(detect_border(&g, &|_| true).is_none());
    }

    #[test]
    fn detects_top_and_bottom_rules() {
        // snacks' picker preview: a rule with a title on top, no sides.
        let mut g = grid_from(&["── main.rs ──", "fn main() {} "], 7);
        g.put_line(1, 0, &[LineCell { text: "f".into(), hl_id: 9, repeat: 1 }]);
        let b = detect_border(&g, &|_| false).unwrap();
        assert_eq!(b, Border { border_hl: 7, inner_hl: 9, kind: BorderKind::Rules { top: true, bottom: false }, blank: false });
        let top: String = b.mask_row(&g, 0).iter().map(|c| c.text.as_str()).collect();
        assert_eq!(top, "   main.rs   ");
        assert_eq!(b.label_span(&g, 0), Some((2, 11)));
        assert!(!b.is_header(0));
        // Content rows keep their text, side glyphs included.
        let row: String = b.mask_row(&g, 1).iter().map(|c| c.text.as_str()).collect();
        assert_eq!(row, "fn main() {} ");

        // The picker's input: a rule below.
        let b = detect_border(&grid_from(&["> query", "───────"], 3), &|_| false).unwrap();
        assert_eq!(b.kind, BorderKind::Rules { top: false, bottom: true });
        assert_eq!(b.edge_rows(2), [1]);

        // Not rules: line glyphs that don't reach both edges, two titles,
        // or a float with nothing but rules.
        assert!(detect_border(&grid_from(&["── x", "text"], 0), &|_| false).is_none());
        assert!(detect_border(&grid_from(&["─ a ─ b ─", "text text"], 0), &|_| false).is_none());
        assert!(detect_border(&grid_from(&["───", "───"], 0), &|_| false).is_none());
    }

    #[test]
    fn dominant_picks_the_most_frequent_first_on_ties() {
        use super::dominant;
        // A list whose first row is the selection.
        assert_eq!(dominant([5, 1, 1, 1].into_iter()), Some(1));
        assert_eq!(dominant([2, 3].into_iter()), Some(2));
        assert_eq!(dominant(std::iter::empty::<u8>()), None);
    }

    #[test]
    fn a_backdrop_dims_what_it_covers() {
        use super::{blended, over};
        use egui::Color32;
        let bg = Color32::from_rgb(0x2b, 0x30, 0x3b);
        // Black at winblend 60: 40% black over the editor.
        let dimmed = over(blended(Color32::BLACK, 60), bg);
        assert_eq!(dimmed, Color32::from_rgb(0x1a, 0x1d, 0x23));
        assert_eq!(over(Color32::WHITE, bg), Color32::WHITE);
        assert_eq!(over(blended(Color32::WHITE, 100), bg), bg);
    }

    #[test]
    fn blend_makes_backgrounds_see_through() {
        use super::blended;
        let black = egui::Color32::BLACK;
        assert_eq!(blended(black, 0), black);
        // lazy.nvim's backdrop: 60 lets 60% of what's below through.
        assert_eq!(blended(black, 60).a(), 102);
        assert_eq!(blended(black, 100).a(), 0);
    }

    #[test]
    fn labels_keep_a_right_placement_only() {
        use super::label_at_right;
        // "╰─── esc close ─╯": hints at the right stay there.
        assert!(label_at_right(20, (12, 18)));
        // Centered and left-placed labels start at the left.
        assert!(!label_at_right(20, (7, 13)));
        assert!(!label_at_right(20, (1, 6)));
        // A label filling the row.
        assert!(!label_at_right(8, (1, 7)));
    }

    #[test]
    fn cursor_colors_prefer_cursor_group_over_cell_swap() {
        use grid::{Editor, HlAttr, RedrawEvent};
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::DefaultColorsSet { fg: 0xdddddd, bg: 0x222222, sp: 0 });
        // No Cursor group: default colors swapped.
        assert_eq!(super::cursor_colors(&ed, 0), (0x222222, 0xdddddd));
        let cursor = HlAttr { foreground: Some(0x111111), background: Some(0xcccccc), ..Default::default() };
        ed.apply(RedrawEvent::HlAttrDefine { id: 7, attr: cursor, groups: vec![] });
        ed.apply(RedrawEvent::HlGroupSet { name: "Cursor".into(), id: 7 });
        assert_eq!(super::cursor_colors(&ed, 0), (0x111111, 0xcccccc));
        // A mode's own cursor highlight wins.
        let lcursor = HlAttr { foreground: Some(1), background: Some(2), ..Default::default() };
        ed.apply(RedrawEvent::HlAttrDefine { id: 8, attr: lcursor, groups: vec![] });
        assert_eq!(super::cursor_colors(&ed, 8), (1, 2));
    }

    #[test]
    fn text_off_its_cells_is_caught() {
        use egui::text::LayoutJob;
        use egui::{Color32, FontId, TextFormat};
        let ctx = egui::Context::default();
        let mut result = (false, false);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().fonts_mut(|f| {
                let job = |text: &str, font: FontId| LayoutJob::single_section(text.into(), TextFormat::simple(font, Color32::WHITE));
                let mono = FontId::monospace(14.0);
                let cw = f.glyph_width(&mono, 'M');
                // Monospace: every character on its cell.
                result.0 = super::on_cells(&f.layout_job(job("fn a() -> b != c", mono)), cw);
                // Proportional ("i" narrower than "M"): caught.
                result.1 = super::on_cells(&f.layout_job(job("iiMM", FontId::proportional(14.0))), cw);
            });
        });
        out.textures_delta.clear();
        assert_eq!(result, (true, false));
    }

    #[test]
    fn placeholder_keeps_icon_slot() {
        // The em space replacing a glyph is still laid out like an icon.
        assert!(super::is_icon("\u{2003}"));
    }

    #[test]
    fn finds_filetype_icon() {
        use super::icon_before_word;
        let cells: Vec<Cell> = ["a", " ", "\u{e7a8}", " ", "r", "u", "s", "t", " ", "\u{e0b2}"]
            .iter()
            .map(|t| Cell { text: t.to_string(), hl_id: 0 })
            .collect();
        assert_eq!(icon_before_word(&cells, "rust"), Some(2));
        assert_eq!(icon_before_word(&cells, "lua"), None);
        assert_eq!(icon_before_word(&cells, ""), None);
    }

    #[test]
    fn mode_labels() {
        use super::{is_mode_label, mode_span};
        assert!(is_mode_label(" NORMAL "));
        assert!(is_mode_label("V-LINE"));
        assert!(!is_mode_label("main.rs"));
        assert!(!is_mode_label("Normal"));
        let cells: Vec<Cell> = " NORMAL  x"
            .chars()
            .enumerate()
            .map(|(i, c)| Cell { text: c.to_string(), hl_id: if i < 8 { 1 } else { 2 } })
            .collect();
        let xs: Vec<f32> = (0..=cells.len()).map(|i| i as f32 * 10.0).collect();
        assert_eq!(mode_span(&cells, &xs), Some((10.0, 70.0)));
    }

    #[test]
    fn filler_is_the_longest_interior_blank_run() {
        let cells = |s: &str| s.chars().map(|c| Cell { text: c.to_string(), hl_id: 0 }).collect::<Vec<_>>();
        assert_eq!(find_filler(&cells(" N  main.rs      rust  1:1 ")), Some((11, 17)));
        assert_eq!(find_filler(&cells("abc")), None);
        assert_eq!(find_filler(&cells("   abc   ")), None);
    }

    #[test]
    fn icons_vs_text() {
        assert!(is_icon("\u{e7a8}")); // Nerd Font rust icon
        assert!(is_icon("\u{f0214}")); // Nerd Font (plane 15)
        assert!(is_icon("│"));
        assert!(is_icon("✗"));
        assert!(!is_icon("a"));
        assert!(!is_icon(" "));
        assert!(!is_icon("é"));
        assert!(!is_icon("漢"));
    }
}
