//! The agent panel: a column on the window's
//! right, full height, with a tab per agent. Each agent is a process on a PTY (`crates/term`)
//! drawn by a `TerminalView`, started in the explorer's folder. `:EshviAgent` only tells the GUI
//! (`gui_panel` with kind "agent"), as `:EshviPanel` does; Neovim keeps what the IDE connection
//! (`ide.lua`) needs: the shown agent's process and folder, which this pushes to it when they
//! change (`Eshvi.agent_set_shown`).
//!
//! The strip (tabs, "+", the history button, the Changes chip, "×") is `agent::paint_strip`.
//! The pieces here that need no window are pure and tested: tab titles (`tab_label`), activity,
//! the tools "+" offers, and the panel's widths.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use super::agent::{paint_strip, AgentButton, Strip, StripTab};
use super::bottom_panel::TabSet;
use super::*;
use crate::focus::Focus;
use crate::terminal_view::{Look, TerminalView};

/// How long an agent must print nothing to stop being "active".
pub(super) const IDLE: Duration = Duration::from_millis(1500);

/// An agent "+" can start: the default tool (`[agent] command`) first, then `[agent.commands]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Tool {
    pub name: String,
    pub cmd: Vec<String>,
}

fn basename(program: &str) -> String {
    std::path::Path::new(program).file_name().map_or_else(|| program.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// The tools: the default one (named after its program), then the others by name.
pub(super) fn tools(default: &[String], commands: &BTreeMap<String, Vec<String>>) -> Vec<Tool> {
    let default = if default.is_empty() { vec!["claude".to_owned()] } else { default.to_vec() };
    let default_name = basename(&default[0]);
    let mut out = vec![Tool { name: default_name.clone(), cmd: default }];
    out.extend(commands.iter().filter(|(name, _)| **name != default_name).map(|(name, cmd)| Tool { name: name.clone(), cmd: cmd.clone() }));
    out
}

/// The command a start runs: the tool's, and `extra` arguments for this start only (`--resume`;
/// the tab keeps the plain tool, so its restart doesn't).
pub(super) fn start_argv(tool: &Tool, extra: &[String]) -> Vec<String> {
    let mut argv = tool.cmd.clone();
    argv.extend(extra.iter().cloned());
    argv
}

/// What a tab says until the agent sets a title: the tool's name and how many of it were
/// started, "Claude 2".
pub(super) fn default_title(tool: &str, n: usize) -> String {
    let mut chars = tool.chars();
    let first: String = chars.next().into_iter().flat_map(char::to_uppercase).collect();
    format!("{first}{} {n}", chars.as_str())
}

/// Characters a title may start with that aren't part of it: a status glyph (Claude Code's
/// spinner "◐ ", the idle "✳ ", an emoji) and the spaces after it.
fn leading_glyph(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '*' | '\u{B7}')
        || ('\u{2000}'..='\u{2BFF}').contains(&c) // punctuation, arrows, shapes, dingbats, braille
        || ('\u{FE00}'..='\u{FE0F}').contains(&c)
        || ('\u{1F300}'..='\u{1FAFF}').contains(&c) // emoji
}

/// The terminal title an agent set (OSC 0/2) as a tab title:
/// Claude Code sets the session's topic behind a status glyph, so only the text after the glyphs
/// counts. `None` until it is a real title: not empty, a path, "Claude Code" (its default), nor
/// the tool's own name or command.
pub(super) fn tab_label(raw: &str, tool: &Tool) -> Option<String> {
    let text = raw.trim_start_matches(leading_glyph).trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'));
    let lower = text.to_lowercase();
    let mut chars = text.chars();
    let drive = matches!((chars.next(), chars.next(), chars.next()), (Some(d), Some(':'), Some('/' | '\\')) if d.is_ascii_alphabetic());
    let program = tool.cmd.first().map_or_else(String::new, |p| basename(p));
    let not_a_title = text.is_empty()
        || text.starts_with("term://")
        || text.starts_with(['/', '~'])
        || drive
        || lower == "claude code"
        || lower == tool.name.to_lowercase()
        || text == tool.cmd.join(" ")
        || text == program;
    (!not_a_title).then(|| text.to_owned())
}

/// An agent's activity: active while output arrives, "unseen" once it went quiet while not
/// shown (done, or waiting for an answer), until it is shown.
#[derive(Debug, Default)]
pub(super) struct Activity {
    pub active: bool,
    pub unseen: bool,
    output: u64,
    last: Option<Instant>,
}

impl Activity {
    /// Look at the terminal's output counter at `now`. Returns whether `active` or `unseen`
    /// changed.
    pub fn update(&mut self, output: u64, now: Instant, shown: bool) -> bool {
        let before = (self.active, self.unseen);
        if output != self.output {
            self.output = output;
            self.last = Some(now);
            self.active = true;
        }
        if self.active && self.last.is_some_and(|t| now.saturating_duration_since(t) > IDLE) {
            self.active = false;
            if !shown {
                self.unseen = true;
            }
        }
        if shown {
            self.unseen = false;
        }
        (self.active, self.unseen) != before
    }

    /// The agent stopped (exited): it isn't working any more.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// How long until an active agent counts as quiet.
    pub fn quiet_in(&self, now: Instant) -> Option<Duration> {
        let last = self.last.filter(|_| self.active)?;
        Some((IDLE + Duration::from_millis(20)).saturating_sub(now.saturating_duration_since(last)))
    }
}

/// The panel's smallest, first and largest width in points, given the `window`'s width and the
/// room left of it (`available`, without the side bar): 20 columns of its font at least;
/// `[agent] width` columns at first, at most 40% of the window;
/// at most what leaves the editor 20 columns (a dragged width may pass the 40%).
pub(super) fn widths(window: f32, available: f32, cols: u32, agent_cell: f32, editor_cell: f32) -> (f32, f32, f32) {
    let margins = 2.0 * style::GAP;
    let min = (20.0 * agent_cell + margins).min(available);
    let max = (available - 20.0 * editor_cell).max(min);
    let first = (cols as f32 * agent_cell + margins).min(0.4 * window).clamp(min, max);
    (min, first, max)
}

/// A tab: an agent and what the strip says of it.
pub(super) struct AgentTab {
    /// The tool as configured: a restart runs this, also for a tab that was resumed.
    pub tool: Tool,
    /// "Claude 2".
    pub title: String,
    /// The title the agent set, once it is a real one.
    pub label: Option<String>,
    /// The terminal title that `label` was made from.
    seen_title: Option<String>,
    /// Where it was started: relative paths in its output are from here.
    pub cwd: PathBuf,
    pub exited: bool,
    pub activity: Activity,
}

impl AgentTab {
    /// What the tab says: the agent's title, else "Claude 2".
    pub fn shown_title(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.title)
    }

    /// The terminal's title now: a changed one is looked at again.
    fn follow_title(&mut self, title: Option<String>) {
        if title != self.seen_title {
            self.label = title.as_deref().and_then(|t| tab_label(t, &self.tool));
            self.seen_title = title;
        }
    }
}

