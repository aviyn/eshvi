//! Typed notifications from the `gui_bridge` Lua plugin.

use nvim_rs::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    /// Byte range of the parameter in the signature label.
    pub start: usize,
    pub end: usize,
    pub documentation: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub label: String,
    pub documentation: String,
    pub parameters: Vec<Parameter>,
    /// Per-signature active parameter (overrides the global one).
    pub active_parameter: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SignatureHelp {
    pub signatures: Vec<Signature>,
    pub active_signature: usize,
    pub active_parameter: Option<usize>,
}

impl SignatureHelp {
    pub fn active(&self) -> Option<(&Signature, Option<usize>)> {
        let sig = self.signatures.get(self.active_signature).or(self.signatures.first())?;
        Some((sig, sig.active_parameter.or(self.active_parameter)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub source: String,
    pub code: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    Hover,
    Signature,
    Diagnostic,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GuiEvent {
    Hover { contents: String, kind: String },
    /// A completion item's documentation, in Markdown, beside the menu in
    /// window `win` (empty: closed).
    Docs { contents: String, win: i64 },
    /// Scroll the documentation by rows (negative: up).
    DocsScroll(i64),
    /// Documentation for the native completion menu's item `selected`, asked
    /// for by the user (empty: none). `selected` -1 turns on on-request mode:
    /// the menu then shows only these, not the items' own `info`.
    PumDoc { selected: i64, contents: String },
    Signature(SignatureHelp),
    Diagnostic(Vec<Diagnostic>),
    /// `None` closes all popups.
    PopupClose(Option<PopupKind>),
    /// Window handles currently showing a sidebar filetype (e.g. NvimTree).
    Sidebar(Vec<i64>),
    /// This tab page's Markdown previews (`:EshviPreview`).
    Preview(Vec<PreviewWin>),
    /// A previewed buffer's text (again when it changes).
    PreviewText { buf: i64, text: String },
    /// The native status bar's data: the mode and the file in use.
    Status(Status),
    /// Editor options the GUI cares about.
    Options { laststatus: i64 },
    /// Global working directory.
    Cwd(String),
    /// The current file buffer: full path ("" if unnamed) and filetype.
    Buffer { path: String, filetype: String },
    /// `:EshviExplorer <action>`.
    Explorer(String),
    /// `:EshviGit <action>`.
    Git(String),
    /// `:EshviTermDebug <action>`: the hidden window with a native terminal.
    TermDebug(String),
    /// A file at a commit, to open as the Source Control Graph opens one
    /// (a picker of a file's history): `parent` is empty for a root commit.
    GitDiff { top: String, oid: String, parent: String, path: String, left_path: Option<String>, deleted: bool },
    /// Files with errors/warnings (all others have none).
    Diagnostics(Vec<FileDiagnostics>),
    /// Every diagnostic (hints left out) for the Problems list: `gui_problems`.
    Problems(Vec<Problem>),
    /// `:EshviPanel`, `:EshviTerminal`, `:EshviProblems` and `:EshviAgent`: which panel, and the
    /// arguments.
    Panel { kind: String, args: Vec<String> },
    /// The environment the agent panel gives its agents (Claude Code's IDE connection:
    /// `CLAUDE_CODE_SSE_PORT`, `ENABLE_IDE_INTEGRATION`), sent once when `agent.lua` loads.
    AgentEnv(Vec<(String, String)>),
    /// ⌃h/j/k/l from the edge of Neovim's windows (`Eshvi.navigate`): "h", "j", "k" or "l".
    Navigate(String),
    /// The file shown in each window: `(window handle, full path, modified)`.
    Windows(Vec<(i64, String, bool)>),
    /// A buffer was written to this file (full path).
    Saved(String),
    /// A file was saved with its Changes diff on screen: the end of undoing
    /// all of its hunks, when it was created by the agents.
    DiffSaved(String),
    /// An edit the diff view asked Neovim for is done (`gui_diff_edit`): `op` is
    /// "revert" (lines put back and the file written) or "guard" (whether
    /// the files may be discarded: none has unsaved edits); `ok` and what to
    /// tell the user when not.
    DiffEdit { op: String, path: String, ok: bool, message: String },
    /// The colorscheme's syntax colors by role ("keyword", "string", …), for
    /// code in popups.
    Syntax(std::collections::BTreeMap<String, SyntaxStyle>),
    /// The colorscheme's foreground (0xRRGGBB) for git status and
    /// diagnostics: "added", "modified", "deleted", "error", "warn", "info".
    /// A role the theme does not color is missing.
    DecoColors(std::collections::BTreeMap<String, u32>),
    /// The colorscheme's backgrounds for the diff view: "add", "delete",
    /// "change", "text" (`DiffAdd`, `DiffDelete`, `DiffChange`, `DiffText`);
    /// a group with no background is missing.
    DiffColors(std::collections::BTreeMap<String, u32>),
    /// `g:terminal_color_0..15` of the colorscheme (0xRRGGBB), `None` where unset.
    TermColors([Option<u32>; 16]),
    /// Text for the clipboard: a Visual selection the mouse made (`[neovim] copy_on_select`).
    Copy(String),
    /// Neovim's menus (`:menu`), for the native menu bar.
    Menu(Vec<MenuNode>),
    /// Neovim shows its right-click menu (PopUp) with these items and waits
    /// for a choice.
    PopupMenu(Vec<MenuNode>),
    /// The latest running LSP progress (`$/progress`); `None` once all
    /// are done.
    Progress(Option<Progress>),
    /// Something only the GUI can do, asked for by a menu item:
    /// "open_file", "open_folder", "save_as", "zoom_in", "zoom_out",
    /// "zoom_reset".
    Action(String),
    /// Open the picker (`picker.lua`).
    PickerOpen(PickerOpen),
    /// Neovim closes the picker `id` (quitting): no answer is expected.
    PickerClose(i64),
    /// The answer to a preview request (`Eshvi.picker.preview`).
    PickerPreview(PickerPreview),
    /// Rows for a live picker's query `gen` (`Eshvi.picker.query`'s answer).
    PickerItems { id: i64, gen: i64, items: Vec<PickerItem> },
    /// What was typed before the picker `id` opened, which Neovim took out of
    /// its typeahead (`keytrans` notation: `a`, `<CR>`, `<Esc>`, `<BS>`,
    /// `<Space>`, `<lt>`), in order. The picker takes them as typed in its field.
    PickerTyped { id: i64, keys: Vec<String> },
    /// A tree view's rows (`Eshvi.tree.set`): the roots (`parent` is `None`; a
    /// new `gen`) or the children of `parent`, answering the app's request.
    TreeSet(TreeSet),
    /// Select a node and show it: `path` is the ids from the root down to it.
    TreeSelect { view: String, id: String, path: Vec<String> },
    /// A node's right-click menu, asked for with `Eshvi.tree.menu`.
    TreeMenu { view: String, id: String, items: Vec<String> },
    TreeUnregister(String),
    /// A tree view registered with a side bar view of its own or keys
    /// (`Eshvi.tree.register`).
    TreeRegister(TreeRegister),
    /// `Eshvi.tree.show/hide` and `:EshviTree`: `action` is `show`, `focus`
    /// (again: back to the editor), `hide` or `toggle`; `focus` makes `show`
    /// take the keyboard.
    TreeShow { view: String, action: String, focus: bool },
    /// `:EshviOutline <action>`.
    Outline(String),
    /// A session was loaded: the GUI's part of it, to apply (`session.lua`).
    Session(GuiSession),
}

/// The GUI's part of a session, which `:mksession` can't hold: the side bar. The GUI
/// sends it to Neovim whenever it changes (`Eshvi.session.set_gui`), and gets it back
/// when a session is loaded (`gui_session`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuiSession {
    /// The side bar is shown.
    pub visible: bool,
    /// Its width in points.
    pub width: f32,
    /// What it shows: "explorer", "git", or a tree view's name.
    pub view: String,
    /// The explorer's expanded folders, parents before their children.
    pub expanded: Vec<String>,
    /// The bottom panel; none when the GUI has none to save.
    pub panel: Option<PanelSession>,
    /// The agent panel; none when the GUI has none to save.
    pub agent: Option<AgentSession>,
}

/// The agent panel's part of a session: shown or not, and how wide (points). Its agents
/// aren't restored: a shown panel starts a fresh one on load.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentSession {
    pub shown: bool,
    pub width: f32,
}

/// The bottom panel's part of a session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelSession {
    pub shown: bool,
    /// Its height in points.
    pub height: f32,
    /// "problems", "git" or "terminal" (terminals aren't restored: it comes back as Problems).
    pub view: String,
}

impl GuiSession {
    pub fn to_value(&self) -> Value {
        let mut map = vec![
            ("visible".into(), self.visible.into()),
            ("width".into(), Value::F64(self.width as f64)),
            ("view".into(), self.view.as_str().into()),
            ("expanded".into(), Value::Array(self.expanded.iter().map(|p| Value::from(p.as_str())).collect())),
        ];
        if let Some(p) = &self.panel {
            map.push((
                "panel".into(),
                Value::Map(vec![
                    ("shown".into(), p.shown.into()),
                    ("height".into(), Value::F64(p.height as f64)),
                    ("view".into(), p.view.as_str().into()),
                ]),
            ));
        }
        if let Some(a) = &self.agent {
            map.push(("agent".into(), Value::Map(vec![("shown".into(), a.shown.into()), ("width".into(), Value::F64(a.width as f64))])));
        }
        Value::Map(map)
    }

