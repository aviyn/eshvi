//! Runs `nvim --embed` on a background tokio runtime and exchanges
//! messages with the UI thread over channels. The UI never awaits RPC.

pub mod gui;
mod ide;
pub mod parse;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use grid::{Editor, RedrawEvent};
use nvim_rs::{compat::tokio::Compat, create::tokio::new_child_cmd, Handler, Neovim, UiAttachOptions, Value};
use tokio::process::{ChildStdin, Command};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub use nvim_rs::Value as RpcValue;

type Writer = Compat<ChildStdin>;
type Nvim = Neovim<Writer>;

/// Called from the bridge thread whenever new events are queued, so the
/// UI can request a repaint.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Requests from the UI to Neovim.
#[derive(Debug, Clone)]
pub enum UiCommand {
    Input(String),
    InputMouse { button: String, action: String, modifier: String, grid: u64, row: usize, col: usize },
    TryResize { width: usize, height: usize },
    Command(String),
    /// `nvim_select_popupmenu_item`
    SelectPopupmenuItem { item: i64, insert: bool, finish: bool },
    /// Open a file in a normal window (`:drop`), from the native explorer;
    /// with `split`, beside it (`:vsplit`).
    OpenFile { path: std::path::PathBuf, split: bool },
    /// Files dropped on the window, in order: in window `win` if it can take them.
    OpenDropped { paths: Vec<std::path::PathBuf>, win: Option<i64> },
    /// Change the global working directory (explorer root changed in the GUI).
    Cd(std::path::PathBuf),
    /// The explorer's root: where new panel terminals start.
    WorkspaceRoot(std::path::PathBuf),
    /// A file or folder was renamed in the GUI: move matching buffers.
    Renamed { from: std::path::PathBuf, to: std::path::PathBuf },
    /// A file or folder was moved to the Trash: wipe unmodified buffers.
    Deleted(std::path::PathBuf),
    /// Paste text as if typed (`nvim_paste`), e.g. from Cmd+V.
    Paste(String),
    SetFocus(bool),
    /// Lua code run with `args` as its `...` (`nvim_exec_lua`), in order with
    /// the other calls of the lane. For anything that carries a path or other
    /// text from outside: sent as arguments, not pasted into a command, it
    /// can hold anything.
    Lua { code: String, args: Vec<RpcValue> },
    /// Size one window's grid apart from its place in the layout
    /// (`nvim_ui_try_resize_grid`), for a window drawn in another font.
    ResizeGrid { grid: u64, width: usize, height: usize },
    /// A reply to a query in `ui_send` data, as a terminal would answer
    /// (`nvim_ui_term_event("termresponse", …)`).
    TermResponse(String),
    /// The picker's user chose these (`Eshvi.picker.accept`); `how` is
    /// "edit", "split", "vsplit", "tab" or "quickfix". Sent as arguments, not
    /// pasted into a command: a path can hold anything.
    /// `state` is the query and selected row, which `resume` reopens with.
    PickerAccept { id: i64, chosen: Vec<gui::PickerChoice>, how: String, state: gui::PickerState },
    /// The picker was closed without choosing (`Eshvi.picker.cancel`).
    PickerCancel { id: i64, state: gui::PickerState },
    /// The picker `id` is on screen with its field focused (`Eshvi.picker.opened`):
    /// what was typed before is Neovim's to hand over, and no more comes.
    PickerOpened(i64),
    /// The selection has stayed on row `idx` (from 1, Neovim's own list) a
    /// moment (`Eshvi.picker.selected`).
    PickerSelected { id: i64, idx: usize },
    /// Ask for the picker's preview of a file (`path`) or a buffer (`buf`)
    /// around line `lnum`; the answer is `gui_picker_preview` with this `req`.
    PickerPreview { req: i64, path: Option<String>, buf: Option<i64>, lnum: usize },
    /// A live picker's query changed (`Eshvi.picker.query`): the rows for
    /// `text` come back as `gui_picker_items` carrying `gen`.
    PickerQuery { id: i64, gen: i64, text: String },
    /// A tree view node was expanded with no children yet (`Eshvi.tree.expand`);
    /// they come back as `gui_tree_set` with `parent` and this `gen`.
    TreeExpand { view: String, id: String, gen: i64 },
    /// A node was clicked or entered (`Eshvi.tree.accept`); `how` is "open" or "peek".
    TreeAccept { view: String, id: String, how: String },
    /// Ask for a node's right-click menu (`Eshvi.tree.menu`); it comes back as `gui_tree_menu`.
    TreeMenu { view: String, id: String },
    /// A menu item was chosen (`Eshvi.tree.run`), by its place (from 1).
    TreeRun { view: String, id: String, index: usize },
    /// A header button of a side view was clicked (`Eshvi.tree.button`), by its place (from 1), with
    /// the selected node and the ids open as drawn.
    TreeButton { view: String, index: usize, selected: Option<String>, expanded: Vec<String> },
    /// A key the source listed (`keys`) was pressed in the view (`Eshvi.tree.key`);
    /// `id` is the selected node, if any.
    TreeKey { view: String, id: Option<String>, key: String },
    /// A toggle of the view's Filter menu changed (`Eshvi.tree.filter`), to be remembered.
    TreeFilter { view: String, group: String, shown: bool },
}

/// Messages from Neovim to the UI.
#[derive(Debug)]
pub enum BridgeEvent {
    /// A complete batch of redraw events, ending at a `flush`.
    Redraw(Vec<RedrawEvent>),
    /// A notification from the `gui_bridge` Lua plugin.
    Gui(gui::GuiEvent),
    /// Neovim exited (or failed to start). `error`: why, when it didn't end
    /// by quitting (it couldn't start, was killed by a signal, exited with a
    /// status other than 0, or the connection broke).
    Exited { error: Option<String> },
}

/// Redraw batches the UI may leave waiting before they are applied for it
/// (see `Bridge::park`).
const MAX_QUEUED_REDRAWS: usize = 256;

/// What Neovim sent and the UI hasn't taken yet.
#[derive(Default)]
struct Inbox {
    events: VecDeque<BridgeEvent>,
    /// How many of `events` are redraw batches.
    redraws: usize,
    /// The UI's editor, while no frame is using it.
    parked: Option<Editor>,
    /// Batches were applied to the parked editor.
    applied: bool,
}

/// The bridge thread's end of the inbox.
#[derive(Clone)]
struct Sink(Arc<Mutex<Inbox>>);