/// The "+" menu, open at a place.
struct Chooser {
    pos: Pos2,
    opened: bool,
}

/// What the chooser's entries start.
#[derive(Clone, Copy)]
enum Choice {
    Tool(usize),
    Resume,
}

/// Everything the agent panel holds.
pub(super) struct NativeAgents {
    pub shown: bool,
    pub tools: Vec<Tool>,
    /// Their order, which is shown and what closing shows next (shared with the bottom panel).
    pub tabs: TabSet,
    pub data: HashMap<u64, AgentTab>,
    pub views: HashMap<u64, TerminalView>,
    /// Agents started per tool, for "Claude 3".
    count: HashMap<String, usize>,
    /// What agents get in their environment (Claude Code's IDE connection).
    pub env: Vec<(String, String)>,
    /// The panel as last drawn, the width it had (kept while hidden), and a width to give it
    /// when next drawn (a loaded session's).
    pub rect: Option<Rect>,
    pub last_width: Option<f32>,
    pub force_width: Option<f32>,
    /// The strip's buttons, from the last frame.
    pub buttons: Vec<(Rect, AgentButton)>,
    /// A press began in the panel: its release is the panel's too.
    pub pressed: bool,
    chooser: Option<Chooser>,
    /// The shown agent's process and folder as `ide.lua` was last told.
    told: Option<Option<(u32, String)>>,
}

impl NativeAgents {
    pub fn new(tools: Vec<Tool>) -> Self {
        Self {
            shown: false,
            tools,
            tabs: TabSet::default(),
            data: HashMap::new(),
            views: HashMap::new(),
            count: HashMap::new(),
            env: Vec::new(),
            rect: None,
            last_width: None,
            force_width: None,
            buttons: Vec::new(),
            pressed: false,
            chooser: None,
            told: None,
        }
    }

    /// The strip's button under `pos` (a tab's "×" before the tab).
    pub fn button_at(&self, pos: Pos2) -> Option<AgentButton> {
        self.buttons.iter().find(|(r, _)| r.contains(pos)).map(|&(_, b)| b)
    }

    /// The tool named `name`.
    fn tool_named(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// The default tool is Claude: the strip has the history button, and "resume" works.
    pub fn can_resume(&self) -> bool {
        self.tools.first().is_some_and(|t| t.name == "claude")
    }

    /// The agent shown (or last shown) when it still runs: its process and folder, which
    /// `ide.lua` sends files to.
    fn shown_process(&self) -> Option<(u32, String)> {
        let id = self.tabs.current?;
        let tab = self.data.get(&id).filter(|t| !t.exited)?;
        Some((self.views.get(&id)?.pid(), tab.cwd.to_string_lossy().into_owned()))
    }

    fn next_title(&mut self, tool: &Tool) -> String {
        let n = self.count.entry(tool.name.clone()).or_insert(0);
        *n += 1;
        default_title(&tool.name, *n)
    }
}

impl VimApp {
    /// The agent panel is on screen.
    pub(super) fn agent_shown(&self) -> bool {
        self.config.agent.enabled && self.agents.shown
    }

    /// The panel is on screen with an agent in it: a region ⌃l can go to.
    pub(super) fn agent_on_screen(&self) -> bool {
        self.config.agent.enabled && self.agents.shown && self.agents.tabs.current.is_some()
    }

    /// The pointer is over the panel.
    pub(super) fn over_agent(&self, pos: Pos2) -> bool {
        self.config.agent.enabled && self.agents.shown && self.agents.rect.is_some_and(|r| r.contains(pos))
    }

    // Commands.