    /// Reads what `to_value` made, and what came back from JSON (an empty list can
    /// have become an empty object).
    pub fn from_value(v: &Value) -> Self {
        Self {
            visible: get(v, "visible").and_then(Value::as_bool).unwrap_or(true),
            width: get(v, "width").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            view: string(v, "view"),
            expanded: list(v, "expanded").iter().filter_map(|p| p.as_str().map(str::to_owned)).collect(),
            panel: get(v, "panel").filter(|p| p.is_map()).map(|p| PanelSession {
                shown: get(p, "shown").and_then(Value::as_bool).unwrap_or(false),
                height: get(p, "height").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                view: string(p, "view"),
            }),
            agent: get(v, "agent").filter(|a| a.is_map()).map(|a| AgentSession {
                shown: get(a, "shown").and_then(Value::as_bool).unwrap_or(false),
                width: get(a, "width").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            }),
        }
    }
}

/// What `gui_tree_set` carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeSet {
    pub view: String,
    pub gen: i64,
    pub parent: Option<String>,
    pub nodes: Vec<TreeNode>,
    /// Shown in place of rows ("No symbols found").
    pub message: Option<String>,
    /// How many levels are open at first.
    pub expand: Option<usize>,
    /// The buffer the tree is of.
    pub buf: Option<i64>,
    /// Nodes have a right-click menu (`menu` in the source).
    pub menu: bool,
    /// `Eshvi.tree.update`: merge into what the app has, by id, instead of
    /// replacing it.
    pub merge: bool,
}

/// What `gui_tree_register` carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeRegister {
    pub view: String,
    /// The view has a place of its own in the side bar.
    pub side: Option<TreeSide>,
    /// Keys the source wants while the view has the keyboard.
    pub keys: Vec<String>,
}

/// A tree view's side bar button, title and header buttons.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeSide {
    pub title: String,
    /// A codicon's name.
    pub icon: String,
    pub tooltip: String,
    pub buttons: Vec<TreeButton>,
    /// The groups of nodes the header's Filter menu shows or hides; none, no menu.
    pub filters: Vec<TreeFilter>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeButton {
    pub icon: String,
    pub tooltip: String,
}

/// One toggle of a tree view's Filter menu: the nodes whose `group` is `id`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeFilter {
    pub id: String,
    pub label: String,
    /// Off when the view registers (the default, or what the user last chose).
    pub hidden: bool,
}

/// One node of a tree view, as `tree.lua` sends it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeNode {
    pub id: String,
    pub label: String,
    pub hint: String,
    /// An LSP `SymbolKind`'s name.
    pub kind: String,
    /// A glyph drawn before the label, in the color `fg` (of the group `hl`).
    pub icon: String,
    pub hl: String,
    pub fg: Option<u32>,
    pub tooltip: String,
    pub line: Option<usize>,
    pub end_line: Option<usize>,
    /// It has children, even if they aren't here yet.
    pub expandable: bool,
    /// The filter it answers to (`TreeFilter::id`); empty, it is always shown.
    pub group: String,
    /// Starts open the first time the app receives it (a later update doesn't reopen it once the user closed it).
    pub open: bool,
    /// Only holds other rows: accepting it opens or closes it.
    pub folder: bool,
    pub children: Vec<TreeNode>,
}

/// A picker's contents and kind, as `picker.lua` sends them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerOpen {
    pub id: i64,
    /// The field's placeholder ("Find files", a select's prompt).
    pub title: String,
    /// "files" and "git_files" (the app walks `cwd`), "smart" (Neovim's
    /// `items` first, then the walk) or "items" (Neovim's alone).
    pub source: String,
    pub cwd: String,
    /// Rows are files: an icon, the name, then the folder.
    pub files: bool,
    /// Tab marks rows.
    pub multi: bool,
    /// A file the walk leaves out (the one being edited, for "smart").
    pub exclude: Option<String>,
    pub items: Vec<PickerItem>,
    /// A preview of the selected row goes beside the list.
    pub preview: bool,
    /// What the field starts with (grep's query).
    pub query: String,
    /// Grep: the query is text, not a pattern.
    pub literal: bool,
    /// Grep: only these files (the listed buffers'), else the folder's.
    pub grep_files: Option<Vec<String>>,
    /// Grep only the files under the folder with one of these extensions.
    pub extensions: Option<Vec<String>>,
    /// The row selected to start with, from 1 (0: the first).
    pub selected: usize,
    /// Rows show their line number (the buffer's lines).
    pub numbered: bool,
    /// The app doesn't filter: it asks Neovim (`Eshvi.picker.query`) for the
    /// rows of each query, and Neovim answers with `gui_picker_items`.
    pub live: bool,
    /// What is shown in place of rows while the query is empty ("Type to
    /// search symbols").
    pub hint: String,
    /// Tell Neovim (`Eshvi.picker.selected`) when the selection has stayed on
    /// a row a moment (the colorschemes, applied live).
    pub on_select: bool,
    /// The row to select once the rows are there (a resumed picker): the
    /// row with this `text`, `path` and `lnum`.
    pub want: Option<PickerWant>,
}

/// A row named by what it shows, for a resumed picker's selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerWant {
    pub text: String,
    pub path: Option<String>,
    pub lnum: Option<usize>,
}

/// What the picker held when it closed, for `Eshvi.picker.resume`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerState {
    pub query: String,
    /// The selected row.
    pub row: Option<PickerWant>,
}

