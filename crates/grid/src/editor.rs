use std::collections::{BTreeMap, HashMap};

use crate::event::{ChunkStyle, OptionValue, PmItem, RedrawEvent, StyledChunk};
use crate::grid::Grid;
use crate::hl::{DefaultColors, HlAttr, HlId, ModeInfo};

/// The global grid in multigrid mode.
pub const DEFAULT_GRID: u64 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub grid: u64,
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WindowKind {
    Normal,
    Float { anchor_grid: u64, mouse_enabled: bool, zindex: i64, compindex: i64 },
    Message { scrolled: bool, sep_char: String, compindex: i64 },
    External,
}

/// Where a grid is shown on the global grid, in cells.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub grid: u64,
    pub win: i64,
    pub row: i64,
    pub col: i64,
    pub width: usize,
    pub height: usize,
    pub kind: WindowKind,
    pub hidden: bool,
    pub viewport: Option<Viewport>,
    /// Rows/columns of the grid outside the text area (`top` is 1 when the
    /// window has a winbar).
    pub margins: Margins,
    /// Rows the GUI shows a float below where Neovim put it
    /// (`settle_floats`); `grid_origin` counts them.
    pub shift: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Margins {
    pub top: usize,
    pub bottom: usize,
    pub left: usize,
    pub right: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    pub topline: i64,
    pub botline: i64,
    pub curline: i64,
    pub curcol: i64,
    pub line_count: i64,
    pub scroll_delta: i64,
}

/// The completion menu, when shown.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupMenu {
    pub items: Vec<PmItem>,
    /// Selected index, -1 for none.
    pub selected: i64,
    /// Anchor: where the first character of the completed word is.
    pub row: i64,
    pub col: i64,
    /// Anchor grid; -1 means the external cmdline.
    pub grid: i64,
}

/// One level of the external command line.
#[derive(Clone, Debug, PartialEq)]
pub struct Cmdline {
    pub content: Vec<StyledChunk>,
    /// Cursor position, a byte offset into the concatenated content.
    pub pos: usize,
    pub firstc: String,
    pub prompt: String,
    pub indent: usize,
    pub level: u64,
    /// Pending special char shown at the cursor (`<C-v>` etc.), and
    /// whether it shifts the text after it.
    pub special: Option<(String, bool)>,
}

impl Cmdline {
    pub fn text(&self) -> String {
        self.content.iter().map(|c| c.text.as_str()).collect()
    }
}

/// A message from `msg_show` (or `:messages` history).
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// Unique per message; changes when the message is replaced, so a UI
    /// can restart its display timer.
    pub seq: u64,
    pub id: Option<String>,
    pub kind: String,
    pub content: Vec<StyledChunk>,
}

impl Message {
    pub fn text(&self) -> String {
        self.content.iter().map(|c| c.text.as_str()).collect()
    }

    pub fn is_error(&self) -> bool {
        matches!(self.kind.as_str(), "emsg" | "echoerr" | "lua_error" | "rpc_error" | "shell_err")
    }

    pub fn is_warning(&self) -> bool {
        self.kind == "wmsg"
    }

    /// A prompt that waits for an answer (`:confirm`, `inputlist()`, ...).
    pub fn is_prompt(&self) -> bool {
        matches!(self.kind.as_str(), "confirm" | "return_prompt")
    }
}

/// A statusline on the default grid: `row`, columns `[col_start, col_end)`.
/// `win` is the window it belongs to, or `None` for the global statusline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusSegment {
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub win: Option<i64>,
}

/// Visible messages are capped; the oldest go first.
const MAX_MESSAGES: usize = 30;