    /// `:EshviAgent [toggle|show|hide|focus|new [name]|resume|next|prev|close [id]|select id|restart]`.
    pub(super) fn agent_command(&mut self, ctx: &egui::Context, args: &[String]) {
        if !self.config.agent.enabled {
            return;
        }
        let action = args.first().map_or("toggle", String::as_str);
        let arg = args.get(1).map(String::as_str);
        let id_arg = arg.and_then(|a| a.parse::<u64>().ok());
        match action {
            "toggle" if self.agents.shown => self.hide_agents(),
            "toggle" | "focus" => self.show_agents(ctx, true),
            "show" => self.show_agents(ctx, false),
            "hide" => self.hide_agents(),
            "new" => self.new_command(ctx, arg),
            "resume" => self.resume_agent(ctx),
            "next" | "prev" => {
                match self.agents.tabs.cycle(self.agents.tabs.current, if action == "next" { 1 } else { -1 }) {
                    Some(id) => self.select_agent(id, true),
                    None => self.show_agents(ctx, true),
                }
            }
            "close" => {
                if let Some(id) = id_arg.or(self.agents.tabs.current) {
                    self.close_agent(id);
                }
            }
            "select" => {
                if let Some(id) = id_arg {
                    self.select_agent(id, true);
                }
            }
            "restart" => self.restart_agent(ctx),
            other => self.editor.push_message("emsg", &format!("EshviAgent: unknown action {other}")),
        }
    }

    /// Show the panel, with its last agent (a new one when there is none); `focus`: it gets
    /// the keyboard.
    fn show_agents(&mut self, ctx: &egui::Context, focus: bool) {
        if self.agents.shown {
            if let (true, Some(id)) = (focus, self.agents.tabs.current) {
                self.set_focus(Focus::Agent(id));
            }
            return;
        }
        match self.agents.tabs.current {
            Some(id) => self.select_agent(id, focus),
            None => {
                let tool = self.agents.tools[0].clone();
                self.start_agent(ctx, &tool, &[], focus);
            }
        }
    }

    pub(super) fn hide_agents(&mut self) {
        self.agents.shown = false;
        self.agents.chooser = None;
        if self.focus.in_agent() {
            self.set_focus(Focus::Editor);
        }
    }

    /// `new [name]`: the tool by name, else the default; with several and no name, "+"'s menu.
    fn new_command(&mut self, ctx: &egui::Context, name: Option<&str>) {
        match name.filter(|n| !n.is_empty()) {
            Some(name) => match self.agents.tool_named(name).cloned() {
                Some(tool) => {
                    self.start_agent(ctx, &tool, &[], true);
                }
                None => self.editor.push_message("emsg", &format!("EshviAgent: no agent named {name} ([agent.commands])")),
            },
            None => self.new_from_button(ctx, None),
        }
    }

    /// "+": a new agent of the default tool, or, with several tools, the menu to pick one at
    /// `at` (the button).
    fn new_from_button(&mut self, ctx: &egui::Context, at: Option<Pos2>) {
        if self.agents.tools.len() > 1 {
            let pos = at.unwrap_or_else(|| self.agents.rect.map_or_else(|| ctx.content_rect().right_top() + Vec2::new(-200.0, 60.0), |r| r.left_top() + Vec2::new(20.0, 30.0)));
            self.agents.chooser = Some(Chooser { pos, opened: false });
        } else {
            let tool = self.agents.tools[0].clone();
            self.start_agent(ctx, &tool, &[], true);
        }
    }

    /// A new tab running the default tool as `claude --resume`, Claude's own conversation
    /// picker; only when the default tool is Claude. Its restart runs the plain command.
    fn resume_agent(&mut self, ctx: &egui::Context) {
        if !self.agents.can_resume() {
            return self.editor.push_message("emsg", "EshviAgent: resume needs claude as the default agent ([agent] command)");
        }
        let tool = self.agents.tools[0].clone();
        self.start_agent(ctx, &tool, &["--resume".to_owned()], true);
    }

    /// Start `tool` (with `extra` arguments for this start only) in a new tab, shown; `focus`:
    /// it gets the keyboard. Says why when it doesn't start.
    fn start_agent(&mut self, ctx: &egui::Context, tool: &Tool, extra: &[String], focus: bool) -> Option<u64> {
        let (id, tab) = self.spawn_agent(ctx, tool, extra, None)?;
        self.agents.tabs.add(id, false);
        self.agents.data.insert(id, tab);
        self.agents.shown = true;
        if focus {
            self.set_focus(Focus::Agent(id));
        }
        Some(id)
    }