/// One row Neovim knows. Its place in `PickerOpen::items` (from 1) is what
/// the answer names it by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerItem {
    /// What is matched and shown first (a file's path relative to the
    /// picker's folder, as far as it is under it).
    pub text: String,
    /// Dim, after `text`.
    pub desc: String,
    /// The file's full path.
    pub path: Option<String>,
    pub lnum: Option<usize>,
    pub col: Option<usize>,
    /// "buffer" or "recent": ranked a little above the rest.
    pub group: Option<String>,
    /// The buffer it is in, whose text (saved or not) the preview shows.
    pub buf: Option<i64>,
    /// A symbol's kind ("Function", "Method"…): drawn as the completion
    /// menu's badge.
    pub kind: Option<String>,
    /// How deep in its parents a symbol is.
    pub depth: usize,
    /// A diagnostic's severity, 1 (error) to 4 (hint).
    pub severity: Option<u8>,
    /// What the filter matches, when it isn't `text` alone.
    pub search: Option<String>,
    /// A location's line, shown after its file.
    pub line: Option<PickerLine>,
    /// Stretches of the target line (bytes) the preview marks.
    pub marks: Vec<(usize, usize)>,
    /// Dim, at the row's right (a shortcut).
    pub hint: String,
    /// Dim, before `text` (a keymap's mode).
    pub prefix: String,
    /// `desc` in the grid's font (a register's text).
    pub mono: bool,
    /// A menu item's action: the row is left out while the menu greys that item.
    pub gate: Option<String>,
}

/// The line a location is on, as the row shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerLine {
    /// Trimmed, and cut if long.
    pub text: String,
    /// The location's stretches in `text` (bytes).
    pub ranges: Vec<(usize, usize)>,
    /// The same in the whole line, which the preview shows.
    pub line_ranges: Vec<(usize, usize)>,
}

/// What the picker's user chose, for `Eshvi.picker.accept`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerChoice {
    /// The row's place in the open message's items (from 1), if it had one.
    pub idx: Option<usize>,
    /// A file the app found itself (the walk).
    pub path: Option<String>,
    pub lnum: Option<usize>,
    pub col: Option<usize>,
    /// The row's text, for a quickfix entry (a grep match's line).
    pub text: Option<String>,
}

/// What the picker's preview shows, from `Eshvi.picker.preview`: a window of
/// a file (or buffer) around `lnum`, with the colors treesitter gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerPreview {
    /// The request this answers.
    pub req: i64,
    /// The file asked for, as asked (the buffer's name for a buffer).
    pub path: String,
    /// The buffer asked for, if it was one.
    pub buf: Option<i64>,
    /// The file's line number of `lines[0]` (from 1).
    pub first_line: usize,
    /// A window of the text (a line's bytes that aren't UTF-8 are `?`).
    pub lines: Vec<String>,
    /// Colors, in the order to apply them.
    pub spans: Vec<PreviewSpan>,
    /// The line the window is around.
    pub lnum: usize,
    /// How many lines the file has.
    pub total: usize,
    /// Instead of lines: "Binary file", "Too large to preview", …
    pub note: String,
}

/// A colored stretch of one line of a preview.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PreviewSpan {
    /// Index into the answer's `lines`.
    pub line: usize,
    /// Bytes from the line's start, `start` to `end` (not included).
    pub start: usize,
    pub end: usize,
    /// 0xRRGGBB.
    pub fg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
}

/// What the native status bar shows besides the cursor's place (`status.lua`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// `nvim_get_mode().mode`: "n", "V", "\x16", "no", "Rv"...
    pub mode: String,
    /// The file's full path ("" if it has none).
    pub path: String,
    /// The buffer's own short name ("" if none, or a terminal's).
    pub name: String,
    pub filetype: String,
    pub buftype: String,
    pub buf: i64,
    /// The cursor's column in characters, 1-based (0: not known yet).
    pub col: i64,
    pub modified: bool,
    /// Readonly or not modifiable.
    pub readonly: bool,
}

/// One piece of LSP work in progress.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// The server's name ("rust_analyzer").
    pub source: String,
    pub title: String,
    pub message: String,
    /// 0..=100, when the server reports it.
    pub percent: Option<u8>,
}

/// A menu, menu item or separator (a name like `-sep-`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuNode {
    pub name: String,
    /// The shortcut shown after the name (`File.Save<Tab>⌘S`).
    pub key: String,
    /// eshvi's action for its own items ("copy", "save", …).
    pub action: Option<String>,
    /// Greyed out (`:menu disable`).
    pub disabled: bool,
    /// A checkmark item, checked or not (Buffers marks the current one).
    pub checked: Option<bool>,
    /// Items of a menu; `None` for an item.
    pub children: Option<Vec<MenuNode>>,
}

impl MenuNode {
    pub fn is_separator(&self) -> bool {
        self.name.len() > 1 && self.name.starts_with('-') && self.name.ends_with('-')
    }
}

fn menu_node(v: &Value) -> MenuNode {
    MenuNode {
        name: string(v, "name"),
        key: string(v, "key"),
        action: get(v, "action").and_then(Value::as_str).map(str::to_owned),
        disabled: get(v, "enabled").and_then(Value::as_bool) == Some(false),
        checked: get(v, "checked").and_then(Value::as_bool),
        children: get(v, "children").map(|c| c.as_array().map(|a| a.iter().map(menu_node).collect()).unwrap_or_default()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxStyle {
    /// 0xRRGGBB.
    pub fg: u32,
    pub italic: bool,
    pub bold: bool,
}

/// A window showing a Markdown preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewWin {
    pub win: i64,
    /// The Markdown buffer shown.
    pub buf: i64,
    /// The source line to show (1-based): the cursor's.
    pub line: usize,
    /// A window of its own beside the file (else the file's, in place).
    pub side: bool,
    /// The file (for its links to files beside it).
    pub path: String,
    /// The search highlighted in the buffer ('hlsearch'), if any.
    pub search: Option<PreviewSearch>,
}

/// A search to mark in a preview: the different texts it matches in the
/// buffer, and the Search group's colors (0xRRGGBB); the match under the
/// cursor, and CurSearch's colors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviewSearch {
    pub texts: Vec<String>,
    pub bg: Option<u32>,
    pub fg: Option<u32>,
    pub current: Option<SearchMatch>,
    pub cur_bg: Option<u32>,
    pub cur_fg: Option<u32>,
}

/// One match of a search: its line (1-based), its byte column (0-based) and text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchMatch {
    pub line: usize,
    pub col: usize,
    pub text: String,
}

/// One diagnostic of the native Problems list. Lines and columns count from 1.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub lnum: usize,
    pub col: usize,
    pub end_lnum: usize,
    pub end_col: usize,
    /// 1 error, 2 warning, 3 info.
    pub severity: u8,
    pub message: String,
    pub source: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileDiagnostics {
    pub path: String,
    pub errors: u32,
    pub warnings: u32,
}

fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_map()?.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
}

fn string(v: &Value, key: &str) -> String {
    get(v, key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// A string whose bytes may not be UTF-8, with each bad byte turned into a
/// "?" so that every byte offset into it stays where it was.
fn keep_length(v: &Value) -> String {
    let bytes: &[u8] = match v {
        Value::String(s) => s.as_bytes(),
        Value::Binary(b) => b,
        _ => return String::new(),
    };
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        out.extend(std::iter::repeat_n('?', chunk.invalid().len()));
    }
    out
}

/// `{ line, start, end, fg, bold, italic }` as an array.
fn preview_span(v: &Value) -> Option<PreviewSpan> {
    let a = v.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_i64);
    Some(PreviewSpan {
        line: usize::try_from(n(0)?).ok()?,
        start: usize::try_from(n(1)?).ok()?,
        end: usize::try_from(n(2)?).ok()?,
        fg: n(3).and_then(|c| u32::try_from(c).ok()),
        bold: n(4) == Some(1),
        italic: n(5) == Some(1),
    })
}

/// `{ {start, end}, … }` as byte ranges.
fn byte_ranges(v: &Value, key: &str) -> Vec<(usize, usize)> {
    list(v, key)
        .iter()
        .filter_map(|r| {
            let a = r.as_array()?;
            let n = |i: usize| a.get(i).and_then(Value::as_i64).and_then(|n| usize::try_from(n).ok());
            Some((n(0)?, n(1)?))
        })
        .collect()
}