/// The whole UI state as described by Neovim's redraw events.
#[derive(Debug, Default)]
pub struct Editor {
    pub grids: BTreeMap<u64, Grid>,
    pub windows: BTreeMap<u64, Window>,
    pub hl: HashMap<HlId, HlAttr>,
    pub hl_groups: HashMap<String, HlId>,
    /// The colorscheme's colors for git status and diagnostics ("added",
    /// "modified", "deleted", "error", "warn", "info"; 0xRRGGBB), reported by
    /// the GUI plugin: `hl_groups` has only Neovim's built-in UI groups.
    pub deco_theme: HashMap<String, u32>,
    /// The colorscheme's background colors for the diff view ("add",
    /// "delete", "change", "text": `DiffAdd`, `DiffDelete`, `DiffChange`,
    /// `DiffText`; 0xRRGGBB), reported by the GUI plugin; a group with no
    /// background is missing.
    pub diff_theme: HashMap<String, u32>,
    /// The colorscheme's `g:terminal_color_0..15` (0xRRGGBB), reported by the
    /// GUI plugin; `None` where unset (terminals use their own default).
    pub term_colors: [Option<u32>; 16],
    /// Highlight group names per id (`ext_hlstate`), for UI decisions
    /// such as recognizing file-tree icons.
    pub hl_names: HashMap<HlId, Vec<String>>,
    pub default_colors: DefaultColors,
    pub cursor: Cursor,
    pub cursor_style_enabled: bool,
    pub modes: Vec<ModeInfo>,
    pub mode_idx: usize,
    pub mode: String,
    pub options: HashMap<String, OptionValue>,
    pub title: String,
    pub busy: bool,
    pub mouse_enabled: bool,
    pub popupmenu: Option<PopupMenu>,
    /// Active command lines, innermost (highest level) last.
    pub cmdline: Vec<Cmdline>,
    pub cmdline_block: Vec<Vec<StyledChunk>>,
    /// Visible `msg_show` messages, oldest first.
    pub messages: Vec<Message>,
    pub showmode: Vec<StyledChunk>,
    pub showcmd: Vec<StyledChunk>,
    pub ruler: Vec<StyledChunk>,
    /// `:messages` output, shown until dismissed.
    pub message_history: Option<Vec<Message>>,
    next_msg_seq: u64,
    /// Margins by grid; `win_viewport_margins` can arrive before `win_pos`.
    margins: HashMap<u64, Margins>,
    /// Bumped whenever highlight definitions or default colors change, so
    /// renderers know cached colors are stale.
    pub hl_generation: u64,
    /// `ui_send` data not yet taken by the UI.
    pub tty_out: Vec<u8>,
    /// Full-width `grid_scroll`s not yet taken by the UI (for smooth
    /// scrolling).
    pub scrolled: Vec<Scrolled>,
    /// Tab pages (`ext_tabline`): handle and name; and the current one.
    pub tabs: Vec<(i64, String)>,
    pub current_tab: i64,
}

/// Rows `top..bot` of a grid scrolled by `rows` (up when positive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scrolled {
    pub grid: u64,
    pub top: usize,
    pub bot: usize,
    pub rows: i64,
}

static DEFAULT_HL: HlAttr = HlAttr {
    foreground: None,
    background: None,
    special: None,
    reverse: false,
    italic: false,
    bold: false,
    strikethrough: false,
    underline: crate::hl::Underline::None,
    blend: 0,
};

impl Editor {
    pub fn new() -> Self {
        Self { mouse_enabled: true, ..Default::default() }
    }

    pub fn hl_attr(&self, id: HlId) -> &HlAttr {
        self.hl.get(&id).unwrap_or(&DEFAULT_HL)
    }

    pub fn current_mode(&self) -> Option<&ModeInfo> {
        self.modes.get(self.mode_idx)
    }

    /// Position of a grid on the global grid, in cells.
    pub fn grid_origin(&self, grid: u64) -> (i64, i64) {
        match self.windows.get(&grid) {
            Some(w) => (w.row + w.shift, w.col),
            None => (0, 0),
        }
    }

    /// A float placed against the float it's anchored to, below it, that
    /// Neovim pushed back over it to fit the screen (blink.cmp's
    /// documentation below its menu, sized from the window's top rather
    /// than the screen's, a row too tall under a winbar): shown below it
    /// again, `shift` rows down, and the floats anchored to it (their
    /// scroll bars) with it. Call after each batch: positions are
    /// Neovim's, the shifts worked out again from them.
    pub fn settle_floats(&mut self) {
        fn shift_of(windows: &BTreeMap<u64, Window>, grid: u64, depth: usize) -> i64 {
            let Some(w) = windows.get(&grid) else { return 0 };
            let WindowKind::Float { anchor_grid, .. } = w.kind else { return 0 };
            let Some(a) = windows.get(&anchor_grid).filter(|a| anchor_grid != grid && matches!(a.kind, WindowKind::Float { .. }))
            else {
                return 0;
            };
            if depth > 8 {
                return 0;
            }
            let base = shift_of(windows, anchor_grid, depth + 1);
            let a_bottom = a.row + a.height as i64;
            let columns = w.col < a.col + a.width as i64 && a.col < w.col + w.width as i64;
            let over = w.row > a.row && w.row < a_bottom && w.row + w.height as i64 > a_bottom && columns;
            base + if over { a_bottom - w.row } else { 0 }
        }
        let shifts: Vec<(u64, i64)> = self.windows.keys().map(|&g| (g, shift_of(&self.windows, g, 0))).collect();
        for (grid, shift) in shifts {
            if let Some(w) = self.windows.get_mut(&grid) {
                w.shift = shift;
            }
        }
    }