    /// The terminal of an agent and its tab (`title`: kept by a restart).
    fn spawn_agent(&mut self, ctx: &egui::Context, tool: &Tool, extra: &[String], title: Option<String>) -> Option<(u64, AgentTab)> {
        // Through the terminals' shell, so the agent gets what the user's startup files set
        // (PATH from .bashrc, as in a terminal they'd start it from), not only Eshvi's own
        // environment.
        let shell = term::shell_argv(self.config.terminal.shell.as_deref(), &self.config.terminal.shell_args);
        let argv = term::in_shell(shell, &start_argv(tool, extra));
        let cwd = self.explorer.root().map(std::path::Path::to_path_buf).or_else(|| std::env::current_dir().ok());
        // The size it will have, roughly: it is set again at the first frame.
        let (cols, rows) = match self.agents.rect {
            Some(r) => (((r.width() / 8.0) as u16).max(20), ((r.height() / 16.0) as u16).max(10)),
            None => (self.config.agent.width.clamp(20, 400) as u16, 40),
        };
        let size = term::Size { cols, rows, cell_w: 8, cell_h: 16 };
        match self.spawn_terminal(ctx, argv.clone(), cwd.clone(), self.agents.env.clone(), size) {
            Ok(t) => {
                let view = TerminalView::new(t);
                let id = view.id();
                self.agents.views.insert(id, view);
                let title = title.unwrap_or_else(|| self.agents.next_title(tool));
                let tab = AgentTab {
                    tool: tool.clone(),
                    title,
                    label: None,
                    seen_title: None,
                    cwd: cwd.unwrap_or_default(),
                    exited: false,
                    activity: Activity::default(),
                };
                Some((id, tab))
            }
            Err(e) => {
                // The shell didn't start (an agent missing from PATH is the shell's to say,
                // in the tab). Usually not on the PATH the app got (opened from the Finder).
                let program = argv.first().cloned().unwrap_or_default();
                let why = if on_path(&program) {
                    format!("{program} did not start: {e}")
                } else {
                    format!("{program} is not on PATH ({})", std::env::var("PATH").unwrap_or_default())
                };
                log::warn!("agent: {why}");
                self.editor.push_message("emsg", &format!("EshviAgent: {why}. Set [terminal] shell to its full path in ~/.config/eshvi/config.toml"));
                None
            }
        }
    }

    /// Show agent `id` in the panel (opening it), and give it the keyboard when `focus`.
    fn select_agent(&mut self, id: u64, focus: bool) {
        if self.agents.tabs.get(id).is_none() {
            return;
        }
        self.agents.tabs.select(id);
        self.agents.shown = true;
        if focus {
            self.set_focus(Focus::Agent(id));
        }
    }

    /// End agent `id` (hanging up its process). The panel shows a neighbour, or hides when it
    /// was the last.
    pub(super) fn close_agent(&mut self, id: u64) {
        let was_focused = self.focus == Focus::Agent(id);
        let left = self.agents.tabs.remove(id);
        self.agents.views.remove(&id);
        self.agents.data.remove(&id);
        if !left {
            self.hide_agents();
        } else if was_focused {
            if let Some(next) = self.agents.tabs.current {
                self.set_focus(Focus::Agent(next));
            }
        }
    }

    /// Start the shown agent again in its tab (the plain command, even if it was resumed).
    fn restart_agent(&mut self, ctx: &egui::Context) {
        let Some(old) = self.agents.tabs.current else { return self.show_agents(ctx, true) };
        let Some(tool) = self.agents.data.get(&old).map(|t| t.tool.clone()) else { return };
        let title = self.agents.data.get(&old).map(|t| t.title.clone());
        let Some((id, tab)) = self.spawn_agent(ctx, &tool, &[], title) else { return };
        self.agents.tabs.replace(old, id);
        self.agents.views.remove(&old);
        self.agents.data.remove(&old);
        self.agents.data.insert(id, tab);
        self.agents.shown = true;
        self.set_focus(Focus::Agent(id));
    }

    // Time.

    /// Once a frame: titles, exits and activity of every agent (hidden ones too), the shown
    /// agent's process for `ide.lua`, and the agents for the Changes list.
    pub(super) fn tick_agents(&mut self, ctx: &egui::Context) {
        if !self.config.agent.enabled {
            return;
        }
        let now = Instant::now();
        let current = self.agents.shown.then_some(self.agents.tabs.current).flatten();
        let mut done = Vec::new();
        let mut next: Option<Duration> = None;
        for (&id, view) in &self.agents.views {
            let Some(tab) = self.agents.data.get_mut(&id) else { continue };
            let term = view.terminal();
            tab.follow_title(term.title());
            if let Some((code, _)) = term.exit_status() {
                if !tab.exited {
                    tab.exited = true;
                    tab.activity.stop();
                }
                // A clean exit (`/exit`) closes the tab; an error stays readable.
                if code == Some(0) {
                    done.push(id);
                }
            }
            if !tab.exited {
                tab.activity.update(term.output_count(), now, current == Some(id));
                next = [next, tab.activity.quiet_in(now)].into_iter().flatten().min();
            }
            // (A shown view empties its own events as it draws.)
            if current != Some(id) {
                term.events();
            }
        }
        for id in done {
            self.close_agent(id);
        }
        if let Some(wait) = next {
            ctx.request_repaint_after(wait);
        }
        self.tell_ide_about_the_shown_agent();
        let agents = self.agents_for_changes();
        self.changes.on_agents(&agents);
    }

    /// The agents' folders and activity, for the Changes list.
    fn agents_for_changes(&self) -> Vec<crate::changes::AgentInfo> {
        self.agents
            .tabs
            .tabs
            .iter()
            .filter_map(|t| {
                let d = self.agents.data.get(&t.id)?;
                Some(crate::changes::AgentInfo { id: t.id, cwd: d.cwd.to_string_lossy().into_owned(), active: d.activity.active })
            })
            .collect()
    }

    /// `Eshvi.agent_pid` (what "send to the agent" goes by: `ide_connected`'s pid is the
    /// agent's process) and the folder its relative paths are in: pushed to Neovim when they
    /// change.
    fn tell_ide_about_the_shown_agent(&mut self) {
        let now = self.agents.shown_process();
        if self.agents.told.as_ref() == Some(&now) || (self.agents.told.is_none() && now.is_none()) {
            return;
        }
        let (pid, cwd) = now.clone().unwrap_or((0, String::new()));
        self.bridge.send(UiCommand::Lua {
            code: "return Eshvi.agent_set_shown and Eshvi.agent_set_shown(...)".into(),
            args: vec![RpcValue::from(pid as i64), RpcValue::from(cwd.as_str())],
        });
        self.agents.told = Some(now);
    }

