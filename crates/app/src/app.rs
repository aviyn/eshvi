use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Event, ImeEvent, MouseWheelUnit, PointerButton, Pos2, Rect, Vec2, ViewportCommand};
use grid::{Editor, OptionValue, RedrawEvent, WindowKind, DEFAULT_GRID};
use nvim_bridge::gui::{GuiEvent, Progress};
use nvim_bridge::{Bridge, BridgeEvent, RpcValue, SpawnOptions, UiCommand};

use crate::cmdline;
use crate::focus::Focus;
use crate::config::Config;
use crate::debug::Screenshotter;
use crate::explorer_view::{self, ExplorerColors, ExplorerView};
use crate::decorations::{DecoColors, Decorations, Problems};
use crate::git::{GitOpsWorker, GitStatusWorker};
use crate::scm_view::{self, ScmView};
use crate::icons::{IconTextures, IconTheme};
use crate::fonts::{self, FontSpec, GridFonts};
use crate::lsp_popups::{LspPopups, PopupCtx, PopupLimits};
use crate::messages::MessageView;
use crate::popupmenu::{self, PopupMenuView};
use crate::input::{self, KeyTranslator};
use crate::config::{StatusColorMode, StatusColors};
use crate::popup::mix;
use crate::side_bar::{self, SideView};
use crate::renderer::{rgb, EguiGridRenderer, GridRenderer, LayerStyle, StatusLook};

mod actions;
mod agent;
mod agent_panel;
mod bottom_panel;
mod chrome;
mod diff;
mod events;
mod file_drop;
mod layout;
mod markdown_preview;
mod outline;
mod picker;
mod session;
mod scm;
mod settings;
mod status;
mod tabs;
mod term_debug;
mod windows;

use chrome::mode_color;
use crate::style::{self, panel_bg, Chrome, Glyph};
use tabs::{digits_middle, problems_filter_id, truncate_chars};

pub const DEFAULT_FONT_SIZE: f32 = 13.0;

pub struct Options {
    pub nvim: String,
    pub nvim_args: Vec<String>,
    /// `--font-size`; overrides the config file's size.
    pub font_size: Option<f32>,
    pub config: Config,
    /// Why the config file was ignored, if it was (`config::load`).
    pub config_error: Option<String>,
}

