//! Menu items, GUI actions and explorer commands.

use super::*;

impl VimApp {
    /// The native explorer as a resizable left panel; colors follow the
    /// editor background like the tab strip.
    pub(super) fn show_explorer(&mut self, ui: &mut egui::Ui) {
        if !self.explorer_enabled {
            self.explorer_rect = None;
            return;
        }
        let chrome = Chrome::from_editor(&self.editor);
        let colors = ExplorerColors::from_chrome(&chrome);
        let theme = crate::popup::PopupTheme::from_editor(&self.editor);
        // Fonts become usable one pass after they are installed.
        if !self.grid_fonts.is_live(ui.ctx()) {
            ui.ctx().request_repaint();
            return;
        }
        let frame = egui::Frame::new().fill(colors.bg);
        let mut visible = self.explorer.visible;
        // The two views draw with the same sizes.
        self.scm.font_size = self.explorer.font_size;
        self.scm.agent = self.explorer.agent;
        self.scm.set_root(self.explorer.root());
        if !(visible && self.side == SideView::Git) {
            self.scm.close_dialogs();
        }
        let (side, focused) = (self.side, self.explorer.focused);
        let count = self.scm.change_count();
        let mut panel = egui::Panel::left("eshvi-explorer")
            .resizable(true)
            .default_size(self.explorer_width)
            .size_range(140.0..=800.0)
            .drag_to_open(false)
            .frame(frame);
        // A loaded session's width, once, for as long as the panel is hidden.
        let forced = self.force_side_width;
        if let Some(width) = forced {
            panel = panel.exact_size(width);
        }
        let shown = panel
            .show_collapsible(ui, &mut visible, |ui| {
                let deco_colors = DecoColors::from_editor(&self.editor);
                let mut font = self.grid_fonts.sidebar_font_id(false);
                font.size = self.explorer.font_size;
                let row_h = side_bar::row_height(self.explorer.font_size);
                let extra = match side {
                    SideView::Tree(id) => self.side_views.by_id(id),
                    _ => None,
                };
                let reserved = match side {
                    SideView::Explorer => ExplorerView::header_reserved(row_h),
                    SideView::Git => ScmView::header_reserved(row_h),
                    // (Collapse All, and the source's buttons before it.)
                    SideView::Tree(_) => ExplorerView::header_reserved(row_h) + row_h * extra.map_or(0, |i| i.header_buttons()) as f32,
                };
                let (header, picked) = side_bar::header(ui, side, count, reserved, row_h, &font, &colors, self.side_views.as_slice(), self.explorer.icons());
                match side {
                    SideView::Tree(_) => {
                        let actions = match extra {
                            Some(info) => self.explorer.show_tree_side(ui, header, info, &colors, &self.grid_fonts),
                            None => Vec::new(),
                        };
                        (actions, Vec::new(), picked)
                    }
                    SideView::Explorer => {
                        let actions = self.explorer.show(ui, header, &colors, &theme, &self.grid_fonts, &self.decos, &deco_colors);
                        (actions, Vec::new(), picked)
                    }
                    SideView::Git => {
                        let actions = self.scm.show(ui, header, &colors, &theme, &self.grid_fonts, &deco_colors, focused);
                        (Vec::new(), actions, picked)
                    }
                }
            });
        self.explorer.visible = visible;
        self.explorer_rect = shown.as_ref().map(|r| r.response.rect);
        if let Some(rect) = self.explorer_rect.filter(|_| forced.is_none()) {
            self.side_width = rect.width();
        }
        if shown.is_some() {
            self.force_side_width = None;
        }
        if let Some((explorer_actions, scm_actions, picked)) = shown.map(|r| r.inner) {
            if let Some(view) = picked {
                self.side = view;
                self.session_view = None;
            }
            for action in explorer_actions {
                self.explorer_action(ui.ctx(), action);
            }
            self.diff_opened = false;
            for action in scm_actions {
                self.scm_action(ui.ctx(), action);
            }
            // "View All Changes" (or a file or commit) opened the view: the same click's
            // "focus the side bar" came with it, and the view keeps the keyboard.
            if std::mem::take(&mut self.diff_opened) && self.diff.is_some() {
                self.set_focus(Focus::Diff);
            }
        }
    }