    // Drawing.

    /// The panel as a column at the window's right, if shown: the strip, then the shown agent's
    /// terminal. Before the editor's area is worked out, so Neovim's size is what is left.
    pub(super) fn show_agent_column(&mut self, ui: &mut egui::Ui) {
        self.agents.buttons.clear();
        if !self.config.agent.enabled || !self.agents.shown || !self.grid_fonts.is_live(ui.ctx()) {
            self.agents.rect = None;
            return;
        }
        let ctx = ui.ctx().clone();
        let chrome = Chrome::from_editor(&self.editor);
        let row = self.renderer.metrics().height;
        let fonts = self.grid_fonts.agent.as_deref().unwrap_or_else(|| self.grid_fonts.terminal());
        let agent_cell = crate::renderer::EguiGridRenderer::measure(&ctx, fonts, self.linespace).width;
        let available = ui.available_width();
        let (min, first, max) = widths(ctx.content_rect().width(), available, self.config.agent.width.clamp(20, 400), agent_cell, self.renderer.metrics().width);
        let mut panel = egui::Panel::right("eshvi-agent")
            .resizable(true)
            .default_size(first)
            .size_range(min..=max)
            .frame(egui::Frame::new().fill(chrome.panel));
        if let Some(width) = self.agents.force_width.take() {
            panel = panel.exact_size(width.clamp(min, max));
        }
        let shown = panel.show(ui, |ui| {
            // Fill what the panel is given (else it keeps to its content's minimum, and the
            // width it was asked for is lost).
            ui.set_min_size(ui.available_size());
            let rect = ui.max_rect();
            let strip = Rect::from_min_size(rect.min, Vec2::new(rect.width(), row));
            let content = Rect::from_min_max(Pos2::new(rect.min.x, strip.max.y), rect.max);
            let painter = ui.painter_at(rect);
            let tabs: Vec<StripTab> = self
                .agents
                .tabs
                .tabs
                .iter()
                .filter_map(|t| {
                    let d = self.agents.data.get(&t.id)?;
                    Some(StripTab {
                        id: t.id as i64,
                        title: d.shown_title().to_owned(),
                        current: self.agents.tabs.current == Some(t.id),
                        exited: d.exited,
                        active: d.activity.active,
                        unseen: d.activity.unseen,
                    })
                })
                .collect();
            let mut font = self.grid_fonts.sidebar_font_id(false);
            font.size = self.status_font_size + 1.0;
            let model = Strip {
                tabs: &tabs,
                focused: self.focus.in_agent(),
                content_bg: chrome.panel,
                resume: self.agents.can_resume(),
                changes: self.changes_chip(),
            };
            let out = paint_strip(&painter, strip, &model, &chrome, &font, &mut self.icons);
            self.agents.buttons = out.buttons;
            self.agent_strip = Some(strip);
            if out.animating {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            // The shown agent's terminal below it.
            let Some(id) = self.agents.tabs.current else { return };
            let look = Look {
                fonts: self.grid_fonts.agent.as_deref().unwrap_or_else(|| self.grid_fonts.terminal()),
                linespace: self.linespace,
                fg: rgb(self.editor.default_colors.fg),
                bg: chrome.panel,
                accent: chrome.accent,
                ansi: self.editor.term_colors,
                option_as_meta: self.config.terminal.option_as_meta,
                blink: self.blink.enabled,
                copy_on_select: self.config.terminal.copy_on_select,
                middle_click_paste: self.config.terminal.middle_click_paste,
            };
            let Some(view) = self.agents.views.get_mut(&id) else { return };
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content));
            child.set_clip_rect(content.intersect(ui.clip_rect()));
            let o = view.show(&mut child, &look, self.focus == Focus::Agent(id));
            let dir = view.link_dir();
            if o.pressed {
                self.set_focus(Focus::Agent(id));
            }
            if let Some(target) = o.link {
                let dir = dir.or_else(|| self.agents.data.get(&id).map(|t| t.cwd.clone()));
                self.open_term_link(&ctx, target, dir);
            }
        });
        let rect = shown.response.rect;
        self.agents.rect = Some(rect);
        self.agents.last_width = Some(rect.width());
        self.show_agent_chooser(&ctx);
    }

    /// "+"'s menu of tools (and Resume Conversation… for Claude), at the button.
    fn show_agent_chooser(&mut self, ctx: &egui::Context) {
        let Some(chooser) = &mut self.agents.chooser else { return };
        let id = egui::Id::unique("eshvi-agent-chooser");
        let first = !chooser.opened;
        let open = first.then_some(egui::SetOpenCommand::Bool(true));
        let mut chosen = None;
        let names: Vec<String> = self.agents.tools.iter().map(|t| t.name.clone()).collect();
        let resume = self.agents.tools.first().is_some_and(|t| t.name == "claude");
        let shown = egui::Popup::new(id, ctx.clone(), egui::PopupAnchor::Position(chooser.pos), egui::LayerId::new(egui::Order::Foreground, id))
            .kind(egui::PopupKind::Menu)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .style(egui::containers::menu::menu_style)
            .gap(0.0)
            .open_memory(open)
            .show(|ui| {
                for (i, name) in names.iter().enumerate() {
                    if ui.button(name).clicked() {
                        chosen = Some(Choice::Tool(i));
                    }
                }
                if resume {
                    ui.separator();
                    if ui.button("Resume Conversation…").clicked() {
                        chosen = Some(Choice::Resume);
                    }
                }
            });
        chooser.opened = true;
        if shown.is_none() && !first {
            self.agents.chooser = None;
        }
        if let Some(choice) = chosen {
            self.agents.chooser = None;
            egui::Popup::close_id(ctx, id);
            match choice {
                Choice::Tool(i) => {
                    let tool = self.agents.tools[i].clone();
                    self.start_agent(ctx, &tool, &[], true);
                }
                Choice::Resume => self.resume_agent(ctx),
            }
        }
    }