pub struct VimApp {
    editor: Editor,
    bridge: Bridge,
    renderer: Box<dyn GridRenderer>,
    /// Last grid size sent with `nvim_ui_try_resize`.
    requested_size: (usize, usize),
    title: String,
    /// Title last given to the OS window.
    os_title: String,
    focused: bool,
    screenshot: Option<Screenshotter>,
    /// The popup look egui's own widgets were last given (`style::apply_to_egui`).
    egui_theme: Option<crate::popup::PopupTheme>,
    keys: KeyTranslator,
    /// Top-left of the grid area in points, from the last frame.
    origin: Pos2,
    /// The wheel's rows still to scroll a file window (`pump_wheel`).
    wheel_rows: Option<(i64, f32)>,
    /// When the GUI last scrolled a window itself (the wheel, a scroll
    /// bar): those scrolls glide, whatever the cursor does.
    gui_scrolled: Option<Instant>,
    /// The command line as drawn last frame (floats placed against
    /// Neovim's own command line go under it, `cmdline_float_offset`).
    cmdline_layout: Option<cmdline::CmdlineLayout>,
    /// Floats drawn away from their cells this frame, by how far (for
    /// clicks on them).
    float_offsets: HashMap<u64, Vec2>,
    /// The editor area (the grids and the leftover pixels around them).
    grid_area: Rect,
    /// Mouse button held down and the grid it was pressed on.
    drag: Option<Drag>,
    /// Fractional wheel lines not yet sent.
    wheel: Vec2,
    exited: bool,
    /// Why the config file was ignored at startup: said once Neovim has
    /// drawn (a message before that would go with its startup's).
    config_error: Option<String>,
    /// The Eshnv folders that weren't moved to Eshvi's, said once at start (`config::old_folders_note`).
    old_folders_note: Option<String>,
    /// Closing the window while Neovim doesn't answer.
    close_ask: CloseAsk,
    pending_start: Option<SpawnOptions>,
    /// Why Neovim didn't start, or how it ended when it didn't quit, shown
    /// in the window instead of the grid.
    start_error: Option<String>,
    grid_fonts: GridFonts,
    pum: PopupMenuView,
    /// The native picker (`picker.lua`), when one is open.
    picker: crate::picker::PickerView,
    /// Screen rects of native popups from the last frame; mouse input
    /// there belongs to egui, not Neovim.
    popup_rects: Vec<Rect>,
    lsp: LspPopups,
    /// Last cursor position inside a window (global cell), where LSP
    /// popups are anchored. Not updated while the cursor is on the cmdline.
    popup_anchor: (i64, i64),
    /// Neovim background last used to pick egui's dark/light theme.
    theme_bg: Option<u32>,
    /// Font from 'guifont' (or the config file when 'guifont' is empty).
    base_font: FontSpec,
    config_font: FontSpec,
    /// Font for sidebar windows, from the config file.
    sidebar_font: Option<FontSpec>,
    /// `ui_font`.
    ui_font: Option<FontSpec>,
    /// The agent panel's tab strip, from the last frame (the Changes popup hangs from it).
    agent_strip: Option<Rect>,
    /// Smooth scrolling (`smooth_scroll`), and when it was last moved along
    /// while scrolled text is gliding.
    smooth_scroll: bool,
    scroll_clock: Option<Instant>,
    scrollbars: crate::scrollbar::Scrollbars,
    /// The scroll bars of the terminals drawn this frame, for `paint_scrollbars` to put on.
    term_bars: Vec<crate::scrollbar::Target>,
    /// Sizes asked for external windows' grids (grid, cols, rows).
    external_resized: Vec<(u64, usize, usize)>,
    /// What the agents changed, and the Changes popup (and where it was).
    changes: crate::changes::Changes,
    changes_rect: Option<Rect>,
    /// The diff view, while it's open over the editor; the layout it was left
    /// in (kept for the session), where it was drawn, the file Neovim had
    /// when it opened (another one closes it), and a press on it is held.
    diff: Option<crate::diff_view::DiffView>,
    diff_layout: crate::diff_view::Layout,
    diff_split: f32,
    diff_rect: Option<Rect>,
    diff_buffer: Option<std::path::PathBuf>,
    diff_pressed: bool,
    /// An edit that waits for Neovim to say no file of it has unsaved changes.
    diff_pending: Option<crate::diff_view::Edit>,
    /// The view was opened by something the side bar reported this frame (`open_diff`):
    /// the side bar's own "focus me" from the same click doesn't take the keyboard back.
    diff_opened: bool,
    /// `:` was sent from the view: whether Neovim's command line has shown yet
    /// (when it has gone again, the keyboard is the view's once more).
    diff_cmd: Option<bool>,
    /// When `:` was sent from the view: the text event of the same key press is not Neovim's again.
    diff_colon: Option<Instant>,
    /// `[agent] font`: the agent panel's own font, if any.
    agent_font: Option<FontSpec>,
    terminal_font: Option<FontSpec>,
    /// Window handles drawn as a side panel (e.g. NvimTree).
    sidebar_wins: HashSet<i64>,
    /// 'laststatus' (3 = one global statusline).
    laststatus: i64,
    statusline_gui: bool,
    /// Neovim runs the own config (`status.lua` reports what the native status bar shows).
    own_config: bool,
    /// What `status.lua` last reported: the mode and the file in use.
    status: nvim_bridge::gui::Status,
    /// The view of the last normal window the cursor was in (the bar keeps it while the cursor is in a float).
    status_view: Option<grid::Viewport>,
    /// The branch in the native status bar (clickable), from the last frame.
    branch_rect: Option<Rect>,
    /// The native status bar, from the last frame: with the bottom panel
    /// shown, a drag of it resizes the panel.
    native_bar_rect: Option<Rect>,
    /// Fixed bar colors, or `None` for the theme's statusline colors.
    status_colors: StatusColorMode,
    status_font_size: f32,
    /// Configured UI sizes before zoom (status/tab bars, explorer).
    status_font_base: f32,
    explorer_font_base: f32,
    highlight_mode: bool,
    sidebar_hides_statusline: bool,
    msgs: MessageView,
    /// Rasterized file icons for tabs and the status bar.
    icons: IconTextures,
    /// LSP problems and git status per file.
    decos: Decorations,
    git: GitStatusWorker,
    /// What changes the repository (Source Control's buttons), one at a time.
    git_ops: GitOpsWorker,
    /// Read the status now (an operation finished, the branch menu opened).
    git_force: bool,
    /// The branches and graph were asked for with the last status.
    git_details: bool,
    /// File shown in each window (by window handle).
    win_paths: HashMap<i64, std::path::PathBuf>,
    /// Windows whose file has unsaved changes (from the same report).
    win_modified: HashSet<i64>,
    /// Neovim wrote a file since the last frame (counts as a disk change).
    saved: bool,
    /// Root whose git status was last requested.
    git_root: Option<std::path::PathBuf>,
    /// The status bar's problems indicator (click: diagnostics list).
    problems_rect: Option<Rect>,
    /// Reads `ui_send` data (image protocol, queries, progress bars).
    tty: crate::tty::TtyParser,
    /// The latest running LSP progress, for the status bar.
    lsp_progress: Option<Progress>,
    /// A progress bar from OSC 9;4 (|Progress| events): `Some(None)` has
    /// no known amount.
    tty_progress: Option<Option<u8>>,
    /// Window file tabs (winbar) from the last frame: the tab, its "×", and
    /// the window.
    /// Whether it's in the separator row above the window (see
    /// `tab_in_separator`), where a press on the tab focuses the window.
    winbar_tabs: Vec<(Rect, Rect, i64, bool)>,
    /// Buttons at the right of a window's file-tab row (a Markdown file's
    /// previews), by window.
    winbar_buttons: Vec<(Rect, i64, tabs::WinbarButton)>,
    /// `[terminal] enabled`: the bottom panel (terminals, Problems) exists.
    panel_enabled: bool,
    /// The native menu bar (`[menu] enabled`, macOS).
    menu_bar: Option<crate::menu::MenuBar>,
    /// Neovim's right-click menu (its items, when reported), to show once
    /// its grid is there.
    popup_menu: Option<(Vec<nvim_bridge::gui::MenuNode>, Instant)>,
    /// That menu, shown by the GUI (Neovim's own grid isn't drawn).
    popup_open: Option<actions::OpenPopup>,
    /// Markdown previews (`:EshviPreview`), drawn over their windows.
    preview: crate::preview::Previews,
    /// Where they were drawn (last frame), by window: the mouse there is
    /// theirs (the wheel scrolls them, a press focuses the window).
    preview_rects: Vec<(Rect, i64)>,
    /// A press the GUI took (opening or closing that menu): its release
    /// isn't Neovim's.
    swallow_release: Option<egui::PointerButton>,
    /// The settings in effect, the config file's time, when it was last
    /// looked at (`None`: look now), and `--font-size`.
    config: Config,
    config_modified: Option<std::time::SystemTime>,
    config_checked: Option<Instant>,
    cli_font_size: Option<f32>,
    /// `[terminal] problems_font = "ui"`: the Problems tab in the sidebar's
    /// proportional font.
    problems_proportional: bool,
    /// Explorer root last sent to Neovim (where panel terminals start).
    sent_root: Option<std::path::PathBuf>,
    /// The side bar for a session: what was sent to Neovim, its width in points as last
    /// drawn, a width to give it at the next draw (a loaded session's), and a tree view
    /// the session wants shown once it registers.
    session_sync: session::SessionSync,
    side_width: f32,
    force_side_width: Option<f32>,
    session_view: Option<String>,
    /// Height of the custom title bar (macOS), if enabled.
    titlebar: Option<f32>,
    /// How far below the window's top the traffic lights' middle is (moved
    /// to the title bar's middle): the bar's content is centered on it.
    lights_middle: Option<f32>,
    explorer: ExplorerView,
    /// Source Control, in the explorer's place.
    scm: ScmView,
    /// What the side bar shows (not kept between runs).
    side: SideView,
    /// The tree views that have a place of their own in the side bar, in the
    /// order they registered, and the number the next gets.
    side_views: side_bar::SideViews,
    explorer_enabled: bool,
    explorer_width: f32,
    /// Screen rect of the explorer panel from the last frame.
    explorer_rect: Option<Rect>,
    /// File in Neovim's current window, as last reported.
    active_file: Option<std::path::PathBuf>,
    active_filetype: String,
    /// Ctrl/Cmd +/- adjustment on top of `base_font.size`.
    zoom: f32,
    linespace: f32,
    /// Fonts installed in egui but not live until the next pass.
    pending_fonts: Option<GridFonts>,
    /// Last 'guifont' / 'linespace' values seen in `option_set`.
    seen_guifont: Option<String>,
    seen_linespace: Option<i64>,
    blink: Blink,
    /// The hidden terminal window (`:EshviTermDebug`).
    term_debug: Option<term_debug::TermDebug>,
    /// Who has the keyboard: Neovim, the side bar, or a part of the bottom panel or the agent panel.
    focus: Focus,
    /// The bottom panel: Problems and terminals (`[terminal]`).
    bottom: bottom_panel::BottomPanel,
    /// The agent panel (`[agent]`).
    agents: agent_panel::NativeAgents,
}

/// Cursor blinking per 'guicursor' (blinkwait/blinkon/blinkoff).
struct Blink {
    enabled: bool,
    /// When the cursor last moved or changed mode.
    since: Instant,
    key: (u64, usize, usize, usize),
}

impl Blink {
    /// Whether the cursor is visible now, and when that next changes.
    fn state(&mut self, editor: &Editor) -> (bool, Option<Duration>) {
        let key = (editor.cursor.grid, editor.cursor.row, editor.cursor.col, editor.mode_idx);
        if key != self.key {
            self.key = key;
            self.since = Instant::now();
        }
        let Some(mode) = editor.current_mode() else { return (true, None) };
        let (wait, on, off) = (mode.blinkwait, mode.blinkon, mode.blinkoff);
        if !self.enabled || wait == 0 || on == 0 || off == 0 {
            return (true, None);
        }
        let t = self.since.elapsed().as_millis() as u64;
        if t < wait {
            return (true, Some(Duration::from_millis(wait - t)));
        }
        // After blinkwait: off for blinkoff, then on for blinkon, repeating.
        let phase = (t - wait) % (on + off);
        if phase < off {
            (false, Some(Duration::from_millis(off - phase)))
        } else {
            (true, Some(Duration::from_millis(on + off - phase)))
        }
    }
}

/// How long Neovim may say nothing after it was asked to quit before a
/// second close request counts as "it doesn't answer".
const QUIT_PATIENCE: Duration = Duration::from_secs(2);

/// Closing the window asks Neovim to quit (`confirm qa`), which a Neovim
/// that hangs (a plugin looping, a command that never ends) never does.
/// Asked again with nothing heard from it since, the window says so; asked
/// once more, it closes without Neovim.
#[derive(Default)]
struct CloseAsk {
    /// When Neovim was first asked, with nothing heard from it since.
    asked: Option<Instant>,
    warned: bool,
}

#[derive(Debug, PartialEq)]
enum CloseStep {
    /// Ask Neovim to quit.
    Ask,
    /// Say that it doesn't answer and what closing again does.
    Warn,
    /// Close the window without it.
    Quit,
}