impl Sink {
    /// Queue an event for the UI. A window that isn't painting (minimized,
    /// the screen locked) takes nothing, and a terminal printing all night
    /// would queue batches without end: past `MAX_QUEUED_REDRAWS` they are
    /// applied here to the editor the UI left (`park`), so what waits stays
    /// the size of the grids. Other events keep their order among themselves.
    fn send(&self, event: BridgeEvent) {
        let mut inbox = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let inbox = &mut *inbox;
        inbox.redraws += usize::from(matches!(event, BridgeEvent::Redraw(_)));
        inbox.events.push_back(event);
        if inbox.redraws <= MAX_QUEUED_REDRAWS {
            return;
        }
        let Some(editor) = &mut inbox.parked else { return };
        let mut kept = VecDeque::new();
        for event in inbox.events.drain(..) {
            match event {
                BridgeEvent::Redraw(batch) => batch.into_iter().for_each(|e| editor.apply(e)),
                other => kept.push_back(other),
            }
        }
        // (Scrolls are kept for the glide of the next frame: none is due.)
        editor.scrolled.clear();
        inbox.events = kept;
        inbox.redraws = 0;
        inbox.applied = true;
    }
}

/// Sends `Exited` when the bridge thread ends without having said so (a
/// panic): the window must not stay up with nothing behind it.
struct ExitGuard {
    sink: Sink,
    waker: Waker,
    sent: bool,
}

impl ExitGuard {
    fn exited(&mut self, error: Option<String>) {
        self.sent = true;
        self.sink.send(BridgeEvent::Exited { error });
        (self.waker)();
    }
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        if !self.sent {
            self.exited(Some("the connection to Neovim stopped (an internal error)".into()));
        }
    }
}

pub struct SpawnOptions {
    pub nvim: String,
    pub args: Vec<String>,
    pub width: usize,
    pub height: usize,
    /// Filetypes whose windows are reported with `gui_sidebar`.
    pub sidebar_filetypes: Vec<String>,
    /// Keep a built-in style `vim.notify` even if plugins replace it, so
    /// notifications arrive as regular messages (installed with `--cmd`,
    /// before the user's config).
    pub restore_notify: bool,
    /// Use one global statusline ('laststatus' 3).
    pub global_statusline: bool,
    /// Give normal file windows a filename winbar.
    pub winbar: bool,
    /// Provide `:EshviTerminal`, `:EshviPanel` and `:EshviProblems` (the GUI's bottom panel;
    /// the commands only tell the GUI, and `gui_problems` carries the diagnostics).
    pub terminal_enabled: bool,
    /// Define eshvi's menus and report all menus (`gui_menu`) for a native
    /// menu bar.
    pub menu: bool,
    /// Provide `:EshviAgent` (the agent panel). `None`: no agent panel.
    pub agent: Option<AgentOptions>,
    /// Tab pages drawn by the GUI (`ext_tabline`) instead of Neovim's tab
    /// line (which then also replaces plugins drawing there).
    pub native_tabs: bool,
    /// `NVIM_APPNAME` to start Neovim with (eshvi's own config folder);
    /// `None`: leave the environment as it is.
    pub appname: Option<String>,
    /// `ESHVI_BIN` for Neovim: the folder of the running app, so the config can find what
    /// ships beside it. `None`: leave the environment as it is.
    pub bin_dir: Option<std::path::PathBuf>,
    /// Neovim's own completion, set up by `completion.lua` (menu while typing,
    /// in the command line too).
    pub native_completion: bool,
    /// What the native status bar needs (`status.lua`: `gui_status`), and
    /// one global statusline. Set with the own config, apart from
    /// `native_completion` so each says what it does.
    pub native_status: bool,
    /// The native picker (`picker.lua`: `Eshvi.picker`, `vim.ui.select`).
    /// Set with the own config, like the other two.
    pub native_picker: bool,
    /// `:checkhealth eshvi` and `:EshviHealth` (`health.lua`), and the record
    /// of which plugin opens which floating window (`float_log.lua`, by
    /// `--cmd`). Set with the own config, like the other three.
    pub health_check: bool,
    /// LSP hover, signature help and diagnostics as native popups: the repo's
    /// `lua/gui_bridge`, shipped in the app (`lsp_popups.lua`), so the config
    /// needs no checkout of it. Set with the own config, like the others.
    pub lsp_popups: bool,
    /// SystemVerilog support: the Hierarchy and Trace views, `:EshviSlang` and svtrace's
    /// language server, shipped in the app (`slang/`, loaded by `slang_load.lua`). Set with
    /// the own config, like the others; the views appear only once a slang-server or
    /// svtrace client attaches.
    pub slang: bool,
    /// Automatic sessions (`[session] auto`): a folder's session saved when Neovim quits
    /// and restored when it starts without files (`session.lua`). Named sessions and
    /// `:EshviSession` work either way.
    pub session_auto: bool,
    /// A mouse selection in the editor is sent to the GUI's clipboard (`gui_copy`).
    pub copy_on_select: bool,
}

/// The `NVIM_APPNAME` of eshvi's own Neovim config (`~/.config/eshvi/nvim`).
pub const OWN_APPNAME: &str = "eshvi/nvim";

/// The folder of the running executable (`ESHVI_BIN` for Neovim): in a bundle,
/// `Eshvi.app/Contents/MacOS`, where the helpers that ship with the app are.
pub fn running_bin_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

/// What `[neovim] config` and the environment's `NVIM_APPNAME` (if any) mean:
/// the appname to start Neovim with, and whether native completion is on.
/// `shared`: the terminal's config, no native completion. Otherwise eshvi's
/// own, unless the environment already names one: it is left as it is, and
/// native completion is on only when it is eshvi's.
pub fn appname_for(shared: bool, env: Option<&str>) -> (Option<String>, bool) {
    if shared {
        return (None, false);
    }
    match env {
        Some(name) if !name.is_empty() => (None, name == OWN_APPNAME),
        _ => (Some(OWN_APPNAME.to_owned()), true),
    }
}

/// The agent panel's agents.
#[derive(Clone, Debug, Default)]
pub struct AgentOptions {
    /// The default agent's command.
    pub command: Vec<String>,
    /// More agents "+" offers, by name.
    pub commands: Vec<(String, Vec<String>)>,
    /// Where Claude Code's IDE lock file goes; `None`: `~/.claude/ide`
    /// (`$CLAUDE_CONFIG_DIR/ide`). Tests use their own.
    pub ide_dir: Option<std::path::PathBuf>,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            nvim: "nvim".into(),
            args: vec![],
            width: 80,
            height: 24,
            sidebar_filetypes: vec![],
            restore_notify: false,
            global_statusline: false,
            winbar: false,
            terminal_enabled: false,
            menu: false,
            agent: None,
            native_tabs: false,
            appname: None,
            bin_dir: None,
            native_completion: false,
            native_status: false,
            native_picker: false,
            health_check: false,
            lsp_popups: false,
            slang: false,
            session_auto: false,
            copy_on_select: false,
        }
    }
}