/// One row, as `picker.lua` sends it.
fn picker_item(i: &Value) -> PickerItem {
    PickerItem {
        text: string(i, "text"),
        desc: string(i, "desc"),
        path: get(i, "path").and_then(Value::as_str).map(str::to_owned),
        lnum: index(i, "lnum"),
        col: index(i, "col"),
        group: get(i, "group").and_then(Value::as_str).map(str::to_owned),
        buf: get(i, "buf").and_then(Value::as_i64),
        kind: get(i, "kind").and_then(Value::as_str).map(str::to_owned),
        depth: index(i, "depth").unwrap_or(0),
        severity: get(i, "severity").and_then(Value::as_i64).and_then(|n| u8::try_from(n).ok()),
        search: get(i, "search").and_then(Value::as_str).map(str::to_owned),
        line: get(i, "line").filter(|l| l.is_map()).map(|l| PickerLine {
            text: get(l, "text").map(keep_length).unwrap_or_default(),
            ranges: byte_ranges(l, "ranges"),
            line_ranges: byte_ranges(l, "line_ranges"),
        }),
        marks: byte_ranges(i, "marks"),
        hint: string(i, "hint"),
        prefix: string(i, "prefix"),
        mono: get(i, "mono").and_then(Value::as_bool).unwrap_or(false),
        gate: get(i, "gate").and_then(Value::as_str).map(str::to_owned),
    }
}

fn tree_node(n: &Value) -> TreeNode {
    TreeNode {
        id: string(n, "id"),
        label: string(n, "label"),
        hint: string(n, "hint"),
        kind: string(n, "kind"),
        icon: string(n, "icon"),
        hl: string(n, "hl"),
        fg: get(n, "fg").and_then(Value::as_i64).and_then(|c| u32::try_from(c).ok()),
        tooltip: string(n, "tooltip"),
        line: index(n, "line"),
        end_line: index(n, "end_line"),
        expandable: get(n, "expandable").and_then(Value::as_bool).unwrap_or(false),
        group: string(n, "group"),
        open: get(n, "open").and_then(Value::as_bool).unwrap_or(false),
        folder: get(n, "folder").and_then(Value::as_bool).unwrap_or(false),
        children: list(n, "children").iter().map(tree_node).collect(),
    }
}