    pub(super) fn explorer_action(&mut self, ctx: &egui::Context, action: explorer_view::Action) {
        use explorer_view::Action;
        match action {
            Action::Open(path) => {
                self.bridge.send(UiCommand::OpenFile { path, split: false });
                self.set_focus(Focus::Editor);
            }
            Action::OpenSplit(path) => {
                self.bridge.send(UiCommand::OpenFile { path, split: true });
                self.set_focus(Focus::Editor);
            }
            Action::FocusEditor => self.set_focus(Focus::Editor),
            Action::SetRoot(path) => {
                self.explorer.set_root(path.clone(), ctx);
                self.bridge.send(UiCommand::Cd(path));
            }
            Action::PickRoot => {
                let start = self.explorer.root().map(|p| p.to_path_buf());
                let mut dialog = rfd::FileDialog::new().set_title("Open Folder");
                if let Some(dir) = start {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(path) = dialog.pick_folder() {
                    self.explorer_action(ctx, Action::SetRoot(path));
                }
            }
            Action::Renamed { from, to } => self.bridge.send(UiCommand::Renamed { from, to }),
            Action::Deleted(path) => self.bridge.send(UiCommand::Deleted(path)),
            Action::CopyText(text) => ctx.copy_text(text),
            Action::Reveal(path) => reveal_in_file_manager(&path),
            Action::SendToAgent(path) => {
                self.set_focus(Focus::Editor);
                let path = path.to_string_lossy();
                self.bridge.send(UiCommand::Lua { code: "return Eshvi.ide.send(...)".into(), args: vec![RpcValue::from(path.as_ref())] });
            }
            Action::Error(message) => self.editor.push_message("emsg", &message),
            Action::Note(message) => self.editor.push_message("wmsg", &message),
            Action::Tree { view, action } => self.tree_action(view, action),
            Action::OutlineFollow(on) => {
                self.explorer.outline_follow = on;
                self.send_outline_config();
            }
        }
    }

    /// Items chosen in the native menu bar.
    pub(super) fn run_menu_commands(&mut self, ctx: &egui::Context) {
        use crate::menu::MenuCommand;
        let Some(bar) = &mut self.menu_bar else { return };
        for command in bar.poll() {
            match command {
                MenuCommand::Quit => self.request_quit(ctx),
                MenuCommand::Settings => self.open_settings(),
                // Edit › Copy etc. while a GUI text field (the explorer's
                // name field, the picker's) has the keyboard: the field gets
                // them, as the keys would without the menu.
                MenuCommand::Neovim { action: Some(action), .. }
                    if crate::menu::is_text_edit(&action)
                        && (self.side_wants_keyboard(ctx) || self.picker.is_open() || self.term_debug_focused() || self.terminal_focused()) =>
                {
                    let key = |key, shift| Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers { mac_cmd: true, command: true, shift, ..Default::default() },
                    };
                    let event = match action.as_str() {
                        "copy" => Event::Copy,
                        "cut" => Event::Cut,
                        // (An image and no text: an empty paste, as for ⌘V.)
                        "paste" => {
                            let Ok(mut clip) = arboard::Clipboard::new() else { continue };
                            match clip.get_text() {
                                Ok(text) if !text.is_empty() => Event::Paste(text),
                                _ if clip.get_image().is_ok() => Event::Paste(String::new()),
                                _ => continue,
                            }
                        }
                        "select_all" => key(egui::Key::A, false),
                        "undo" => key(egui::Key::Z, false),
                        _ => key(egui::Key::Z, true),
                    };
                    ctx.input_mut(|i| i.events.push(event));
                }
                // Edit › Find, Find Next and Find Previous (⌘F, ⌘G, ⇧⌘G) in the terminal
                // window: its own find bar.
                MenuCommand::Neovim { action: Some(action), .. }
                    if self.term_debug_focused() && matches!(action.as_str(), "find" | "find_next" | "find_prev") =>
                {
                    self.term_debug_find(&action);
                }
                // And in the diff view: its find field, and the matches.
                MenuCommand::Neovim { action: Some(action), .. }
                    if self.focus == Focus::Diff && self.diff.is_some() && matches!(action.as_str(), "find" | "find_next" | "find_prev") =>
                {
                    self.diff_find(&action);
                }
                // The same for a terminal of the bottom panel, and ⌘F in Problems: its filter.
                MenuCommand::Neovim { action: Some(action), .. }
                    if (self.focus.in_panel() || self.focus.in_agent()) && matches!(action.as_str(), "find" | "find_next" | "find_prev") =>
                {
                    self.panel_find(&action);
                }
                // Edit › Copy (⌘C) with the message dialog open: copies from it.
                MenuCommand::Neovim { action: Some(action), .. }
                    if action == "copy" && crate::messages::MessageView::dialog_open(&self.editor) =>
                {
                    ctx.input_mut(|i| i.events.push(Event::Copy));
                }
                MenuCommand::Neovim { path, action } => {
                    // Typing goes to Neovim afterwards (Find, Command Line),
                    // except for the side bar's own items.
                    // (The bottom panel's items decide the focus themselves: ⌃` hides the
                    // terminal while it has the keyboard.)
                    let panel_item = self.panel_enabled
                        && matches!(
                            action.as_deref(),
                            Some("panel" | "problems" | "git_output" | "terminal" | "new_terminal" | "next_terminal" | "prev_terminal" | "close_terminal")
                        );
                    if !panel_item && !matches!(action.as_deref(), Some("explorer" | "explorer_view" | "source_control" | "outline")) {
                        self.set_focus(Focus::Editor);
                    }
                    self.bridge.send(UiCommand::Input(crate::menu::emenu_keys(&path)));
                }
            }
        }
    }