/// Handle owned by the UI thread.
pub struct Bridge {
    commands: UnboundedSender<UiCommand>,
    events: Arc<Mutex<Inbox>>,
    /// The commands' other end, handed to the bridge thread once it starts.
    unstarted: Option<UnboundedReceiver<UiCommand>>,
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Bridge {
    /// Create the channels without starting Neovim. Commands sent before
    /// `start` are queued and delivered after attach.
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = unbounded_channel();
        Self { commands: cmd_tx, events: Default::default(), unstarted: Some(cmd_rx) }
    }

    pub fn is_started(&self) -> bool {
        self.unstarted.is_none()
    }

    /// Spawn Neovim on a dedicated thread. Never blocks on Neovim.
    pub fn start(&mut self, opts: SpawnOptions, waker: Waker) -> std::io::Result<()> {
        let Some(cmd_rx) = self.unstarted.take() else { return Ok(()) };
        let sink = Sink(self.events.clone());
        std::thread::Builder::new().name("nvim-bridge".into()).spawn(move || {
            let mut guard = ExitGuard { sink: sink.clone(), waker: waker.clone(), sent: false };
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            let error = rt.block_on(run(opts, sink, cmd_rx, waker)).err();
            guard.exited(error.map(|e| format!("{e:#}")));
        })?;
        Ok(())
    }

    pub fn spawn(opts: SpawnOptions, waker: Waker) -> std::io::Result<Self> {
        let mut bridge = Self::new();
        bridge.start(opts, waker)?;
        Ok(bridge)
    }

    pub fn send(&self, cmd: UiCommand) {
        let _ = self.commands.send(cmd);
    }

    /// Drain all pending events without blocking.
    pub fn try_recv(&self) -> Option<BridgeEvent> {
        let mut inbox = self.inbox();
        let event = inbox.events.pop_front()?;
        if matches!(event, BridgeEvent::Redraw(_)) {
            inbox.redraws = inbox.redraws.saturating_sub(1);
        }
        Some(event)
    }

    fn inbox(&self) -> std::sync::MutexGuard<'_, Inbox> {
        self.events.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Leave the editor with the bridge between frames (an empty one stays
    /// in its place). While it is here, redraw batches that pile up because
    /// no frame comes are applied to it instead of being kept (`Sink::send`).
    pub fn park(&self, editor: &mut Editor) {
        self.inbox().parked = Some(std::mem::take(editor));
    }

    /// Take the parked editor back, if one is there. True when batches were
    /// applied to it meanwhile: the UI does then what it does after a batch.
    pub fn unpark(&self, editor: &mut Editor) -> bool {
        let mut inbox = self.inbox();
        match inbox.parked.take() {
            Some(parked) => *editor = parked,
            None => return false,
        }
        std::mem::take(&mut inbox.applied)
    }

    /// Redraw batches waiting to be taken.
    pub fn queued_redraws(&self) -> usize {
        self.inbox().redraws
    }
}

#[derive(Clone)]
struct NvimHandler {
    /// Claude Code's IDE connection, once started (with the agent panel).
    ide: Arc<std::sync::OnceLock<ide::Ide>>,
    pending: Arc<Mutex<Vec<RedrawEvent>>>,
    events: Sink,
    waker: Waker,
}

#[async_trait]
impl Handler for NvimHandler {
    type Writer = Writer;

    async fn handle_request(&self, name: String, _args: Vec<Value>, _nvim: Nvim) -> Result<Value, Value> {
        match name.as_str() {
            "eshvi_clipboard_get" => Ok(clipboard_get()),
            _ => Err(Value::from(format!("unknown request {name}"))),
        }
    }