impl CloseAsk {
    /// Neovim sent something: it is alive.
    fn heard(&mut self) {
        *self = Self::default();
    }

    /// The window was asked to close (or Quit was chosen), at `now`.
    fn request(&mut self, now: Instant) -> CloseStep {
        match self.asked {
            None => {
                self.asked = Some(now);
                CloseStep::Ask
            }
            // Too soon to tell (a double click, a slow moment).
            Some(t) if now.duration_since(t) < QUIT_PATIENCE => CloseStep::Ask,
            Some(_) if !self.warned => {
                self.warned = true;
                CloseStep::Warn
            }
            Some(_) => CloseStep::Quit,
        }
    }
}

struct Drag {
    button: &'static str,
    grid: u64,
    cell: (i64, i64),
    /// From where the pointer is to where Neovim holds the drag: a separator
    /// grabbed from the pixels beside its cell (`separator_grab`) moves with
    /// the pointer as if grabbed on its cell's middle, not a column off.
    shift: Vec2,
    /// Begun on the last status line: Neovim isn't sent its moves (a drag of
    /// that line changes 'cmdheight': the status bar moved off the panel).
    still: bool,
}

impl VimApp {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: Options) -> anyhow::Result<Self> {
        let ctx = cc.egui_ctx.clone();
        let config = opts.config;
        crate::popup::set_settings(crate::popup::ThemeSettings {
            colors: config.colors.resolve(),
            corner_radius: config.popup.corner_radius,
            shadow: config.popup.shadow,
            restyle_floats: config.popup.restyle_floats,
        });
        let mut config_font = config
            .font
            .as_deref()
            .and_then(|f| FontSpec::parse_guifont(f, DEFAULT_FONT_SIZE))
            .unwrap_or(FontSpec::default_with_size(DEFAULT_FONT_SIZE));
        if let Some(size) = opts.font_size {
            config_font.size = size;
        }
        let ui_font = config.ui_font.as_deref().and_then(|f| FontSpec::parse_guifont(f, config_font.size - 1.0));
        let sidebar_font =
            config.sidebar.font.as_deref().and_then(|f| FontSpec::parse_guifont(f, config_font.size - 1.0));
        let agent_font = config.agent.font.as_deref().and_then(|f| FontSpec::parse_guifont(f, config_font.size));
        let terminal_font = config.terminal.font.as_deref().and_then(|f| FontSpec::parse_guifont(f, config_font.size));
        let grid_fonts =
            fonts::install(&ctx, &config_font, ui_font.as_ref(), sidebar_font.as_ref(), agent_font.as_ref(), terminal_font.as_ref());
        let renderer = EguiGridRenderer::new(grid_fonts.clone());
        set_text_sizes(&ctx, grid_fonts.ui_size);
        let mut keys = KeyTranslator::default();
        if let Some(meta) = config.alt_is_meta {
            keys.alt_is_meta = meta;
        }
        let mut lsp = LspPopups::default();
        lsp.limits = PopupLimits {
            hover_max_width: config.popup.hover_max_width,
            hover_max_height: config.popup.hover_max_height,
            signature_max_width: config.popup.signature_max_width,
        };

        // Neovim is started on the first frame, once the window size and
        // font metrics are known, so startup commands see the real size.
        let sidebar_filetypes = if config.sidebar.enabled { config.sidebar.filetypes.clone() } else { vec![] };
        // The native menu bar (macOS), from Neovim's menus.
        let menu_enabled = config.menu.enabled && cfg!(target_os = "macos");
        let (appname, native_completion) = nvim_bridge::appname_for(
            config.neovim.config == crate::config::NeovimConfigKind::Shared,
            std::env::var("NVIM_APPNAME").ok().as_deref(),
        );
        let pending_start = Some(SpawnOptions {
            nvim: opts.nvim,
            args: opts.nvim_args,
            width: 0,
            height: 0,
            sidebar_filetypes,
            restore_notify: config.messages.restore_notify,
            // The native bar is one bar: a global statusline whatever the layout says.
            global_statusline: config.layout.global_statusline || (native_completion && config.statusline.gui),
            winbar: config.layout.winbar,
            terminal_enabled: config.terminal.enabled,
            menu: menu_enabled,
            agent: config.agent.enabled.then(|| nvim_bridge::AgentOptions {
                command: config.agent.command.clone(),
                commands: config.agent.commands.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                ide_dir: None,
            }),
            native_tabs: config.layout.native_tabs,
            appname,
            bin_dir: nvim_bridge::running_bin_dir(),
            native_completion,
            native_status: native_completion,
            native_picker: native_completion,
            health_check: native_completion,
            lsp_popups: native_completion,
            slang: native_completion,
            session_auto: config.session.auto,
            copy_on_select: config.neovim.copy_on_select,
        });