    // Input.

    /// An input event, before it is Neovim's: while an agent has the keyboard its keys are its
    /// own (⌃h/j/k/l move to the next region), and the pointer over the panel is its own.
    /// Returns whether the event was taken.
    pub(super) fn agent_event(&mut self, ctx: &egui::Context, ev: &Event) -> bool {
        if !self.config.agent.enabled {
            return false;
        }
        match ev {
            Event::Key { key, pressed, modifiers, .. } if self.focus.in_agent() => {
                if let Some(dir) = self.takes_navigation(*key, *modifiers) {
                    // (The terminal draws after this: it must not see the key either.)
                    let key = *key;
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Key { key: k, modifiers, .. } if *k == key && modifiers.ctrl)));
                    if *pressed {
                        self.navigate(dir);
                    }
                    return true;
                }
                // Esc closes the Changes popup first; it is not the agent's then.
                if *key == egui::Key::Escape && *pressed && self.changes.popup {
                    self.changes.popup = false;
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Key { key: egui::Key::Escape, .. })));
                }
                true
            }
            Event::Text(_) | Event::Ime(_) | Event::Paste(_) | Event::Copy | Event::Cut if self.focus.in_agent() => true,
            Event::PointerButton { pos, pressed: true, button, .. } if self.over_agent(*pos) && !self.over_popup(ctx, *pos) => {
                self.agents.pressed = true;
                self.agent_press(ctx, *pos, *button);
                true
            }
            Event::PointerButton { pressed: false, .. } if std::mem::take(&mut self.agents.pressed) => true,
            Event::PointerMoved(_) if self.agents.pressed => true,
            Event::PointerMoved(pos) if self.over_agent(*pos) && !self.over_popup(ctx, *pos) => true,
            Event::MouseWheel { .. } if ctx.input(|i| i.pointer.latest_pos()).is_some_and(|p| self.over_agent(p) && !self.over_popup(ctx, p)) => true,
            _ => false,
        }
    }

    /// A press in the panel: a button of the strip does its thing, the edge is the splitter's
    /// (egui's), anywhere else gives the keyboard to the shown agent.
    fn agent_press(&mut self, ctx: &egui::Context, pos: Pos2, button: PointerButton) {
        let Some(rect) = self.agents.rect else { return };
        if pos.x < rect.min.x + 4.0 {
            return;
        }
        let primary = button == PointerButton::Primary;
        match (self.agents.button_at(pos), button) {
            (Some(AgentButton::Tab(id)), PointerButton::Middle) | (Some(AgentButton::CloseTab(id)), PointerButton::Primary) => self.close_agent(id as u64),
            (Some(AgentButton::Tab(id)), PointerButton::Primary) => self.select_agent(id as u64, true),
            (Some(AgentButton::Changes), PointerButton::Primary) => {
                if self.changes.popup {
                    self.changes.popup = false;
                } else {
                    self.changes.open();
                }
            }
            (Some(AgentButton::New), PointerButton::Primary) => {
                let at = self.agents.buttons.iter().find(|(_, b)| *b == AgentButton::New).map(|(r, _)| r.left_bottom());
                self.new_from_button(ctx, at);
            }
            (Some(AgentButton::Resume), PointerButton::Primary) => self.resume_agent(ctx),
            (Some(AgentButton::Hide), PointerButton::Primary) => self.hide_agents(),
            (Some(_), _) => {}
            // The rest of the strip, and the terminal: the keyboard goes to the shown agent.
            (None, _) if primary || button == PointerButton::Secondary => {
                if let Some(id) = self.agents.tabs.current {
                    self.set_focus(Focus::Agent(id));
                }
            }
            (None, _) => {}
        }
    }

    // Sessions.

    /// The panel's part of a session.
    pub(super) fn agent_session(&self) -> Option<nvim_bridge::gui::AgentSession> {
        self.config.agent.enabled.then(|| nvim_bridge::gui::AgentSession {
            shown: self.agents.shown,
            width: self.agents.rect.map(|r| r.width()).or(self.agents.last_width).unwrap_or(0.0).round(),
        })
    }

    /// A loaded session's panel: shown as it was saved, at its width, with a fresh agent
    /// (agents aren't restored); hidden when it was hidden.
    pub(super) fn apply_agent_session(&mut self, ctx: &egui::Context, a: &nvim_bridge::gui::AgentSession) {
        if !self.config.agent.enabled {
            return;
        }
        if a.width >= 1.0 {
            self.agents.last_width = Some(a.width);
            self.agents.force_width = Some(a.width);
        }
        if a.shown {
            self.show_agents(ctx, false);
        } else {
            self.hide_agents();
        }
    }

    /// The find field of an agent's terminal keeps egui's focus.
    pub(super) fn agents_keep_focus(&self, id: egui::Id) -> bool {
        self.agents.views.values().any(|v| v.find_id() == id)
    }
}