    async fn handle_notify(&self, name: String, args: Vec<Value>, nvim: Nvim) {
        match name.as_str() {
            "redraw" => {}
            "eshvi_clipboard_set" => return clipboard_set(&args),
            // From ide.lua: the selection, files sent as context, diff
            // decisions.
            "eshvi_ide_selection" | "eshvi_ide_mention" | "eshvi_ide_diff" => {
                if let (Some(ide), Some(params)) = (self.ide.get(), args.first()) {
                    let params = ide::to_json(params);
                    match name.as_str() {
                        "eshvi_ide_selection" => ide.selection_changed(params),
                        "eshvi_ide_mention" => {
                            let pid = params["pid"].as_i64();
                            let mut mention = params;
                            if let Some(m) = mention.as_object_mut() {
                                m.remove("pid");
                            }
                            ide.at_mentioned(mention, pid)
                        }
                        _ => ide.diff_done(params["id"].as_u64().unwrap_or_default(), params["content"].clone()),
                    }
                }
                return;
            }
            n if n.starts_with("gui_") => {
                if let Some(ev) = gui::parse(n, &args) {
                    self.events.send(BridgeEvent::Gui(ev));
                    (self.waker)();
                }
                return;
            }
            _ => {
                log::debug!("unhandled notification {name}");
                return;
            }
        }
        let mut parsed = Vec::new();
        parse::parse_redraw(&args, &mut parsed);
        // Queries for the terminal are answered here, not after the next
        // flush: Neovim waits for the answer (100 ms at startup, before a
        // flush), and so does snacks.image.
        for ev in &parsed {
            if let RedrawEvent::UiSend(data) = ev {
                for reply in query_replies(data) {
                    let nvim = nvim.clone();
                    tokio::spawn(async move {
                        let _ = nvim.ui_term_event("termresponse", Value::from(reply)).await;
                    });
                }
            }
        }
        let mut pending = self.pending.lock().unwrap();
        let mut flushed = false;
        for ev in parsed {
            let is_flush = ev == RedrawEvent::Flush;
            pending.push(ev);
            if is_flush {
                let batch = std::mem::take(&mut *pending);
                self.events.send(BridgeEvent::Redraw(batch));
                flushed = true;
            }
        }
        drop(pending);
        if flushed {
            (self.waker)();
        }
    }
}

async fn run(
    opts: SpawnOptions,
    events: Sink,
    mut commands: UnboundedReceiver<UiCommand>,
    waker: Waker,
) -> anyhow::Result<()> {
    use anyhow::Context;

    let ide_slot: Arc<std::sync::OnceLock<ide::Ide>> = Default::default();
    let handler = NvimHandler { ide: ide_slot.clone(), pending: Default::default(), events, waker };
    let mut cmd = Command::new(&opts.nvim);
    cmd.arg("--embed");
    // snacks.image draws with the kitty graphics protocol's unicode
    // placeholders, which the GUI renders (see `ui_send`); this is its own
    // switch for terminals it doesn't detect.
    cmd.env("SNACKS_KITTY", "1");
    if let Some(name) = &opts.appname {
        cmd.env("NVIM_APPNAME", name);
    }
    if let Some(dir) = &opts.bin_dir {
        cmd.env("ESHVI_BIN", dir);
    }
    if opts.restore_notify {
        cmd.arg("--cmd").arg(format!("lua {}", one_line_lua(include_str!("early.lua"))));
    }
    if opts.health_check {
        cmd.arg("--cmd").arg(format!("lua {}", one_line_lua(include_str!("float_log.lua"))));
    }
    cmd.args(&opts.args);
    let (nvim, io, mut child) =
        new_child_cmd(&mut cmd, handler).await.with_context(|| format!("failed to start `{}`", opts.nvim))?;

    // Our channel id lets Lua code (clipboard fallback, gui_bridge plugin)
    // talk back to the GUI. Asked before attaching, so a Neovim that is too
    // old is refused before it draws anything.
    let info = nvim.get_api_info().await.context("nvim_get_api_info")?;
    if let Some(why) = version_error(&info) {
        anyhow::bail!(why);
    }
    let chan = info.first().and_then(Value::as_i64).unwrap_or(0);

    let mut ui_opts = UiAttachOptions::new();
    ui_opts.set_rgb(true).set_linegrid_external(true).set_multigrid_external(true).set_popupmenu_external(true)
        .set_cmdline_external(true)
        .set_tabline_external(opts.native_tabs)
        .set_messages_externa(true)
        .set_hlstate_external(true)
        // Receive `nvim_ui_send` data (image protocol, progress) as
        // `ui_send` events.
        .set_stdout_tty(true);
    nvim.ui_attach(opts.width as i64, opts.height as i64, &ui_opts).await.context("nvim_ui_attach")?;

    nvim.set_var("gui_bridge_chan", Value::from(chan)).await.context("set gui_bridge_chan")?;
    let setup_opts = Value::Map(vec![
        (
            "sidebar_filetypes".into(),
            Value::Array(opts.sidebar_filetypes.iter().map(|s| Value::from(s.as_str())).collect()),
        ),
        ("global_statusline".into(), opts.global_statusline.into()),
        ("winbar".into(), opts.winbar.into()),
        ("terminal_enabled".into(), opts.terminal_enabled.into()),
        ("agent_enabled".into(), opts.agent.is_some().into()),
        ("session_auto".into(), opts.session_auto.into()),
        ("copy_on_select".into(), opts.copy_on_select.into()),
    ]);
    if let Err(e) = nvim.exec_lua(include_str!("setup.lua"), vec![Value::from(chan), setup_opts.clone()]).await {
        log::warn!("setup.lua failed: {e}");
    }
    if let Err(e) = nvim.exec_lua(include_str!("panel.lua"), vec![Value::from(chan), setup_opts.clone()]).await {
        log::warn!("panel.lua failed: {e}");
    }
    if let Err(e) = nvim.exec_lua(include_str!("blink_docs.lua"), vec![Value::from(chan)]).await {
        log::warn!("blink_docs.lua failed: {e}");
    }
    if let Err(e) = nvim.exec_lua(include_str!("preview.lua"), vec![Value::from(chan)]).await {
        log::warn!("preview.lua failed: {e}");
    }
    if let Err(e) = nvim.exec_lua(include_str!("diff.lua"), vec![Value::from(chan)]).await {
        log::warn!("diff.lua failed: {e}");
    }
    // The native tree view and the Outline, with any config (they need only
    // `vim.lsp` and Neovim's own Markdown parser).
    for (name, src) in [("tree.lua", include_str!("tree.lua")), ("symbols.lua", include_str!("symbols.lua")), ("outline.lua", include_str!("outline.lua"))] {
        if let Err(e) = nvim.exec_lua(src, vec![Value::from(chan)]).await {
            log::warn!("{name} failed: {e}");
        }
    }
    if opts.native_completion {
        if let Err(e) = nvim.exec_lua(include_str!("completion.lua"), vec![Value::from(chan)]).await {
            log::warn!("completion.lua failed: {e}");
        }
    }
    if opts.native_status {
        if let Err(e) = nvim.exec_lua(include_str!("status.lua"), vec![Value::from(chan)]).await {
            log::warn!("status.lua failed: {e}");
        }
    }
    if opts.native_picker {
        if let Err(e) = nvim.exec_lua(include_str!("picker.lua"), vec![Value::from(chan)]).await {
            log::warn!("picker.lua failed: {e}");
        }
    }
    if opts.lsp_popups {
        let src = Value::from(include_str!("../../../lua/gui_bridge/init.lua"));
        if let Err(e) = nvim.exec_lua(include_str!("lsp_popups.lua"), vec![src]).await {
            log::warn!("lsp_popups.lua failed: {e}");
        }
    }
    if opts.slang {
        let sources = Value::Map(vec![
            ("init".into(), Value::from(include_str!("slang/init.lua"))),
            ("hierarchy".into(), Value::from(include_str!("slang/hierarchy.lua"))),
            ("trace".into(), Value::from(include_str!("slang/trace.lua"))),
        ]);
        if let Err(e) = nvim.exec_lua(include_str!("slang_load.lua"), vec![sources]).await {
            log::warn!("slang_load.lua failed: {e}");
        }
    }
    if opts.health_check {
        install_health(&nvim).await;
    }
    // Sessions, before the menu (File › Open Session lists them).
    if let Err(e) = nvim.exec_lua(include_str!("session.lua"), vec![Value::from(chan), setup_opts.clone()]).await {
        log::warn!("session.lua failed: {e}");
    }
    if opts.menu {
        if let Err(e) = nvim.exec_lua(include_str!("menu.lua"), vec![Value::from(chan), setup_opts]).await {
            log::warn!("menu.lua failed: {e}");
        }
    }
    if let Some(agent) = &opts.agent {
        // Claude Code's IDE connection; agents started in the panel find it
        // through CLAUDE_CODE_SSE_PORT.
        let mut env = Vec::new();
        let cwd = nvim.call_function("getcwd", vec![]).await.ok().and_then(|v| v.as_str().map(str::to_owned));
        match ide::Ide::start(nvim.clone(), cwd.into_iter().collect(), agent.ide_dir.clone()).await {
            Ok(ide) => {
                env.push(("CLAUDE_CODE_SSE_PORT".into(), Value::from(ide.port().to_string())));
                env.push(("ENABLE_IDE_INTEGRATION".into(), Value::from("true")));
                let _ = ide_slot.set(ide);
                if let Err(e) = nvim.exec_lua(include_str!("ide.lua"), vec![Value::from(chan)]).await {
                    log::warn!("ide.lua failed: {e}");
                }
            }
            Err(e) => log::warn!("ide server: {e}"),
        }
        let argv = |cmd: &[String]| Value::Array(cmd.iter().map(|s| Value::from(s.as_str())).collect());
        let agent_opts = Value::Map(vec![
            ("agent_env".into(), Value::Map(env)),
            ("agent_command".into(), argv(&agent.command)),
            (
                "agent_commands".into(),
                Value::Map(agent.commands.iter().map(|(name, cmd)| (Value::from(name.as_str()), argv(cmd))).collect()),
            ),
        ]);
        if let Err(e) = nvim.exec_lua(include_str!("agent.lua"), vec![Value::from(chan), agent_opts]).await {
            log::warn!("agent.lua failed: {e}");
        }
    }

    // Last, so the panels exist when a session is restored.
    if let Err(e) = nvim.exec_lua("Eshvi.session.start()", vec![]).await {
        log::warn!("session start failed: {e}");
    }

    let (lane, lane_task) = start_lane(nvim.clone());
    let mut io = io;
    // How it ended: the UI went away (`None`), or Neovim did, with its exit
    // status if it is known and what broke the connection, if anything did.
    let ended = loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break None };
                if let (UiCommand::WorkspaceRoot(root), Some(ide)) = (&cmd, ide_slot.get()) {
                    ide.set_workspace(vec![root.to_string_lossy().into_owned()]);
                }
                handle_command(&nvim, &lane, cmd).await;
            }
            res = &mut io => {
                let broke = match res {
                    Ok(Err(e)) if !e.is_channel_closed() => {
                        log::warn!("rpc loop ended: {e}");
                        Some(e.to_string())
                    }
                    _ => None,
                };
                // The process follows its closed channel within a moment, if it is going at all.
                let status = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await.ok().and_then(Result::ok);
                break Some((status, broke));
            }
            status = child.wait() => break Some((status.ok(), None)),
        }
    };
    // What was sent last (a `qa!`) still goes out.
    drop(lane);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), lane_task).await;
    if let Some(ide) = ide_slot.get() {
        ide.shutdown();
    }
    match ended.and_then(|(status, broke)| exit_error(status, broke)) {
        Some(why) => Err(anyhow::anyhow!(why)),
        None => Ok(()),
    }
}