    /// What the GUI knows of eshvi's menu items: whether one can be chosen
    /// now (Copy needs a selection) and whether a panel's is shown. The facts
    /// are taken once, here.
    pub(super) fn menu_item_state<'a>(&'a self, ctx: &egui::Context) -> impl Fn(&str) -> Option<crate::menu::ItemState> + 'a {
        use crate::menu::ItemState;
        let dialog = crate::messages::MessageView::dialog_open(&self.editor);
        // A selection to copy: Visual or Select mode, or a GUI text field.
        let selection = self.editor.mode.starts_with("visual")
            || self.side_wants_keyboard(ctx)
            || self.picker.is_open()
            || self.term_debug_has_selection()
            || self.focused_terminal().is_some_and(|v| v.terminal().has_selection())
            || ctx.memory(|m| m.has_focus(problems_filter_id()));
        // The Markdown previews: for a Markdown file (or its preview).
        let cur_win = self.editor.windows.get(&self.editor.cursor.grid).map(|w| w.win);
        let cur_preview = cur_win.and_then(|w| self.preview.win(w));
        let markdown = cur_preview.is_some()
            || self
                .active_file
                .as_ref()
                .and_then(|p| p.extension())
                .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        move |action: &str| {
            Some(match action {
                "explorer" => ItemState { enabled: self.explorer_enabled, checked: Some(self.explorer.visible) },
                "explorer_view" => ItemState {
                    enabled: self.explorer_enabled,
                    checked: Some(self.explorer.visible && self.side == SideView::Explorer),
                },
                "outline" => ItemState {
                    enabled: self.explorer_enabled && self.explorer.outline_enabled,
                    checked: Some(self.explorer.visible && self.side == SideView::Explorer && self.explorer.outline_open),
                },
                "source_control" => ItemState {
                    enabled: self.explorer_enabled,
                    checked: Some(self.explorer.visible && self.side == SideView::Git),
                },
                "panel" => ItemState::shown(self.bottom_shown()),
                "problems" => ItemState::shown(self.bottom.shown && self.bottom.view == bottom_panel::View::Problems),
                "git_output" => ItemState::shown(self.bottom.shown && self.bottom.view == bottom_panel::View::Git),
                "terminal" => ItemState::shown(self.bottom.shown && self.bottom.view == bottom_panel::View::Terminals),
                "agent" => ItemState::shown(self.agent_shown()),
                "cut" => ItemState::enabled(selection),
                "copy" => ItemState::enabled(selection || dialog),
                "next_terminal" | "prev_terminal" => ItemState::enabled(self.bottom.tabs.tabs.len() > 1),
                "close_terminal" => ItemState::enabled(!self.bottom.tabs.tabs.is_empty()),
                "preview" => ItemState { enabled: markdown, checked: Some(cur_preview.is_some_and(|p| !p.side)) },
                "preview_side" => ItemState::enabled(markdown),
                _ => return None,
            })
        }
    }

    /// Grey out eshvi's menu items that can't be used now and check the
    /// panels' items while shown.
    pub(super) fn update_menu_state(&mut self, ctx: &egui::Context) {
        let Some(mut bar) = self.menu_bar.take() else { return };
        bar.update(&self.menu_item_state(ctx));
        self.menu_bar = Some(bar);
    }

    /// Neovim's right-click menu, drawn by the GUI like its other menus
    /// (the explorer's) once Neovim has drawn its own, which then waits for
    /// a choice: a choice clicks that row of Neovim's menu; a press
    /// elsewhere closes it (`handle_input`), so does Neovim (a key).
    pub(super) fn show_popup_menu(&mut self, ctx: &egui::Context) {
        if self.popup_open.as_ref().is_some_and(|p| !self.editor.windows.contains_key(&p.grid)) {
            self.popup_open = None;
        }
        if let Some((items, since)) = &self.popup_menu {
            // Its grid: a float the mouse doesn't focus, a row per item.
            let grid = self
                .editor
                .windows
                .values()
                .find(|w| {
                    matches!(w.kind, WindowKind::Float { mouse_enabled: false, .. })
                        && self.editor.grids.get(&w.grid).is_some_and(|g| g.height() == items.len())
                })
                .map(|w| w.grid);
            match grid {
                Some(grid) => {
                    let pos = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default();
                    let items = self.popup_menu.take().map(|(items, _)| items).unwrap_or_default();
                    self.popup_open = Some(OpenPopup { items, grid, pos, rect: None });
                    // (The right button's release, after the press that
                    // opened it, isn't Neovim's.)
                    if ctx.input(|i| i.pointer.secondary_down()) {
                        self.swallow_release = Some(egui::PointerButton::Secondary);
                    }
                }
                // Not drawn after all (another menu): Neovim's stays.
                None if since.elapsed() > Duration::from_secs(1) => self.popup_menu = None,
                None => {}
            }
        }
        let Some(open) = &mut self.popup_open else { return };
        let id = egui::Id::unique("eshvi-popup-menu");
        let mut chosen = None;
        let shown = egui::Popup::new(id, ctx.clone(), egui::PopupAnchor::Position(open.pos), egui::LayerId::new(egui::Order::Foreground, id))
            .kind(egui::PopupKind::Menu)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .style(egui::containers::menu::menu_style)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .show(|ui| {
                for (row, item) in open.items.iter().enumerate() {
                    if item.is_separator() {
                        ui.separator();
                    } else if ui
                        .add_enabled(!item.disabled, egui::Button::new(item.name.replace('&', "")).shortcut_text(item.key.as_str()))
                        .clicked()
                    {
                        chosen = Some(row);
                    }
                }
            });
        open.rect = shown.map(|r| r.response.rect);
        if let Some(row) = chosen {
            let grid = open.grid;
            self.popup_open = None;
            for action in ["press", "release"] {
                self.send_mouse("left", action, egui::Modifiers::NONE, grid, row as i64, 0);
            }
        }
    }

    /// What a menu item asked the GUI to do (`gui_action`).
    pub(super) fn gui_action(&mut self, ctx: &egui::Context, action: &str) {
        let dir = self
            .active_file
            .as_ref()
            .and_then(|f| f.parent().map(|p| p.to_path_buf()))
            .or_else(|| self.explorer.root().map(|p| p.to_path_buf()));
        match action {
            "open_file" => {
                let mut dialog = rfd::FileDialog::new().set_title("Open");
                if let Some(dir) = &dir {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(path) = dialog.pick_file() {
                    self.bridge.send(UiCommand::OpenFile { path, split: false });
                }
            }
            "open_folder" => self.explorer_action(ctx, explorer_view::Action::PickRoot),
            "save_as" => {
                let mut dialog = rfd::FileDialog::new().set_title("Save As");
                if let Some(dir) = &dir {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(name) = self.active_file.as_ref().and_then(|f| f.file_name()) {
                    dialog = dialog.set_file_name(name.to_string_lossy());
                }
                if let Some(path) = dialog.save_file() {
                    self.bridge.send(UiCommand::Lua { code: "return Eshvi.save_as(...)".into(), args: vec![RpcValue::from(path.to_string_lossy().as_ref())] });
                }
            }
            "zoom_in" => self.set_zoom(ctx, self.zoom + 1.0),
            "zoom_out" => self.set_zoom(ctx, self.zoom - 1.0),
            "zoom_reset" => self.set_zoom(ctx, 0.0),
            other => log::warn!("unknown gui action {other:?}"),
        }
    }
}

/// Show a file or folder in Finder or Explorer (or open its folder
/// elsewhere).
pub(super) fn reveal_in_file_manager(path: &std::path::Path) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg("-R").arg(path).spawn()
    } else if cfg!(windows) {
        let mut select = std::ffi::OsString::from("/select,");
        select.push(path);
        std::process::Command::new("explorer").arg(select).spawn()
    } else {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        std::process::Command::new("xdg-open").arg(dir).spawn()
    };
    if let Err(e) = result {
        log::warn!("reveal {}: {e}", path.display());
    }
}

/// Neovim's right-click menu while the GUI shows it.
pub(super) struct OpenPopup {
    pub(super) items: Vec<nvim_bridge::gui::MenuNode>,
    /// Neovim's own menu, not drawn; a choice clicks its row.
    pub(super) grid: u64,
    /// Where it opened (the pointer).
    pub(super) pos: Pos2,
    /// Where it is (last drawn): presses there are its.
    pub(super) rect: Option<Rect>,
}