        let app = Self {
            editor: Editor::new(),
            bridge: Bridge::new(),
            renderer: Box::new(renderer),
            requested_size: (0, 0),
            title: String::new(),
            os_title: String::new(),
            focused: true,
            screenshot: Screenshotter::from_env(),
            egui_theme: None,
            keys,
            origin: Pos2::ZERO,
            wheel_rows: None,
            gui_scrolled: None,
            cmdline_layout: None,
            float_offsets: HashMap::new(),
            grid_area: Rect::NOTHING,
            drag: None,
            wheel: Vec2::ZERO,
            exited: false,
            config_error: opts.config_error,
            old_folders_note: crate::config::old_folders_note(),
            close_ask: CloseAsk::default(),
            pending_start,
            start_error: None,
            grid_fonts,
            pum: PopupMenuView::default(),
            picker: Default::default(),
            popup_rects: Vec::new(),
            lsp,
            popup_anchor: (0, 0),
            theme_bg: None,
            base_font: config_font.clone(),
            config_font,
            sidebar_font,
            ui_font: ui_font.clone(),
            agent_strip: None,
            changes: {
                let ctx = ctx.clone();
                crate::changes::Changes::new(move || ctx.request_repaint())
            },
            changes_rect: None,
            diff: None,
            diff_layout: config.diff.layout,
            diff_split: 0.5,
            diff_rect: None,
            diff_buffer: None,
            diff_pressed: false,
            diff_pending: None,
            diff_opened: false,
            diff_cmd: None,
            diff_colon: None,
            smooth_scroll: config.smooth_scroll.unwrap_or(true),
            scroll_clock: None,
            scrollbars: crate::scrollbar::Scrollbars::new(config.scrollbar.unwrap_or_default()),
            term_bars: Vec::new(),
            external_resized: Vec::new(),
            agent_font,
            terminal_font,
            sidebar_wins: HashSet::new(),
            laststatus: 2,
            statusline_gui: config.statusline.gui,
            own_config: native_completion,
            status: Default::default(),
            status_view: None,
            branch_rect: None,
            native_bar_rect: None,
            status_colors: config.statusline.resolve_colors(),
            status_font_size: fonts::status_base_size(config.statusline.font_size, ui_font.as_ref()),
            status_font_base: fonts::status_base_size(config.statusline.font_size, ui_font.as_ref()),
            explorer_font_base: fonts::explorer_base_size(config.explorer.font_size, ui_font.as_ref()),
            highlight_mode: config.statusline.highlight_mode,
            sidebar_hides_statusline: config.sidebar.hide_statusline,
            msgs: MessageView::default(),
            icons: IconTextures::default(),
            decos: Decorations::default(),
            git: {
                let wake = ctx.clone();
                GitStatusWorker::new(move || wake.request_repaint())
            },
            git_ops: {
                let wake = ctx.clone();
                GitOpsWorker::new(move || wake.request_repaint())
            },
            git_force: false,
            git_details: false,
            win_paths: HashMap::new(),
            win_modified: HashSet::new(),
            saved: false,
            git_root: None,
            problems_rect: None,
            tty: Default::default(),
            lsp_progress: None,
            tty_progress: None,
            winbar_tabs: Vec::new(),
            winbar_buttons: Vec::new(),
            panel_enabled: config.terminal.enabled,
            menu_bar: menu_enabled.then(|| crate::menu::MenuBar::new(&cc.egui_ctx)),
            popup_menu: None,
            popup_open: None,
            preview: Default::default(),
            preview_rects: Vec::new(),
            swallow_release: None,
            config_modified: crate::config::modified(),
            config_checked: Some(Instant::now()),
            cli_font_size: opts.font_size,
            problems_proportional: config.terminal.problems_font == crate::config::ListFont::Ui,
            sent_root: None,
            session_sync: Default::default(),
            side_width: config.explorer.width.clamp(120.0, 800.0),
            force_side_width: None,
            session_view: None,
            titlebar: config.window.titlebar(),
            lights_middle: None,
            explorer: {
                let mut e = ExplorerView::new(
                    config.explorer.visible,
                    config.explorer.show_ignored,
                    fonts::explorer_base_size(config.explorer.font_size, ui_font.as_ref()),
                );
                e.agent = config.agent.enabled;
                (e.bookmarks, e.bookmarks_note) = crate::bookmarks::Bookmarks::load();
                e.nesting = config.explorer.nesting;
                e.outline_enabled = config.outline.enabled;
                e.outline_follow = config.outline.follow_cursor;
                e.outline().sort = config.outline.sort;
                // The workspace is the launch directory; Neovim reports
                // explicit `:cd`s later.
                let root = std::env::current_dir().ok().and_then(|d| explorer_view::initial_root(&d));
                if let (true, Some(root)) = (config.explorer.enabled, root) {
                    e.set_root(root, &ctx);
                }
                e
            },
            scm: ScmView::new(fonts::explorer_base_size(config.explorer.font_size, ui_font.as_ref()), config.agent.enabled),
            side: SideView::default(),
            side_views: side_bar::SideViews::default(),
            explorer_enabled: config.explorer.enabled,
            explorer_width: config.explorer.width.clamp(120.0, 800.0),
            explorer_rect: None,
            active_file: None,
            active_filetype: String::new(),
            zoom: 0.0,
            linespace: 0.0,
            pending_fonts: None,
            seen_guifont: None,
            seen_linespace: None,
            term_debug: None,
            focus: Focus::Editor,
            bottom: bottom_panel::BottomPanel::new(),
            agents: agent_panel::NativeAgents::new(agent_panel::tools(&config.agent.command, &config.agent.commands)),
            blink: Blink { enabled: config.cursor_blink.unwrap_or(true), since: Instant::now(), key: (0, 0, 0, 0) },
            config,
        };
        // (Queued until Neovim is up.)
        app.send_outline_config();
        Ok(app)
    }

    /// Take the editor back from the bridge, where it waits between frames
    /// (`Bridge::park`). True when redraws were applied to it there, because
    /// no frame came to take them (the window minimized, the screen locked).
    fn take_editor(&mut self, ctx: &egui::Context) -> bool {
        let applied = self.bridge.unpark(&mut self.editor);
        if applied {
            self.apply_batch(ctx, Vec::new());
        }
        applied
    }

    /// Ask Neovim to quit, so unsaved changes prompt (closing the window,
    /// Quit in the menu). One that doesn't answer is said so the second time
    /// and left behind the third (`CloseAsk`).
    pub(super) fn request_quit(&mut self, ctx: &egui::Context) {
        match self.close_ask.request(Instant::now()) {
            CloseStep::Ask => self.bridge.send(UiCommand::Command("confirm qa".into())),
            CloseStep::Warn => self.editor.push_message(
                "wmsg",
                "Neovim isn't answering (a command or a plugin is still running; Ctrl-C may stop it). \
                 Close the window again to quit without it: unsaved changes are lost.",
            ),
            CloseStep::Quit => {
                log::warn!("closing without Neovim: it didn't answer");
                self.exited = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    fn apply_batch(&mut self, ctx: &egui::Context, batch: Vec<RedrawEvent>) {
        let cursor_before = (self.editor.cursor.grid, self.editor.cursor.row);
        for ev in batch {
            self.editor.apply(ev);
        }
        self.editor.settle_floats();
        // Scrolled text in windows glides (not in floats or messages).
        for s in std::mem::take(&mut self.editor.scrolled) {
            let window = self.editor.windows.get(&s.grid).is_some_and(|w| w.kind == WindowKind::Normal);
            // The keyboard pushing the cursor along at the edge (a held `j`):
            // it stays on its screen row, and a glide at the key's rate
            // would jump its line down and back each step. The text steps.
            let by_gui = self.gui_scrolled.is_some_and(|t| t.elapsed() < Duration::from_millis(250));
            let cursor = (self.editor.cursor.grid, self.editor.cursor.row);
            let pushed = cursor == cursor_before && cursor.0 == s.grid && s.rows.abs() <= 2;
            if pushed && !by_gui {
                self.renderer.end_scroll(s.grid);
            } else if self.smooth_scroll && window {
                self.renderer.begin_scroll(s);
                // The clock starts now, not at the last (maybe long ago) frame.
                self.scroll_clock.get_or_insert_with(Instant::now);
            }
        }
        self.read_tty(ctx);
        self.sync_ui_options(ctx);
    }

    /// Act on what Neovim wrote to our "terminal" (`ui_send`): images for
    /// placeholder cells, answers to its queries, the progress bar.
    fn read_tty(&mut self, ctx: &egui::Context) {
        if self.editor.tty_out.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.editor.tty_out);
        for ev in self.tty.feed(&data) {
            match ev {
                crate::tty::TtyEvent::Graphics(cmd) => {
                    if let Some(reply) = self.renderer.images().apply(ctx, cmd) {
                        self.bridge.send(UiCommand::TermResponse(reply));
                    }
                }
                crate::tty::TtyEvent::Progress(p) => self.tty_progress = p,
            }
        }
    }

    /// React to 'guifont' and 'linespace' from `option_set`.
    fn sync_ui_options(&mut self, ctx: &egui::Context) {
        if let Some(OptionValue::Str(gf)) = self.editor.options.get("guifont") {
            if self.seen_guifont.as_ref() != Some(gf) {
                self.seen_guifont = Some(gf.clone());
                self.base_font =
                    FontSpec::parse_guifont(gf, self.config_font.size).unwrap_or_else(|| self.config_font.clone());
                self.zoom = 0.0;
                let spec = self.base_font.clone();
                self.pending_fonts =
                    Some(fonts::install(
                    ctx,
                    &spec,
                    self.ui_font.as_ref(),
                    self.sidebar_font.as_ref(),
                    self.agent_font.as_ref(),
                    self.terminal_font.as_ref(),
                ));
            }
        }
        if let Some(OptionValue::Int(ls)) = self.editor.options.get("linespace") {
            if self.seen_linespace != Some(*ls) {
                self.seen_linespace = Some(*ls);
                self.linespace = *ls as f32;
                self.renderer.set_fonts(self.grid_fonts.clone(), self.linespace);
            }
        }
    }

    /// Keep git status current: right away when files change on disk or the
    /// root changes, otherwise every few seconds while the window has focus.
    fn refresh_git(&mut self, ctx: &egui::Context, disk_changed: bool) {
        for result in self.git.results.try_iter().collect::<Vec<_>>() {
            let crate::git::StatusResult::Status(repo, details) = result;
            self.decos.set_git(repo.as_ref().map(crate::git::Repo::decorations).unwrap_or_default());
            // A commit accepts the agents' changes that went into it.
            if let Some(r) = &repo {
                self.changes.on_head(&r.top, r.head.as_deref());
            }
            self.scm.set_repo(repo);
            if let Some(details) = details {
                self.scm.set_details(details);
            }
        }
        // The Changes count (and popup) follows the disk.
        for done in self.changes.poll() {
            match done {
                // Put back on disk: buffers of it read it again.
                crate::changes::Done::Rejected => self.bridge.send(UiCommand::Command("silent! checktime".into())),
                crate::changes::Done::Failed(message) => self.editor.push_message("emsg", &message),
                crate::changes::Done::Note(message) => self.editor.push_message("wmsg", &message),
            }
        }
        if disk_changed {
            self.changes.refresh();
        }
        let Some(root) = self.explorer.root().map(|p| p.to_path_buf()) else { return };
        const PERIOD: std::time::Duration = scm::GIT_PERIOD;
        // (`self.focused` is brought up to date after this: a window that has
        // just gained focus still reads as unfocused here.)
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        let due = self.git.since_last().is_none_or(|t| t >= PERIOD);
        // The branches and the graph, while Source Control is shown; read
        // again when it comes up.
        let details = self.explorer.visible && self.side == SideView::Git;
        let shown = details && !self.git_details;
        self.git_details = details;
        // Coming back to the window reads it at once: git may have moved meanwhile.
        let gained = focused && !self.focused;
        let now = std::mem::take(&mut self.git_force) | shown | gained;
        if disk_changed || now || (due && focused) || self.git_root.as_ref() != Some(&root) {
            let force = now || self.git_root.as_ref() != Some(&root);
            self.git_root = Some(root.clone());
            self.git.refresh(&root, force, details);
        }
        // The next read, with no input needed to wake the window for it: when
        // it falls due (a wait of a full period from now would find it a hair
        // early, or after a forced read not due yet, and put it off a whole
        // period more).
        if focused {
            ctx.request_repaint_after(scm::wait_for_git(self.git.since_last()));
        }
    }

    /// Ctrl/Cmd +, -, 0: change the font size without touching 'guifont'.
    fn set_zoom(&mut self, ctx: &egui::Context, zoom: f32) {
        let size = (self.base_font.size + zoom).clamp(6.0, 72.0);
        self.zoom = size - self.base_font.size;
        self.grid_fonts = self.grid_fonts.with_size(size);
        if let Some(p) = &mut self.pending_fonts {
            *p = p.with_size(size);
        }
        self.renderer.set_fonts(self.grid_fonts.clone(), self.linespace);
        set_text_sizes(ctx, self.grid_fonts.ui_size);
        // The rest of the UI scales with the editor font.
        let factor = size / self.base_font.size;
        self.status_font_size = (self.status_font_base * factor).clamp(6.0, 40.0);
        self.explorer.font_size = (self.explorer_font_base * factor).clamp(6.0, 40.0);
    }

    fn sync_size(&mut self, ctx: &egui::Context, available: Vec2) {
        let m = self.renderer.metrics();
        let cols = ((available.x / m.width).floor() as usize).max(1);
        let rows = ((available.y / m.height).floor() as usize).max(1);
        if let Some(mut opts) = self.pending_start.take() {
            (opts.width, opts.height) = (cols, rows);
            self.requested_size = (cols, rows);
            let waker_ctx = ctx.clone();
            if let Err(e) = self.bridge.start(opts, Arc::new(move || waker_ctx.request_repaint())) {
                eprintln!("eshvi: failed to start nvim: {e}");
                self.start_error = Some(start_error_text(&e.to_string()));
            }
        } else if (cols, rows) != self.requested_size {
            self.requested_size = (cols, rows);
            self.bridge.send(UiCommand::TryResize { width: cols, height: rows });
        }
    }
}

impl eframe::App for VimApp {
    /// ⌘V with an image and no text on the clipboard is an empty paste: a
    /// terminal sends it, and a program like Claude Code then reads the image.
    /// And files copied (in the Finder or the explorer) paste into a terminal as
    /// their paths, not the names that come with them as text.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        for ev in &mut raw_input.events {
            if matches!(ev, egui::Event::PasteImage(_)) {
                *ev = egui::Event::Paste(String::new());
            }
            if let egui::Event::Paste(text) = ev {
                if self.focus.in_terminal() {
                    if let Some(paths) = crate::file_clipboard::pasted_paths() {
                        *text = paths;
                    }
                }
            }
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut flushed = self.take_editor(ctx);
        while let Some(ev) = self.bridge.try_recv() {
            self.close_ask.heard();
            match ev {
                BridgeEvent::Redraw(batch) => {
                    self.apply_batch(ctx, batch);
                    flushed = true;
                }
                BridgeEvent::Gui(GuiEvent::Sidebar(wins)) => self.sidebar_wins = wins.into_iter().collect(),
                BridgeEvent::Gui(GuiEvent::Preview(wins)) => self.preview.set_wins(wins),
                BridgeEvent::Gui(GuiEvent::PreviewText { buf, text }) => self.preview.set_text(buf, &text),
                BridgeEvent::Gui(GuiEvent::DecoColors(colors)) => self.editor.deco_theme = colors.into_iter().collect(),
                BridgeEvent::Gui(GuiEvent::DiffColors(colors)) => self.editor.diff_theme = colors.into_iter().collect(),
                BridgeEvent::Gui(GuiEvent::TermColors(colors)) => self.editor.term_colors = colors,
                BridgeEvent::Gui(GuiEvent::Copy(text)) => ctx.copy_text(text),
                BridgeEvent::Gui(GuiEvent::Cwd(cwd)) if self.explorer_enabled => {
                    self.explorer.set_root(cwd.into(), ctx);
                }
                BridgeEvent::Gui(GuiEvent::Buffer { path, filetype }) => {
                    self.active_filetype = filetype;
                    let path = (!path.is_empty()).then(|| std::path::PathBuf::from(path));
                    if let Some(p) = &path {
                        self.explorer.set_active(p.clone(), ctx);
                    }
                    // Another file in Neovim (an explorer click, a command): the diff
                    // that covered it goes.
                    if self.diff.is_some() && path != self.diff_buffer {
                        self.close_diff();
                    }
                    self.active_file = path;
                }
                BridgeEvent::Gui(GuiEvent::Session(gui)) => self.apply_session(ctx, gui),
                BridgeEvent::Gui(GuiEvent::Explorer(action)) => self.explorer_command(&action),
                BridgeEvent::Gui(GuiEvent::Git(action)) => self.git_command(&action),
                BridgeEvent::Gui(GuiEvent::TermDebug(action)) => self.term_debug_command(ctx, &action),
                BridgeEvent::Gui(GuiEvent::Panel { kind, args }) => self.panel_command(ctx, &kind, &args),
                BridgeEvent::Gui(GuiEvent::Problems(items)) => self.set_problems(&items),
                BridgeEvent::Gui(GuiEvent::AgentEnv(env)) => self.agents.env = env,
                BridgeEvent::Gui(GuiEvent::Navigate(dir)) => {
                    if let Some(dir) = crate::focus::Dir::from_letter(&dir) {
                        self.navigate_from_editor(dir);
                    }
                }
                BridgeEvent::Gui(GuiEvent::Outline(action)) => self.outline_command(&action),
                BridgeEvent::Gui(GuiEvent::TreeSet(set)) => self.tree_set(ctx, set),
                BridgeEvent::Gui(GuiEvent::TreeSelect { view, id, path }) => self.tree_select(ctx, &view, &id, &path),
                BridgeEvent::Gui(GuiEvent::TreeMenu { view, id, items }) => self.tree_menu(ctx, &view, &id, items),
                BridgeEvent::Gui(GuiEvent::TreeRegister(reg)) => self.tree_register(ctx, reg),
                BridgeEvent::Gui(GuiEvent::TreeShow { view, action, focus }) => self.tree_show(&view, &action, focus),
                BridgeEvent::Gui(GuiEvent::TreeUnregister(view)) => self.tree_unregister(ctx, &view),
                BridgeEvent::Gui(GuiEvent::GitDiff { top, oid, parent, path, left_path, deleted }) => {
                    let parent = (!parent.is_empty()).then_some(parent);
                    let spec = scm_view::file_at_commit(std::path::Path::new(&top), &oid, parent.as_deref(), &path, left_path.as_deref(), deleted);
                    self.git_file_diff(ctx, spec);
                }
                BridgeEvent::Gui(GuiEvent::Diagnostics(files)) => self.decos.set_problems(files.into_iter().map(|f| {
                    (std::path::PathBuf::from(f.path), Problems { errors: f.errors, warnings: f.warnings })
                })),
                BridgeEvent::Gui(GuiEvent::Saved(_)) => {
                    self.saved = true;
                    self.config_checked = None;
                }
                // Only from the diff view, so that an empty file the user saves stays.
                BridgeEvent::Gui(GuiEvent::DiffSaved(path)) => {
                    if let Some(rel) = self.changes.remove_if_emptied(std::path::Path::new(&path)) {
                        let args = vec![RpcValue::from(path.as_str()), RpcValue::from(rel.as_str())];
                        self.bridge.send(UiCommand::Lua { code: "return Eshvi.changes_removed(...)".into(), args });
                    } else {
                        // Saved with its diff open: what is on disk is accepted.
                        self.changes.accept_saved(std::path::Path::new(&path));
                    }
                }
                BridgeEvent::Gui(GuiEvent::DiffEdit { op, path, ok, message }) => self.diff_edit_done(&op, &path, ok, &message),
                BridgeEvent::Gui(GuiEvent::Windows(wins)) => {
                    self.win_modified = wins.iter().filter(|(_, _, m)| *m).map(|(w, _, _)| *w).collect();
                    self.win_paths = wins.into_iter().map(|(w, p, _)| (w, std::path::PathBuf::from(p))).collect();
                }
                BridgeEvent::Gui(GuiEvent::Options { laststatus }) => self.laststatus = laststatus,
                BridgeEvent::Gui(GuiEvent::Status(status)) => self.set_status(status),
                BridgeEvent::Gui(GuiEvent::Menu(menus)) => {
                    if let Some(bar) = &mut self.menu_bar {
                        bar.set_menus(&menus);
                    }
                }
                BridgeEvent::Gui(GuiEvent::PopupMenu(items)) => self.popup_menu = Some((items, Instant::now())),
                BridgeEvent::Gui(GuiEvent::Action(action)) => self.gui_action(ctx, &action),
                BridgeEvent::Gui(GuiEvent::Progress(p)) => self.lsp_progress = p,
                BridgeEvent::Gui(GuiEvent::PumDoc { selected, contents }) => self.pum.set_requested_doc(selected, contents),
                BridgeEvent::Gui(GuiEvent::PickerOpen(open)) => self.open_picker(ctx, open),
                BridgeEvent::Gui(GuiEvent::PickerPreview(answer)) => {
                    if self.picker.set_preview(answer) {
                        ctx.request_repaint();
                    }
                }
                BridgeEvent::Gui(GuiEvent::PickerItems { id, gen, items }) => {
                    if self.picker.set_items(id, gen, items) {
                        ctx.request_repaint();
                    }
                }
                BridgeEvent::Gui(GuiEvent::PickerTyped { id, keys }) => self.picker.typed(ctx, id, &keys),
                BridgeEvent::Gui(GuiEvent::PickerClose(id)) => {
                    if self.picker.id() == Some(id) {
                        self.picker.close();
                    }
                }
                BridgeEvent::Gui(ev) => {
                    log::debug!("gui event: {ev:?}");
                    self.lsp.handle(ev);
                }
                BridgeEvent::Exited { error } => {
                    if let Some(e) = &error {
                        eprintln!("eshvi: {e}");
                    }
                    self.exited = true;
                    match error {
                        // Neovim never drew anything: say why in the window,
                        // since opened from the Finder there is no terminal.
                        Some(e) if self.editor.grids.is_empty() => {
                            self.start_error = Some(start_error_text(&e));
                            flushed = true;
                        }
                        // It didn't quit (a crash, a kill): say so instead
                        // of vanishing; the window closes when asked to.
                        Some(e) => {
                            self.start_error = Some(crash_text(&e));
                            flushed = true;
                        }
                        None => ctx.send_viewport_cmd(ViewportCommand::Close),
                    }
                }
            }
        }
        if flushed && !self.editor.grids.is_empty() {
            if let Some(e) = self.config_error.take() {
                self.editor.push_message("emsg", &e);
            }
            if let Some(e) = self.explorer.bookmarks_note.take() {
                self.editor.push_message("wmsg", &e);
            }
            if let Some(e) = self.old_folders_note.take() {
                self.editor.push_message("wmsg", &e);
            }
        }
        if let Some(s) = &mut self.screenshot {
            if flushed {
                s.on_flush();
            }
            if s.is_done() {
                self.screenshot = None;
                self.bridge.send(UiCommand::Command("qa!".into()));
            }
        }
        // Follow Neovim's background so egui (and markdown code themes) match.
        let bg = self.editor.default_colors.bg;
        if self.theme_bg != Some(bg) {
            self.theme_bg = Some(bg);
            let luma = 0.299 * ((bg >> 16) & 0xff) as f32 + 0.587 * ((bg >> 8) & 0xff) as f32 + 0.114 * (bg & 0xff) as f32;
            ctx.set_theme(if luma < 128.0 { egui::Theme::Dark } else { egui::Theme::Light });
        }
        self.title = self.editor.title.clone();
        let title = self.display_title();
        if title != self.os_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.os_title = title;
        }

        // Closing the window asks Neovim to quit, so unsaved changes prompt.
        if ctx.input(|i| i.viewport().close_requested()) && !self.exited {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.request_quit(ctx);
        }

        self.run_menu_commands(ctx);
        self.update_menu_state(ctx);
        self.watch_config(ctx);
        self.tick_terminals(ctx);
        self.tick_agents(ctx);
        self.sync_focus();
        self.handle_input(ctx);
        let disk_changed = self.explorer.poll(ctx) | std::mem::take(&mut self.saved);
        if disk_changed {
            if let Some(d) = &mut self.diff {
                d.on_disk_changed(Instant::now());
            }
        }
        if let Some(root) = self.explorer.root().filter(|r| self.sent_root.as_deref() != Some(*r)) {
            self.sent_root = Some(root.to_path_buf());
            self.bridge.send(UiCommand::WorkspaceRoot(root.to_path_buf()));
        }
        self.sync_session(ctx);
        self.refresh_git(ctx, disk_changed);
        self.poll_git_ops(ctx);

        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if focused != self.focused {
            self.focused = focused;
            self.bridge.send(UiCommand::SetFocus(focused));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // (Still with the bridge, should a frame come without `logic` before it.)
        self.take_editor(ui.ctx());
        self.show_frame(ui, frame);
        self.filter_links(ui.ctx());
        // Until the next frame the editor waits with the bridge, which
        // applies redraws to it if they pile up with no frame to take them.
        self.bridge.park(&mut self.editor);
    }
}

