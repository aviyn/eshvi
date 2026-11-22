//! The diff view in the app (`diff_view/`): opened from Source Control, a
//! commit or the Changes popup, drawn over the editor area until closed, with
//! its own keys and pointer while it has the keyboard (`Focus::Diff`).

use super::*;
use crate::diff_view::{self, Action, Coloring, DiffColors, DiffView, Edit, FileSpec, Files, Look, Mode, Options, Request, Side, Status};

impl VimApp {
    /// Open the view with `req` over the editor, with the keyboard.
    pub(super) fn open_diff(&mut self, ctx: &egui::Context, req: Request) {
        let wake = ctx.clone();
        let opts = Options { layout: self.diff_layout, context: self.config.diff.context, max_lines: diff_view::model::MAX_LINES, split: self.diff_split };
        self.diff = Some(DiffView::new(req, opts, move || wake.request_repaint()));
        self.diff_buffer = self.active_file.clone();
        self.set_focus(Focus::Diff);
        self.diff_opened = true;
        ctx.request_repaint();
    }

    /// Close it; the keyboard goes back to Neovim. The layout chosen, and where the
    /// sides divide, stay for the session.
    pub(super) fn close_diff(&mut self) {
        if let Some(d) = self.diff.take() {
            self.diff_layout = d.layout;
            self.diff_split = d.split;
        }
        self.diff_rect = None;
        self.diff_pressed = false;
        self.diff_pending = None;
        self.diff_cmd = None;
        if self.focus == Focus::Diff {
            self.set_focus(Focus::Editor);
        }
    }

    /// The agents' changes as a view's files: each against the snapshot, as it is now
    /// (`only`: that file alone).
    pub(super) fn changes_request(&self, only: Option<&str>) -> Option<Request> {
        let s = self.changes.current()?;
        let files: Vec<FileSpec> = self
            .changes
            .files
            .iter()
            .filter(|f| only.is_none_or(|p| f.path == p))
            .map(|f| FileSpec {
                path: f.path.clone(),
                old_path: None,
                status: match f.status {
                    'A' => Status::Added,
                    'D' => Status::Deleted,
                    _ => Status::Modified,
                },
                left: Side::Ref(s.base.clone()),
                right: Side::Disk,
            })
            .collect();
        (!files.is_empty()).then(|| Request {
            top: s.top.clone(),
            compared: "before the agents → now".into(),
            focus: None,
            files: Files::List(files),
            mode: Mode::Agents,
        })
    }

    /// Whether `pos` is over the view.
    pub(super) fn over_diff(&self, pos: Pos2) -> bool {
        self.diff_rect.is_some_and(|r| r.contains(pos))
    }

