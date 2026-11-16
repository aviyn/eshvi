use crate::grid::LineCell;
use crate::hl::{HlAttr, HlId, ModeInfo};

/// Value of an `option_set` event.
#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    Bool(bool),
    Int(i64),
    Str(String),
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatAnchor {
    #[default]
    NW,
    NE,
    SW,
    SE,
}

/// One completion item of `popupmenu_show`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PmItem {
    pub word: String,
    pub kind: String,
    pub menu: String,
    pub info: String,
}

/// Highlight of a cmdline chunk: an attribute id, or inline attributes.
#[derive(Clone, Debug, PartialEq)]
pub enum ChunkStyle {
    Id(HlId),
    Attr(HlAttr),
}

/// A piece of highlighted text (cmdline content, messages).
#[derive(Clone, Debug, PartialEq)]
pub struct StyledChunk {
    pub style: ChunkStyle,
    pub text: String,
}

/// A typed `redraw` event. See `:h ui-events`.
#[derive(Clone, Debug, PartialEq)]
pub enum RedrawEvent {
    // Global
    SetTitle(String),
    ModeInfoSet { cursor_style_enabled: bool, modes: Vec<ModeInfo> },
    ModeChange { mode: String, mode_idx: usize },
    OptionSet { name: String, value: OptionValue },
    MouseOn,
    MouseOff,
    BusyStart,
    BusyStop,
    Flush,
    /// Raw data for the UI's terminal (`nvim_ui_send`, to UIs attached with
    /// `stdout_tty`): image protocol commands, queries, progress.
    UiSend(Vec<u8>),

    // Linegrid
    GridResize { grid: u64, width: usize, height: usize },
    DefaultColorsSet { fg: u32, bg: u32, sp: u32 },
    /// `groups`: highlight group names behind this id (from `ext_hlstate`),
    /// e.g. `["NvimTreeFolderIcon"]`.
    HlAttrDefine { id: HlId, attr: HlAttr, groups: Vec<String> },
    HlGroupSet { name: String, id: HlId },
    GridLine { grid: u64, row: usize, col_start: usize, cells: Vec<LineCell> },
    GridClear { grid: u64 },
    GridDestroy { grid: u64 },
    GridCursorGoto { grid: u64, row: usize, col: usize },
    GridScroll { grid: u64, top: usize, bot: usize, left: usize, right: usize, rows: i64, cols: i64 },

    // Multigrid
    WinPos { grid: u64, win: i64, start_row: usize, start_col: usize, width: usize, height: usize },
    WinFloatPos {
        grid: u64,
        win: i64,
        anchor: FloatAnchor,
        anchor_grid: u64,
        anchor_row: f64,
        anchor_col: f64,
        mouse_enabled: bool,
        zindex: i64,
        compindex: i64,
        screen_row: i64,
        screen_col: i64,
    },
    WinExternalPos { grid: u64, win: i64 },
    WinHide { grid: u64 },
    WinClose { grid: u64 },
    MsgSetPos { grid: u64, row: usize, scrolled: bool, sep_char: String, zindex: i64, compindex: i64 },
    WinViewport {
        grid: u64,
        win: i64,
        topline: i64,
        botline: i64,
        curline: i64,
        curcol: i64,
        line_count: i64,
        scroll_delta: i64,
    },
    WinViewportMargins { grid: u64, win: i64, top: usize, bottom: usize, left: usize, right: usize },

    /// `tabline_update` (ext_tabline): the tab pages, by handle and name
    /// (the current window's buffer), and the current one.
    TablineUpdate { current: i64, tabs: Vec<(i64, String)> },

    // Popupmenu (ext_popupmenu)
    /// `grid` is -1 when anchored to an external cmdline.
    PopupmenuShow { items: Vec<PmItem>, selected: i64, row: i64, col: i64, grid: i64 },
    PopupmenuSelect { selected: i64 },
    PopupmenuHide,

    // Cmdline (ext_cmdline)
    CmdlineShow {
        content: Vec<StyledChunk>,
        pos: usize,
        firstc: String,
        prompt: String,
        indent: usize,
        level: u64,
    },
    CmdlinePos { pos: usize, level: u64 },
    CmdlineSpecialChar { c: String, shift: bool, level: u64 },
    CmdlineHide { level: u64 },
    CmdlineBlockShow { lines: Vec<Vec<StyledChunk>> },
    CmdlineBlockAppend { line: Vec<StyledChunk> },
    CmdlineBlockHide,

    // Messages (ext_messages)
    MsgShow {
        kind: String,
        content: Vec<StyledChunk>,
        replace_last: bool,
        history: bool,
        append: bool,
        /// Message id as a string (Neovim sends an integer or a string).
        id: Option<String>,
    },
    MsgClear,
    MsgShowmode(Vec<StyledChunk>),
    MsgShowcmd(Vec<StyledChunk>),
    MsgRuler(Vec<StyledChunk>),
    /// `:messages` output: `(kind, content, append)` entries.
    MsgHistoryShow(Vec<(String, Vec<StyledChunk>, bool)>),
}
