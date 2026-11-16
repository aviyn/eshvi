//! Optional config file: `$XDG_CONFIG_HOME/eshvi/config.toml`
//! (default `~/.config/eshvi/config.toml`; on Windows
//! `%APPDATA%\eshvi\config.toml`). Every key is optional.
//!
//! ```toml
//! font = "JetBrains_Mono:h14"   # same syntax as 'guifont'; 'guifont' wins
//! ui_font = "SF Pro:h13"        # all UI text (panels, popups, tabs, status and title bar); same syntax;
//!                               # default: the system UI font. [sidebar] font and [explorer] font_size override it
//! alt_is_meta = true            # Option/Alt sends <M-x>
//! cursor_blink = true
//! smooth_scroll = true          # scrolled text glides instead of jumping
//! scrollbar = "auto"            # "auto" (while scrolling or near), "always", "never"
//!
//! [popup]
//! hover_max_width = 620
//! hover_max_height = 420
//! signature_max_width = 720
//! corner_radius = 6
//! shadow = true
//! restyle_floats = true         # draw plugins' floats in the popup colors
//!
//! [sidebar]                     # file trees drawn as a VS Code-like panel
//! enabled = true
//! filetypes = ["NvimTree", "neo-tree"]
//! font = "SF Pro:h13"           # default: ui_font, else the system UI font 1pt below the grid
//! hide_statusline = true        # draw the panel over its statusline
//!
//! [window]
//! custom_titlebar = true        # macOS: thin title bar in the theme's panel color
//! titlebar_height = 32
//! tabs = "wide"                 # tab pages in the title bar: "wide" (equal pills) or "compact"
//!
//! [explorer]                    # native file explorer (Cmd+B toggles, Cmd+Shift+E focuses)
//! enabled = true
//! visible = true                # open at startup
//! width = 260
//! font_size = 13                # default: the size of ui_font, else 13
//! show_ignored = false          # show files ignored by .gitignore
//! nesting = true               # related files under their main one (Cargo.lock under Cargo.toml)
//!
//! [preview]                     # the Markdown preview (:EshviPreview)
//! font_size = 14                # prose, headings (scaled from it) and code; default: the editor font's size.
//!                               # Follows zoom (Cmd +/-)
//!
//! [session]                     # :EshviSession, File › Save Session…
//! auto = true                   # a folder's session is saved on quit and restored when Eshvi
//!                               # starts there (or from the Dock) without files
//!
//! [outline]                     # the Outline pane at the foot of the explorer
//! enabled = true
//! follow_cursor = true          # select the symbol the cursor is in
//! sort = "position"             # "position", "name" or "kind"
//!
//! [picker]
//! preview = "auto"              # grep's source: "auto" (beside the list if it gets
//!                               # 100 columns there, else under it), "beside" or "under"
//!
//! [menu]                        # macOS: the native menu bar, from Neovim's :menu
//! enabled = true                # (eshvi's menus and your own :amenu items)
//!
//! [terminal]                    # the bottom panel (the GUI's own: Problems and terminals): :EshviTerminal, :EshviProblems
//! enabled = true
//! height = 12                   # rows of the bottom panel at first (a dragged height is kept for the session)
//! problems_font = "ui"          # Problems tab: "ui" (the sidebar font) or "editor"
//! scrollback = 10000            # lines kept above the screen by the GUI's own terminals
//! option_as_meta = true         # Option sends ESC before the key (macOS), else it composes characters
//! font = "SF Mono:h12"          # 'guifont' syntax; default: the editor font
//! shell = "/opt/homebrew/bin/bash"  # what the GUI's own terminals run; default: $SHELL (not Neovim's 'shell')
//! copy_on_select = true         # a mouse selection in a terminal goes on the clipboard
//! middle_click_paste = true     # a middle click in a terminal pastes the clipboard
//! shell_args = ["-l"]           # its arguments (a login shell, so an app opened from the Dock gets your PATH)
//! keep_ctrl_hjkl = ["nvim", "vim", "fzf", "lazygit"]  # programs that get ⌃h/j/k/l in a terminal
//!
//! [agent]                       # the agent panel on the right: :EshviAgent, a tab per agent
//! enabled = false               # true: the agent panel, its IDE connection and the agents' Changes
//! command = ["claude"]          # the agent to run, e.g. ["claude", "--continue"]
//! width = 80                    # columns of the agent font at first (at most 40% of the window)
//! font = "SF Mono:h12"          # 'guifont' syntax; default: the editor font
//! diff = "native"               # a file's diff from Changes: "native" (the GUI's own view over the editor),
//!                               # "tab" (Neovim's diff mode in a tab page), or "buffer" (as `git diff` prints
//!                               # it, in a buffer of filetype diff). "float" is the old name of "native".
//!
//! [git]                         # the Source Control panel
//! diff = "native"               # a file's or a commit's diff from the panel: as `[agent] diff`
//!
//! [diff]                        # the GUI's diff view (what "native" opens)
//! layout = "unified"            # "unified" (as `git diff`) or "split" (side by side); `s` switches in the view
//! context = 3                   # unchanged lines shown around each change; longer runs fold
//!
//! [agent.commands]              # more agents "+" offers, by name
//! codex = ["codex"]
//!
//! [layout]                      # applied inside eshvi only
//! global_statusline = true      # 'laststatus' 3: one status bar, thin window separators
//! winbar = true                 # filename header on each window
//! native_tabs = true            # tab pages as GUI tabs (replaces 'tabline' plugins)
//!
//! [statusline]
//! gui = true                    # GUI bar: system font, right part right-aligned
//! colors = "panel"              # "panel" (theme's darker panel shade, like the explorer),
//!                               # "fixed" (colors below) or "theme" (statusline highlights)
//! font_size = 12               # default: 1pt below the size of ui_font, else 12
//! bg = "#181818"                # bar of the window with the cursor
//! fg = "#cccccc"
//! inactive_bg = "#181818"       # other windows' bars
//! inactive_fg = "#8b8b8b"
//! border = "#2b2b2b"
//! highlight_mode = true         # colored pill behind "NORMAL"/"INSERT"/...
//!
//! [neovim]
//! copy_on_select = false        # true: a mouse selection in the editor goes on the clipboard (not a yank; "* and "+ then give it)
//! config = "own"                # "own": Neovim's config is ~/.config/eshvi/nvim (NVIM_APPNAME=eshvi/nvim)
//!                               # with Neovim's native completion; "shared": ~/.config/nvim as in a terminal.
//!                               # An NVIM_APPNAME already in the environment wins.
//!
//! [cmdline]                     # Neovim's command line: `:`, `/`, `?`, `input()` (not the picker)
//! position = "bottom"          # "bottom" (above the status bar) or "top"; menus open away from it
//!
//! [messages]
//! restore_notify = true         # built-in vim.notify (toasts) instead of e.g. nvim-notify
//!
//! [colors]                      # override colors derived from highlights
//! popup_bg = "#1e1e2e"
//! popup_fg = "#cdd6f4"
//! popup_border = "#45475a"
//! popup_selection = "#313244"
//! accent = "#89b4fa"
//! ```