impl VimApp {
    fn show_frame(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Keys belong to Neovim (or the explorer): egui must not move focus to
        // a widget with Tab and "click" it with Enter, e.g. the explorer's
        // "⋯" menu after Tab-completing `:e file`. Only the text fields keep
        // focus (the explorer's name field, the Problems filter, Source
        // Control's message and prompts).
        ctx.memory_mut(|m| match m.focused() {
            Some(id) if !self.keeps_focus(id) => m.surrender_focus(id),
            _ => {}
        });
        // Click targets are re-made by this frame's painting.
        self.agent_strip = None;
        self.winbar_tabs.clear();
        self.winbar_buttons.clear();
        // egui's menus and tooltips in the popups' look (again when the
        // colorscheme changes).
        let theme = crate::popup::PopupTheme::from_editor(&self.editor);
        if self.egui_theme != Some(theme) {
            style::apply_to_egui(&ctx, &theme);
            self.egui_theme = Some(theme);
        }
        if let Some(h) = self.titlebar {
            if !ctx.input(|i| i.viewport().fullscreen).unwrap_or(false) {
                self.lights_middle = chrome::center_traffic_lights(frame, h / 2.0);
            }
        }
        self.show_titlebar(ui);
        self.track_file_drag(&ctx, frame);
        self.show_explorer(ui);
        self.show_agent_column(ui);
        self.sync_focus();
        self.show_tabline(ui);
        // The grid takes what the explorer panel leaves.
        let rect = ui.available_rect_before_wrap();
        // A margin before the first column (half a cell, as between windows
        // side by side), so signs and text don't touch the panel's edge.
        let inset = Vec2::new(self.left_inset(), 0.0);
        self.origin = rect.min + inset;
        self.grid_area = rect;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, rgb(self.editor.default_colors.bg));