    /// An input event while the view is open, before it is Neovim's: its keys
    /// while it has the keyboard (⌃h/j/k/l move to the next region), and the
    /// pointer over it. Returns whether the event was taken.
    pub(super) fn diff_event(&mut self, ctx: &egui::Context, ev: &Event) -> bool {
        if self.diff.is_none() {
            return false;
        }
        match ev {
            // The text of the key press that sent `:` to the command line (it was sent already).
            Event::Text(t) if t == ":" && self.diff_colon.take().is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(200)) => true,
            Event::PointerButton { pos, pressed: true, .. } if self.over_diff(*pos) && !self.over_popup(ctx, *pos) => {
                self.set_focus(Focus::Diff);
                self.diff_pressed = true;
                true
            }
            Event::PointerButton { pressed: false, .. } if std::mem::take(&mut self.diff_pressed) => true,
            Event::PointerMoved(_) if self.diff_pressed => true,
            Event::MouseWheel { .. } if ctx.input(|i| i.pointer.latest_pos()).is_some_and(|p| self.over_diff(p) && !self.over_popup(ctx, p)) => true,
            Event::Key { key, pressed, modifiers, .. } if self.focus == Focus::Diff => {
                // ⌘F opens the find field, ⌘G and ⇧⌘G step through the matches (also from the field).
                // (With the menu bar these arrive as its Edit items: `diff_find`.)
                if modifiers.mac_cmd && !modifiers.alt && !modifiers.ctrl && matches!(key, egui::Key::F | egui::Key::G) {
                    if *pressed {
                        match key {
                            egui::Key::F if !modifiers.shift => self.diff_find("find"),
                            egui::Key::G => self.diff_find(if modifiers.shift { "find_prev" } else { "find_next" }),
                            _ => {}
                        }
                        ctx.request_repaint();
                    }
                    return true;
                }
                // The find field has the keyboard (egui's): its keys are not the view's.
                if self.diff.as_ref().is_some_and(DiffView::find_focused) && !modifiers.mac_cmd {
                    return true;
                }
                if let Some(dir) = self.takes_navigation(*key, *modifiers) {
                    if *pressed {
                        self.navigate(dir);
                    }
                    return true;
                }
                // ⌘ shortcuts (the sidebar's, zoom) are the app's, as ever.
                if modifiers.mac_cmd || (modifiers.ctrl && !matches!(key, egui::Key::D | egui::Key::U)) {
                    return false;
                }
                if *pressed {
                    if let Some(action) = self.diff.as_mut().and_then(|d| d.key(*key, *modifiers)) {
                        self.diff_action(action);
                    }
                    ctx.request_repaint();
                }
                true
            }
            Event::Copy if self.focus == Focus::Diff && !self.diff.as_ref().is_some_and(DiffView::find_focused) => {
                if let Some(text) = self.diff.as_ref().and_then(DiffView::selected_text) {
                    ctx.copy_text(text);
                }
                true
            }
            Event::Text(_) | Event::Ime(_) | Event::Paste(_) | Event::Cut if self.focus == Focus::Diff => true,
            _ => false,
        }
    }

    /// What the view asked for.
    pub(super) fn diff_action(&mut self, action: Action) {
        match action {
            Action::Close => self.close_diff(),
            Action::Open { path, line } => {
                self.close_diff();
                let args = vec![RpcValue::from(path.to_string_lossy().as_ref()), RpcValue::from(line as i64)];
                self.bridge.send(UiCommand::Lua { code: "return Eshvi.open_at(...)".into(), args });
                self.set_focus(Focus::Editor);
            }
            Action::Command => {
                // Neovim's command line: the keyboard is Neovim's for it (not
                // `set_focus`, which would give it back to the view).
                self.diff_cmd = Some(false);
                self.diff_colon = Some(Instant::now());
                self.focus = Focus::Editor;
                self.explorer.focused = false;
                self.bridge.send(UiCommand::Input(":".into()));
            }
            Action::Edit(edit) => self.diff_edit(edit),
            Action::Neovim(spec) => {
                if let Some(top) = self.diff.as_ref().map(|d| d.top.clone()) {
                    self.bridge.send(scm_view::neovim_spec(&top, &spec).lua("tab"));
                }
                self.close_diff();
                self.set_focus(Focus::Editor);
            }
        }
    }

    /// ⌘F, ⌘G and ⇧⌘G in the view ("find", "find_next", "find_prev").
    pub(super) fn diff_find(&mut self, action: &str) {
        let Some(d) = self.diff.as_mut() else { return };
        match action {
            "find" => d.open_find(),
            "find_next" => d.find_step(true),
            _ => d.find_step(false),
        }
    }

    /// Make an edit the view asked for: a revert through Neovim's buffer, git's
    /// operations for the rest (a discard after Neovim says no file of it has
    /// unsaved changes).
    fn diff_edit(&mut self, edit: Edit) {
        let Some(top) = self.diff.as_ref().map(|d| d.top.clone()) else { return };
        match &edit {
            Edit::Revert { path, start, expect, put } => {
                let lines = |v: &[String]| RpcValue::Array(v.iter().map(|l| RpcValue::from(l.as_str())).collect());
                let args = vec![
                    RpcValue::from(top.join(path).to_string_lossy().as_ref()),
                    RpcValue::from(*start as i64),
                    lines(expect),
                    lines(put),
                ];
                self.bridge.send(UiCommand::Lua { code: "return Eshvi.diff_revert(...)".into(), args });
            }
            Edit::Accept { path } => {
                // (The snapshot moves on: the view reads its files against the new one.)
                self.changes.accept(path);
            }
            _ if edit.discards() => {
                let paths: Vec<RpcValue> = match &edit {
                    Edit::Patch { path, .. } => vec![top.join(path)],
                    Edit::Files { paths, .. } => paths.iter().map(|p| top.join(p)).collect(),
                    _ => Vec::new(),
                }
                .into_iter()
                .map(|p| RpcValue::from(p.to_string_lossy().as_ref()))
                .collect();
                self.diff_pending = Some(edit);
                self.bridge.send(UiCommand::Lua { code: "return Eshvi.diff_guard(...)".into(), args: vec![RpcValue::Array(paths)] });
            }
            _ => self.run_diff_edit(&top, edit),
        }
    }

    /// Hand an edit to git.
    fn run_diff_edit(&mut self, top: &std::path::Path, edit: Edit) {
        if let Some(op) = edit.git_op() {
            self.git_ops.run(top, op);
        }
    }

    /// Neovim answered an edit (`gui_diff_edit`): a revert is done (or why not),
    /// or a discard may go on (or why not).
    pub(super) fn diff_edit_done(&mut self, op: &str, path: &str, ok: bool, _message: &str) {
        // (Neovim said why not, in its own messages.)
        let Some(top) = self.diff.as_ref().map(|d| d.top.clone()) else { return };
        match op {
            "guard" => match self.diff_pending.take() {
                Some(edit) if ok => self.run_diff_edit(&top, edit),
                _ => {
                    if let Some(d) = &mut self.diff {
                        d.cancel_edit();
                    }
                }
            },
            "revert" if ok => {
                // A file the agents created and every line of which was put back is removed, as ever.
                if let Some(rel) = self.changes.remove_if_emptied(std::path::Path::new(path)) {
                    let args = vec![RpcValue::from(path), RpcValue::from(rel.as_str())];
                    self.bridge.send(UiCommand::Lua { code: "return Eshvi.changes_removed(...)".into(), args });
                }
                self.changes.refresh();
                if let Some(d) = &mut self.diff {
                    let rel = std::path::Path::new(path).strip_prefix(&top).map(|r| r.to_string_lossy().replace('\\', "/")).ok();
                    d.refresh(rel.as_deref());
                }
            }
            _ => {
                if let Some(d) = &mut self.diff {
                    d.cancel_edit();
                }
            }
        }
    }

    /// Draw the view over the editor area (down to `bottom`) and do what it asks.
    pub(super) fn show_diff(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, rect: Rect, bottom: f32, m: crate::renderer::CellMetrics) {
        let Some(mut view) = self.diff.take() else { return };
        let area = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, bottom.max(rect.min.y)));
        self.diff_rect = Some(area);
        view.poll();
        // The agents' snapshot moved (a file was accepted): read against the new one.
        if view.mode == Mode::Agents {
            if let Some(s) = self.changes.current() {
                view.rebase(&s.base);
            }
        }
        // Neovim's command line, opened from the view, has been and gone: the view has the keyboard again.
        match (self.diff_cmd, self.editor.cmdline.is_empty()) {
            (Some(false), false) => self.diff_cmd = Some(true),
            (Some(true), true) => {
                self.diff_cmd = None;
                if self.focus == Focus::Editor {
                    self.focus = Focus::Diff;
                }
            }
            _ => {}
        }
        let chrome = Chrome::from_editor(&self.editor);
        view.set_coloring(self.lsp.syntax().map(|theme| Coloring { theme, fg: chrome.fg }));
        if let Some(wait) = view.tick(Instant::now()) {
            ctx.request_repaint_after(wait);
        }
        let mut ui_font = self.grid_fonts.sidebar_font_id(false);
        ui_font.size = self.explorer.font_size;
        let fonts = &self.grid_fonts;
        let code = |bold: bool, italic: bool| fonts.font_id(bold, italic);
        let look = Look { chrome, colors: DiffColors::from_editor(&self.editor), ui: ui_font, code: &code, row_h: m.height, char_w: m.width, bar_h: self.tab_height() };
        view.bar_drag = self.scrollbars.dragging();
        let actions = view.show(ui, area, &look, self.focus == Focus::Diff);
        self.diff = Some(view);
        for action in actions {
            self.diff_action(action);
        }
    }
}
