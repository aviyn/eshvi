//! The hidden terminal window (`:EshviTermDebug`): one of the GUI's own
//! terminals (`crates/term`, drawn by `terminal_view`) in a floating window,
//! running the login shell in the explorer's folder. For trying them before
//! the panels that hold them. Its keyboard focus is taken like the
//! explorer's: a click in the window gives it, a click elsewhere takes it
//! back, and while it has it nothing reaches Neovim.

use super::*;
use crate::terminal_view::{Look, TerminalView};

/// The window and the terminal in it.
pub(super) struct TermDebug {
    view: TerminalView,
    pub focused: bool,
    /// Where the window was last drawn.
    pub rect: Option<Rect>,
    /// A press that began in the window, and the drag that follows (its
    /// release is the terminal's too).
    pub pressed: bool,
}

impl VimApp {
    /// `:EshviTermDebug [toggle|show|hide|focus]`.
    pub(super) fn term_debug_command(&mut self, ctx: &egui::Context, action: &str) {
        match (action, self.term_debug.as_mut()) {
            ("hide", _) | ("toggle", Some(_)) => self.term_debug = None,
            (_, Some(_)) => {
                self.set_focus(Focus::Editor);
                if let Some(d) = self.term_debug.as_mut() {
                    d.focused = true;
                }
            }
            (_, None) => {
                let wake = {
                    let ctx = ctx.clone();
                    Arc::new(move || ctx.request_repaint()) as term::Wake
                };
                let size = term::Size { cols: 80, rows: 24, cell_w: 8, cell_h: 16 };
                let mut opts = term::SpawnOptions::new(term::shell_argv(self.config.terminal.shell.as_deref(), &self.config.terminal.shell_args), size);
                opts.cwd = self.explorer.root().map(std::path::Path::to_path_buf);
                opts.scrollback = self.config.terminal.scrollback;
                opts.keys = term::keys::KeyOptions { option_as_meta: self.config.terminal.option_as_meta };
                opts.wake = Some(wake);
                match term::Terminal::spawn(opts) {
                    Ok(t) => {
                        self.set_focus(Focus::Editor);
                        self.term_debug = Some(TermDebug { view: TerminalView::new(t), focused: true, rect: None, pressed: false });
                    }
                    Err(e) => log::warn!("terminal: {e}"),
                }
            }
        }
    }

    /// Whether the terminal window has the keyboard.
    pub(super) fn term_debug_focused(&self) -> bool {
        self.term_debug.as_ref().is_some_and(|d| d.focused)
    }

    /// Whether `id` is the terminal's find field, which keeps egui's focus.
    pub(super) fn term_debug_keeps_focus(&self, id: egui::Id) -> bool {
        self.term_debug.as_ref().is_some_and(|d| d.view.find_id() == id)
    }

    /// Edit › Find, Find Next and Find Previous, for the terminal.
    pub(super) fn term_debug_find(&mut self, action: &str) {
        let Some(d) = self.term_debug.as_mut() else { return };
        match action {
            "find" => d.view.open_find(),
            _ => {
                d.view.terminal().search_step(action == "find_next");
            }
        }
    }

    /// Whether the terminal has a selection to copy.
    pub(super) fn term_debug_has_selection(&self) -> bool {
        self.term_debug.as_ref().is_some_and(|d| d.focused && d.view.terminal().has_selection())
    }

    /// Draw the window and what its terminal asks for.
    pub(super) fn show_term_debug(&mut self, ctx: &egui::Context) {
        let Some(d) = self.term_debug.as_mut() else { return };
        let chrome = Chrome::from_editor(&self.editor);
        let look = Look {
            fonts: self.grid_fonts.terminal(),
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
        let pid = d.view.pid();
        let title = match (d.view.foreground_process(), d.view.title()) {
            (_, Some(t)) if !t.is_empty() => format!("Terminal {pid}: {t}"),
            (Some(p), _) => format!("Terminal {pid}: {p}"),
            _ => format!("Terminal {pid}"),
        };
        let mut out = None;
        let focused = d.focused;
        let shown = egui::Window::new(title)
            .id(egui::Id::unique("eshvi-term-debug"))
            .default_size([760.0, 440.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                out = Some(d.view.show(ui, &look, focused));
            });
        d.rect = shown.map(|r| r.response.rect);
        if let Some(rect) = d.rect {
            self.popup_rects.push(rect);
        }
        let Some(out) = out else { return };
        let mut close = false;
        for ev in &out.events {
            // As a shell's clean exit closes a tab: this window.
            if matches!(ev, term::Event::Exit { code: Some(0), .. }) {
                close = true;
            }
        }
        let dir = d.view.link_dir();
        if out.pressed {
            self.set_focus(Focus::Editor);
            if let Some(d) = self.term_debug.as_mut() {
                d.focused = true;
            }
        }
        if let Some(target) = out.link {
            self.open_term_link(ctx, target, dir);
        }
        if close {
            self.term_debug = None;
        }
    }

    /// Open a link from a terminal's output: a file in an editor window (by
    /// Neovim, relative to where the terminal is), a URL in the browser.
    pub(super) fn open_term_link(&self, ctx: &egui::Context, target: crate::links::Target, dir: Option<std::path::PathBuf>) {
        use crate::links::Target;
        match target {
            Target::Url(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Target::File { path, line, col } => {
                let path = std::path::Path::new(&path);
                let full = if path.is_absolute() { path.to_path_buf() } else { dir.unwrap_or_default().join(path) };
                self.open_file_at(&full, line.unwrap_or(0) as usize, col.unwrap_or(1) as usize);
            }
        }
    }
}