/// The Neovim this needs: floats are placed by `screen_row`/`screen_col` and
/// ordered by `compindex`, which `win_float_pos` carries since 0.12. An older
/// one would draw every float at the top-left corner with no word of why.
const MIN_NVIM: (i64, i64) = (0, 12);

/// Why this Neovim can't be used, from `nvim_get_api_info`'s answer
/// (`[channel, {version = {major, minor, patch}, …}]`); `None`: it can
/// (also when the version can't be read: it is then left to show).
fn version_error(info: &[Value]) -> Option<String> {
    let field = |map: &Value, key: &str| map.as_map()?.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v.clone());
    let version = field(info.get(1)?, "version")?;
    let part = |key: &str| field(&version, key).and_then(|v| v.as_i64());
    let (major, minor) = (part("major")?, part("minor")?);
    ((major, minor) < MIN_NVIM).then(|| {
        format!(
            "Neovim {major}.{minor}.{} is too old: Eshvi needs {}.{} or later",
            part("patch").unwrap_or(0),
            MIN_NVIM.0,
            MIN_NVIM.1
        )
    })
}

/// What to say about how Neovim ended; `None` when it quit (status 0), or
/// when nothing is known to be wrong. `broke`: what ended the connection
/// other than Neovim closing it.
fn exit_error(status: Option<std::process::ExitStatus>, broke: Option<String>) -> Option<String> {
    match status {
        Some(s) if s.success() => None,
        Some(s) => {
            #[cfg(unix)]
            if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&s) {
                return Some(format!("Neovim was killed by signal {signal}{}", signal_name(signal)));
            }
            Some(match s.code() {
                Some(code) => format!("Neovim exited with status {code}"),
                None => format!("Neovim ended: {s}"),
            })
        }
        // Still running, or its status can't be had: only a broken connection is worth a word.
        None => broke.map(|e| format!("the connection to Neovim broke: {e}")),
    }
}

/// " (SIGSEGV)" for the signals a crash or a kill comes with.
#[cfg(unix)]
fn signal_name(signal: i32) -> &'static str {
    match signal {
        1 => " (SIGHUP)",
        2 => " (SIGINT)",
        4 => " (SIGILL)",
        6 => " (SIGABRT)",
        8 => " (SIGFPE)",
        9 => " (SIGKILL)",
        10 if cfg!(target_os = "macos") => " (SIGBUS)",
        7 if cfg!(target_os = "linux") => " (SIGBUS)",
        11 => " (SIGSEGV)",
        15 => " (SIGTERM)",
        _ => "",
    }
}

/// A path as Neovim gets it: its bytes as they are (a msgpack string need
/// not be UTF-8, and Lua's strings are bytes), so a name that isn't UTF-8
/// names the same file there. Elsewhere, as text.
fn path_value(path: &std::path::Path) -> Value {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Value::Binary(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        Value::from(path.to_string_lossy().as_ref())
    }
}

