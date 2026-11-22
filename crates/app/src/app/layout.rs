//! Where windows, separators and panels are, and the separators' painting.

use super::*;

impl VimApp {
    /// Statuslines live on the default grid. Draw them in the GUI style and
    /// blank the ones under sidebar windows (they belong to the panel).
    ///
    /// The window is rarely an exact number of cells tall/wide. A bar on the
    /// last row stays one row tall and sits on the window's bottom edge (the
    /// leftover pixels above it get the editor background); bars reaching
    /// the last column extend to the right edge.
    /// The column between two windows side by side: Neovim draws it as a
    /// cell of its own color with a line through the middle, a third band
    /// between the two windows (which themes color apart: NormalNC). Here
    /// each half takes its neighbor's background, so the line is the edge
    /// between them; in a tab strip row, the strip's, so the strip runs on.
    pub(super) fn paint_separators(&self, painter: &egui::Painter, cell_pos: &dyn Fn(i64, i64) -> Pos2) {
        let m = self.renderer.metrics();
        let Some(base) = self.editor.grids.get(&DEFAULT_GRID) else { return };
        let defaults = &self.editor.default_colors;
        let strip_bg = panel_bg(rgb(defaults.bg));
        let wins: Vec<&grid::Window> = self
            .editor
            .windows
            .values()
            .filter(|w| w.kind == WindowKind::Normal && !w.hidden && w.grid != DEFAULT_GRID)
            .collect();
        let backgrounds: HashMap<u64, egui::Color32> = wins.iter().filter_map(|w| Some((w.grid, self.window_bg(w)?))).collect();
        // A window's first row, counting a file tab in the separator row
        // above it; and its color on a screen row: its strip's in its tab's.
        let top = |w: &grid::Window| w.row - self.tab_in_separator(w) as i64;
        // Elsewhere, its edge cell's on that row (a cursorline or selection
        // runs on to the line): the last column's on the left of a
        // separator, the first's on its right. A window showing a preview has the
        // preview's background (`half_color`).
        let color_at = |w: &grid::Window, row: i64, last: bool| {
            let window = backgrounds.get(&w.grid).copied();
            half_color(&self.editor, w, row, last, strip_bg, self.preview_rect(w).and(window), window)
        };
        for l in &wins {
            let col = l.col + l.width as i64;
            for r in wins.iter().filter(|r| r.col == col + 1) {
                for row in top(l).max(top(r))..(l.row + l.height as i64).min(r.row + r.height as i64) {
                    let (Some(left), Some(right)) = (color_at(l, row, true), color_at(r, row, false)) else { continue };
                    let Some(cell) = base.cell(row as usize, col as usize) else { continue };
                    let min = cell_pos(row, col);
                    // (Where the right window's tab strip starts: `separator_overlap`.)
                    let mid = min.x + m.width - (m.width / 2.0).round();
                    let halves = [(l, left, min.x, mid), (r, right, mid, min.x + m.width)];
                    for &(_, color, x0, x1) in &halves {
                        painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, min.y..=min.y + m.height), 0.0, color);
                    }
                    let in_strip = |w: &grid::Window| row - w.row < w.margins.top as i64;
                    let strip_line = mix(strip_bg, rgb(defaults.fg), style::LINE);
                    let line = if in_strip(l) && in_strip(r) {
                        strip_line
                    } else {
                        separator_line(&self.editor, cell.hl_id)
                    };
                    // A strip's edges run on over its half, to the line: its
                    // bottom one, and the top one of a tab in a separator row.
                    for &(w, _, x0, x1) in &halves {
                        if in_strip(w) {
                            let stroke = egui::Stroke::new(style::HAIRLINE, strip_line);
                            painter.hline(x0..=x1, min.y + m.height - 0.5, stroke);
                            if row < w.row {
                                painter.hline(x0..=x1, min.y + 0.5, stroke);
                            }
                        }
                    }
                    // The line last: those edges' smoothed ends would cover
                    // its first pixel.
                    painter.vline(mid + 0.5, min.y..=min.y + m.height, egui::Stroke::new(style::HAIRLINE, line));
                }
            }
        }
    }

    /// A window's background: the most common one down its last column
    /// (its Normal or NormalNC; not a cursorline or a sign).
    pub(super) fn window_bg(&self, w: &grid::Window) -> Option<egui::Color32> {
        let g = self.editor.grids.get(&w.grid)?;
        let col = g.width().checked_sub(1)?;
        let mut counts: HashMap<egui::Color32, usize> = HashMap::new();
        for row in w.margins.top..g.height() {
            if let Some(cell) = g.cell(row, col) {
                *counts.entry(rgb(self.editor.hl_attr(cell.hl_id).colors(&self.editor.default_colors).bg)).or_default() += 1;
            }
        }
        counts.into_iter().max_by_key(|&(_, n)| n).map(|(c, _)| c)
    }

    /// The separator under `pos`, if it is within reach of one: the cursor to
    /// show (a resize one) and where Neovim should see a press, the
    /// separator's cell (so the drag starts from the extra pixels too). The
    /// reach is the cell and `SEPARATOR_REACH` more on either side. Not over
    /// what takes presses itself: a window's tab (in a separator row, the lower
    /// window's), its buttons, the panels' strips; nor under a float.
    pub(super) fn separator_grab(&self, pos: Pos2) -> Option<(egui::CursorIcon, Pos2)> {
        let m = self.renderer.metrics();
        let wins: Vec<_> = self
            .editor
            .windows
            .values()
            .filter(|w| w.kind == WindowKind::Normal && !w.hidden && w.grid != DEFAULT_GRID)
            .map(|w| ((w.row, w.col, w.height, w.width), self.tab_in_separator(w)))
            .collect();
        let cell = |row: i64, col: i64| Rect::from_min_size(self.origin + Vec2::new(col as f32 * m.width, row as f32 * m.height), Vec2::new(m.width, m.height));
        let taken = |p: Pos2| {
            self.winbar_tabs.iter().any(|(tab, ..)| tab.contains(p))
                || self.winbar_buttons.iter().any(|(r, ..)| r.contains(p))
        };
        let reach = SEPARATOR_REACH;
        let (icon, snapped) = separators(&wins).into_iter().find_map(|sep| match sep {
            Separator::Vertical { col, rows } => {
                let (top, bottom) = (cell(rows.start, col), cell(rows.end - 1, col));
                let zone = Rect::from_min_max(top.min, bottom.max).expand2(Vec2::new(reach, 0.0));
                let at = Pos2::new(top.center().x, pos.y);
                (zone.contains(pos) && !taken(pos)).then_some((egui::CursorIcon::ResizeHorizontal, at))
            }
            Separator::Horizontal { row, cols } => {
                let (left, right) = (cell(row, cols.start), cell(row, cols.end - 1));
                let zone = Rect::from_min_max(left.min, right.max).expand2(Vec2::new(0.0, reach));
                let at = Pos2::new(pos.x, left.center().y);
                (zone.contains(pos) && !taken(pos) && !taken(at)).then_some((egui::CursorIcon::ResizeVertical, at))
            }
        })?;
        let float = self.grid_cell_at(pos).and_then(|(grid, ..)| self.editor.windows.get(&grid)).is_some_and(|w| matches!(w.kind, WindowKind::Float { .. }));
        (!float).then_some((icon, snapped))
    }

    /// The float drawn below float `w` that `w` lies entirely within (the
    /// topmost, if several).
    pub(super) fn enclosing_float(&self, w: &grid::Window) -> Option<&grid::Window> {
        let WindowKind::Float { zindex, compindex, .. } = w.kind else { return None };
        let (r0, c0) = (w.row, w.col);
        let (r1, c1) = (r0 + w.height as i64, c0 + w.width as i64);
        self.editor
            .windows
            .values()
            .filter_map(|o| match o.kind {
                // (Not a see-through backdrop under a plugin's window, nor a
                // container, which isn't drawn.)
                WindowKind::Float { zindex: oz, compindex: oc, .. }
                    if o.grid != w.grid
                        && !o.hidden
                        && (oz, oc) < (zindex, compindex)
                        && o.row <= r0
                        && o.col <= c0
                        && o.row + o.height as i64 >= r1
                        && o.col + o.width as i64 >= c1
                        && !self.editor.grids.get(&o.grid).is_some_and(|g| crate::renderer::is_see_through(&self.editor, g))
                        && !self.is_container(o) =>
                {
                    Some(((oz, oc), o))
                }
                _ => None,
            })
            .max_by_key(|(order, _)| *order)
            .map(|(_, o)| o)
    }

    /// What float `w` covers: the editor's background, dimmed by the
    /// see-through floats (backdrops) below it that it lies within.
    pub(super) fn color_under(&self, w: &grid::Window) -> egui::Color32 {
        let WindowKind::Float { zindex, compindex, .. } = w.kind else { return rgb(self.editor.default_colors.bg) };
        let mut below: Vec<_> = self
            .editor
            .windows
            .values()
            .filter(|o| match o.kind {
                WindowKind::Float { zindex: oz, compindex: oc, .. } => {
                    !o.hidden
                        && (oz, oc) < (zindex, compindex)
                        && o.row <= w.row
                        && o.col <= w.col
                        && o.row + o.height as i64 >= w.row + w.height as i64
                        && o.col + o.width as i64 >= w.col + w.width as i64
                }
                _ => false,
            })
            .filter_map(|o| match o.kind {
                WindowKind::Float { zindex, compindex, .. } => {
                    let g = self.editor.grids.get(&o.grid)?;
                    Some(((zindex, compindex), crate::renderer::see_through_color(&self.editor, g)?))
                }
                _ => None,
            })
            .collect();
        below.sort_by_key(|(order, _)| *order);
        below.into_iter().fold(rgb(self.editor.default_colors.bg), |under, (_, c)| crate::renderer::over(c, under))
    }

    /// A plugin's scroll bar beside its popup (blink.cmp's gutter and
    /// thumb): a float one column wide, anchored to a float, on that float's
    /// last column or the one after it, within its rows, all blank. It
    /// isn't drawn: the popup gets the GUI's own bar (`paint_scrollbars`),
    /// as one popup with the others' look.
    pub(super) fn is_plugin_scrollbar(&self, w: &grid::Window) -> bool {
        let WindowKind::Float { anchor_grid, .. } = w.kind else { return false };
        let Some(p) = self.editor.windows.get(&anchor_grid).filter(|p| matches!(p.kind, WindowKind::Float { .. })) else {
            return false;
        };
        let right = p.col + p.width as i64;
        let beside = w.width == 1 && (w.col == right || w.col == right - 1);
        let within = w.row >= p.row && w.row + w.height as i64 <= p.row + p.height as i64;
        beside
            && within
            && self.editor.grids.get(&w.grid).is_some_and(|g| {
                (0..g.height()).all(|r| g.row(r).iter().all(|c| c.text.trim().is_empty()))
            })
    }

    /// The default grid's row of the lowest status line(s).
    pub(super) fn last_status_row(&self) -> Option<usize> {
        let global = self.laststatus == 3 || self.native_status();
        self.editor.status_segments(global).iter().map(|s| s.row).max()
    }

    /// Where the status bar starts (the lowest one's top), for the command
    /// line at the bottom to sit on; `None` with no bar (the editor area's
    /// bottom then). A bar on the last row is at the area's bottom edge.
    pub(super) fn status_bar_top(&self, area: Rect, m: crate::renderer::CellMetrics) -> Option<f32> {
        let rows = self.editor.grids.get(&DEFAULT_GRID)?.height();
        let row = self.last_status_row()?;
        Some(if row + 1 == rows { area.max.y - m.height } else { self.origin.y + row as f32 * m.height })
    }

    /// How far to move a float that sits on Neovim's own command line (the
    /// bottom rows: blink.cmp's command-line completion menu, placed above
    /// it) so it hangs under the GUI's command line instead, its left edge
    /// at the command-line column it was at (above the box instead, with the
    /// box at the bottom: Neovim put the float over its command line, which
    /// is the bottom edge the box is at). `None` for others, and while
    /// no command line is shown.
    pub(super) fn cmdline_float_offset(&self, w: &grid::Window) -> Option<Vec2> {
        let WindowKind::Float { anchor_grid, .. } = w.kind else { return None };
        let cmdline = self.cmdline_layout.as_ref().filter(|_| !self.editor.cmdline.is_empty())?;
        let rows = self.editor.grids.get(&DEFAULT_GRID)?.height() as i64;
        let (row, col) = self.editor.grid_origin(w.grid);
        // (In the lower half: not a big float that happens to reach the
        // bottom, a terminal or a picker.)
        if w.hidden || anchor_grid != DEFAULT_GRID || row + (w.height as i64) < rows - 2 || row < rows / 2 {
            return None;
        }
        let m = self.renderer.metrics();
        let x = cmdline.pos_of_screen_col(col.max(0) as usize).x;
        let target = if cmdline.at_bottom {
            Pos2::new(x, cmdline.rect.min.y - 4.0 - w.height as f32 * m.height)
        } else {
            Pos2::new(x, cmdline.rect.max.y + 4.0)
        };
        Some(target - (self.origin + Vec2::new(col as f32 * m.width, row as f32 * m.height)))
    }

    /// Whether a float sits right below the float it's anchored to, their
    /// columns overlapping (as shown, `grid_origin`).
    pub(super) fn below_anchor_float(&self, w: &grid::Window) -> bool {
        let WindowKind::Float { anchor_grid, .. } = w.kind else { return false };
        let Some(a) = self.editor.windows.get(&anchor_grid).filter(|a| matches!(a.kind, WindowKind::Float { .. })) else {
            return false;
        };
        let (row, _) = self.editor.grid_origin(w.grid);
        let (a_row, _) = self.editor.grid_origin(a.grid);
        let columns = w.col < a.col + a.width as i64 && a.col < w.col + w.width as i64;
        row == a_row + a.height as i64 && columns
    }

    /// A float that only lays out others: no border of its own, and every
    /// cell covered by floats above it (snacks' picker in a wide window: a
    /// box holding the framed list and preview side by side). The TUI never
    /// shows it; eshvi's frames sit inside their border cells, so drawn it
    /// would show around them as a slab with a shadow of its own. It isn't
    /// drawn, and the floats in it are drawn as floats of their own.
    pub(super) fn is_container(&self, w: &grid::Window) -> bool {
        let WindowKind::Float { zindex, compindex, .. } = w.kind else { return false };
        let Some(g) = self.editor.grids.get(&w.grid) else { return false };
        if crate::renderer::is_see_through(&self.editor, g) || crate::renderer::has_border(g) {
            return false;
        }
        let rect = |o: &grid::Window| (o.row, o.col, o.height, o.width);
        let above: Vec<CellRect> = self
            .editor
            .windows
            .values()
            .filter(|o| match o.kind {
                WindowKind::Float { zindex: oz, compindex: oc, .. } => {
                    o.grid != w.grid && !o.hidden && (oz, oc) > (zindex, compindex)
                }
                _ => false,
            })
            .map(rect)
            .collect();
        covers(rect(w), &above)
    }

    /// How far float `w`'s border rules reach left and right to meet the
    /// frame of `outer`, the float it lies in, on each side that touches the
    /// frame's border column, as snacks' picker lays its input and preview
    /// into its box.
    pub(super) fn rule_extend(&self, w: &grid::Window, outer: &grid::Window) -> (f32, f32) {
        let framed = self.editor.grids.get(&outer.grid).is_some_and(crate::renderer::has_frame);
        if !framed {
            return (0.0, 0.0);
        }
        // Ending inside the frame's 1px stroke, not past it.
        let reach = crate::renderer::frame_margin(self.renderer.metrics()).x - 0.5;
        let col = w.col + self.border_nudge(w);
        let left = if col == outer.col + 1 { reach } else { 0.0 };
        let right = if col + w.width as i64 == outer.col + outer.width as i64 - 1 { reach } else { 0.0 };
        (left, right)
    }

    /// Columns to draw float `w` to the right: 1 when it starts on the left
    /// border column of the framed float it lies in. Neovim moves a float
    /// that doesn't fit (a picker's box at the screen's left edge, its
    /// border off it) a column in, but not the floats a plugin laid into it
    /// (snacks' input and list): they'd overlap the frame's edge.
    pub(super) fn border_nudge(&self, w: &grid::Window) -> i64 {
        let Some(outer) = self.enclosing_float(w) else { return 0 };
        let framed = self.editor.grids.get(&outer.grid).is_some_and(crate::renderer::has_frame);
        (framed && w.col == outer.col && w.col + (w.width as i64) < outer.col + outer.width as i64 - 1) as i64
    }

    /// The height of every strip of tabs or buttons: `[layout] tab_height`,
    /// else by the UI font (what the diff view's bar was).
    pub(super) fn tab_height(&self) -> f32 {
        crate::config::tab_height(self.config.layout.tab_height, self.explorer.font_size)
    }

    /// Screen rect of a window's place in the layout.
    pub(super) fn window_rect(&self, w: &grid::Window) -> Rect {
        let m = self.renderer.metrics();
        let min = self.origin + Vec2::new(w.col as f32 * m.width, w.row as f32 * m.height);
        Rect::from_min_size(min, Vec2::new(w.width as f32 * m.width, w.height as f32 * m.height))
    }

    /// Whether a window's file tab is drawn in the separator row above it:
    /// a file window below another with the global statusline, which
    /// `apply_layout` (setup.lua) gives no winbar.
    pub(super) fn tab_in_separator(&self, w: &grid::Window) -> bool {
        self.config.layout.winbar == crate::config::WinbarMode::Tab
            && self.separator_above(w)
            && self.win_paths.contains_key(&w.win)
            && !self.sidebar_wins.contains(&w.win)
    }

    /// Whether a window has Neovim's separator row right above it (its line
    /// through the row's middle): one below another with the global
    /// statusline, and no winbar of its own.
    pub(super) fn separator_above(&self, w: &grid::Window) -> bool {
        self.laststatus == 3 && w.margins.top == 0 && w.row > 0 && w.kind == WindowKind::Normal
    }

    /// How far past its last column a window at the editor's right edge
    /// reaches: whole cells leave a sliver of pixels there, which it takes
    /// (0 for a window with columns after it).
    pub(super) fn right_sliver(&self, w: &grid::Window) -> f32 {
        let cols = self.editor.grids.get(&DEFAULT_GRID).map_or(0, |g| g.width());
        if (w.col.max(0) as usize) + w.width < cols {
            return 0.0;
        }
        let end = self.origin.x + (w.col as f32 + w.width as f32) * self.renderer.metrics().width;
        (self.grid_area.max.x - end).max(0.0)
    }

    /// The margin before the editor area's first column.
    pub(super) fn left_inset(&self) -> f32 {
        (self.renderer.metrics().width / 2.0).round()
    }

    /// A window at the editor's left or right edge continues each row's
    /// background (its Normal, a cursorline) over the margin before its
    /// first column and the sliver after its last, not the editor's. Its tab
    /// strip reaches there itself.
    pub(super) fn paint_edge_slivers(&self, painter: &egui::Painter, grid: u64) {
        let Some(w) = self.editor.windows.get(&grid).filter(|w| w.kind == WindowKind::Normal && !w.hidden) else { return };
        let Some(g) = self.editor.grids.get(&grid) else { return };
        let Some(last) = g.width().checked_sub(1) else { return };
        let r = self.window_rect(w);
        let m = self.renderer.metrics();
        let defaults = &self.editor.default_colors;
        let left = if w.col == 0 { self.origin.x - self.grid_area.min.x } else { 0.0 };
        let right = self.right_sliver(w);
        for row in w.margins.top..g.height() {
            let y = r.min.y + row as f32 * m.height;
            for (width, col, x) in [(left, 0, r.min.x - left), (right, last, r.max.x)] {
                let Some(cell) = g.cell(row, col).filter(|_| width > 0.0) else { continue };
                let rect = Rect::from_x_y_ranges(x..=x + width, y..=y + m.height);
                painter.rect_filled(rect, 0.0, rgb(self.editor.hl_attr(cell.hl_id).colors(defaults).bg));
            }
        }
    }

    /// How far a window's tab strip reaches right over the separator column
    /// after it, to the separator's line (with no window there, over the
    /// sliver at the editor's edge, if any).
    pub(super) fn separator_overlap_right(&self, w: &grid::Window) -> f32 {
        let beside = self.editor.windows.values().any(|r| {
            r.kind == WindowKind::Normal
                && !r.hidden
                && r.grid != DEFAULT_GRID
                && r.col == w.col + w.width as i64 + 1
                && r.row < w.row + w.height as i64
                && w.row < r.row + r.height as i64
        });
        let m = self.renderer.metrics();
        if beside { m.width - (m.width / 2.0).round() } else { self.right_sliver(w) }
    }

    /// How far a window's tab strip reaches left over the separator column
    /// before it: to the separator's line, half a cell (at the editor's left
    /// edge, over the margin there).
    pub(super) fn separator_overlap(&self, w: &grid::Window) -> f32 {
        let beside = self.editor.windows.values().any(|l| {
            l.kind == WindowKind::Normal
                && !l.hidden
                && l.grid != DEFAULT_GRID
                && l.col + l.width as i64 + 1 == w.col
                && l.row < w.row + w.height as i64
                && w.row < l.row + l.height as i64
        });
        if beside {
            (self.renderer.metrics().width / 2.0).round()
        } else if w.col == 0 {
            self.origin.x - self.grid_area.min.x
        } else {
            0.0
        }
    }
}