fn index(v: &Value, key: &str) -> Option<usize> {
    get(v, key).and_then(Value::as_i64).and_then(|i| usize::try_from(i).ok())
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    get(v, key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// Parse a `gui_*` notification. `args` is the rpcnotify argument list.
pub fn parse(name: &str, args: &[Value]) -> Option<GuiEvent> {
    let p = args.first().unwrap_or(&Value::Nil);
    Some(match name {
        "gui_hover" => GuiEvent::Hover { contents: string(p, "contents"), kind: string(p, "kind") },
        "gui_docs" => GuiEvent::Docs { contents: string(p, "contents"), win: get(p, "win").and_then(Value::as_i64).unwrap_or(0) },
        "gui_pum_doc" => GuiEvent::PumDoc {
            selected: get(p, "selected").and_then(Value::as_i64).unwrap_or(-1),
            contents: string(p, "contents"),
        },
        "gui_docs_scroll" => GuiEvent::DocsScroll(get(p, "rows").and_then(Value::as_i64).unwrap_or(0)),
        "gui_signature" => GuiEvent::Signature(SignatureHelp {
            signatures: list(p, "signatures")
                .iter()
                .map(|s| Signature {
                    label: string(s, "label"),
                    documentation: string(s, "documentation"),
                    parameters: list(s, "parameters")
                        .iter()
                        .map(|p| Parameter {
                            start: index(p, "start").unwrap_or(0),
                            end: index(p, "end").unwrap_or(0),
                            documentation: string(p, "documentation"),
                        })
                        .collect(),
                    active_parameter: index(s, "active_parameter"),
                })
                .collect(),
            active_signature: index(p, "active_signature").unwrap_or(0),
            active_parameter: index(p, "active_parameter"),
        }),
        "gui_diagnostic" => GuiEvent::Diagnostic(
            list(p, "diagnostics")
                .iter()
                .map(|d| Diagnostic {
                    severity: match string(d, "severity").as_str() {
                        "Warning" => Severity::Warning,
                        "Info" => Severity::Info,
                        "Hint" => Severity::Hint,
                        _ => Severity::Error,
                    },
                    message: string(d, "message"),
                    source: string(d, "source"),
                    code: string(d, "code"),
                })
                .collect(),
        ),
        "gui_popup_close" => GuiEvent::PopupClose(match get(p, "kind").and_then(Value::as_str) {
            Some("hover") => Some(PopupKind::Hover),
            Some("signature") => Some(PopupKind::Signature),
            Some("diagnostic") => Some(PopupKind::Diagnostic),
            _ => None,
        }),
        "gui_options" => GuiEvent::Options { laststatus: get(p, "laststatus").and_then(Value::as_i64).unwrap_or(2) },
        "gui_status" => {
            let flag = |k| get(p, k).and_then(Value::as_bool).unwrap_or(false);
            GuiEvent::Status(Status {
                mode: string(p, "mode"),
                path: string(p, "path"),
                name: string(p, "name"),
                filetype: string(p, "filetype"),
                buftype: string(p, "buftype"),
                buf: get(p, "buf").and_then(Value::as_i64).unwrap_or(0),
                col: get(p, "col").and_then(Value::as_i64).unwrap_or(0),
                modified: flag("modified"),
                readonly: flag("readonly"),
            })
        }
        "gui_picker_open" => GuiEvent::PickerOpen(PickerOpen {
            id: get(p, "id").and_then(Value::as_i64).unwrap_or(0),
            title: string(p, "title"),
            source: string(p, "source"),
            cwd: string(p, "cwd"),
            files: get(p, "files").and_then(Value::as_bool).unwrap_or(false),
            multi: get(p, "multi").and_then(Value::as_bool).unwrap_or(false),
            exclude: get(p, "exclude").and_then(Value::as_str).map(str::to_owned),
            items: list(p, "items").iter().map(picker_item).collect(),
            preview: get(p, "preview").and_then(Value::as_bool).unwrap_or(false),
            query: string(p, "query"),
            literal: get(p, "literal").and_then(Value::as_bool).unwrap_or(false),
            grep_files: get(p, "grep_files").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()),
            extensions: get(p, "extensions").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()),
            selected: index(p, "selected").unwrap_or(0),
            numbered: get(p, "numbered").and_then(Value::as_bool).unwrap_or(false),
            live: get(p, "live").and_then(Value::as_bool).unwrap_or(false),
            hint: string(p, "hint"),
            on_select: get(p, "on_select").and_then(Value::as_bool).unwrap_or(false),
            want: get(p, "want").filter(|w| w.is_map()).map(|w| PickerWant {
                text: string(w, "text"),
                path: get(w, "path").and_then(Value::as_str).map(str::to_owned),
                lnum: index(w, "lnum"),
            }),
        }),
        "gui_picker_typed" => GuiEvent::PickerTyped {
            id: get(p, "id").and_then(Value::as_i64).unwrap_or(0),
            keys: list(p, "keys").iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        },
        "gui_picker_items" => GuiEvent::PickerItems {
            id: get(p, "id").and_then(Value::as_i64).unwrap_or(0),
            gen: get(p, "gen").and_then(Value::as_i64).unwrap_or(0),
            items: list(p, "items").iter().map(picker_item).collect(),
        },
        "gui_picker_preview" => GuiEvent::PickerPreview(PickerPreview {
            req: get(p, "req").and_then(Value::as_i64).unwrap_or(0),
            path: string(p, "path"),
            buf: get(p, "buf").and_then(Value::as_i64),
            first_line: index(p, "first_line").unwrap_or(1),
            lines: list(p, "lines").iter().map(keep_length).collect(),
            spans: list(p, "spans").iter().filter_map(preview_span).collect(),
            lnum: index(p, "lnum").unwrap_or(1),
            total: index(p, "total").unwrap_or(0),
            note: string(p, "note"),
        }),
        "gui_picker_close" => GuiEvent::PickerClose(get(p, "id").and_then(Value::as_i64).unwrap_or(0)),
        "gui_tree_set" => GuiEvent::TreeSet(TreeSet {
            view: string(p, "view"),
            gen: get(p, "gen").and_then(Value::as_i64).unwrap_or(0),
            parent: get(p, "parent").and_then(Value::as_str).map(str::to_owned),
            nodes: list(p, "nodes").iter().map(tree_node).collect(),
            message: get(p, "message").and_then(Value::as_str).map(str::to_owned),
            expand: index(p, "expand"),
            buf: get(p, "buf").and_then(Value::as_i64),
            menu: get(p, "menu").and_then(Value::as_bool).unwrap_or(false),
            merge: get(p, "merge").and_then(Value::as_bool).unwrap_or(false),
        }),
        "gui_tree_register" => GuiEvent::TreeRegister(TreeRegister {
            view: string(p, "view"),
            side: get(p, "side").filter(|s| !matches!(s, Value::Nil)).map(|s| TreeSide {
                title: string(s, "title"),
                icon: string(s, "icon"),
                tooltip: string(s, "tooltip"),
                buttons: list(s, "buttons").iter().map(|b| TreeButton { icon: string(b, "icon"), tooltip: string(b, "tooltip") }).collect(),
                filters: list(s, "filters")
                    .iter()
                    .map(|f| TreeFilter { id: string(f, "id"), label: string(f, "label"), hidden: get(f, "hidden").and_then(Value::as_bool).unwrap_or(false) })
                    .collect(),
            }),
            keys: list(p, "keys").iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        }),
        "gui_tree_show" => GuiEvent::TreeShow {
            view: string(p, "view"),
            action: string(p, "action"),
            focus: get(p, "focus").and_then(Value::as_bool).unwrap_or(false),
        },
        "gui_tree_select" => GuiEvent::TreeSelect {
            view: string(p, "view"),
            id: string(p, "id"),
            path: list(p, "path").iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        },
        "gui_tree_menu" => GuiEvent::TreeMenu {
            view: string(p, "view"),
            id: string(p, "id"),
            items: list(p, "items").iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        },
        "gui_tree_unregister" => GuiEvent::TreeUnregister(string(p, "view")),
        "gui_outline" => GuiEvent::Outline(string(p, "action")),
        "gui_cwd" => GuiEvent::Cwd(string(p, "cwd")),
        "gui_buffer" => GuiEvent::Buffer { path: string(p, "path"), filetype: string(p, "filetype") },
        "gui_explorer" => GuiEvent::Explorer(string(p, "action")),
        "gui_git" => GuiEvent::Git(string(p, "action")),
        "gui_term_debug" => GuiEvent::TermDebug(string(p, "action")),
        "gui_git_diff" => GuiEvent::GitDiff {
            top: string(p, "top"),
            oid: string(p, "oid"),
            parent: string(p, "parent"),
            path: string(p, "path"),
            left_path: get(p, "left_path").and_then(Value::as_str).map(str::to_owned),
            deleted: get(p, "deleted").and_then(Value::as_bool).unwrap_or(false),
        },
        "gui_diagnostics" => GuiEvent::Diagnostics(
            list(p, "files")
                .iter()
                .map(|f| FileDiagnostics {
                    path: string(f, "path"),
                    errors: index(f, "errors").unwrap_or(0) as u32,
                    warnings: index(f, "warnings").unwrap_or(0) as u32,
                })
                .collect(),
        ),
        "gui_problems" => GuiEvent::Problems(
            list(p, "items")
                .iter()
                .map(|d| Problem {
                    file: string(d, "file"),
                    lnum: index(d, "lnum").unwrap_or(1),
                    col: index(d, "col").unwrap_or(1),
                    end_lnum: index(d, "end_lnum").unwrap_or(1),
                    end_col: index(d, "end_col").unwrap_or(1),
                    severity: index(d, "severity").unwrap_or(1).clamp(1, 3) as u8,
                    message: string(d, "message"),
                    source: string(d, "source"),
                    // (A code is a string or a number.)
                    code: get(d, "code").map(|c| c.as_str().map(str::to_owned).unwrap_or_else(|| c.to_string())).filter(|c| c != "nil").unwrap_or_default(),
                })
                .collect(),
        ),
        "gui_panel" => GuiEvent::Panel {
            kind: string(p, "kind"),
            args: list(p, "args").iter().filter_map(|a| a.as_str().map(str::to_owned)).collect(),
        },
        "gui_navigate" => GuiEvent::Navigate(string(p, "dir")),
        "gui_agent_env" => GuiEvent::AgentEnv(
            p.as_map()
                .map(|m| m.iter().filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.as_str()?.to_owned()))).collect())
                .unwrap_or_default(),
        ),
        "gui_syntax" => GuiEvent::Syntax(
            p.as_map()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| {
                            let style = SyntaxStyle {
                                fg: get(v, "fg")?.as_u64()? as u32,
                                italic: get(v, "italic").and_then(Value::as_bool).unwrap_or(false),
                                bold: get(v, "bold").and_then(Value::as_bool).unwrap_or(false),
                            };
                            Some((k.as_str()?.to_owned(), style))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "gui_deco_colors" => GuiEvent::DecoColors(
            p.as_map()
                .map(|m| m.iter().filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.as_u64()? as u32))).collect())
                .unwrap_or_default(),
        ),
        "gui_diff_colors" => GuiEvent::DiffColors(
            p.as_map()
                .map(|m| m.iter().filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.as_u64()? as u32))).collect())
                .unwrap_or_default(),
        ),
        "gui_term_colors" => {
            // A list of 16: a color, or -1 where the scheme sets none.
            let mut colors = [None; 16];
            for (slot, v) in colors.iter_mut().zip(p.as_array().map(|a| a.as_slice()).unwrap_or_default()) {
                *slot = v.as_i64().and_then(|n| u32::try_from(n).ok());
            }
            GuiEvent::TermColors(colors)
        }
        "gui_copy" => GuiEvent::Copy(p.as_str().unwrap_or_default().to_owned()),
        "gui_menu" => GuiEvent::Menu(list(p, "menus").iter().map(menu_node).collect()),
        "gui_popup_menu" => GuiEvent::PopupMenu(list(p, "items").iter().map(menu_node).collect()),
        "gui_action" => GuiEvent::Action(p.as_str().unwrap_or_default().to_owned()),
        "gui_windows" => GuiEvent::Windows(
            list(p, "wins")
                .iter()
                .filter_map(|w| {
                    let modified = get(w, "modified").and_then(Value::as_bool).unwrap_or(false);
                    Some((get(w, "win")?.as_i64()?, string(w, "path"), modified))
                })
                .collect(),
        ),
        "gui_diff_saved" => GuiEvent::DiffSaved(p.as_str().unwrap_or_default().to_owned()),
        "gui_diff_edit" => GuiEvent::DiffEdit {
            op: string(p, "op"),
            path: string(p, "path"),
            ok: get(p, "ok").and_then(Value::as_bool).unwrap_or(false),
            message: string(p, "message"),
        },
        "gui_saved" => GuiEvent::Saved(p.as_str().unwrap_or_default().to_owned()),
        "gui_progress" => GuiEvent::Progress(p.as_map().map(|_| Progress {
            source: string(p, "source"),
            title: string(p, "title"),
            message: string(p, "message"),
            percent: get(p, "percent").and_then(Value::as_u64).map(|n| n.min(100) as u8),
        })),
        "gui_sidebar" => GuiEvent::Sidebar(list(p, "wins").iter().filter_map(Value::as_i64).collect()),
        "gui_preview" => GuiEvent::Preview(
            list(p, "wins")
                .iter()
                .filter_map(|w| {
                    Some(PreviewWin {
                        win: get(w, "win")?.as_i64()?,
                        buf: get(w, "buf")?.as_i64()?,
                        line: get(w, "line").and_then(Value::as_u64).unwrap_or(1) as usize,
                        side: get(w, "side").and_then(Value::as_bool).unwrap_or(false),
                        path: string(w, "path"),
                        search: get(w, "search").filter(|s| s.is_map()).map(|s| PreviewSearch {
                            texts: list(s, "texts").iter().filter_map(|t| t.as_str().map(str::to_owned)).collect(),
                            bg: get(s, "bg").and_then(Value::as_u64).map(|c| c as u32),
                            fg: get(s, "fg").and_then(Value::as_u64).map(|c| c as u32),
                            current: get(s, "current").filter(|c| c.is_map()).and_then(|c| {
                                Some(SearchMatch {
                                    line: get(c, "line")?.as_u64()? as usize,
                                    col: get(c, "col")?.as_u64()? as usize,
                                    text: string(c, "text"),
                                })
                            }),
                            cur_bg: get(s, "cur_bg").and_then(Value::as_u64).map(|c| c as u32),
                            cur_fg: get(s, "cur_fg").and_then(Value::as_u64).map(|c| c as u32),
                        }),
                    })
                })
                .collect(),
        ),
        "gui_session" => GuiEvent::Session(GuiSession::from_value(get(p, "gui")?)),
        "gui_preview_text" => GuiEvent::PreviewText { buf: get(p, "buf")?.as_i64()?, text: string(p, "text") },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: Vec<(&str, Value)>) -> Value {
        Value::Map(pairs.into_iter().map(|(k, v)| (Value::from(k), v)).collect())
    }

    #[test]
    fn the_guis_session_part_round_trips_through_neovim() {
        let gui = GuiSession {
            visible: false,
            width: 312.5,
            view: "git".into(),
            expanded: vec!["/p/src".into(), "/p/src/app".into()],
            panel: Some(PanelSession { shown: true, height: 240.0, view: "problems".into() }),
            agent: Some(AgentSession { shown: true, width: 410.0 }),
        };
        // What the GUI sends, and what Neovim sends back in `gui_session`.
        assert_eq!(GuiSession::from_value(&gui.to_value()), gui);
        let sent = map(vec![("gui", gui.to_value())]);
        assert_eq!(parse("gui_session", &[sent]), Some(GuiEvent::Session(gui)));
        // Neovim's JSON turns an empty list into an empty object, and may lack keys.
        let json = map(vec![("view", "Outline".into()), ("expanded", map(vec![]))]);
        let back = GuiSession::from_value(&json);
        assert_eq!((back.view.as_str(), back.expanded.len(), back.visible), ("Outline", 0, true));
        // No `gui` in the payload (nothing was saved): no event.
        assert_eq!(parse("gui_session", &[map(vec![])]), None);
    }

    #[test]
    fn the_agent_panels_environment_is_a_list_of_pairs() {
        let env = map(vec![("CLAUDE_CODE_SSE_PORT", "4242".into()), ("ENABLE_IDE_INTEGRATION", "true".into())]);
        let want = vec![("CLAUDE_CODE_SSE_PORT".to_owned(), "4242".to_owned()), ("ENABLE_IDE_INTEGRATION".to_owned(), "true".to_owned())];
        assert_eq!(parse("gui_agent_env", &[env]), Some(GuiEvent::AgentEnv(want)));
        // No variables (the IDE server didn't start): an empty list.
        assert_eq!(parse("gui_agent_env", &[map(vec![])]), Some(GuiEvent::AgentEnv(vec![])));
    }

    #[test]
    fn parses_signature() {
        let param = |s: i64, e: i64| map(vec![("start", s.into()), ("end", e.into()), ("documentation", "doc".into())]);
        let sig = map(vec![
            ("label", "fn foo(a: i32, b: u8)".into()),
            ("documentation", "".into()),
            ("parameters", Value::Array(vec![param(7, 13), param(15, 20)])),
        ]);
        let payload = map(vec![
            ("signatures", Value::Array(vec![sig])),
            ("active_signature", 0.into()),
            ("active_parameter", 1.into()),
        ]);
        let Some(GuiEvent::Signature(help)) = parse("gui_signature", &[payload]) else { panic!() };
        let (sig, active) = help.active().unwrap();
        assert_eq!(active, Some(1));
        let p = &sig.parameters[1];
        assert_eq!(&sig.label[p.start..p.end], "b: u8");
    }

    #[test]
    fn parses_menus() {
        let item = |name: &str, key: &str| map(vec![("name", name.into()), ("key", key.into())]);
        let save = map(vec![("name", "Save".into()), ("key", "⌘S".into()), ("action", "save".into())]);
        let file = map(vec![
            ("name", "File".into()),
            ("key", "".into()),
            ("children", Value::Array(vec![save, item("-sep2-", ""), item("Mine", "")])),
        ]);
        let Some(GuiEvent::Menu(menus)) = parse("gui_menu", &[map(vec![("menus", Value::Array(vec![file]))])]) else {
            panic!()
        };
        let children = menus[0].children.as_ref().unwrap();
        let save = MenuNode { name: "Save".into(), key: "⌘S".into(), action: Some("save".into()), ..Default::default() };
        assert_eq!(children[0], save);
        assert!(children[1].is_separator() && !children[2].is_separator());
        // Greyed-out and checkmark items; the right-click menu's items.
        let paste = map(vec![("name", "Paste".into()), ("enabled", false.into()), ("checked", true.into())]);
        let popup = parse("gui_popup_menu", &[map(vec![("items", Value::Array(vec![paste]))])]);
        let paste = MenuNode { name: "Paste".into(), disabled: true, checked: Some(true), ..Default::default() };
        assert_eq!(popup, Some(GuiEvent::PopupMenu(vec![paste])));
        // An empty menu arrives as an empty table.
        let empty = map(vec![("name", "Empty".into()), ("children", Value::Map(vec![]))]);
        let Some(GuiEvent::Menu(m)) = parse("gui_menu", &[map(vec![("menus", Value::Array(vec![empty]))])]) else { panic!() };
        assert_eq!(m[0].children, Some(vec![]));
        assert_eq!(parse("gui_action", &["save_as".into()]), Some(GuiEvent::Action("save_as".into())));
    }

    #[test]
    fn parses_syntax_colors() {
        let style = |fg: i64, italic: bool| map(vec![("fg", fg.into()), ("italic", italic.into()), ("bold", false.into())]);
        let p = map(vec![("keyword", style(0xc678dd, false)), ("comment", style(0x5c6370, true))]);
        let Some(GuiEvent::Syntax(colors)) = parse("gui_syntax", &[p]) else { panic!() };
        assert_eq!(colors["keyword"], SyntaxStyle { fg: 0xc678dd, italic: false, bold: false });
        assert!(colors["comment"].italic);
        // An empty table arrives as an empty array.
        let Some(GuiEvent::Syntax(empty)) = parse("gui_syntax", &[Value::Array(vec![])]) else { panic!() };
        assert!(empty.is_empty());
    }

    #[test]
    fn parses_terminal_colors() {
        let mut list: Vec<Value> = (0..16).map(|_| (-1_i64).into()).collect();
        list[1] = 0xe06c75_i64.into();
        list[15] = 0xffffff_i64.into();
        let Some(GuiEvent::TermColors(colors)) = parse("gui_term_colors", &[Value::Array(list)]) else { panic!() };
        assert_eq!((colors[0], colors[1], colors[15]), (None, Some(0xe06c75), Some(0xffffff)));
        // Nothing reported: nothing set.
        let Some(GuiEvent::TermColors(none)) = parse("gui_term_colors", &[Value::Array(vec![])]) else { panic!() };
        assert_eq!(none, [None; 16]);
    }

    #[test]
    fn parses_decoration_colors() {
        let p = map(vec![("added", 0x81b29a_i64.into()), ("error", 0xc94f6d_i64.into())]);
        let Some(GuiEvent::DecoColors(colors)) = parse("gui_deco_colors", &[p]) else { panic!() };
        assert_eq!(colors.len(), 2);
        assert_eq!(colors["added"], 0x81b29a);
        // An empty table arrives as an empty array.
        let Some(GuiEvent::DecoColors(empty)) = parse("gui_deco_colors", &[Value::Array(vec![])]) else { panic!() };
        assert!(empty.is_empty());
    }

    #[test]
    fn parses_diff_colors() {
        let p = map(vec![("add", 0x2b3a2e_i64.into()), ("text", 0x4b5a3f_i64.into())]);
        let Some(GuiEvent::DiffColors(colors)) = parse("gui_diff_colors", &[p]) else { panic!() };
        assert_eq!((colors.len(), colors["add"], colors["text"]), (2, 0x2b3a2e, 0x4b5a3f));
        let Some(GuiEvent::DiffColors(empty)) = parse("gui_diff_colors", &[Value::Array(vec![])]) else { panic!() };
        assert!(empty.is_empty());
    }

    #[test]
    fn parses_a_file_at_a_commit_to_open() {
        let ev = map(vec![
            ("top", "/r".into()),
            ("oid", "abc123".into()),
            ("parent", "".into()),
            ("path", "b.txt".into()),
            ("left_path", "a.txt".into()),
        ]);
        assert_eq!(
            parse("gui_git_diff", &[ev]),
            Some(GuiEvent::GitDiff {
                top: "/r".into(),
                oid: "abc123".into(),
                parent: String::new(),
                path: "b.txt".into(),
                left_path: Some("a.txt".into()),
                deleted: false
            })
        );
    }

    #[test]
    fn parses_a_diff_edit() {
        let ev = map(vec![("op", "revert".into()), ("path", "/r/a.txt".into()), ("ok", false.into()), ("message", "a.txt has unsaved changes".into())]);
        assert_eq!(
            parse("gui_diff_edit", &[ev]),
            Some(GuiEvent::DiffEdit { op: "revert".into(), path: "/r/a.txt".into(), ok: false, message: "a.txt has unsaved changes".into() })
        );
    }

    #[test]
    fn parses_close_and_hover() {
        assert_eq!(parse("gui_popup_close", &[map(vec![])]), Some(GuiEvent::PopupClose(None)));
        assert_eq!(
            parse("gui_popup_close", &[map(vec![("kind", "signature".into())])]),
            Some(GuiEvent::PopupClose(Some(PopupKind::Signature)))
        );
        assert_eq!(
            parse("gui_hover", &[map(vec![("contents", "# hi".into()), ("kind", "markdown".into())])]),
            Some(GuiEvent::Hover { contents: "# hi".into(), kind: "markdown".into() })
        );
    }

    #[test]
    fn parses_the_picker_messages() {
        let item = map(vec![("text", "src/a.rs".into()), ("path", "/w/src/a.rs".into()), ("group", "buffer".into()), ("lnum", 3.into())]);
        let open = map(vec![
            ("id", 7.into()),
            ("title", "Find files".into()),
            ("source", "smart".into()),
            ("cwd", "/w".into()),
            ("files", true.into()),
            ("exclude", "/w/here.rs".into()),
            ("items", Value::Array(vec![item, map(vec![("text", "plain".into())])])),
        ]);
        let Some(GuiEvent::PickerOpen(open)) = parse("gui_picker_open", &[open]) else { panic!() };
        assert_eq!((open.id, open.source.as_str(), open.files, open.multi), (7, "smart", true, false));
        assert_eq!(open.exclude.as_deref(), Some("/w/here.rs"));
        assert_eq!(open.items[0].path.as_deref(), Some("/w/src/a.rs"));
        assert_eq!((open.items[0].lnum, open.items[0].col, open.items[0].group.as_deref()), (Some(3), None, Some("buffer")));
        assert_eq!(open.items[1], PickerItem { text: "plain".into(), ..Default::default() });
        // An empty list arrives as an empty table.
        let Some(GuiEvent::PickerOpen(empty)) = parse("gui_picker_open", &[map(vec![("id", 1.into()), ("items", Value::Map(vec![]))])]) else {
            panic!()
        };
        assert!(empty.items.is_empty());
        assert_eq!(parse("gui_picker_close", &[map(vec![("id", 7.into())])]), Some(GuiEvent::PickerClose(7)));
        let grep = map(vec![
            ("id", 2.into()),
            ("preview", true.into()),
            ("query", "foo".into()),
            ("literal", true.into()),
            ("grep_files", Value::Array(vec!["/w/a".into(), "/w/b".into()])),
            ("selected", 4.into()),
            ("numbered", true.into()),
        ]);
        let Some(GuiEvent::PickerOpen(grep)) = parse("gui_picker_open", &[grep]) else { panic!() };
        assert_eq!((grep.preview, grep.query.as_str(), grep.literal, grep.selected, grep.numbered), (true, "foo", true, 4, true));
        assert_eq!(grep.grep_files, Some(vec!["/w/a".to_owned(), "/w/b".to_owned()]));
    }

    #[test]
    fn parses_rows_with_kinds_lines_and_marks_and_a_live_pickers_items() {
        let pair = |a: i64, b: i64| Value::Array(vec![a.into(), b.into()]);
        let row = map(vec![
            ("text", "foo".into()),
            ("kind", "Function".into()),
            ("depth", 2.into()),
            ("severity", 3.into()),
            ("search", "foo a.lua".into()),
            ("marks", Value::Array(vec![pair(1, 4)])),
            ("line", map(vec![("text", "local foo".into()), ("ranges", Value::Array(vec![pair(6, 9)])), ("line_ranges", Value::Array(vec![pair(10, 13)]))])),
        ]);
        let ev = map(vec![("id", 4.into()), ("gen", 7.into()), ("items", Value::Array(vec![row, map(vec![("text", "plain".into())])]))]);
        let Some(GuiEvent::PickerItems { id, gen, items }) = parse("gui_picker_items", &[ev]) else { panic!() };
        assert_eq!((id, gen, items.len()), (4, 7, 2));
        let r = &items[0];
        assert_eq!((r.kind.as_deref(), r.depth, r.severity, r.search.as_deref(), r.marks.clone()), (Some("Function"), 2, Some(3), Some("foo a.lua"), vec![(1, 4)]));
        let l = r.line.as_ref().unwrap();
        assert_eq!((l.text.as_str(), l.ranges.clone(), l.line_ranges.clone()), ("local foo", vec![(6, 9)], vec![(10, 13)]));
        assert_eq!(items[1], PickerItem { text: "plain".into(), ..Default::default() });
        let open = map(vec![("id", 1.into()), ("live", true.into()), ("hint", "Type".into())]);
        let Some(GuiEvent::PickerOpen(open)) = parse("gui_picker_open", &[open]) else { panic!() };
        assert_eq!((open.live, open.hint.as_str()), (true, "Type"));
    }

    #[test]
    fn parses_what_was_typed_before_the_box_opened_and_the_phase_four_fields() {
        let typed = map(vec![("id", 3.into()), ("keys", Value::Array(vec!["a".into(), "<CR>".into(), "<Space>".into()]))]);
        assert_eq!(
            parse("gui_picker_typed", &[typed]),
            Some(GuiEvent::PickerTyped { id: 3, keys: vec!["a".into(), "<CR>".into(), "<Space>".into()] })
        );
        let row = map(vec![
            ("text", "File: Save".into()),
            ("hint", "⌘S".into()),
            ("prefix", "n".into()),
            ("mono", true.into()),
            ("gate", "save".into()),
        ]);
        let want = map(vec![("text", "a.txt".into()), ("path", "/w/a.txt".into()), ("lnum", 4.into())]);
        let open = map(vec![("id", 1.into()), ("on_select", true.into()), ("want", want), ("items", Value::Array(vec![row]))]);
        let Some(GuiEvent::PickerOpen(open)) = parse("gui_picker_open", &[open]) else { panic!() };
        assert!(open.on_select);
        assert_eq!(open.want, Some(PickerWant { text: "a.txt".into(), path: Some("/w/a.txt".into()), lnum: Some(4) }));
        let r = &open.items[0];
        assert_eq!((r.hint.as_str(), r.prefix.as_str(), r.mono, r.gate.as_deref()), ("⌘S", "n", true, Some("save")));
        // Without them: as before.
        let Some(GuiEvent::PickerOpen(plain)) = parse("gui_picker_open", &[map(vec![("id", 1.into())])]) else { panic!() };
        assert!(!plain.on_select && plain.want.is_none());
    }

    #[test]
    fn parses_a_preview_and_keeps_the_length_of_text_that_is_not_utf8() {
        let bad = Value::Binary(vec![b'a', 0xff, b'b']);
        let span = |l: i64, s: i64, e: i64, fg: i64, b: i64| Value::Array(vec![l.into(), s.into(), e.into(), fg.into(), b.into(), 0.into()]);
        let answer = map(vec![
            ("req", 9.into()),
            ("path", "/w/a.lua".into()),
            ("first_line", 5.into()),
            ("lines", Value::Array(vec!["local x".into(), bad])),
            ("spans", Value::Array(vec![span(0, 0, 5, 0xff0000, 1), span(1, 0, 1, -1, 0)])),
            ("lnum", 6.into()),
            ("total", 99.into()),
        ]);
        let Some(GuiEvent::PickerPreview(p)) = parse("gui_picker_preview", &[answer]) else { panic!() };
        assert_eq!((p.req, p.first_line, p.lnum, p.total), (9, 5, 6, 99));
        assert_eq!(p.lines, ["local x", "a?b"]);
        assert_eq!(p.spans[0], PreviewSpan { line: 0, start: 0, end: 5, fg: Some(0xff0000), bold: true, italic: false });
        assert_eq!(p.spans[1].fg, None);
        assert!(p.note.is_empty());
    }

    #[test]
    fn parses_the_tree_messages() {
        let node = map(vec![
            ("id", "a".into()),
            ("label", "alpha".into()),
            ("kind", "Function".into()),
            ("fg", 0xff8800.into()),
            ("line", 3.into()),
            ("children", Value::Array(vec![map(vec![("id", "a/b".into()), ("label", "beta".into()), ("expandable", true.into()), ("open", true.into())])])),
        ]);
        let set = map(vec![
            ("view", "outline".into()),
            ("gen", 4.into()),
            ("nodes", Value::Array(vec![node])),
            ("expand", 1.into()),
            ("buf", 7.into()),
            ("menu", true.into()),
        ]);
        let Some(GuiEvent::TreeSet(set)) = parse("gui_tree_set", &[set]) else { panic!() };
        assert_eq!((set.view.as_str(), set.gen, set.parent, set.expand, set.buf, set.menu), ("outline", 4, None, Some(1), Some(7), true));
        let a = &set.nodes[0];
        assert_eq!((a.label.as_str(), a.kind.as_str(), a.fg, a.line, a.expandable), ("alpha", "Function", Some(0xff8800), Some(3), false));
        assert_eq!((a.children[0].id.as_str(), a.children[0].expandable), ("a/b", true));
        assert_eq!((a.open, a.children[0].open), (false, true));
        let lazy = map(vec![("view", "v".into()), ("gen", 2.into()), ("parent", "a".into()), ("nodes", Value::Map(vec![])), ("message", "None".into())]);
        let Some(GuiEvent::TreeSet(lazy)) = parse("gui_tree_set", &[lazy]) else { panic!() };
        assert_eq!((lazy.parent.as_deref(), lazy.nodes.len(), lazy.message.as_deref(), lazy.menu), (Some("a"), 0, Some("None"), false));
        let select = map(vec![("view", "outline".into()), ("id", "a/b".into()), ("path", Value::Array(vec!["a".into(), "a/b".into()]))]);
        assert_eq!(
            parse("gui_tree_select", &[select]),
            Some(GuiEvent::TreeSelect { view: "outline".into(), id: "a/b".into(), path: vec!["a".into(), "a/b".into()] })
        );
        let menu = map(vec![("view", "v".into()), ("id", "a".into()), ("items", Value::Array(vec!["Copy".into()]))]);
        assert_eq!(parse("gui_tree_menu", &[menu]), Some(GuiEvent::TreeMenu { view: "v".into(), id: "a".into(), items: vec!["Copy".into()] }));
        assert_eq!(parse("gui_tree_unregister", &[map(vec![("view", "v".into())])]), Some(GuiEvent::TreeUnregister("v".into())));
        // A side view with a button, keys; a merge; show.
        let reg = map(vec![
            ("view", "slang".into()),
            ("side", map(vec![
                ("title", "HIERARCHY".into()),
                ("icon", "type-hierarchy".into()),
                ("tooltip", "Hierarchy".into()),
                ("buttons", Value::Array(vec![map(vec![("icon", "refresh".into()), ("tooltip", "Refresh".into())])])),
                ("filters", Value::Array(vec![
                    map(vec![("id", "port".into()), ("label", "Ports".into()), ("hidden", true.into())]),
                    map(vec![("id", "param".into()), ("label", "Parameters".into())]),
                ])),
            ])),
            ("keys", Value::Array(vec!["/".into(), "y".into()])),
        ]);
        let Some(GuiEvent::TreeRegister(reg)) = parse("gui_tree_register", &[reg]) else { panic!() };
        let side = reg.side.expect("a side view");
        assert_eq!((reg.view.as_str(), side.title.as_str(), side.icon.as_str(), side.buttons.len(), reg.keys.len()), ("slang", "HIERARCHY", "type-hierarchy", 1, 2));
        assert_eq!((side.buttons[0].icon.as_str(), side.buttons[0].tooltip.as_str()), ("refresh", "Refresh"));
        assert_eq!(side.filters, [
            TreeFilter { id: "port".into(), label: "Ports".into(), hidden: true },
            TreeFilter { id: "param".into(), label: "Parameters".into(), hidden: false },
        ]);
        let only_keys = map(vec![("view", "v".into()), ("keys", Value::Array(vec!["x".into()]))]);
        let Some(GuiEvent::TreeRegister(only_keys)) = parse("gui_tree_register", &[only_keys]) else { panic!() };
        assert!(only_keys.side.is_none());
        let merge = map(vec![("view", "v".into()), ("gen", 1.into()), ("merge", true.into())]);
        assert!(matches!(parse("gui_tree_set", &[merge]), Some(GuiEvent::TreeSet(s)) if s.merge));
        let show = map(vec![("view", "v".into()), ("action", "focus".into()), ("focus", true.into())]);
        assert_eq!(parse("gui_tree_show", &[show]), Some(GuiEvent::TreeShow { view: "v".into(), action: "focus".into(), focus: true }));
        assert_eq!(parse("gui_outline", &[map(vec![("action", "focus".into())])]), Some(GuiEvent::Outline("focus".into())));
    }

    #[test]
    fn parses_requested_menu_docs() {
        assert_eq!(
            parse("gui_pum_doc", &[map(vec![("selected", 3.into()), ("contents", "*docs*".into())])]),
            Some(GuiEvent::PumDoc { selected: 3, contents: "*docs*".into() })
        );
        assert_eq!(
            parse("gui_pum_doc", &[map(vec![("selected", (-1).into()), ("contents", "".into())])]),
            Some(GuiEvent::PumDoc { selected: -1, contents: String::new() })
        );
    }
}