async fn handle_command(nvim: &Nvim, lane: &UnboundedSender<LaneCall>, cmd: UiCommand) {
    // Input goes through in order on this task. `nvim_input` and friends
    // are "fast" calls that return even while Neovim waits for input.
    let res = match cmd {
        UiCommand::Input(keys) => nvim.input(&keys).await.map(|_| ()),
        UiCommand::InputMouse { button, action, modifier, grid, row, col } => {
            nvim.input_mouse(&button, &action, &modifier, grid as i64, row as i64, col as i64).await
        }
        // Not "fast": while Neovim runs `:!cmd` or `system()` these don't return,
        // so they go through the lane and never hold up the keys behind them.
        UiCommand::TryResize { width, height } => {
            lane.send(LaneCall::api("nvim_ui_try_resize", vec![Value::from(width.max(1) as i64), Value::from(height.max(1) as i64)])).ok();
            Ok(())
        }
        UiCommand::SetFocus(gained) => {
            lane.send(LaneCall::api("nvim_ui_set_focus", vec![Value::from(gained)])).ok();
            Ok(())
        }
        UiCommand::ResizeGrid { grid, width, height } => {
            let args = vec![Value::from(grid as i64), Value::from(width.max(1) as i64), Value::from(height.max(1) as i64)];
            lane.send(LaneCall::api("nvim_ui_try_resize_grid", args)).ok();
            Ok(())
        }
        UiCommand::TermResponse(seq) => {
            lane.send(LaneCall::api("nvim_ui_term_event", vec![Value::from("termresponse"), Value::from(seq)])).ok();
            Ok(())
        }
        UiCommand::SelectPopupmenuItem { item, insert, finish } => {
            let args = vec![Value::from(item), Value::from(insert), Value::from(finish), Value::Map(vec![])];
            lane.send(LaneCall::api("nvim_select_popupmenu_item", args)).ok();
            Ok(())
        }
        UiCommand::OpenFile { path, split } => {
            // Opening can stop at a prompt (e.g. a stale swap file), and the
            // call only returns once it's answered, so run it off this task
            // to keep input flowing.
            let nvim = nvim.clone();
            tokio::spawn(async move {
                let args = vec![path_value(&path), Value::from(split)];
                if let Err(e) = nvim.exec_lua(include_str!("open_file.lua"), args).await {
                    log::warn!("opening {} failed: {e}", path.display());
                }
            });
            Ok(())
        }
        UiCommand::OpenDropped { paths, win } => {
            // One after another, and off this task as `OpenFile` is (a prompt).
            let nvim = nvim.clone();
            tokio::spawn(async move {
                for path in paths {
                    // (No window: left out, as a nil would reach Lua as `vim.NIL`.)
                    let mut args = vec![path_value(&path), Value::from(false)];
                    args.extend(win.map(Value::from));
                    if let Err(e) = nvim.exec_lua(include_str!("open_file.lua"), args).await {
                        log::warn!("opening {} failed: {e}", path.display());
                    }
                }
            });
            Ok(())
        }
        UiCommand::Lua { code, args } => {
            let what = format!("lua `{code}`");
            lane.send(LaneCall::lua(&code, args, &what)).ok();
            Ok(())
        }
        UiCommand::PickerAccept { id, chosen, how, state } => {
            // (Left out when absent: a nil would reach Lua as `vim.NIL`.)
            let chosen = chosen
                .iter()
                .map(|c| {
                    let mut pairs = vec![];
                    for (key, n) in [("idx", c.idx), ("lnum", c.lnum), ("col", c.col)] {
                        pairs.extend(n.map(|n| (Value::from(key), Value::from(n as i64))));
                    }
                    pairs.extend(c.path.as_deref().map(|p| (Value::from("path"), Value::from(p))));
                    pairs.extend(c.text.as_deref().map(|t| (Value::from("text"), Value::from(t))));
                    Value::Map(pairs)
                })
                .collect();
            let args = vec![Value::from(id), Value::Array(chosen), Value::from(how), picker_state(&state)];
            lane.send(LaneCall::lua("return Eshvi.picker.accept(...)", args, "picker accept")).ok();
            Ok(())
        }
        UiCommand::PickerCancel { id, state } => {
            lane.send(LaneCall::lua("return Eshvi.picker.cancel(...)", vec![Value::from(id), picker_state(&state)], "picker cancel")).ok();
            Ok(())
        }
        UiCommand::PickerOpened(id) => {
            lane.send(LaneCall::lua("return Eshvi.picker.opened(...)", vec![Value::from(id)], "picker opened")).ok();
            Ok(())
        }
        UiCommand::PickerSelected { id, idx } => {
            lane.send(LaneCall::lua("return Eshvi.picker.selected(...)", vec![Value::from(id), Value::from(idx as i64)], "picker selected")).ok();
            Ok(())
        }
        UiCommand::PickerPreview { req, path, buf, lnum } => {
            // Off this task: reading and parsing a file takes a while, and
            // only the preview waits for it.
            let nvim = nvim.clone();
            tokio::spawn(async move {
                let target = match (buf, path) {
                    (Some(b), _) => Value::from(b),
                    (None, Some(p)) => Value::from(p),
                    (None, None) => return,
                };
                let args = vec![Value::from(req), target, Value::from(lnum as i64)];
                if let Err(e) = nvim.exec_lua("return Eshvi.picker.preview(...)", args).await {
                    log::warn!("picker preview failed: {e}");
                }
            });
            Ok(())
        }
        UiCommand::PickerQuery { id, gen, text } => {
            let args = vec![Value::from(id), Value::from(gen), Value::from(text)];
            lane.send(LaneCall::lua("return Eshvi.picker.query(...)", args, "picker query")).ok();
            Ok(())
        }
        UiCommand::TreeExpand { view, id, gen } => {
            let args = vec![Value::from(view), Value::from(id), Value::from(gen)];
            lane.send(LaneCall::lua("return Eshvi.tree.expand(...)", args, "tree expand")).ok();
            Ok(())
        }
        UiCommand::TreeAccept { view, id, how } => {
            let args = vec![Value::from(view), Value::from(id), Value::from(how)];
            lane.send(LaneCall::lua("return Eshvi.tree.accept(...)", args, "tree accept")).ok();
            Ok(())
        }
        UiCommand::TreeMenu { view, id } => {
            lane.send(LaneCall::lua("return Eshvi.tree.menu(...)", vec![Value::from(view), Value::from(id)], "tree menu")).ok();
            Ok(())
        }
        UiCommand::TreeRun { view, id, index } => {
            let args = vec![Value::from(view), Value::from(id), Value::from(index as i64)];
            lane.send(LaneCall::lua("return Eshvi.tree.run(...)", args, "tree run")).ok();
            Ok(())
        }
        UiCommand::TreeButton { view, index, selected, expanded } => {
            let args = vec![
                Value::from(view),
                Value::from(index as i64),
                selected.map_or(Value::Nil, Value::from),
                Value::Array(expanded.into_iter().map(Value::from).collect()),
            ];
            lane.send(LaneCall::lua("return Eshvi.tree.button(...)", args, "tree button")).ok();
            Ok(())
        }
        UiCommand::TreeKey { view, id, key } => {
            let args = vec![Value::from(view), id.map_or(Value::Nil, Value::from), Value::from(key)];
            lane.send(LaneCall::lua("return Eshvi.tree.key(...)", args, "tree key")).ok();
            Ok(())
        }
        UiCommand::TreeFilter { view, group, shown } => {
            let args = vec![Value::from(view), Value::from(group), Value::from(shown)];
            lane.send(LaneCall::lua("return Eshvi.tree.filter(...)", args, "tree filter")).ok();
            Ok(())
        }
        UiCommand::Cd(path) => file_op(nvim, "cd", &[&path]),
        UiCommand::WorkspaceRoot(path) => file_op(nvim, "root", &[&path]),
        UiCommand::Renamed { from, to } => file_op(nvim, "rename", &[&from, &to]),
        UiCommand::Deleted(path) => file_op(nvim, "delete", &[&path]),
        UiCommand::Paste(text) => {
            // Awaited here so the keys after it stay behind it, but only for a
            // moment: it doesn't return while Neovim runs a command.
            let nvim = nvim.clone();
            let task = tokio::spawn(async move {
                if let Err(e) = nvim.paste(&text, true, -1).await {
                    log::warn!("paste failed: {e}");
                }
            });
            let _ = tokio::time::timeout(LANE_PATIENCE, task).await;
            Ok(())
        }
        UiCommand::Command(c) => {
            // Ex commands can block (e.g. on a prompt): the lane lets go of one that does.
            let what = format!("command `{c}`");
            lane.send(LaneCall { method: "nvim_command", args: vec![Value::from(c)], what }).ok();
            Ok(())
        }
    };
    if let Err(e) = res {
        log::warn!("rpc call failed: {e}");
    }
}