/// The color a window's half of a separator cell takes on screen row `row`:
/// its tab strip's in the strip rows; in a window showing a preview, the
/// preview's background (`preview`) on every row of its text area (its hidden
/// text's edge cells hold render-markdown's heading and code backgrounds,
/// which would show through as patches); else its edge cell's on that row
/// (the last column's if `last`, the first's), so a cursorline reaches the
/// line, and failing that `window`'s.
#[allow(clippy::too_many_arguments)]
pub(super) fn half_color(
    editor: &Editor,
    w: &grid::Window,
    row: i64,
    last: bool,
    strip_bg: egui::Color32,
    preview: Option<egui::Color32>,
    window: Option<egui::Color32>,
) -> Option<egui::Color32> {
    let line = row - w.row;
    if line < w.margins.top as i64 {
        return Some(strip_bg);
    }
    if let Some(bg) = preview.filter(|_| line < w.height as i64 - w.margins.bottom as i64) {
        return Some(bg);
    }
    let edge = editor.grids.get(&w.grid).and_then(|g| {
        let col = if last { g.width().checked_sub(1)? } else { 0 };
        g.cell(line as usize, col)
    });
    match edge {
        Some(cell) => Some(rgb(editor.hl_attr(cell.hl_id).colors(&editor.default_colors).bg)),
        None => window,
    }
}