/// Whether `program` is found as a command: a path that exists, or a file on `PATH`.
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).exists();
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> Tool {
        Tool { name: "claude".into(), cmd: vec!["claude".into()] }
    }

    #[test]
    fn a_title_loses_its_status_glyph_and_keeps_its_text() {
        let t = claude();
        // Claude Code's spinner while it works, and its idle mark, before the session's topic.
        assert_eq!(tab_label("◐ Fix the bug", &t).as_deref(), Some("Fix the bug"));
        assert_eq!(tab_label("◑ Fix the bug", &t).as_deref(), Some("Fix the bug"));
        assert_eq!(tab_label("✳ Fix the bug", &t).as_deref(), Some("Fix the bug"));
        // So the spinner doesn't change the tab.
        assert_eq!(tab_label("◐ Fix the bug", &t), tab_label("✳ Fix the bug", &t));
        assert_eq!(tab_label("Fix the bug", &t).as_deref(), Some("Fix the bug"));
        // Asterisks, middle dots, emoji, variation selectors and spaces before it go too.
        assert_eq!(tab_label("* Refactor", &t).as_deref(), Some("Refactor"));
        assert_eq!(tab_label("· · Refactor", &t).as_deref(), Some("Refactor"));
        assert_eq!(tab_label("🚀 Deploy it", &t).as_deref(), Some("Deploy it"));
        assert_eq!(tab_label("\u{2699}\u{FE0F} Settings", &t).as_deref(), Some("Settings"));
        assert_eq!(tab_label("   padded   ", &t).as_deref(), Some("padded"));
        // Letters of any script stay.
        assert_eq!(tab_label("✳ שלום עולם", &t).as_deref(), Some("שלום עולם"));
        assert_eq!(tab_label("✳ 修复错误", &t).as_deref(), Some("修复错误"));
        assert_eq!(tab_label("✳ Ünïcode", &t).as_deref(), Some("Ünïcode"));
        // Only the leading glyphs: the rest of the text is kept as it is.
        assert_eq!(tab_label("✳ a ✳ b", &t).as_deref(), Some("a ✳ b"));
    }

    #[test]
    fn a_title_that_is_not_one_leaves_the_tab_as_it_was_named() {
        let t = claude();
        for raw in [
            "",
            "   ",
            // Only glyphs.
            "✳",
            "✳ ",
            "◐ ",
            // Claude Code's own default, whatever the case.
            "Claude Code",
            "✳ Claude Code",
            "claude code",
            // The tool's name and command.
            "claude",
            "Claude",
            "term://~/work//123:claude",
            // Paths.
            "~/work/repo",
            "/usr/local/bin/claude",
            "C:\\Users\\me",
            "d:/work",
        ] {
            assert_eq!(tab_label(raw, &t), None, "{raw:?}");
        }
        // The command with its arguments, and the program's base name.
        let tool = Tool { name: "codex".into(), cmd: vec!["/opt/bin/codex".into(), "--full-auto".into()] };
        assert_eq!(tab_label("/opt/bin/codex --full-auto", &tool), None);
        assert_eq!(tab_label("codex", &tool), None);
        assert_eq!(tab_label("Codex", &tool), None);
        assert_eq!(tab_label("codex --full-auto", &tool).as_deref(), Some("codex --full-auto"));
        // A letter and a colon isn't a drive.
        assert_eq!(tab_label("a: b", &claude()).as_deref(), Some("a: b"));
    }

    #[test]
    fn a_tab_is_named_after_its_tool_until_the_agent_sets_a_title() {
        assert_eq!(default_title("claude", 1), "Claude 1");
        assert_eq!(default_title("claude", 12), "Claude 12");
        assert_eq!(default_title("codex", 2), "Codex 2");
        assert_eq!(default_title("sh", 1), "Sh 1");
        let mut tab = AgentTab {
            tool: claude(),
            title: "Claude 2".into(),
            label: None,
            seen_title: None,
            cwd: PathBuf::new(),
            exited: false,
            activity: Activity::default(),
        };
        assert_eq!(tab.shown_title(), "Claude 2");
        tab.follow_title(Some("◐ Fix the bug".into()));
        assert_eq!(tab.shown_title(), "Fix the bug");
        // The agent's default title, a path, then no title: the tab keeps its name again.
        tab.follow_title(Some("✳ Claude Code".into()));
        assert_eq!(tab.shown_title(), "Claude 2");
        tab.follow_title(Some("✳ Second topic".into()));
        assert_eq!(tab.shown_title(), "Second topic");
        tab.follow_title(None);
        assert_eq!(tab.shown_title(), "Claude 2");
    }

    #[test]
    fn the_default_tool_is_first_then_the_others_by_name() {
        let commands: BTreeMap<String, Vec<String>> = [
            ("codex".to_owned(), vec!["codex".to_owned(), "--full-auto".to_owned()]),
            ("aider".to_owned(), vec!["aider".to_owned()]),
            ("claude".to_owned(), vec!["/other/claude".to_owned()]),
        ]
        .into();
        let t = tools(&["/opt/bin/claude".to_owned(), "--continue".to_owned()], &commands);
        // Named after the program; a tool of the same name in `[agent.commands]` is the default.
        assert_eq!(t.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["claude", "aider", "codex"]);
        assert_eq!(t[0].cmd, ["/opt/bin/claude", "--continue"]);
        assert_eq!(t[2].cmd, ["codex", "--full-auto"]);
        // No command: claude.
        assert_eq!(tools(&[], &BTreeMap::new()), [claude()]);
    }

    #[test]
    fn a_resumed_agent_runs_the_picker_and_restarts_as_the_plain_command() {
        let mut app_tools = NativeAgents::new(tools(&["claude".to_owned()], &BTreeMap::new()));
        assert!(app_tools.can_resume());
        let tool = app_tools.tools[0].clone();
        // The argv of a resume: the command and `--resume`, for this start only; of a start,
        // a restart, or "+" with an argument-taking tool: as configured.
        assert_eq!(start_argv(&tool, &["--resume".to_owned()]), ["claude", "--resume"]);
        assert_eq!(start_argv(&tool, &[]), ["claude"]);
        let continued = Tool { name: "claude".into(), cmd: vec!["claude".into(), "--continue".into()] };
        assert_eq!(start_argv(&continued, &["--resume".to_owned()]), ["claude", "--continue", "--resume"]);
        // The tab keeps the plain tool, so its restart is `claude`.
        assert_eq!(tool.cmd, ["claude"]);
        // Titles count per tool and go on after one is closed.
        assert_eq!(app_tools.next_title(&tool), "Claude 1");
        assert_eq!(app_tools.next_title(&tool), "Claude 2");
        // Only for Claude.
        assert!(!NativeAgents::new(tools(&["codex".to_owned()], &BTreeMap::new())).can_resume());
    }

    #[test]
    fn an_agent_is_active_while_output_arrives_and_quiet_after_a_second_and_a_half() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut a = Activity::default();
        // Nothing printed: nothing.
        assert!(!a.update(0, at(0), true));
        // Output: active, at once.
        assert!(a.update(3, at(100), true));
        assert!(a.active && !a.unseen);
        // More output keeps it so, and moves the quiet moment.
        assert!(!a.update(4, at(1200), true));
        assert!(!a.update(4, at(2600), true));
        assert!(a.active);
        // Quiet for more than 1.5 s since the last output: no longer active, and nothing to see.
        assert!(a.update(4, at(2800), true));
        assert!(!a.active && !a.unseen);
    }

    #[test]
    fn an_agent_that_goes_quiet_out_of_sight_is_unseen_until_shown() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut a = Activity::default();
        assert!(a.update(1, at(0), false));
        assert!(a.active);
        assert_eq!(a.quiet_in(at(1000)), Some(Duration::from_millis(520)));
        assert!(!a.update(1, at(1500), false), "1.5 s exactly is not yet quiet");
        assert!(a.update(1, at(1501), false));
        assert!(!a.active && a.unseen);
        assert_eq!(a.quiet_in(at(1600)), None);
        // Still unseen while it stays out of sight, also with more output.
        assert!(!a.update(1, at(5000), false));
        assert!(a.unseen);
        // Shown: seen.
        assert!(a.update(1, at(5100), true));
        assert!(!a.unseen);
        // Active while shown, then quiet while shown: not unseen.
        a.update(2, at(6000), true);
        a.update(2, at(8000), true);
        assert!(!a.active && !a.unseen);
        // Hidden while it works, and quiet there: unseen.
        a.update(3, at(9000), true);
        a.update(3, at(9100), false);
        assert!(a.update(3, at(10_700), false));
        assert!(a.unseen);
        // An exited agent stops working at once.
        a.update(4, at(11_000), true);
        a.stop();
        assert!(!a.active);
    }

    #[test]
    fn the_panel_starts_at_its_width_but_no_wider_than_four_tenths_of_the_window() {
        // 1000 points of room, cells of 8 (the agent's) and 7 (the editor's): 80 columns and
        // margins fit under 40%? No: 400 is the cap.
        let (min, first, max) = widths(1000.0, 1000.0, 80, 8.0, 7.0);
        assert_eq!(first, 400.0);
        // 40% of the window, not of what the side bar leaves.
        assert_eq!(widths(1000.0, 700.0, 80, 8.0, 7.0).1, 400.0);
        // A narrower request is kept.
        let (_, first, _) = widths(1000.0, 1000.0, 40, 8.0, 7.0);
        assert_eq!(first, 40.0 * 8.0 + 12.0);
        // Never under 20 columns, and the editor keeps 20 columns of its own; a dragged width
        // may pass the 40%.
        assert_eq!(min, 20.0 * 8.0 + 12.0);
        assert_eq!(max, 1000.0 - 20.0 * 7.0);
        assert!(max > first);
        // A request under the minimum is raised to it.
        assert_eq!(widths(1000.0, 1000.0, 5, 8.0, 7.0).1, min);
        // A small window: the cap, but the minimum when the cap is under it; and nothing
        // wider than the room.
        let (min, first, max) = widths(300.0, 300.0, 80, 8.0, 7.0);
        assert_eq!((min, first), (172.0, 172.0));
        assert_eq!(max, 172.0_f32.max(300.0 - 140.0));
        let (min, _, max) = widths(100.0, 100.0, 80, 8.0, 7.0);
        assert_eq!((min, max), (100.0, 100.0));
    }

    #[test]
    fn a_program_is_found_on_the_path_or_by_its_own() {
        assert!(on_path("/bin/sh"));
        assert!(on_path("sh"));
        assert!(!on_path("/no/such/agent"));
        assert!(!on_path("surely-not-an-agent-here"));
    }
}