/// A call for the lane.
struct LaneCall {
    method: &'static str,
    args: Vec<Value>,
    what: String,
}

impl LaneCall {
    fn api(method: &'static str, args: Vec<Value>) -> Self {
        Self { method, args, what: method.to_owned() }
    }

    fn lua(code: &str, args: Vec<Value>, what: &str) -> Self {
        Self { method: "nvim_exec_lua", args: vec![Value::from(code), Value::Array(args)], what: what.to_owned() }
    }
}

/// How long the lane waits for an answer before it sends the next call.
const LANE_PATIENCE: std::time::Duration = std::time::Duration::from_millis(50);

/// Commands and the picker's answers go to Neovim in the order they were sent.
/// Each used to run on a task of its own, and two in a row could swap (about one
/// pair in ten when sent back to back); a lane sends one, waits for its answer, then
/// the next. A call that is still waiting after a moment (a prompt, a slow
/// command) is let go on its own task, so it doesn't hold up the others as it
/// never did.
fn start_lane(nvim: Nvim) -> (UnboundedSender<LaneCall>, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = unbounded_channel::<LaneCall>();
    let task = tokio::spawn(async move {
        while let Some(call) = rx.recv().await {
            let nvim = nvim.clone();
            let task = tokio::spawn(async move {
                match nvim.call(call.method, call.args).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => log::warn!("{} failed: {e:?}", call.what),
                    Err(e) => log::warn!("{} failed: {e}", call.what),
                }
            });
            let _ = tokio::time::timeout(LANE_PATIENCE, task).await;
        }
    });
    (tx, task)
}

/// A picker's query and selected row as Lua gets them (left out when absent:
/// a nil would reach Lua as `vim.NIL`).
fn picker_state(state: &gui::PickerState) -> Value {
    let mut pairs = vec![(Value::from("query"), Value::from(state.query.as_str()))];
    if let Some(row) = &state.row {
        let mut r = vec![(Value::from("text"), Value::from(row.text.as_str()))];
        r.extend(row.path.as_deref().map(|p| (Value::from("path"), Value::from(p))));
        r.extend(row.lnum.map(|n| (Value::from("lnum"), Value::from(n as i64))));
        pairs.push((Value::from("row"), Value::Map(r)));
    }
    Value::Map(pairs)
}

/// `[lines, regtype]` for the g:clipboard paste function.
fn clipboard_get() -> Value {
    let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
    let text = text.replace("\r\n", "\n");
    let linewise = text.ends_with('\n');
    let body = if linewise { &text[..text.len() - 1] } else { &text[..] };
    let lines: Vec<Value> = body.split('\n').map(Value::from).collect();
    Value::Array(vec![Value::Array(lines), Value::from(if linewise { "V" } else { "v" })])
}

/// Args of `eshvi_clipboard_set`: `[lines, regtype]`.
fn clipboard_set(args: &[Value]) {
    let lines: Vec<&str> = args
        .first()
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut text = lines.join("\n");
    if args.get(1).and_then(Value::as_str) == Some("V") {
        text.push('\n');
    }
    if let Err(e) = arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
        log::warn!("clipboard set failed: {e}");
    }
}

/// Run `file_ops.lua` off the input task (buffer reloads can prompt).
fn file_op(nvim: &Nvim, op: &str, paths: &[&std::path::Path]) -> Result<(), Box<nvim_rs::error::CallError>> {
    let nvim = nvim.clone();
    let mut args = vec![Value::from(op)];
    args.extend(paths.iter().copied().map(path_value));
    let op = op.to_owned();
    tokio::spawn(async move {
        if let Err(e) = nvim.exec_lua(include_str!("file_ops.lua"), args).await {
            log::warn!("file op {op} failed: {e}");
        }
    });
    Ok(())
}

/// Join a Lua chunk into one line for `--cmd "lua ..."`. Full-line comments
/// are dropped; the chunk must not use inline comments.
/// Answers to the terminal queries in `ui_send` data, as a terminal would
/// give them: a device status report ("OK"; Neovim sends one after its
/// background color query, so it stops waiting when no color comes) and
/// XTVERSION (the terminal's name, asked by snacks.image).
fn query_replies(data: &[u8]) -> Vec<String> {
    let has = |seq: &[u8]| data.windows(seq.len()).any(|w| w == seq);
    let mut replies = Vec::new();
    if has(b"\x1b[5n") {
        replies.push("\x1b[0n".to_owned());
    }
    if has(b"\x1b[>q") || has(b"\x1b[>0q") {
        replies.push(format!("\x1bP>|eshvi {}\x1b\\", env!("CARGO_PKG_VERSION")));
    }
    replies
}

/// Write `health.lua` where `:checkhealth` can find it, in a runtime folder of
/// ours that `health_setup.lua` adds to the runtimepath. Written at each start,
/// so an upgrade replaces it. A failure is a warning in the log, not the
/// user's problem.
async fn install_health(nvim: &Nvim) {
    let cache = match nvim.call_function("stdpath", vec![Value::from("cache")]).await {
        Ok(v) => v.as_str().map(str::to_owned),
        Err(e) => {
            log::warn!("health: stdpath: {e}");
            None
        }
    };
    let Some(cache) = cache else { return };
    let runtime = std::path::Path::new(&cache).join("eshvi").join("runtime");
    let dir = runtime.join("lua").join("eshvi");
    // Written aside and renamed in: another Eshvi starting at the same moment must not read half a file.
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let tmp = dir.join(format!("health.lua.{}-{}.tmp", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&tmp, include_str!("health.lua")))
        .and_then(|()| std::fs::rename(&tmp, dir.join("health.lua")));
    if let Err(e) = written {
        log::warn!("health.lua not written to {}: {e}", dir.display());
        return;
    }
    let runtime = Value::from(runtime.to_string_lossy().into_owned());
    if let Err(e) = nvim.exec_lua(include_str!("health_setup.lua"), vec![runtime]).await {
        log::warn!("health_setup.lua failed: {e}");
    }
}