/// The 1px line down a separator cell: the theme's WinSeparator foreground
/// (`hl_groups` holds the built-in groups), else the cell's own (`cell_hl`:
/// Neovim draws it in that group, or what a window's 'winhighlight' made it).
pub(super) fn separator_line(editor: &Editor, cell_hl: u64) -> egui::Color32 {
    let themed = editor.hl_groups.get("WinSeparator").and_then(|id| editor.hl_attr(*id).foreground);
    rgb(themed.unwrap_or_else(|| editor.hl_attr(cell_hl).colors(&editor.default_colors).fg))
}

/// How far past a separator's cell, on either side, the pointer still grabs it.
pub(super) const SEPARATOR_REACH: f32 = 3.0;

/// A separator between windows: the cells of its column (rows `rows`), or of
/// its row (columns `cols`).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Separator {
    Vertical { col: i64, rows: std::ops::Range<i64> },
    Horizontal { row: i64, cols: std::ops::Range<i64> },
}

/// The separators between windows (`(cells, tab in the separator row above)`):
/// a column between windows side by side, over the rows they share (counting a
/// tab in the row above a window as the window's), and the row between
/// windows above each other, over the columns they share.
pub(super) fn separators(wins: &[(CellRect, bool)]) -> Vec<Separator> {
    let mut out = Vec::new();
    for &((lr, lc, lh, lw), lstrip) in wins {
        for &((rr, rc, rh, rw), rstrip) in wins {
            let col = lc + lw as i64;
            let rows = (lr - lstrip as i64).max(rr - rstrip as i64)..(lr + lh as i64).min(rr + rh as i64);
            if rc == col + 1 && !rows.is_empty() {
                out.push(Separator::Vertical { col, rows });
            }
            let row = lr + lh as i64;
            let cols = lc.max(rc)..(lc + lw as i64).min(rc + rw as i64);
            if rr == row + 1 && !cols.is_empty() {
                out.push(Separator::Horizontal { row, cols });
            }
        }
    }
    out
}