        // Switch to newly installed fonts once egui has loaded them.
        if self.pending_fonts.as_ref().is_some_and(|f| f.is_live(&ctx)) {
            let mut fonts = self.pending_fonts.take().unwrap();
            fonts = fonts.with_size((self.base_font.size + self.zoom).clamp(6.0, 72.0));
            log::info!("font: {} {}pt", fonts.primary, fonts.size);
            set_text_sizes(&ctx, fonts.ui_size);
            self.renderer.set_fonts(fonts.clone(), self.linespace);
            self.grid_fonts = fonts;
        }

        self.renderer.begin_frame(&ctx, &self.editor);
        if let Some(last) = self.scroll_clock {
            let now = Instant::now();
            self.scroll_clock = Some(now);
            if self.renderer.animate_scroll(now.duration_since(last).as_secs_f32()) {
                ctx.request_repaint();
            } else {
                self.scroll_clock = None;
            }
        }
        // The bottom panel under the grid, as wide as the editor: Neovim's size is the
        // rest. With the status bar drawn by the GUI it stays at the window's bottom (below
        // the panel, which sits on its row); else the panel is at the bottom.
        let row = self.renderer.metrics().height;
        let panel_h = if self.panel_enabled && self.bottom.shown { self.bottom_height(rect.height()) } else { 0.0 };
        let status_below = panel_h > 0.0 && self.statusline_gui && (self.laststatus == 3 || self.native_status());
        let panel_rect = (panel_h > 0.0).then(|| {
            let bottom = rect.max.y - if status_below { row } else { 0.0 };
            Rect::from_min_max(Pos2::new(rect.min.x, bottom - panel_h), Pos2::new(rect.max.x, bottom))
        });
        self.sync_size(&ctx, rect.size() - inset - Vec2::new(0.0, panel_h));
        if let Some(error) = &self.start_error {
            let color = ui.visuals().strong_text_color();
            let width = (rect.width() - 64.0).clamp(0.0, 640.0);
            let galley = painter.layout(error.clone(), egui::FontId::proportional(14.0), color, width);
            painter.galley(rect.center() - galley.size() / 2.0, galley, color);
            if let Some(s) = &mut self.screenshot {
                s.after_paint(&ctx);
            }
            return;
        }