    /// Cursor position on the global grid, in cells.
    pub fn cursor_screen_pos(&self) -> (i64, i64) {
        let (r, c) = self.grid_origin(self.cursor.grid);
        (r + self.cursor.row as i64, c + self.cursor.col as i64)
    }

    /// Grids to draw, bottom to top: the default grid, normal windows,
    /// then floats and the message grid in Neovim's composition order.
    pub fn draw_order(&self) -> Vec<u64> {
        let mut order = Vec::with_capacity(self.windows.len() + 1);
        if self.grids.contains_key(&DEFAULT_GRID) {
            order.push(DEFAULT_GRID);
        }
        let mut layered: Vec<(i64, u64)> = Vec::new();
        for (id, w) in &self.windows {
            if w.hidden || !self.grids.contains_key(id) {
                continue;
            }
            match &w.kind {
                WindowKind::Normal => order.push(*id),
                WindowKind::Float { compindex, .. } => layered.push((*compindex, *id)),
                WindowKind::Message { compindex, .. } => layered.push((*compindex, *id)),
                WindowKind::External => {}
            }
        }
        layered.sort();
        order.extend(layered.into_iter().map(|(_, id)| id));
        order
    }

    /// The topmost mouse-enabled grid at a global cell position, with the
    /// position translated into that grid's coordinates.
    pub fn grid_at(&self, row: i64, col: i64) -> Option<(u64, usize, usize)> {
        for id in self.draw_order().into_iter().rev() {
            let grid = &self.grids[&id];
            if let Some(WindowKind::Float { mouse_enabled: false, .. }) = self.windows.get(&id).map(|w| &w.kind) {
                continue;
            }
            let (r0, c0) = self.grid_origin(id);
            let (r, c) = (row - r0, col - c0);
            if r >= 0 && c >= 0 && (r as usize) < grid.height() && (c as usize) < grid.width() {
                return Some((id, r as usize, c as usize));
            }
        }
        None
    }