use std::path::PathBuf;

use egui::Color32;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: Option<String>,
    pub ui_font: Option<String>,
    pub alt_is_meta: Option<bool>,
    pub cursor_blink: Option<bool>,
    pub smooth_scroll: Option<bool>,
    pub scrollbar: Option<crate::scrollbar::Mode>,
    pub popup: PopupConfig,
    pub sidebar: SidebarConfig,
    pub window: WindowConfig,
    pub explorer: ExplorerConfig,
    pub preview: PreviewConfig,
    pub session: SessionConfig,
    pub outline: OutlineConfig,
    pub picker: PickerConfig,
    pub terminal: TerminalConfig,
    pub agent: AgentConfig,
    pub git: GitConfig,
    pub diff: DiffConfig,
    pub menu: MenuConfig,
    pub layout: LayoutConfig,
    pub statusline: StatuslineConfig,
    pub messages: MessagesConfig,
    pub cmdline: CmdlineConfig,
    pub neovim: NeovimConfig,
    pub colors: ColorConfig,
}

/// Where the command line (`:`, `/`, `?`, `input()`) is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CmdlinePosition {
    #[default]
    Bottom,
    Top,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CmdlineConfig {
    pub position: CmdlinePosition,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NeovimConfig {
    pub config: NeovimConfigKind,
    /// A Visual selection made with the mouse goes on the clipboard when the
    /// button is let go, as in the GUI's terminals (`[terminal] copy_on_select`).
    /// Not a yank (`"0` and `""` aren't written), but `"*` and `"+` read the
    /// clipboard, so they give it, and so does `p` with 'clipboard' unnamed or
    /// unnamedplus: Vim's 'autoselect' (X11's primary selection), on the one clipboard.
    pub copy_on_select: bool,
}

impl Default for NeovimConfig {
    fn default() -> Self {
        // Off, as in Neovide, MacVim and Neovim in a terminal: a mouse selection is only a
        // selection until it's yanked (the user's call).
        Self { config: NeovimConfigKind::default(), copy_on_select: false }
    }
}

/// Which Neovim config Eshvi starts Neovim with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NeovimConfigKind {
    /// `NVIM_APPNAME=eshvi/nvim`, and Neovim's native completion.
    #[default]
    Own,
    /// The terminal's config (`~/.config/nvim`), as it is.
    Shared,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MenuConfig {
    pub enabled: bool,
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MessagesConfig {
    pub restore_notify: bool,
}

impl Default for MessagesConfig {
    fn default() -> Self {
        Self { restore_notify: true }
    }
}

/// Sessions (`:EshviSession`, `session.lua`).
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    /// Save a folder's session on quit, restore it at start.
    pub auto: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self { auto: true }
    }
}