        let m = self.renderer.metrics();
        let cell_pos = |row: i64, col: i64| rect.min + inset + Vec2::new(col as f32 * m.width, row as f32 * m.height);

        // While the external cmdline is open the cursor lives there.
        let (blink_on, next_blink) = self.blink.state(&self.editor);
        if let (Some(d), true) = (next_blink, self.focused) {
            ctx.request_repaint_after(d);
        }
        let show_cursor = self.editor.cmdline.is_empty() && (blink_on || !self.focused);

        // Each grid is a layer, bottom to top. The cursor is painted with its
        // grid so floats above it cover it, as in the TUI.
        self.show_popup_menu(&ctx);
        self.finish_file_drag(&ctx);
        self.preview_rects.clear();
        self.float_offsets.clear();
        for grid in self.editor.draw_order() {
            if self.editor.windows.get(&grid).is_some_and(|w| self.is_container(w) || self.is_plugin_scrollbar(w))
                || self.popup_open.as_ref().is_some_and(|p| p.grid == grid)
            {
                continue;
            }
            let (row, col) = self.editor.grid_origin(grid);
            let style = match self.editor.windows.get(&grid) {
                // A float laid inside another one (which-key's help line,
                // snacks' picker input, list and preview) is part of it: flat,
                // no rounded corners or shadow; its rules meet the frame.
                Some(w) if matches!(w.kind, WindowKind::Float { .. }) => match self.enclosing_float(w) {
                    Some(outer) => LayerStyle { float: true, rule_extend: self.rule_extend(w, outer), ..Default::default() },
                    None => LayerStyle { under: Some(self.color_under(w)), ..LayerStyle::float() },
                },
                Some(w) if self.sidebar_wins.contains(&w.win) => LayerStyle::SIDEBAR,
                _ => LayerStyle::default(),
            };
            let layer = painter.clone();
            // A float right below the float it's anchored to (blink.cmp's
            // docs below its menu; maybe moved there, `settle_floats`):
            // its frame doesn't grow up over that one (a header strip grows
            // by a margin), and one moved down is cut at the grid's bottom.
            let shifted = self.editor.windows.get(&grid).is_some_and(|w| w.shift > 0 || self.below_anchor_float(w));
            let rows = self.editor.grids.get(&DEFAULT_GRID).map_or(0, |g| g.height() as i64);
            let layer = if shifted {
                let within = Rect::from_min_max(Pos2::new(rect.min.x, cell_pos(row, 0).y), Pos2::new(rect.max.x, cell_pos(rows, 0).y));
                layer.with_clip_rect(within.intersect(layer.clip_rect()))
            } else {
                layer
            };
            // (A float on its box's border column is drawn a column in.)
            let nudge = self.editor.windows.get(&grid).map_or(0, |w| self.border_nudge(w));
            let origin = cell_pos(row, col + nudge);
            // A float against Neovim's own command line goes under ours.
            let offset = self.editor.windows.get(&grid).and_then(|w| self.cmdline_float_offset(w));
            let origin = match offset {
                Some(d) => {
                    self.float_offsets.insert(grid, d);
                    origin + d
                }
                None => origin,
            };
            self.renderer.paint_grid(&layer, &self.editor, grid, origin, style);
            self.paint_edge_slivers(&painter, grid);
            if grid == DEFAULT_GRID {
                self.paint_statuslines(&painter, &cell_pos, rect);
                self.paint_separators(&painter, &cell_pos);
            } else {
                self.paint_winbar(&painter, grid, cell_pos(row, col));
            }
            // A Markdown preview covers its window's text (and cursor),
            // under the floats drawn after it.
            if let Some(r) = self.editor.windows.get(&grid).and_then(|w| self.preview_rect(w)) {
                self.show_preview(ui, &painter, grid, r);
                continue;
            }
            if grid == self.editor.cursor.grid && show_cursor {
                let editor_focus = self.focused && self.focus == Focus::Editor;
                self.renderer.paint_cursor(&layer, &self.editor, origin, editor_focus);
            }
        }

        match panel_rect {
            Some(r) => self.show_bottom_panel(ui, r),
            None => {
                self.bottom.rect = None;
                self.bottom.hits = Default::default();
            }
        }
        self.show_external_windows(&ctx);
        // The diff view, over the grid down to the panel (or the status bar).
        if self.diff.is_some() {
            let bottom = panel_rect.map_or_else(|| self.status_bar_top(rect, m).map_or(rect.max.y, |t| t.min(rect.max.y)), |r| r.min.y);
            self.show_diff(ui, &ctx, rect, bottom, m);
        }
        // (After the diff view, whose bar is over it.)
        self.paint_scrollbars(&ctx, &painter);