    /// Apply one event. `Flush` is handled by the caller, which only hands
    /// complete batches to the model.
    pub fn apply(&mut self, event: RedrawEvent) {
        use RedrawEvent::*;
        match event {
            SetTitle(t) => self.title = t,
            ModeInfoSet { cursor_style_enabled, modes } => {
                self.cursor_style_enabled = cursor_style_enabled;
                self.modes = modes;
            }
            ModeChange { mode, mode_idx } => {
                self.mode = mode;
                self.mode_idx = mode_idx;
            }
            OptionSet { name, value } => {
                self.options.insert(name, value);
            }
            MouseOn => self.mouse_enabled = true,
            MouseOff => self.mouse_enabled = false,
            BusyStart => self.busy = true,
            BusyStop => self.busy = false,
            Flush => {}
            UiSend(data) => self.tty_out.extend_from_slice(&data),

            GridResize { grid, width, height } => {
                self.grids
                    .entry(grid)
                    .and_modify(|g| g.resize(width, height))
                    .or_insert_with(|| Grid::new(width, height));
                // A float's size comes from its grid, and Neovim can move it
                // before resizing it: `win_float_pos` then has the old size.
                if let Some(w) = self.windows.get_mut(&grid).filter(|w| matches!(w.kind, WindowKind::Float { .. })) {
                    w.width = width;
                    w.height = height;
                }
            }
            DefaultColorsSet { fg, bg, sp } => {
                self.default_colors = DefaultColors { fg, bg, sp };
                self.hl_generation += 1;
            }
            HlAttrDefine { id, attr, groups } => {
                self.hl.insert(id, attr);
                self.hl_names.insert(id, groups);
                self.hl_generation += 1;
            }
            HlGroupSet { name, id } => {
                self.hl_groups.insert(name, id);
            }
            GridLine { grid, row, col_start, cells } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.put_line(row, col_start, &cells);
                }
            }
            GridClear { grid } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.clear();
                }
            }
            GridDestroy { grid } => {
                self.grids.remove(&grid);
                self.windows.remove(&grid);
                self.margins.remove(&grid);
            }
            GridCursorGoto { grid, row, col } => self.cursor = Cursor { grid, row, col },
            GridScroll { grid, top, bot, left, right, rows, cols: _ } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    if left == 0 && right >= g.width() && rows != 0 {
                        match self.scrolled.last_mut() {
                            Some(s) if s.grid == grid && (s.top, s.bot) == (top, bot) => s.rows += rows,
                            _ => self.scrolled.push(Scrolled { grid, top, bot, rows }),
                        }
                    }
                    g.scroll(top, bot, left, right, rows);
                }
            }

            WinPos { grid, win, start_row, start_col, width, height } => {
                let viewport = self.windows.get(&grid).and_then(|w| w.viewport);
                let margins = self.window_margins(grid);
                self.windows.insert(
                    grid,
                    Window {
                        grid,
                        win,
                        row: start_row as i64,
                        col: start_col as i64,
                        width,
                        height,
                        kind: WindowKind::Normal,
                        hidden: false,
                        viewport,
                        margins,
                        shift: 0,
                    },
                );
            }
            WinFloatPos {
                grid,
                win,
                anchor_grid,
                mouse_enabled,
                zindex,
                compindex,
                screen_row,
                screen_col,
                ..
            } => {
                let (width, height) = self
                    .grids
                    .get(&grid)
                    .map(|g| (g.width(), g.height()))
                    .unwrap_or((0, 0));
                let viewport = self.windows.get(&grid).and_then(|w| w.viewport);
                let margins = self.window_margins(grid);
                self.windows.insert(
                    grid,
                    Window {
                        grid,
                        win,
                        row: screen_row,
                        col: screen_col,
                        width,
                        height,
                        kind: WindowKind::Float { anchor_grid, mouse_enabled, zindex, compindex },
                        hidden: false,
                        viewport,
                        margins,
                        shift: 0,
                    },
                );
            }
            WinExternalPos { grid, win } => {
                let w = self.windows.entry(grid).or_insert_with(|| Window {
                    grid,
                    win,
                    row: 0,
                    col: 0,
                    width: 0,
                    height: 0,
                    kind: WindowKind::External,
                    hidden: false,
                    viewport: None,
                    margins: Margins::default(),
                    shift: 0,
                });
                w.kind = WindowKind::External;
            }
            WinHide { grid } => {
                if let Some(w) = self.windows.get_mut(&grid) {
                    w.hidden = true;
                }
            }
            WinClose { grid } => {
                self.windows.remove(&grid);
            }
            MsgSetPos { grid, row, scrolled, sep_char, zindex: _, compindex } => {
                let width = self.grids.get(&grid).map(|g| g.width()).unwrap_or(0);
                let height = self.grids.get(&grid).map(|g| g.height()).unwrap_or(0);
                self.windows.insert(
                    grid,
                    Window {
                        grid,
                        win: 0,
                        row: row as i64,
                        col: 0,
                        width,
                        height,
                        kind: WindowKind::Message { scrolled, sep_char, compindex },
                        hidden: false,
                        viewport: None,
                        margins: Margins::default(),
                        shift: 0,
                    },
                );
            }
            WinViewport { grid, topline, botline, curline, curcol, line_count, scroll_delta, .. } => {
                if let Some(w) = self.windows.get_mut(&grid) {
                    w.viewport =
                        Some(Viewport { topline, botline, curline, curcol, line_count, scroll_delta });
                }
            }
            WinViewportMargins { grid, top, bottom, left, right, .. } => {
                let margins = Margins { top, bottom, left, right };
                self.margins.insert(grid, margins);
                if let Some(w) = self.windows.get_mut(&grid) {
                    w.margins = margins;
                }
            }

            PopupmenuShow { items, selected, row, col, grid } => {
                self.popupmenu = Some(PopupMenu { items, selected, row, col, grid });
            }
            PopupmenuSelect { selected } => {
                if let Some(pm) = &mut self.popupmenu {
                    pm.selected = selected;
                }
            }
            PopupmenuHide => self.popupmenu = None,
            TablineUpdate { current, tabs } => {
                self.current_tab = current;
                self.tabs = tabs;
            }

            CmdlineShow { content, pos, firstc, prompt, indent, level } => {
                let line = Cmdline { content, pos, firstc, prompt, indent, level, special: None };
                self.cmdline.retain(|c| c.level < level);
                self.cmdline.push(line);
            }
            CmdlinePos { pos, level } => {
                if let Some(c) = self.cmdline.iter_mut().find(|c| c.level == level) {
                    c.pos = pos;
                }
            }
            CmdlineSpecialChar { c, shift, level } => {
                if let Some(line) = self.cmdline.iter_mut().find(|l| l.level == level) {
                    line.special = Some((c, shift));
                }
            }
            CmdlineHide { level } => self.cmdline.retain(|c| c.level < level),
            CmdlineBlockShow { lines } => self.cmdline_block = lines,
            CmdlineBlockAppend { line } => self.cmdline_block.push(line),
            CmdlineBlockHide => self.cmdline_block.clear(),

            MsgShow { kind, content, replace_last, history: _, append, id } => {
                self.next_msg_seq += 1;
                let seq = self.next_msg_seq;
                if kind == "empty" {
                    self.messages.clear();
                    return;
                }
                if let Some(m) = id.as_ref().and_then(|id| self.messages.iter_mut().find(|m| m.id.as_ref() == Some(id))) {
                    m.content = content;
                    m.kind = kind;
                    m.seq = seq;
                } else if append && !self.messages.is_empty() {
                    let m = self.messages.last_mut().unwrap();
                    m.content.extend(content);
                    m.seq = seq;
                } else {
                    if replace_last {
                        self.messages.pop();
                    }
                    self.messages.push(Message { seq, id, kind, content });
                    if self.messages.len() > MAX_MESSAGES {
                        self.messages.remove(0);
                    }
                }
            }
            // Prompts end with a clear; other messages are left to the UI,
            // which shows them as toasts that time out.
            MsgClear => self.messages.retain(|m| !m.is_prompt()),
            MsgShowmode(content) => self.showmode = content,
            MsgShowcmd(content) => self.showcmd = content,
            MsgRuler(content) => self.ruler = content,
            MsgHistoryShow(entries) => {
                let mut out: Vec<Message> = Vec::new();
                for (kind, content, append) in entries {
                    self.next_msg_seq += 1;
                    match out.last_mut() {
                        Some(m) if append => m.content.extend(content),
                        _ => out.push(Message { seq: self.next_msg_seq, id: None, kind, content }),
                    }
                }
                self.message_history = Some(out);
            }
        }
    }

    fn window_margins(&self, grid: u64) -> Margins {
        self.margins.get(&grid).copied().unwrap_or_default()
    }

    /// Statuslines on the default grid. With `global` ('laststatus' 3) there
    /// is one full-width statusline below the lowest windows; otherwise
    /// each normal window has one directly below it.
    pub fn status_segments(&self, global: bool) -> Vec<StatusSegment> {
        let Some(grid1) = self.grids.get(&DEFAULT_GRID) else { return vec![] };
        let normals: Vec<&Window> =
            self.windows.values().filter(|w| w.kind == WindowKind::Normal && !w.hidden).collect();
        let covered = |row: i64, col: i64| {
            normals.iter().any(|w| row >= w.row && row < w.row + w.height as i64 && col >= w.col && col < w.col + w.width as i64)
        };
        let fits = |row: i64| row >= 0 && (row as usize) < grid1.height();
        if global {
            let Some(row) = normals.iter().map(|w| w.row + w.height as i64).max() else { return vec![] };
            if !fits(row) || covered(row, 0) {
                return vec![];
            }
            return vec![StatusSegment { row: row as usize, col_start: 0, col_end: grid1.width(), win: None }];
        }
        let mut out: Vec<StatusSegment> = normals
            .iter()
            .filter_map(|w| {
                let row = w.row + w.height as i64;
                (fits(row) && !covered(row, w.col)).then(|| StatusSegment {
                    row: row as usize,
                    col_start: w.col.max(0) as usize,
                    col_end: ((w.col + w.width as i64) as usize).min(grid1.width()),
                    win: Some(w.win),
                })
            })
            .collect();
        out.sort_by_key(|s| (s.row, s.col_start));
        out
    }

    /// Show a message from the UI itself (e.g. a failed file operation),
    /// like a `msg_show` of `kind`.
    pub fn push_message(&mut self, kind: &str, text: &str) {
        self.next_msg_seq += 1;
        let content = vec![StyledChunk { style: ChunkStyle::Id(0), text: text.to_owned() }];
        self.messages.push(Message { seq: self.next_msg_seq, id: None, kind: kind.to_owned(), content });
        // (Capped like Neovim's: they expire as toasts only while frames are drawn.)
        if self.messages.len() > MAX_MESSAGES {
            self.messages.remove(0);
        }
    }

    /// Resolve a chunk's highlight.
    pub fn chunk_attr<'a>(&'a self, style: &'a ChunkStyle) -> &'a HlAttr {
        match style {
            ChunkStyle::Id(id) => self.hl_attr(*id),
            ChunkStyle::Attr(a) => a,
        }
    }

    /// Text of a grid, for tests and debugging.
    pub fn grid_text(&self, grid: u64) -> Vec<String> {
        self.grids
            .get(&grid)
            .map(|g| (0..g.height()).map(|r| g.row_text(r)).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::LineCell;

    fn line(grid: u64, row: usize, text: &str) -> RedrawEvent {
        RedrawEvent::GridLine {
            grid,
            row,
            col_start: 0,
            cells: text
                .chars()
                .map(|c| LineCell { text: c.to_string(), hl_id: 0, repeat: 1 })
                .collect(),
        }
    }

    #[test]
    fn apply_grid_events() {
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::GridResize { grid: 2, width: 3, height: 3 });
        ed.apply(line(2, 0, "abc"));
        ed.apply(line(2, 1, "def"));
        ed.apply(line(2, 2, "ghi"));
        ed.apply(RedrawEvent::GridScroll { grid: 2, top: 0, bot: 3, left: 0, right: 3, rows: 1, cols: 0 });
        ed.apply(line(2, 2, "jkl"));
        assert_eq!(ed.grid_text(2), ["def", "ghi", "jkl"]);
    }

    #[test]
    fn a_float_resized_after_it_moved_has_its_new_size() {
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::GridResize { grid: 5, width: 15, height: 3 });
        let moved = |row| RedrawEvent::WinFloatPos {
            grid: 5,
            win: 1005,
            anchor: Default::default(),
            anchor_grid: 1,
            anchor_row: 0.0,
            anchor_col: 0.0,
            mouse_enabled: true,
            zindex: 50,
            compindex: 1,
            screen_row: row,
            screen_col: 0,
        };
        ed.apply(moved(26));
        assert_eq!(ed.windows[&5].height, 3);
        // blink's menu going from a short list to a long one.
        ed.apply(moved(19));
        ed.apply(RedrawEvent::GridResize { grid: 5, width: 20, height: 10 });
        assert_eq!((ed.windows[&5].row, ed.windows[&5].width, ed.windows[&5].height), (19, 20, 10));
    }

    #[test]
    fn a_float_pushed_over_its_anchor_float_is_shown_below_it() {
        let mut ed = Editor::new();
        let float = |ed: &mut Editor, grid, anchor_grid, row, col, (width, height)| {
            ed.apply(RedrawEvent::GridResize { grid, width, height });
            ed.apply(RedrawEvent::WinFloatPos {
                grid,
                win: grid as i64 + 1000,
                anchor: Default::default(),
                anchor_grid,
                anchor_row: 0.0,
                anchor_col: 0.0,
                mouse_enabled: true,
                zindex: 50,
                compindex: grid as i64,
                screen_row: row,
                screen_col: col,
            });
        };
        ed.apply(RedrawEvent::GridResize { grid: 2, width: 80, height: 30 });
        ed.apply(RedrawEvent::WinPos { grid: 2, win: 1000, start_row: 0, start_col: 0, width: 80, height: 30 });
        // A menu at rows 11..21, its docs below, pushed up to row 20 (over
        // the menu's last row), and the docs' scroll bar on its right.
        float(&mut ed, 3, 2, 11, 10, (34, 10));
        float(&mut ed, 4, 3, 20, 10, (67, 10));
        float(&mut ed, 5, 4, 20, 78, (1, 10));
        // The menu's own scroll bar, inside its rows: stays.
        float(&mut ed, 6, 3, 11, 44, (1, 10));
        ed.settle_floats();
        assert_eq!(ed.grid_origin(3), (11, 10));
        assert_eq!(ed.grid_origin(4), (21, 10));
        assert_eq!(ed.grid_origin(5), (21, 78));
        assert_eq!(ed.grid_origin(6), (11, 44));
        // Placed below it to begin with: nothing to do.
        float(&mut ed, 4, 3, 21, 10, (67, 8));
        ed.settle_floats();
        assert_eq!(ed.grid_origin(4), (21, 10));
    }

    #[test]
    fn draw_order_puts_floats_on_top_by_compindex() {
        let mut ed = Editor::new();
        for g in 1..=4 {
            ed.apply(RedrawEvent::GridResize { grid: g, width: 2, height: 2 });
        }
        ed.apply(RedrawEvent::WinFloatPos {
            grid: 4,
            win: 1003,
            anchor: Default::default(),
            anchor_grid: 2,
            anchor_row: 0.0,
            anchor_col: 0.0,
            mouse_enabled: true,
            zindex: 50,
            compindex: 2,
            screen_row: 1,
            screen_col: 1,
        });
        ed.apply(RedrawEvent::WinFloatPos {
            grid: 3,
            win: 1002,
            anchor: Default::default(),
            anchor_grid: 2,
            anchor_row: 0.0,
            anchor_col: 0.0,
            mouse_enabled: true,
            zindex: 50,
            compindex: 1,
            screen_row: 0,
            screen_col: 0,
        });
        ed.apply(RedrawEvent::WinPos { grid: 2, win: 1000, start_row: 0, start_col: 0, width: 2, height: 2 });
        assert_eq!(ed.draw_order(), [1, 2, 3, 4]);

        ed.apply(RedrawEvent::WinHide { grid: 3 });
        assert_eq!(ed.draw_order(), [1, 2, 4]);
    }

    #[test]
    fn grid_at_hits_topmost_grid() {
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::GridResize { grid: 1, width: 20, height: 10 });
        ed.apply(RedrawEvent::GridResize { grid: 2, width: 20, height: 9 });
        ed.apply(RedrawEvent::WinPos { grid: 2, win: 1000, start_row: 0, start_col: 0, width: 20, height: 9 });
        ed.apply(RedrawEvent::GridResize { grid: 3, width: 4, height: 2 });
        ed.apply(RedrawEvent::WinFloatPos {
            grid: 3,
            win: 1001,
            anchor: Default::default(),
            anchor_grid: 2,
            anchor_row: 0.0,
            anchor_col: 0.0,
            mouse_enabled: true,
            zindex: 50,
            compindex: 1,
            screen_row: 2,
            screen_col: 5,
        });
        assert_eq!(ed.grid_at(3, 6), Some((3, 1, 1)));
        assert_eq!(ed.grid_at(1, 1), Some((2, 1, 1)));
        assert_eq!(ed.grid_at(9, 3), Some((1, 9, 3)));
        assert_eq!(ed.grid_at(10, 0), None);
    }

    #[test]
    fn popupmenu_show_select_hide() {
        let mut ed = Editor::new();
        let item = |w: &str| PmItem { word: w.into(), ..Default::default() };
        ed.apply(RedrawEvent::PopupmenuShow { items: vec![item("a"), item("b")], selected: -1, row: 1, col: 2, grid: 2 });
        ed.apply(RedrawEvent::PopupmenuSelect { selected: 1 });
        assert_eq!(ed.popupmenu.as_ref().unwrap().selected, 1);
        ed.apply(RedrawEvent::PopupmenuHide);
        assert!(ed.popupmenu.is_none());
    }

    #[test]
    fn cmdline_levels() {
        let mut ed = Editor::new();
        let show = |text: &str, level| RedrawEvent::CmdlineShow {
            content: vec![StyledChunk { style: ChunkStyle::Id(0), text: text.into() }],
            pos: text.len(),
            firstc: ":".into(),
            prompt: String::new(),
            indent: 0,
            level,
        };
        ed.apply(show("echo ", 1));
        ed.apply(show("1+1", 2));
        assert_eq!(ed.cmdline.len(), 2);
        ed.apply(RedrawEvent::CmdlinePos { pos: 1, level: 2 });
        assert_eq!(ed.cmdline[1].pos, 1);
        ed.apply(RedrawEvent::CmdlineHide { level: 2 });
        assert_eq!(ed.cmdline.len(), 1);
        assert_eq!(ed.cmdline[0].text(), "echo ");
        ed.apply(show("echo 2", 1));
        assert_eq!(ed.cmdline.len(), 1);
        ed.apply(RedrawEvent::CmdlineHide { level: 1 });
        assert!(ed.cmdline.is_empty());
    }

    fn chunk(text: &str) -> Vec<StyledChunk> {
        vec![StyledChunk { style: ChunkStyle::Id(0), text: text.into() }]
    }

    fn msg(kind: &str, text: &str, replace_last: bool, append: bool, id: Option<&str>) -> RedrawEvent {
        RedrawEvent::MsgShow {
            kind: kind.into(),
            content: chunk(text),
            replace_last,
            history: true,
            append,
            id: id.map(Into::into),
        }
    }

    #[test]
    fn messages_show_replace_append_and_id() {
        let mut ed = Editor::new();
        ed.apply(msg("echo", "one", false, false, None));
        ed.apply(msg("echo", "two", false, false, None));
        ed.apply(msg("echo", "TWO", true, false, None));
        ed.apply(msg("echo", "!", false, true, None));
        let texts: Vec<String> = ed.messages.iter().map(Message::text).collect();
        assert_eq!(texts, ["one", "TWO!"]);

        ed.apply(msg("progress", "10%", false, false, Some("lsp")));
        let seq = ed.messages.last().unwrap().seq;
        ed.apply(msg("progress", "50%", false, false, Some("lsp")));
        assert_eq!(ed.messages.len(), 3);
        assert_eq!(ed.messages[2].text(), "50%");
        assert_ne!(ed.messages[2].seq, seq);

        ed.apply(msg("confirm", "Save changes?", false, false, None));
        ed.apply(RedrawEvent::MsgClear);
        assert_eq!(ed.messages.len(), 3, "msg_clear only removes prompts");

        ed.apply(msg("empty", "", false, false, None));
        assert!(ed.messages.is_empty());
    }

    #[test]
    fn status_segments_per_window_and_global() {
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::GridResize { grid: 1, width: 80, height: 24 });
        for (g, col, w) in [(2, 0, 30), (3, 31, 49)] {
            ed.apply(RedrawEvent::GridResize { grid: g, width: w, height: 23 });
            ed.apply(RedrawEvent::WinPos { grid: g, win: 1000 + g as i64, start_row: 0, start_col: col, width: w, height: 23 });
        }
        assert_eq!(
            ed.status_segments(false),
            [
                StatusSegment { row: 23, col_start: 0, col_end: 30, win: Some(1002) },
                StatusSegment { row: 23, col_start: 31, col_end: 80, win: Some(1003) },
            ]
        );
        assert_eq!(ed.status_segments(true), [StatusSegment { row: 23, col_start: 0, col_end: 80, win: None }]);
    }

    #[test]
    fn margins_before_win_pos_are_kept() {
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::GridResize { grid: 4, width: 10, height: 5 });
        ed.apply(RedrawEvent::WinViewportMargins { grid: 4, win: 1001, top: 1, bottom: 0, left: 0, right: 0 });
        ed.apply(RedrawEvent::WinPos { grid: 4, win: 1001, start_row: 0, start_col: 0, width: 10, height: 5 });
        assert_eq!(ed.windows[&4].margins.top, 1);
    }

    #[test]
    fn cursor_screen_pos_adds_window_origin() {
        let mut ed = Editor::new();
        ed.apply(RedrawEvent::GridResize { grid: 2, width: 10, height: 10 });
        ed.apply(RedrawEvent::WinPos { grid: 2, win: 1000, start_row: 3, start_col: 5, width: 10, height: 10 });
        ed.apply(RedrawEvent::GridCursorGoto { grid: 2, row: 1, col: 2 });
        assert_eq!(ed.cursor_screen_pos(), (4, 7));
    }
}