/// A block of cells: row, column, height, width.
pub(super) type CellRect = (i64, i64, usize, usize);

/// Whether the `inner` blocks together cover every cell of `outer`.
pub(super) fn covers(outer: CellRect, inner: &[CellRect]) -> bool {
    let (r0, c0, h, w) = outer;
    if h == 0 || w == 0 {
        return false;
    }
    let mut seen = vec![false; h * w];
    for &(r, c, ih, iw) in inner {
        for row in r.max(r0)..(r + ih as i64).min(r0 + h as i64) {
            for col in c.max(c0)..(c + iw as i64).min(c0 + w as i64) {
                seen[(row - r0) as usize * w + (col - c0) as usize] = true;
            }
        }
    }
    seen.into_iter().all(|s| s)
}

#[cfg(test)]
mod tests {
    use egui::Color32;
    use grid::{Editor, HlAttr, LineCell, RedrawEvent};

    /// A window of 6 columns and 4 rows at (2, 0) (the first a winbar row),
    /// each row's last cell with its own background: a Markdown heading's
    /// blue, a code block's gray, a cursorline's.
    fn window_with_colored_edges() -> (Editor, grid::Window) {
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::DefaultColorsSet { fg: 0xdddddd, bg: 0x222222, sp: 0 });
        ed.apply(RedrawEvent::GridResize { grid: 2, width: 6, height: 4 });
        ed.apply(RedrawEvent::WinPos { grid: 2, win: 1000, start_row: 2, start_col: 0, width: 6, height: 4 });
        for (id, bg) in [(1, 0x1d4ed8), (2, 0x444444), (3, 0x2a2a2a)] {
            ed.apply(RedrawEvent::HlAttrDefine { id, attr: HlAttr { background: Some(bg), ..Default::default() }, groups: vec![] });
        }
        for (row, id) in [(1, 1), (2, 2), (3, 3)] {
            let cells = (0..6).map(|_| LineCell { text: " ".into(), hl_id: id, repeat: 1 }).collect();
            ed.apply(RedrawEvent::GridLine { grid: 2, row, col_start: 0, cells });
        }
        let mut w = ed.windows.get(&2).cloned().unwrap();
        w.margins.top = 1;
        (ed, w)
    }

    /// The hidden text's edge cells (a preview's window) must not show through
    /// a separator: its half takes the preview's background on every row.
    #[test]
    fn a_previewed_window_gives_its_separator_half_the_previews_background() {
        let (ed, w) = window_with_colored_edges();
        let (strip, window, preview) = (Color32::from_rgb(1, 1, 1), Color32::from_rgb(0x22, 0x22, 0x22), Color32::from_rgb(9, 9, 9));
        let half = |row, last, preview| super::half_color(&ed, &w, row, last, strip, preview, Some(window));
        // Without a preview: the edge cell's color on that row (a cursorline reaches the line).
        assert_eq!(half(3, true, None), Some(Color32::from_rgb(0x1d, 0x4e, 0xd8)));
        assert_eq!(half(4, true, None), Some(Color32::from_rgb(0x44, 0x44, 0x44)));
        assert_eq!(half(5, false, None), Some(Color32::from_rgb(0x2a, 0x2a, 0x2a)));
        // The tab strip's row stays the strip's.
        assert_eq!(half(2, true, Some(preview)), Some(strip));
        // With one: its background, on every row and on either side.
        for row in 3..6 {
            assert_eq!(half(row, true, Some(preview)), Some(preview), "row {row}");
            assert_eq!(half(row, false, Some(preview)), Some(preview), "row {row}");
        }
    }

    #[test]
    fn the_separator_line_is_win_separator_s_foreground() {
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::DefaultColorsSet { fg: 0xdddddd, bg: 0x222222, sp: 0 });
        ed.apply(RedrawEvent::HlAttrDefine { id: 5, attr: HlAttr { foreground: Some(0x445566), ..Default::default() }, groups: vec![] });
        // No such group: the separator cell's own foreground (the default here).
        assert_eq!(super::separator_line(&ed, 5), Color32::from_rgb(0x44, 0x55, 0x66));
        assert_eq!(super::separator_line(&ed, 0), Color32::from_rgb(0xdd, 0xdd, 0xdd));
        ed.apply(RedrawEvent::HlAttrDefine { id: 6, attr: HlAttr { foreground: Some(0x3b4261), ..Default::default() }, groups: vec![] });
        ed.apply(RedrawEvent::HlGroupSet { name: "WinSeparator".into(), id: 6 });
        assert_eq!(super::separator_line(&ed, 5), Color32::from_rgb(0x3b, 0x42, 0x61));
        // A group without a foreground (cleared) falls back too.
        ed.apply(RedrawEvent::HlAttrDefine { id: 7, attr: HlAttr::default(), groups: vec![] });
        ed.apply(RedrawEvent::HlGroupSet { name: "WinSeparator".into(), id: 7 });
        assert_eq!(super::separator_line(&ed, 5), Color32::from_rgb(0x44, 0x55, 0x66));
    }

    #[test]
    fn separators_are_found_between_windows() {
        use super::Separator::{Horizontal, Vertical};
        // Two windows side by side (columns 0..40 and 41..80), the right one
        // split, its lower 
        let wins = [((0, 0, 24, 40), false), ((0, 41, 10, 39), false), ((11, 41, 12, 39), true)];
        let found = super::separators(&wins);
        assert_eq!(found.len(), 3, "{found:?}");
        // The column: the rows both share, the tab's row included.
        assert!(found.contains(&Vertical { col: 40, rows: 0..10 }));
        assert!(found.contains(&Vertical { col: 40, rows: 10..23 }));
        // The row between the stacked windows: the columns they share.
        assert!(found.contains(&Horizontal { row: 10, cols: 41..80 }));
        // Windows with no gap are not separated by one.
        assert!(super::separators(&[((0, 0, 5, 10), false), ((5, 0, 5, 10), false)]).is_empty());
    }

    #[test]
    fn container_floats_are_covered_by_the_floats_in_them() {
        // snacks' wide picker: a 120×28 box, list and preview side by side.
        let bx = (4, 4, 28, 120);
        assert!(super::covers(bx, &[(4, 4, 28, 60), (4, 64, 28, 60)]));
        // A gap (or a float merely inside it) leaves it drawn.
        assert!(!super::covers(bx, &[(4, 4, 28, 60), (4, 65, 28, 59)]));
        assert!(!super::covers(bx, &[(6, 6, 10, 20)]));
        // Floats reaching past it still count for the part they cover.
        assert!(super::covers((2, 2, 3, 3), &[(0, 0, 10, 10)]));
    }
}