/// The Markdown preview.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PreviewConfig {
    /// None: the editor font's size.
    pub font_size: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SidebarConfig {
    pub enabled: bool,
    pub filetypes: Vec<String>,
    pub font: Option<String>,
    pub hide_statusline: bool,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            filetypes: vec!["NvimTree".into(), "neo-tree".into()],
            font: None,
            hide_statusline: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub custom_titlebar: bool,
    pub titlebar_height: f32,
    pub tabs: TitleTabs,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { custom_titlebar: true, titlebar_height: 32.0, tabs: TitleTabs::default() }
    }
}

/// How the title bar lays out Neovim's tab pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleTabs {
    /// Equal pills sharing the bar (up to a width).
    #[default]
    Wide,
    /// Pills as wide as their names.
    Compact,
}

impl WindowConfig {
    /// The custom title bar is only drawn on macOS (transparent system bar).
    pub fn titlebar(&self) -> Option<f32> {
        (cfg!(target_os = "macos") && self.custom_titlebar).then_some(self.titlebar_height.clamp(16.0, 60.0))
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExplorerConfig {
    pub enabled: bool,
    pub visible: bool,
    pub width: f32,
    /// None: `ui_font`'s size, else 13.
    pub font_size: Option<f32>,
    pub show_ignored: bool,
    pub nesting: bool,
}

impl Default for ExplorerConfig {
    fn default() -> Self {
        Self { enabled: true, visible: true, width: 260.0, font_size: None, show_ignored: false, nesting: true }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutlineConfig {
    pub enabled: bool,
    pub follow_cursor: bool,
    pub sort: crate::tree_view::Sort,
}

impl Default for OutlineConfig {
    fn default() -> Self {
        Self { enabled: true, follow_cursor: true, sort: crate::tree_view::Sort::Position }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PickerConfig {
    pub preview: crate::picker::PreviewPlacement,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    pub enabled: bool,
    pub height: u32,
    pub problems_font: ListFont,
    /// Lines of scrollback in the GUI's own terminals (`crates/term`).
    pub scrollback: usize,
    /// Option sends ESC before the key, as Alacritty and iTerm do, rather
    /// than composing a character.
    pub option_as_meta: bool,
    /// The terminals' font, `font`'s syntax; default: the editor font.
    pub font: Option<String>,
    /// The program the GUI's own terminals run; default: `$SHELL` (else the
    /// password file's). Eshvi's own setting, apart from Neovim's 'shell'.
    pub shell: Option<String>,
    /// The shell's arguments; default `-l`, a login shell.
    pub shell_args: Vec<String>,
    /// A mouse selection in a terminal goes on the clipboard when the button
    /// is let go, as in X11 terminals.
    pub copy_on_select: bool,
    /// A middle click in a terminal pastes the clipboard.
    pub middle_click_paste: bool,
    /// The old switch between the GUI's panels and Neovim-window ones. The
    /// panels are the GUI's only; the key is accepted and ignored (`load`
    /// says so once) so a config that has it still loads.
    pub native: Option<serde::de::IgnoredAny>,
    /// The foreground programs of a terminal that keep ⌃h/j/k/l for
    /// themselves, as vim-tmux-navigator's check does; elsewhere the keys
    /// move to the neighbouring region.
    pub keep_ctrl_hjkl: Vec<String>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            height: 12,
            problems_font: ListFont::Ui,
            scrollback: 10_000,
            option_as_meta: true,
            font: None,
            shell: None,
            shell_args: vec!["-l".into()],
            copy_on_select: true,
            middle_click_paste: true,
            native: None,
            keep_ctrl_hjkl: ["nvim", "vim", "fzf", "lazygit"].map(String::from).to_vec(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    pub enabled: bool,
    pub command: Vec<String>,
    pub commands: std::collections::BTreeMap<String, Vec<String>>,
    pub width: u32,
    pub font: Option<String>,
    pub diff: DiffView,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            // Off unless asked for (the user's call): no agent panel, title bar toggle,
            // IDE connection or Changes.
            enabled: false,
            command: vec!["claude".into()],
            commands: Default::default(),
            width: 80,
            font: None,
            diff: DiffView::Native,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitConfig {
    pub diff: DiffView,
}

/// How the Changes popup (`[agent] diff`) and Source Control (`[git] diff`)
/// open a diff.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffView {
    /// The GUI's own view over the editor (`[diff]`). "float", which was a
    /// frame of Neovim's windows, is accepted and means this.
    #[default]
    #[serde(alias = "float")]
    Native,
    /// Neovim's diff mode in a tab page of its own.
    Tab,
    /// As `git diff` prints it, read-only, in a buffer of filetype `diff`.
    Buffer,
}

impl DiffView {
    /// What `Eshvi.changes_diff` is told (the native view isn't Neovim's).
    pub fn name(self) -> &'static str {
        match self {
            DiffView::Native => "native",
            DiffView::Tab => "tab",
            DiffView::Buffer => "buffer",
        }
    }
}

/// The GUI's diff view (`[diff]`).
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiffConfig {
    /// How it starts: "unified" (as `git diff` prints it) or "split" (side
    /// by side). `s` in the view switches, and the choice is kept for the
    /// session.
    pub layout: crate::diff_view::Layout,
    /// Unchanged lines shown around each change; a longer run folds into a
    /// row that opens when clicked.
    pub context: usize,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self { layout: crate::diff_view::Layout::Unified, context: 3 }
    }
}

/// Font of a list drawn by the GUI (the Problems list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListFont {
    /// Proportional: the sidebar font (`[sidebar] font`, by default the
    /// system UI font).
    Ui,
    /// The editor's monospace grid font.
    Editor,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutConfig {
    pub global_statusline: bool,
    pub winbar: bool,
    pub native_tabs: bool,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self { global_statusline: true, winbar: true, native_tabs: true }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatuslineConfig {
    pub gui: bool,
    /// "fixed" or "theme".
    pub colors: String,
    /// None: a point below `ui_font`'s size, else 12.
    pub font_size: Option<f32>,
    pub bg: String,
    pub fg: String,
    pub inactive_bg: String,
    pub inactive_fg: String,
    pub border: String,
    pub highlight_mode: bool,
}

impl Default for StatuslineConfig {
    fn default() -> Self {
        Self {
            gui: true,
            colors: "panel".into(),
            font_size: None,
            bg: "#181818".into(),
            fg: "#cccccc".into(),
            inactive_bg: "#181818".into(),
            inactive_fg: "#8b8b8b".into(),
            border: "#2b2b2b".into(),
            highlight_mode: true,
        }
    }
}

/// How the GUI status bar is colored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatusColorMode {
    /// Derived from the theme: the darker panel shade used by the explorer.
    Panel,
    Fixed(StatusColors),
    /// The statusline's own highlights.
    Theme,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusColors {
    pub active: (Color32, Color32),
    pub inactive: (Color32, Color32),
    pub border: Color32,
}

impl StatuslineConfig {
    pub fn resolve_colors(&self) -> StatusColorMode {
        match self.colors.as_str() {
            "theme" => return StatusColorMode::Theme,
            "fixed" => {}
            "panel" => return StatusColorMode::Panel,
            other => {
                log::warn!("config: statusline colors {other:?} unknown, using \"panel\"");
                return StatusColorMode::Panel;
            }
        }
        let d = Self::default();
        let c = |v: &str, fallback: &str| parse_color(v).or_else(|| parse_color(fallback)).unwrap_or(Color32::GRAY);
        StatusColorMode::Fixed(StatusColors {
            active: (c(&self.bg, &d.bg), c(&self.fg, &d.fg)),
            inactive: (c(&self.inactive_bg, &d.inactive_bg), c(&self.inactive_fg, &d.inactive_fg)),
            border: c(&self.border, &d.border),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PopupConfig {
    pub hover_max_width: f32,
    pub hover_max_height: f32,
    pub signature_max_width: f32,
    pub corner_radius: f32,
    pub shadow: bool,
    pub restyle_floats: bool,
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            hover_max_width: 620.0,
            hover_max_height: 420.0,
            signature_max_width: 720.0,
            corner_radius: 6.0,
            shadow: true,
            restyle_floats: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColorConfig {
    pub popup_bg: Option<String>,
    pub popup_fg: Option<String>,
    pub popup_border: Option<String>,
    pub popup_selection: Option<String>,
    pub accent: Option<String>,
}

/// Parse `#rrggbb`.
pub fn parse_color(s: &str) -> Option<Color32> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Resolved color overrides.
#[derive(Clone, Copy, Debug, Default)]
pub struct ColorOverrides {
    pub popup_bg: Option<Color32>,
    pub popup_fg: Option<Color32>,
    pub popup_border: Option<Color32>,
    pub popup_selection: Option<Color32>,
    pub accent: Option<Color32>,
}

impl ColorConfig {
    pub fn resolve(&self) -> ColorOverrides {
        let c = |v: &Option<String>| {
            v.as_deref().and_then(|s| {
                let c = parse_color(s);
                if c.is_none() {
                    log::warn!("config: bad color {s:?}, expected #rrggbb");
                }
                c
            })
        };
        ColorOverrides {
            popup_bg: c(&self.popup_bg),
            popup_fg: c(&self.popup_fg),
            popup_border: c(&self.popup_border),
            popup_selection: c(&self.popup_selection),
            accent: c(&self.accent),
        }
    }
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("eshvi").join("config.toml"))
}

/// Eshvi was called Eshnv until 2026-11-15, and kept its things in `eshnv` folders. With no
/// `eshvi` config folder but old folders there, what to tell the user: Eshvi doesn't move them
/// (the user's call), it says where they are and how to move them. `None` otherwise.
pub fn old_folders_note() -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let xdg = |var: &str, default: &str| std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(default));
    old_folders_note_in(&xdg("XDG_CONFIG_HOME", ".config"), &xdg("XDG_DATA_HOME", ".local/share"), &xdg("XDG_STATE_HOME", ".local/state"), &home)
}

fn old_folders_note_in(config: &std::path::Path, data: &std::path::Path, state: &std::path::Path, home: &std::path::Path) -> Option<String> {
    if config.join("eshvi").exists() {
        return None;
    }
    let show = |p: &std::path::Path| match p.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    };
    let moves: Vec<String> = [config, data, state]
        .iter()
        .filter(|base| base.join("eshnv").is_dir())
        // (rsync merges into an `eshvi` folder Neovim may already have made.)
        .map(|base| format!("rsync -a {0}/eshnv/ {0}/eshvi/ && rm -rf {0}/eshnv", show(base)))
        .collect();
    if moves.is_empty() {
        return None;
    }
    Some(format!(
        "No Eshvi config found in {}/eshvi. Eshnv's settings, Neovim config, plugins and sessions are still in the old \"eshnv\" folders. To keep them, quit Eshvi and run in a terminal: {}",
        show(config),
        moves.join("; ")
    ))
}

/// Load the config file; a missing file gives defaults, a broken one is
/// ignored: the defaults, and what to tell the user (in the window: opened
/// from the Finder, nobody reads stderr).
pub fn load() -> (Config, Option<String>) {
    or_defaults(read())
}

fn or_defaults(read: Result<Config, String>) -> (Config, Option<String>) {
    match read {
        Ok(config) => (config, None),
        Err(e) => {
            eprintln!("eshvi: ignoring {e}");
            (Config::default(), Some(format!("eshvi: settings ignored, the defaults are used: {e}")))
        }
    }
}

/// Read the config file: defaults when there is none, the error (with the
/// file's path) when it doesn't parse.
pub fn read() -> Result<Config, String> {
    let Some(path) = path() else { return Ok(Config::default()) };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())).inspect(warn_about_gone_keys),
        Err(_) => Ok(Config::default()),
    }
}

/// The modification time of the config file, to notice changes.
pub fn modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(path()?).and_then(|m| m.modified()).ok()
}

/// Sections (or keys) that differ between two configs and only apply when
/// eshvi starts: they set up Neovim or the window.
pub fn restart_needed(old: &Config, new: &Config) -> Vec<&'static str> {
    let differ = |a: &dyn std::fmt::Debug, b: &dyn std::fmt::Debug| format!("{a:?}") != format!("{b:?}");
    let (o, n) = (old, new);
    [
        ("[sidebar] enabled, filetypes", differ(&(o.sidebar.enabled, &o.sidebar.filetypes), &(n.sidebar.enabled, &n.sidebar.filetypes))),
        // (Not `tabs`: read as it's drawn.)
        ("[window]", differ(&(o.window.custom_titlebar, o.window.titlebar_height), &(n.window.custom_titlebar, n.window.titlebar_height))),
        ("[explorer] enabled", o.explorer.enabled != n.explorer.enabled),
        ("[neovim] config, copy_on_select", differ(&(o.neovim.config, o.neovim.copy_on_select), &(n.neovim.config, n.neovim.copy_on_select))),
        ("[terminal] enabled", o.terminal.enabled != n.terminal.enabled),
        (
            "[agent] enabled, command, commands",
            differ(&(o.agent.enabled, &o.agent.command, &o.agent.commands), &(n.agent.enabled, &n.agent.command, &n.agent.commands)),
        ),
        ("[session] auto", o.session.auto != n.session.auto),
        ("[menu]", differ(&o.menu, &n.menu)),
        ("[layout]", differ(&o.layout, &n.layout)),
        ("[messages]", differ(&o.messages, &n.messages)),
    ]
    .into_iter()
    .filter_map(|(name, changed)| changed.then_some(name))
    .collect()
}

/// Keys that no longer do anything but are still accepted, so an older config loads: say so
/// (once per read) in the log.
fn warn_about_gone_keys(config: &Config) {
    if config.terminal.native.is_some() {
        log::warn!("config: [terminal] native is gone (the panels are always the GUI's own) and is ignored; remove it");
    }
}

pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_file_gives_the_defaults_and_what_to_say() {
        let (_, said) = or_defaults(Ok(Config::default()));
        assert_eq!(said, None);
        let (config, said) = or_defaults(parse("font = [").map_err(|e| format!("/c/config.toml: {e}")));
        assert_eq!(config.font, Config::default().font);
        let said = said.unwrap();
        assert!(said.starts_with("eshvi: settings ignored, the defaults are used: /c/config.toml: "), "{said}");
    }

    #[test]
    fn cmdline_position_defaults_to_the_bottom() {
        assert_eq!(Config::default().cmdline.position, CmdlinePosition::Bottom);
        assert_eq!(parse("[cmdline]\nposition = \"top\"").unwrap().cmdline.position, CmdlinePosition::Top);
        assert_eq!(parse("[cmdline]\nposition = \"bottom\"").unwrap().cmdline.position, CmdlinePosition::Bottom);
        assert!(parse("[cmdline]\nposition = \"middle\"").is_err());
    }

    #[test]
    fn ui_font_and_its_overrides() {
        let c = Config::default();
        assert_eq!((c.ui_font.as_deref(), c.explorer.font_size, c.statusline.font_size), (None, None, None));
        let c = parse("ui_font = \"SF Pro:h13\"\n[explorer]\nfont_size = 14\n[statusline]\nfont_size = 11").unwrap();
        assert_eq!(c.ui_font.as_deref(), Some("SF Pro:h13"));
        assert_eq!((c.explorer.font_size, c.statusline.font_size), (Some(14.0), Some(11.0)));
    }

    /// `config/config.example.toml` sets every key to its default: parsed, it is the default
    /// config (and it parses at all, so no key in it is misspelled or gone).
    #[test]
    fn the_example_config_is_the_defaults() {
        let mut example = parse(include_str!("../../../config/config.example.toml")).expect("the example parses");
        // (These four are optional in the struct, unset meaning the value the example gives.)
        assert_eq!((example.alt_is_meta, example.cursor_blink, example.smooth_scroll), (Some(true), Some(true), Some(true)));
        assert_eq!(example.scrollbar, Some(crate::scrollbar::Mode::Auto));
        (example.alt_is_meta, example.cursor_blink, example.smooth_scroll, example.scrollbar) = (None, None, None, None);
        assert_eq!(format!("{example:#?}"), format!("{:#?}", Config::default()));
    }

    /// Only with no `eshvi` config folder and some old `eshnv` folder: then how to move each.
    #[test]
    fn old_eshnv_folders_are_pointed_out_until_there_is_an_eshvi_config() {
        let home = std::env::temp_dir().join(format!("eshvi-old-folders-{}", std::process::id()));
        let (config, data, state) = (home.join(".config"), home.join(".local/share"), home.join(".local/state"));
        for d in [&config, &data, &state] {
            std::fs::create_dir_all(d).unwrap();
        }
        assert_eq!(old_folders_note_in(&config, &data, &state, &home), None, "nothing old: nothing to say");
        std::fs::create_dir_all(config.join("eshnv")).unwrap();
        std::fs::create_dir_all(state.join("eshnv")).unwrap();
        let note = old_folders_note_in(&config, &data, &state, &home).unwrap();
        assert!(note.starts_with("No Eshvi config found in ~/.config/eshvi."), "{note}");
        assert!(note.contains("rsync -a ~/.config/eshnv/ ~/.config/eshvi/ && rm -rf ~/.config/eshnv"), "{note}");
        assert!(note.contains("~/.local/state/eshnv/") && !note.contains(".local/share"), "{note}");
        std::fs::create_dir_all(config.join("eshvi")).unwrap();
        assert_eq!(old_folders_note_in(&config, &data, &state, &home), None, "an eshvi config: moved, or a fresh start");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn automatic_sessions_are_on_unless_turned_off() {
        assert!(Config::default().session.auto);
        assert!(!parse("[session]\nauto = false").unwrap().session.auto);
        assert!(parse("[session]\nautosave = true").is_err());
        assert_eq!(restart_needed(&Config::default(), &parse("[session]\nauto = false").unwrap()), ["[session] auto"]);
    }

    #[test]
    fn preview_font_size_is_optional() {
        assert_eq!(Config::default().preview.font_size, None);
        assert_eq!(parse("[preview]\nfont_size = 16").unwrap().preview.font_size, Some(16.0));
        assert!(parse("[preview]\nsize = 16").is_err());
    }

    #[test]
    fn parses_full_config() {
        let c = parse(
            r##"
            font = "Menlo:h14"
            alt_is_meta = false
            [popup]
            hover_max_width = 500
            [sidebar]
            filetypes = ["NvimTree"]
            [agent]
            command = ["claude", "--continue"]
            font = "SF Mono:h12"
            [agent.commands]
            codex = ["codex", "--full-auto"]
            [colors]
            accent = "#89b4fa"
            "##,
        )
        .unwrap();
        assert_eq!(c.agent.command, ["claude", "--continue"]);
        // (`enabled` isn't in the file: off by default.)
        assert_eq!((c.agent.enabled, c.agent.width, c.agent.font.as_deref()), (false, 80, Some("SF Mono:h12")));
        assert!(parse("[agent]\nenabled = true").unwrap().agent.enabled);
        assert_eq!(c.agent.commands["codex"], ["codex", "--full-auto"]);
        assert_eq!(Config::default().agent.command, ["claude"]);
        assert_eq!(c.font.as_deref(), Some("Menlo:h14"));
        assert_eq!(c.alt_is_meta, Some(false));
        assert_eq!(c.popup.hover_max_width, 500.0);
        assert_eq!(c.popup.hover_max_height, 420.0);
        assert!(c.sidebar.enabled);
        assert_eq!(c.statusline.resolve_colors(), StatusColorMode::Panel);
        assert!(c.layout.global_statusline && c.layout.winbar);
        let colors = |v: &str| parse(&format!("[statusline]\ncolors = \"{v}\"")).unwrap().statusline.resolve_colors();
        assert_eq!(colors("theme"), StatusColorMode::Theme);
        let StatusColorMode::Fixed(f) = colors("fixed") else { panic!() };
        assert_eq!(f.active.0, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(c.sidebar.filetypes, ["NvimTree"]);
        assert_eq!(c.colors.resolve().accent, Some(Color32::from_rgb(0x89, 0xb4, 0xfa)));
        assert_eq!(c.terminal.problems_font, ListFont::Ui);
        let font = parse("[terminal]\nproblems_font = \"editor\"").unwrap().terminal.problems_font;
        assert_eq!(font, ListFont::Editor);
        assert!(parse("[terminal]\nproblems_font = \"comic\"").is_err());
        // The GUI's own terminals.
        let t = Config::default().terminal;
        // (`native` is accepted, whatever its value, and ignored.)
        assert!(t.native.is_none());
        assert!(parse("[terminal]\nnative = true").unwrap().terminal.native.is_some());
        assert!(parse("[terminal]\nnative = false").unwrap().terminal.native.is_some());
        assert_eq!((t.scrollback, t.option_as_meta, t.font.as_deref()), (10_000, true, None));
        let t = parse("[terminal]\nscrollback = 500\noption_as_meta = false\nfont = \"Menlo:h12\"").unwrap().terminal;
        assert_eq!((t.scrollback, t.option_as_meta, t.font.as_deref()), (500, false, Some("Menlo:h12")));
        assert_eq!((t.shell, t.shell_args), (None, vec!["-l".to_string()]));
        assert!(t.copy_on_select && t.middle_click_paste);
        let t = parse("[terminal]\nshell = \"/opt/homebrew/bin/bash\"\nshell_args = [\"--login\", \"-i\"]").unwrap().terminal;
        assert_eq!((t.shell.as_deref(), t.shell_args), (Some("/opt/homebrew/bin/bash"), vec!["--login".to_string(), "-i".into()]));
        assert!(c.menu.enabled);
        assert_eq!(c.agent.diff, DiffView::Native);
        assert_eq!(parse("[agent]\ndiff = \"tab\"").unwrap().agent.diff, DiffView::Tab);
        // The old "float" is the native view.
        assert_eq!(parse("[agent]\ndiff = \"float\"").unwrap().agent.diff, DiffView::Native);
        assert_eq!(parse("[git]\ndiff = \"float\"").unwrap().git.diff, DiffView::Native);
        assert!(parse("[git]\ndiff = \"popup\"").is_err());
        // The view's own settings.
        let d = Config::default().diff;
        assert_eq!((d.layout, d.context), (crate::diff_view::Layout::Unified, 3));
        let d = parse("[diff]\nlayout = \"split\"\ncontext = 5").unwrap().diff;
        assert_eq!((d.layout, d.context), (crate::diff_view::Layout::Split, 5));
        assert!(parse("[diff]\nlayout = \"stacked\"").is_err());
        assert!(parse("[diff]\ncolors = 1").is_err());
        // Each its own: the panel's doesn't follow the agents'.
        let c2 = parse("[agent]\ndiff = \"buffer\"").unwrap();
        assert_eq!((c2.agent.diff, c2.git.diff), (DiffView::Buffer, DiffView::Native));
        let c2 = parse("[git]\ndiff = \"buffer\"").unwrap();
        assert_eq!((c2.agent.diff, c2.git.diff), (DiffView::Native, DiffView::Buffer));
        assert_eq!(Config::default().window.tabs, TitleTabs::Wide);
        assert_eq!(parse("[window]\ntabs = \"compact\"").unwrap().window.tabs, TitleTabs::Compact);
        assert!(!parse("[menu]\nenabled = false").unwrap().menu.enabled);
        assert_eq!(Config::default().neovim.config, NeovimConfigKind::Own);
        assert_eq!(parse("[neovim]\nconfig = \"shared\"").unwrap().neovim.config, NeovimConfigKind::Shared);
        assert!(!Config::default().neovim.copy_on_select, "off unless asked for");
        assert!(parse("[neovim]\ncopy_on_select = true").unwrap().neovim.copy_on_select);
        assert!(parse("[neovim]\nconfig = \"mine\"").is_err());
        // The Outline: on, following the cursor, in the file's order, unless said.
        let o = Config::default().outline;
        assert_eq!((o.enabled, o.follow_cursor, o.sort), (true, true, crate::tree_view::Sort::Position));
        let o = parse("[outline]\nenabled = false\nfollow_cursor = false\nsort = \"kind\"").unwrap().outline;
        assert_eq!((o.enabled, o.follow_cursor, o.sort), (false, false, crate::tree_view::Sort::Kind));
        assert!(parse("[outline]\nsort = \"size\"").is_err());
        // The picker's preview goes where it gets the room, unless said.
        use crate::picker::PreviewPlacement as Place;
        assert_eq!(Config::default().picker.preview, Place::Auto);
        assert_eq!(parse("[picker]\npreview = \"under\"").unwrap().picker.preview, Place::Under);
        assert_eq!(parse("[picker]\npreview = \"beside\"").unwrap().picker.preview, Place::Beside);
        assert!(parse("[picker]\npreview = \"left\"").is_err());
    }

    #[test]
    fn startup_only_settings_need_a_restart() {
        let old = parse("font = \"Menlo:h13\"\n[agent]\nwidth = 80").unwrap();
        // Fonts, colors and the like apply right away.
        let new = parse("font = \"Menlo:h15\"\n[agent]\nwidth = 80\nfont = \"SF Mono:h12\"\n[statusline]\ngui = false").unwrap();
        assert!(restart_needed(&old, &new).is_empty());
        let new = parse("[agent]\ncommand = [\"codex\"]\n[layout]\nwinbar = false").unwrap();
        assert_eq!(restart_needed(&old, &new), ["[agent] enabled, command, commands", "[layout]"]);
        // The panels' sizes apply at once; `native` is gone and ignored.
        let new = parse("[agent]\nwidth = 90\n[terminal]\nheight = 20\nnative = true").unwrap();
        assert!(restart_needed(&old, &new).is_empty());
        let new = parse("[neovim]\nconfig = \"shared\"").unwrap();
        assert_eq!(restart_needed(&old, &new), ["[neovim] config, copy_on_select"]);
    }

    #[test]
    fn empty_and_bad_configs() {
        assert!(parse("").is_ok());
        assert!(parse("nonsense = 1").is_err());
        assert_eq!(parse_color("#12"), None);
    }
}