        // The resize cursor over a separator and a little beside it (the
        // press is Neovim's).
        let hover = ctx.input(|i| i.pointer.hover_pos());
        if let Some((icon, _)) = hover.filter(|p| !self.over_popup(&ctx, *p) && !self.over_bottom_panel(*p)).and_then(|p| self.separator_grab(p)) {
            ctx.set_cursor_icon(icon);
        }

        self.popup_rects.clear();
        self.changes_rect = None;
        if self.changes.popup {
            match self.agent_strip {
                Some(strip) => {
                    let font_size = self.status_font_size + 1.0;
                    let (r, action) = crate::changes_popup::show(&ctx, &self.editor, &self.changes, strip, font_size);
                    self.popup_rects.push(r);
                    self.changes_rect = Some(r);
                    match action {
                        Some(crate::changes_popup::Action::ReviewAll) => {
                            if let Some(req) = self.changes_request(None) {
                                self.open_diff(&ctx, req);
                            }
                            self.changes.popup = false;
                        }
                        Some(crate::changes_popup::Action::Diff(path)) if self.config.agent.diff == crate::config::DiffView::Native => {
                            // That file alone ("Review All" shows them all).
                            if let Some(req) = self.changes_request(Some(&path)) {
                                self.open_diff(&ctx, req);
                            }
                            self.changes.popup = false;
                        }
                        Some(crate::changes_popup::Action::Diff(path)) => {
                            if let (Some(s), Some(f)) = (self.changes.current(), self.changes.files.iter().find(|f| f.path == path)) {
                                let spec = scm_view::DiffSpec {
                                    top: s.top.clone(),
                                    base: s.base.clone(),
                                    path,
                                    deleted: f.status == 'D',
                                    ..Default::default()
                                };
                                self.bridge.send(spec.lua(self.config.agent.diff.name()));
                            }
                            self.changes.popup = false;
                        }
                        Some(crate::changes_popup::Action::AcceptAll) => self.changes.accept_all(),
                        Some(crate::changes_popup::Action::Accept(path)) => self.changes.accept(&path),
                        Some(crate::changes_popup::Action::Reject(path)) => {
                            if self.changes.confirm_reject.as_deref() == Some(path.as_str()) {
                                self.changes.reject(&path);
                            } else {
                                self.changes.confirm_reject = Some(path);
                            }
                        }
                        None => {}
                    }
                }
                // The agent panel was hidden.
                None => self.changes.popup = false,
            }
        }
        let status_top = self.status_bar_top(rect, m);
        let cmdline =
            cmdline::show(&ctx, &self.editor, &self.grid_fonts, m, rect, self.focused, self.config.cmdline.position, status_top);
        if let Some(c) = &cmdline {
            self.popup_rects.push(c.rect);
        }
        self.cmdline_layout = cmdline.clone();
        if let Some(pm) = &self.editor.popupmenu {
            let syntax = self.lsp.syntax();
            self.pum.bottom = status_top;
            let (r, action) =
                self.pum.show(&ctx, &self.editor, pm, &self.grid_fonts, m, rect, cmdline.as_ref(), syntax.as_deref());
            self.popup_rects.extend(r);
            if let Some(popupmenu::Action::Select(item)) = action {
                self.bridge.send(UiCommand::SelectPopupmenuItem { item, insert: true, finish: true });
            }
        }

        if matches!(
            self.editor.windows.get(&self.editor.cursor.grid).map(|w| &w.kind),
            Some(WindowKind::Normal | WindowKind::Float { .. })
        ) {
            self.popup_anchor = self.editor.cursor_screen_pos();
        }
        if picker::editor_popups_shown(self.picker.is_open(), &mut self.lsp) {
            let pc = PopupCtx {
                editor: &self.editor,
                fonts: &self.grid_fonts,
                metrics: m,
                screen: rect,
                cursor: cell_pos(self.popup_anchor.0, self.popup_anchor.1),
            };
            let rects = self.lsp.show(&ctx, &pc);
            self.popup_rects.extend(rects);
            // A completion item's documentation, around its menu (as drawn:
            // it may be under the command line).
            let menu = self.lsp.docs_win().and_then(|win| self.editor.windows.values().find(|w| w.win == win && !w.hidden));
            if let Some(w) = menu {
                let (row, col) = self.editor.grid_origin(w.grid);
                let offset = self.float_offsets.get(&w.grid).copied().unwrap_or(Vec2::ZERO);
                let size = Vec2::new(w.width as f32 * m.width, w.height as f32 * m.height);
                let menu = Rect::from_min_size(cell_pos(row, col) + offset, size);
                self.popup_rects.extend(self.lsp.show_docs(&ctx, &pc, menu));
            }
        }
        // (Over the other popups, under the messages.)
        self.show_picker(&ctx, rect, m);
        self.show_term_debug(&ctx);
        let rects = self.msgs.show(&ctx, &mut self.editor, &self.grid_fonts, m, rect, cmdline.as_ref().filter(|c| c.at_bottom).map(|c| c.rect));
        self.popup_rects.extend(rects);

        let grids = &self.editor.grids;
        self.renderer.retain_grids(&|id| grids.contains_key(&id));

        if let Some(s) = &mut self.screenshot {
            s.after_paint(&ctx);
        }
    }
}

fn start_error_text(error: &str) -> String {
    format!(
        "Couldn't start Neovim: {error}\n\nPATH: {}\n\nInstall Neovim, or pass its path: eshvi --nvim /path/to/nvim",
        std::env::var("PATH").unwrap_or_default()
    )
}

/// What the window says when Neovim ended without quitting.
fn crash_text(error: &str) -> String {
    format!(
        "Neovim ended unexpectedly: {error}\n\nUnsaved changes may be in swap files: open the file again and choose Recover.\n\nClose the window to quit."
    )
}

/// Size egui's text styles (used by popups) relative to the grid font.
fn set_text_sizes(ctx: &egui::Context, size: f32) {
    ctx.all_styles_mut(|style| {
        for (ts, font) in style.text_styles.iter_mut() {
            font.size = match ts {
                egui::TextStyle::Small => size * style::SMALL,
                egui::TextStyle::Heading => size * 1.3,
                _ => size,
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_neovim_that_does_not_answer_is_left_on_the_third_close() {
        let t0 = Instant::now();
        let later = |secs: u64| t0 + Duration::from_secs(secs);
        let mut ask = CloseAsk::default();
        assert_eq!(ask.request(t0), CloseStep::Ask);
        // At once again (a double click): asked again, no more.
        assert_eq!(ask.request(t0 + Duration::from_millis(300)), CloseStep::Ask);
        // Nothing heard for a while: said first, then closed.
        assert_eq!(ask.request(later(5)), CloseStep::Warn);
        assert_eq!(ask.request(later(6)), CloseStep::Quit);

        // Neovim answered in between (its prompt, a redraw): it is asked again.
        let mut ask = CloseAsk::default();
        assert_eq!(ask.request(t0), CloseStep::Ask);
        ask.heard();
        assert_eq!(ask.request(later(5)), CloseStep::Ask);
        assert_eq!(ask.request(later(10)), CloseStep::Warn);
        ask.heard();
        assert_eq!(ask.request(later(20)), CloseStep::Ask);
    }

    #[test]
    fn a_crash_is_said_with_what_to_do() {
        let text = crash_text("Neovim was killed by signal 11 (SIGSEGV)");
        assert!(text.starts_with("Neovim ended unexpectedly: Neovim was killed by signal 11 (SIGSEGV)"), "{text}");
        assert!(text.contains("Recover"));
    }
}