fn one_line_lua(code: &str) -> String {
    code.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("--")).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{BridgeEvent, Value};

    /// A batch that titles the window `n`.
    fn titled(n: usize) -> BridgeEvent {
        BridgeEvent::Redraw(vec![grid::RedrawEvent::SetTitle(n.to_string()), grid::RedrawEvent::Flush])
    }

    #[test]
    fn batches_nobody_takes_are_applied_to_the_parked_editor() {
        let bridge = super::Bridge::new();
        let sink = super::Sink(bridge.events.clone());
        let gui = || BridgeEvent::Gui(crate::gui::GuiEvent::PickerClose(7));
        // Nothing parked (a frame is using the editor): they wait, however many.
        for n in 0..super::MAX_QUEUED_REDRAWS + 10 {
            sink.send(titled(n));
        }
        assert_eq!(bridge.queued_redraws(), super::MAX_QUEUED_REDRAWS + 10);
        // Parked: the next event applies them all, in order; other events stay.
        let mut editor = grid::Editor::new();
        editor.title = "mine".into();
        bridge.park(&mut editor);
        assert_eq!(editor.title, "");
        sink.send(gui());
        assert_eq!(bridge.queued_redraws(), 0);
        for n in 0..5 {
            sink.send(titled(2000 + n));
        }
        assert_eq!(bridge.queued_redraws(), 5);
        // Taken back: it says batches were applied, once; the rest come as ever.
        assert!(bridge.unpark(&mut editor));
        let last = (super::MAX_QUEUED_REDRAWS + 9).to_string();
        assert_eq!(editor.title, last);
        assert!(!bridge.unpark(&mut editor), "nothing parked now");
        assert!(matches!(bridge.try_recv(), Some(BridgeEvent::Gui(_))));
        assert!(matches!(bridge.try_recv(), Some(BridgeEvent::Redraw(_))));
        assert_eq!(bridge.queued_redraws(), 4);
        // Parked with little waiting: left alone.
        bridge.park(&mut editor);
        sink.send(titled(3000));
        assert!(!bridge.unpark(&mut editor));
        assert_eq!(editor.title, last);
    }

    #[test]
    fn a_thread_that_ends_without_a_word_still_says_exited() {
        let bridge = super::Bridge::new();
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = woken.clone();
        let waker: super::Waker = std::sync::Arc::new(move || w.store(true, std::sync::atomic::Ordering::Relaxed));
        let guard = super::ExitGuard { sink: super::Sink(bridge.events.clone()), waker, sent: false };
        let _ = std::thread::spawn(move || {
            let _guard = guard;
            panic!("the bridge thread panics (expected in this test)");
        })
        .join();
        assert!(matches!(bridge.try_recv(), Some(BridgeEvent::Exited { error: Some(_) })));
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed));
        // Said already: not twice.
        let mut guard = super::ExitGuard { sink: super::Sink(bridge.events.clone()), waker: std::sync::Arc::new(|| {}), sent: false };
        guard.exited(None);
        drop(guard);
        assert!(matches!(bridge.try_recv(), Some(BridgeEvent::Exited { error: None })));
        assert!(bridge.try_recv().is_none());
    }

    #[test]
    fn an_old_neovim_is_refused_by_its_version() {
        let info = |major: i64, minor: i64| {
            let version = Value::Map(vec![
                (Value::from("major"), Value::from(major)),
                (Value::from("minor"), Value::from(minor)),
                (Value::from("patch"), Value::from(4)),
            ]);
            vec![Value::from(1), Value::Map(vec![(Value::from("version"), version)])]
        };
        assert_eq!(super::version_error(&info(0, 12)), None);
        assert_eq!(super::version_error(&info(1, 0)), None);
        assert_eq!(
            super::version_error(&info(0, 11)).as_deref(),
            Some("Neovim 0.11.4 is too old: Eshvi needs 0.12 or later")
        );
        // Unreadable: not refused.
        assert_eq!(super::version_error(&[Value::from(1)]), None);
        assert_eq!(super::version_error(&[Value::from(1), Value::Map(vec![])]), None);
    }

    #[cfg(unix)]
    #[test]
    fn how_neovim_ended_is_said_unless_it_quit() {
        use std::os::unix::process::ExitStatusExt;
        let status = |raw: i32| Some(std::process::ExitStatus::from_raw(raw));
        assert_eq!(super::exit_error(status(0), None), None);
        // Quit, though the connection ended with an error: nothing to say.
        assert_eq!(super::exit_error(status(0), Some("eof".into())), None);
        assert_eq!(super::exit_error(status(11), None).as_deref(), Some("Neovim was killed by signal 11 (SIGSEGV)"));
        assert_eq!(super::exit_error(status(3 << 8), None).as_deref(), Some("Neovim exited with status 3"));
        assert_eq!(super::exit_error(None, None), None);
        assert_eq!(super::exit_error(None, Some("bad message".into())).as_deref(), Some("the connection to Neovim broke: bad message"));
    }

    #[cfg(unix)]
    #[test]
    fn a_path_goes_to_neovim_byte_for_byte() {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9.txt");
        assert_eq!(super::path_value(std::path::Path::new(name)), Value::Binary(b"/tmp/caf\xe9.txt".to_vec()));
    }

    #[test]
    fn own_config_unless_shared_or_the_environment_names_one() {
        use super::appname_for;
        let own = || Some("eshvi/nvim".to_owned());
        assert_eq!(appname_for(false, None), (own(), true));
        assert_eq!(appname_for(false, Some("")), (own(), true));
        assert_eq!(appname_for(true, None), (None, false));
        // The environment wins: left as it is.
        assert_eq!(appname_for(false, Some("other")), (None, false));
        assert_eq!(appname_for(false, Some("eshvi/nvim")), (None, true));
        assert_eq!(appname_for(true, Some("eshvi/nvim")), (None, false));
    }

    #[test]
    fn bin_dir_is_the_folder_of_the_running_executable() {
        let dir = super::running_bin_dir().unwrap();
        assert!(dir.is_absolute());
        assert!(dir.join(std::env::current_exe().unwrap().file_name().unwrap()).exists());
    }

    #[test]
    fn answers_status_and_version_queries() {
        assert_eq!(super::query_replies(b"\x1b]11;?\x07\x1b[5n"), ["\x1b[0n"]);
        assert!(super::query_replies(b"\x1b[>q")[0].starts_with("\x1bP>|eshvi "));
        assert!(super::query_replies(b"\x1b_Ga=p\x1b\\").is_empty());
    }

    #[test]
    fn float_log_lua_has_no_inline_comments() {
        let line = super::one_line_lua(include_str!("float_log.lua"));
        assert!(!line.contains("--"), "{line}");
        assert!(line.starts_with("local api = vim.api"));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn early_lua_has_no_inline_comments() {
        let line = super::one_line_lua(include_str!("early.lua"));
        assert!(!line.contains("--"), "{line}");
        assert!(line.starts_with("local function notify"));
    }
}
