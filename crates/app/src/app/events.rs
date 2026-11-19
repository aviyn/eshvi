//! Keyboard and mouse input: what goes to Neovim and what the GUI takes.

use super::*;
use crate::scrollbar::Bar;

/// Rows a mouse's wheel notch scrolls ('mousescroll''s default).
const WHEEL_NOTCH_ROWS: f32 = 3.0;

/// The rows of `pending` to scroll this frame (at least one while there's a
/// whole row, a quarter of a long way), and what's left.
fn wheel_step(pending: f32) -> (i64, f32) {
    let whole = pending.trunc();
    if whole == 0.0 {
        return (0, pending);
    }
    let step = (whole.abs() / 4.0).ceil().copysign(whole);
    (step as i64, pending - step)
}

/// Whether Neovim waits for more keys: an operator for its motion, or a
/// mapping's next key (shown in 'showcmd'; which-key's popup is up then).
fn waits_for_keys(editor: &grid::Editor) -> bool {
    !matches!(editor.mode.as_str(), "normal" | "") || editor.showcmd.iter().any(|c| !c.text.trim().is_empty())
}

impl VimApp {
    /// Scroll what `bar` is on so that line `top` (0-based) is first in view.
    pub(super) fn scroll_bar_to(&mut self, bar: Bar, top: i64) {
        match bar {
            Bar::Diff => {
                if let Some(d) = &mut self.diff {
                    d.scroll_to(top as f32);
                }
            }
            Bar::Terminal(id) => {
                let views = self.bottom.views.values_mut().chain(self.agents.views.values_mut());
                if let Some(v) = views.into_iter().find(|v| v.id() == id) {
                    v.scroll_to_line(top);
                }
            }
            Bar::Win(win) if self.preview.win(win).is_some() => self.preview.scroll_to(win, top),
            Bar::Win(win) => {
                self.gui_scrolled = Some(Instant::now());
                self.bridge.send(UiCommand::Command(format!("lua Eshvi.scroll_to({win}, {})", top + 1)));
            }
        }
    }

    /// A primary press at `pos`: whether a scroll bar took it (it scrolls
    /// what it is on to where the thumb is grabbed).
    pub(super) fn scrollbar_press(&mut self, pos: Pos2) -> bool {
        let (taken, scroll) = self.scrollbars.press(pos);
        if let Some((bar, top)) = scroll {
            self.scroll_bar_to(bar, top);
        }
        taken
    }

    /// The wheel's rows still to scroll a file window, sent a few at a time
    /// each frame: steps at the frame rate glide evenly, as when dragging
    /// its scroll bar. (A step of many rows, then a wait, shows the current
    /// line jumping and gliding back each time.)
    pub(super) fn pump_wheel(&mut self, ctx: &egui::Context) {
        let Some((win, pending)) = self.wheel_rows else { return };
        let (rows, left) = wheel_step(pending);
        if rows != 0 {
            self.gui_scrolled = Some(Instant::now());
            self.bridge.send(UiCommand::Command(format!("lua Eshvi.scroll_by({win}, {rows})")));
        }
        self.wheel_rows = Some((win, left));
        if left.abs() >= 1.0 {
            ctx.request_repaint();
        }
    }

    /// Whether the wheel over window `w` scrolls it by the scroll bar's path
    /// (`Eshvi.scroll_by`): a file's window.
    pub(super) fn scrolls_by_wheel(&self, w: &grid::Window) -> bool {
        w.kind == WindowKind::Normal && self.win_paths.contains_key(&w.win)
    }

    /// Grid and grid-relative cell under a point.
    pub(super) fn grid_cell_at(&self, pos: Pos2) -> Option<(u64, i64, i64)> {
        // A float drawn away from its cells (under the command line).
        let m = self.renderer.metrics();
        for (&grid, &d) in &self.float_offsets {
            let Some(w) = self.editor.windows.get(&grid) else { continue };
            let (r0, c0) = self.editor.grid_origin(grid);
            let min = self.origin + Vec2::new(c0 as f32 * m.width, r0 as f32 * m.height) + d;
            if Rect::from_min_size(min, Vec2::new(w.width as f32 * m.width, w.height as f32 * m.height)).contains(pos) {
                let p = pos - min;
                return Some((grid, (p.y / m.height).floor() as i64, (p.x / m.width).floor() as i64));
            }
        }
        // (In the margin before the first column: that column.)
        let (row, col) = self.cell_at(pos);
        let (grid, r, c) = self.editor.grid_at(row, col.max(0))?;
        Some((grid, r as i64, self.col_in_grid(grid, r as i64, pos, c as i64)))
    }

    /// Cell of `grid` under a point (may be outside it).
    pub(super) fn cell_in(&self, grid: u64, pos: Pos2) -> (i64, i64) {
        let m = self.renderer.metrics();
        let (r0, c0) = self.editor.grid_origin(grid);
        let p = pos - (self.origin + Vec2::new(c0 as f32 * m.width, r0 as f32 * m.height));
        let row = (p.y / m.height).floor() as i64;
        (row, self.col_in_grid(grid, row, pos, (p.x / m.width).floor() as i64))
    }

    /// Global cell under a point (may be outside the grid).
    pub(super) fn cell_at(&self, pos: Pos2) -> (i64, i64) {
        let m = self.renderer.metrics();
        let p = pos - self.origin;
        ((p.y / m.height).floor() as i64, (p.x / m.width).floor() as i64)
    }

    /// An egui menu is open and on top at `pos`.
    pub(super) fn over_menu(&self, ctx: &egui::Context, pos: Pos2) -> bool {
        egui::Popup::is_any_open(ctx) && ctx.layer_id_at(pos).is_some_and(|l| l.order == egui::Order::Foreground)
    }

    pub(super) fn over_popup(&self, ctx: &egui::Context, pos: Pos2) -> bool {
        // An egui menu open (the explorer's "⋯", the Outline's) is on top of what is
        // at `pos`, which may be the editor: a click on it is the menu's.
        self.over_menu(ctx, pos)
            || self.popup_rects.iter().any(|r| r.contains(pos))
            || self.over_popup_menu(pos)
            || self.over_explorer(pos)
            || (self.explorer.visible && self.side == SideView::Git && self.scm.over_tooltip(pos))
    }

    /// An in-place Markdown preview has the keyboard, and Neovim waits for
    /// nothing (no operator, no keys of a mapping pending: which-key's
    /// popup is Esc's then).
    fn in_place_preview_focused(&self) -> bool {
        let win = self.editor.windows.get(&self.editor.cursor.grid).map(|w| w.win);
        !waits_for_keys(&self.editor) && win.and_then(|w| self.preview.win(w)).is_some_and(|p| !p.side)
    }

    fn over_popup_menu(&self, pos: Pos2) -> bool {
        self.popup_open.as_ref().and_then(|p| p.rect).is_some_and(|r| r.contains(pos))
    }

    pub(super) fn over_explorer(&self, pos: Pos2) -> bool {
        self.explorer.visible && self.explorer_rect.is_some_and(|r| r.contains(pos))
    }

    /// Grid-relative column under `pos`, using the renderer's layout for
    /// proportional (sidebar) rows. `fallback` is the cell-grid column.
    pub(super) fn col_in_grid(&self, grid: u64, row: i64, pos: Pos2, fallback: i64) -> i64 {
        let (_, c0) = self.editor.grid_origin(grid);
        let x = pos.x - self.origin.x - c0 as f32 * self.renderer.metrics().width;
        usize::try_from(row)
            .ok()
            .and_then(|row| self.renderer.col_at(grid, row, x))
            .map_or(fallback, |c| c as i64)
    }

    pub(super) fn send_mouse(&self, button: &str, action: &str, mods: egui::Modifiers, grid: u64, row: i64, col: i64) {
        self.bridge.send(UiCommand::InputMouse {
            button: button.into(),
            action: action.into(),
            modifier: input::mouse_modifiers(mods),
            grid,
            row: row.max(0) as usize,
            col: col.max(0) as usize,
        });
    }

    pub(super) fn handle_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let mut keys = String::new();
        let mut typed = false;
        // A key the side bar took, which answers a message as one for Neovim does.
        let mut side_typed = false;
        let mut flush_keys = |keys: &mut String, bridge: &Bridge| {
            if !keys.is_empty() {
                typed = true;
                bridge.send(UiCommand::Input(std::mem::take(keys)));
            }
        };
        self.keys.begin();
        let dialog_open = crate::messages::MessageView::dialog_open(&self.editor);
        for ev in events {
            // The picker has the keyboard and the mouse: nothing reaches Neovim
            // (or anything else) while it is open. A press outside it closes
            // it, and is only that.
            if self.picker.is_open() {
                match &ev {
                    Event::PointerButton { pos, pressed: true, button, .. } => {
                        if !self.picker.rect().is_some_and(|r| r.contains(*pos)) {
                            self.cancel_picker();
                            self.swallow_release = Some(*button);
                        }
                    }
                    Event::PointerButton { button, pressed: false, .. } if self.swallow_release == Some(*button) => {
                        self.swallow_release = None;
                        self.drag = None;
                    }
                    _ => {}
                }
                if crate::picker::takes_input(&ev) {
                    continue;
                }
            }
            // The terminal window (`:EshviTermDebug`): its keys and, over it, the
            // pointer are its own, drawn by its view. A press away from it ends
            // its focus (and is the editor's as usual).
            if let Some(d) = &mut self.term_debug {
                let inside = |p: &Pos2| d.rect.is_some_and(|r| r.contains(*p));
                match &ev {
                    Event::Key { .. } | Event::Text(_) | Event::Ime(_) | Event::Paste(_) | Event::Copy | Event::Cut if d.focused => continue,
                    Event::PointerButton { pos, pressed: true, .. } => {
                        let hit = inside(pos);
                        d.focused = hit;
                        d.pressed = hit;
                        if hit {
                            self.set_focus(Focus::Editor);
                            if let Some(d) = self.term_debug.as_mut() {
                                d.focused = true;
                                d.pressed = true;
                            }
                            continue;
                        }
                    }
                    Event::PointerButton { pressed: false, .. } if std::mem::take(&mut d.pressed) => continue,
                    Event::PointerButton { pos, .. } | Event::PointerMoved(pos) if inside(pos) => continue,
                    Event::MouseWheel { .. } if ctx.input(|i| i.pointer.latest_pos()).is_some_and(|p| inside(&p)) => continue,
                    Event::PointerMoved(_) if d.pressed => continue,
                    _ => {}
                }
            }
            // Scrollbars take presses on them, and the drags that follow (a drag
            // in progress has the pointer, over whatever it passes).
            match &ev {
                Event::PointerButton { pos, pressed: true, button: PointerButton::Primary, .. } if !self.over_menu(ctx, *pos) => {
                    if self.scrollbar_press(*pos) {
                        continue;
                    }
                }
                Event::PointerMoved(pos) if self.scrollbars.dragging() => {
                    if let Some((bar, top)) = self.scrollbars.drag_to(*pos) {
                        self.scroll_bar_to(bar, top);
                    }
                    continue;
                }
                Event::PointerButton { pressed: false, .. } if self.scrollbars.dragging() => {
                    self.scrollbars.release();
                    continue;
                }
                _ => {}
            }
            // The bottom panel: its keys (while it has the keyboard) and the pointer
            // over it are its own, and ⌃h/j/k/l from it go to the next region.
            if self.panel_enabled {
                // (Keys typed before a click in it are Neovim's, first.)
                if matches!(&ev, Event::PointerButton { pos, pressed: true, .. } if self.over_bottom_panel(*pos)) {
                    flush_keys(&mut keys, &self.bridge);
                }
                // A press elsewhere ends the panel's keyboard (and is the editor's, as usual).
                if let Event::PointerButton { pos, pressed: true, .. } = &ev {
                    if self.focus.in_panel() && !self.over_bottom_panel(*pos) && !self.over_popup(ctx, *pos) {
                        self.set_focus(Focus::Editor);
                    }
                }
                if self.panel_event(ctx, &ev) {
                    continue;
                }
            }
            // The agent panel, the same way: its keys while it has the keyboard, the
            // pointer over it, and ⌃h/j/k/l from it to the next region.
            if self.config.agent.enabled {
                if matches!(&ev, Event::PointerButton { pos, pressed: true, .. } if self.over_agent(*pos)) {
                    flush_keys(&mut keys, &self.bridge);
                }
                if let Event::PointerButton { pos, pressed: true, .. } = &ev {
                    if self.focus.in_agent() && !self.over_agent(*pos) && !self.over_popup(ctx, *pos) {
                        self.set_focus(Focus::Editor);
                    }
                }
                if self.agent_event(ctx, &ev) {
                    continue;
                }
            }
            // The diff view over the editor: its keys while it has the keyboard,
            // and the pointer over it.
            if matches!(&ev, Event::PointerButton { pos, pressed: true, .. } if self.diff.is_some() && self.over_diff(*pos)) {
                flush_keys(&mut keys, &self.bridge);
            }
            if self.diff_event(ctx, &ev) {
                continue;
            }
            // The message dialog copies this frame, in `MessageView::show`:
            // not a key that would close it first.
            if dialog_open && crate::messages::is_copy(&ev) {
                continue;
            }
            // The Changes popup closes on a click elsewhere (the click
            // still counts) and on Esc (which doesn't reach Neovim).
            if self.changes.popup {
                match &ev {
                    Event::PointerButton { pos, pressed: true, .. }
                        if !self.changes_rect.is_some_and(|r| r.contains(*pos))
                            && !self.agent_strip.is_some_and(|r| r.contains(*pos)) =>
                    {
                        self.changes.popup = false;
                    }
                    Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
                        self.changes.popup = false;
                        continue;
                    }
                    _ => {}
                }
            }
            match ev {
                // (With the menu bar its View items take ⌘=, ⌘- and ⌘0 and
                // ⌘B, unless mapped in Neovim, which then gets them.)
                Event::Key { key, pressed: true, modifiers, .. }
                    if (modifiers.ctrl || (modifiers.mac_cmd && self.menu_bar.is_none()))
                        && matches!(key, egui::Key::Equals | egui::Key::Plus | egui::Key::Minus | egui::Key::Num0) =>
                {
                    let zoom = match key {
                        egui::Key::Minus => self.zoom - 1.0,
                        egui::Key::Num0 => 0.0,
                        _ => self.zoom + 1.0,
                    };
                    self.set_zoom(ctx, zoom);
                }
                // VS Code shortcuts: Cmd+B toggles the side bar, Cmd+Shift+E
                // focuses the explorer.
                Event::Key { key: egui::Key::B, pressed: true, modifiers, .. }
                    if modifiers.mac_cmd && !modifiers.shift && self.explorer_enabled && self.menu_bar.is_none() =>
                {
                    self.explorer_command("toggle");
                }
                Event::Key { key: egui::Key::E, pressed: true, modifiers, .. }
                    if modifiers.mac_cmd && modifiers.shift && self.explorer_enabled =>
                {
                    let focused = self.explorer.focused && self.explorer.visible && self.side == SideView::Explorer;
                    self.explorer_command(if focused { "hide" } else { "focus" });
                }
                // Ctrl+Shift+G does the same for Source Control (with the
                // menu bar, its View item does).
                Event::Key { key: egui::Key::G, pressed: true, modifiers, .. }
                    if modifiers.ctrl
                        && modifiers.shift
                        && !modifiers.mac_cmd
                        && !modifiers.alt
                        && self.explorer_enabled
                        && self.menu_bar.is_none() =>
                {
                    let focused = self.explorer.focused && self.explorer.visible && self.side == SideView::Git;
                    self.git_command(if focused { "hide" } else { "focus" });
                }
                // While the explorer has focus, keys navigate it.
                // An inline name field or a dialog in the explorer is being
                // used: keys go to it (egui), not to Neovim.
                Event::Key { .. } | Event::Text(_) | Event::Ime(_) | Event::Paste(_)
                    if self.side_wants_keyboard(ctx) || ctx.memory(|m| m.has_focus(problems_filter_id())) =>
                {
                    side_typed |= matches!(ev, Event::Key { pressed: true, .. } | Event::Text(_));
                }
                // ⌃h/j/k/l in the side bar: to the next region.
                Event::Key { key, pressed, modifiers, .. } if self.focus == Focus::Side && self.takes_navigation(key, modifiers).is_some() => {
                    if let (true, Some(dir)) = (pressed, self.takes_navigation(key, modifiers)) {
                        self.navigate(dir);
                    }
                    side_typed |= pressed;
                }
                Event::Key { key, pressed: true, modifiers, .. } if self.explorer.focused => {
                    // (A message waiting for a key is closed by this one, as
                    // Esc does everywhere.)
                    side_typed = true;
                    if key == egui::Key::Escape {
                        self.msgs.dismiss_all(&mut self.editor);
                    }
                    match self.side {
                        SideView::Explorer => {
                            if let Some(action) = self.explorer.key(key, modifiers) {
                                self.explorer_action(ctx, action);
                            }
                        }
                        SideView::Git => {
                            if let Some(action) = self.scm.key(key, modifiers) {
                                self.scm_action(ctx, action);
                            }
                        }
                        SideView::Tree(id) => self.tree_side_key(id, key, modifiers),
                    }
                }
                Event::Text(_) | Event::Ime(_) | Event::Paste(_) if self.focus == Focus::Side => side_typed = true,
                // Ctrl+F/B scroll a completion item's documentation while
                // it is taller than its popup (as nvim-cmp and blink.cmp do).
                Event::Key { key: key @ (egui::Key::F | egui::Key::B), pressed: true, modifiers, .. }
                    if modifiers.ctrl && !modifiers.shift && self.editor.popupmenu.is_some() && self.pum.doc_scrolls() =>
                {
                    self.pum.scroll_doc(if key == egui::Key::F { 1.0 } else { -1.0 });
                    ctx.request_repaint();
                }
                // Esc in a Markdown preview shown in place: back to the text.
                Event::Key { key: egui::Key::Escape, pressed: true, modifiers, .. }
                    if !modifiers.any() && self.in_place_preview_focused() =>
                {
                    flush_keys(&mut keys, &self.bridge);
                    self.bridge.send(UiCommand::Command("EshviPreview off".into()));
                }
                Event::Key { key, physical_key, pressed: true, modifiers, .. } => {
                    if key == egui::Key::Escape {
                        // Dismissed: not shown again where the cursor is.
                        if self.lsp.has_diagnostics() {
                            self.bridge.send(UiCommand::Command(
                                "lua pcall(function() require('gui_bridge').dismiss_diagnostics() end)".into(),
                            ));
                        }
                        self.lsp.close_all();
                        self.msgs.dismiss_all(&mut self.editor);
                    }
                    if let Some(s) = self.keys.key(key, physical_key, modifiers) {
                        keys.push_str(&s);
                    }
                }
                Event::Text(text) => {
                    if let Some(s) = self.keys.text(&text) {
                        keys.push_str(&s);
                    }
                }
                Event::Ime(ImeEvent::Commit(text)) => keys.push_str(&input::escape_text(&text)),
                Event::Copy => keys.push_str("<D-c>"),
                Event::Cut => keys.push_str("<D-x>"),
                Event::Paste(text) if text.is_empty() => {}
                Event::Paste(text) => {
                    flush_keys(&mut keys, &self.bridge);
                    self.bridge.send(UiCommand::Paste(text));
                }
                // A press away from Neovim's right-click menu closes it
                // (Neovim's waits for a choice: Esc), and is only that.
                Event::PointerButton { pos, pressed: true, button, .. }
                    if self.popup_open.is_some() && !self.over_popup_menu(pos) =>
                {
                    flush_keys(&mut keys, &self.bridge);
                    self.popup_open = None;
                    self.swallow_release = Some(button);
                    self.bridge.send(UiCommand::Input("<Esc>".into()));
                }
                Event::PointerButton { button, pressed: false, .. } if self.swallow_release == Some(button) => {
                    self.swallow_release = None;
                    self.drag = None;
                }
                // A Markdown file's preview buttons: for that window.
                Event::PointerButton { pos, pressed, button: PointerButton::Primary, .. }
                    if self.winbar_buttons.iter().any(|(r, ..)| r.contains(pos)) =>
                {
                    if let (true, Some(&(_, win, b))) = (pressed, self.winbar_buttons.iter().find(|(r, ..)| r.contains(pos))) {
                        flush_keys(&mut keys, &self.bridge);
                        self.set_focus(Focus::Editor);
                        let command = b.command();
                        self.bridge.send(UiCommand::Command(format!(
                            "lua if pcall(vim.api.nvim_set_current_win, {win}) then vim.cmd('{command}') end"
                        )));
                    }
                }
                // A window file tab's "×", or a middle click on the tab:
                // close the window, as :q.
                Event::PointerButton { pos, pressed: true, button, .. }
                    if self.winbar_tabs.iter().any(|&(tab, close, _, _)| match button {
                        PointerButton::Primary => close.contains(pos),
                        PointerButton::Middle => tab.contains(pos),
                        _ => false,
                    }) =>
                {
                    flush_keys(&mut keys, &self.bridge);
                    if let Some(&(_, _, win, _)) = self.winbar_tabs.iter().find(|(tab, ..)| tab.contains(pos)) {
                        self.bridge.send(UiCommand::Command(format!("lua Eshvi.close_window({win})")));
                    }
                }
                Event::PointerButton { pos, pressed: false, .. }
                    if self.winbar_tabs.iter().any(|&(_, close, _, _)| close.contains(pos)) => {}
                // A tab in a separator row focuses its window (elsewhere in
                // that row, a press is Neovim's: dragging resizes).
                Event::PointerButton { pos, pressed, button: PointerButton::Primary, .. }
                    if self.winbar_tabs.iter().any(|&(tab, _, _, above)| above && tab.contains(pos)) =>
                {
                    if pressed {
                        flush_keys(&mut keys, &self.bridge);
                        self.set_focus(Focus::Editor);
                        if let Some(&(_, _, win, _)) = self.winbar_tabs.iter().find(|(tab, ..)| tab.contains(pos)) {
                            self.bridge.send(UiCommand::Command(format!("lua pcall(vim.api.nvim_set_current_win, {win})")));
                        }
                    }
                }
                // The status bar's problems indicator toggles the panel's
                // Problems view (or, without the panel, opens the list).
                Event::PointerButton { pos, pressed: true, button: PointerButton::Primary, .. }
                    if self.problems_rect.is_some_and(|r| r.contains(pos)) =>
                {
                    flush_keys(&mut keys, &self.bridge);
                    self.set_focus(Focus::Editor);
                    let command =
                        if self.panel_enabled { "EshviProblems toggle" } else { "lua vim.diagnostic.setqflist()" };
                    self.bridge.send(UiCommand::Command(command.into()));
                }
                Event::PointerButton { pos, pressed: false, .. } if self.problems_rect.is_some_and(|r| r.contains(pos)) => {}
                // The native status bar's branch shows Source Control.
                Event::PointerButton { pos, pressed: true, button: PointerButton::Primary, .. }
                    if self.branch_rect.is_some_and(|r| r.contains(pos)) =>
                {
                    flush_keys(&mut keys, &self.bridge);
                    self.git_command("show");
                }
                Event::PointerButton { pos, pressed: false, .. } if self.branch_rect.is_some_and(|r| r.contains(pos)) => {}
                Event::PointerButton { pos, pressed: true, .. } if self.over_popup(ctx, pos) => {}
                // A Markdown preview: egui takes the mouse (links, text);
                // a press also makes its window the current one.
                // (Not a separator's reach, which is Neovim's, nor the end of a
                // drag, wherever it ends.)
                Event::PointerButton { pos, pressed, button, .. }
                    if self.preview_at(pos).is_some()
                        && if pressed { self.separator_grab(pos).is_none() } else { self.drag.is_none() } =>
                {
                    if let (true, PointerButton::Primary, Some(win)) = (pressed, button, self.preview_at(pos)) {
                        flush_keys(&mut keys, &self.bridge);
                        self.set_focus(Focus::Editor);
                        self.bridge.send(UiCommand::Command(format!("lua pcall(vim.api.nvim_set_current_win, {win})")));
                    }
                }
                // Clicking the editor gives it keyboard focus back.
                Event::PointerButton { pressed: true, .. } if self.focus == Focus::Side => {
                    self.set_focus(Focus::Editor);
                }
                Event::PointerButton { pos, button, pressed, modifiers } if self.editor.mouse_enabled => {
                    flush_keys(&mut keys, &self.bridge);
                    let name = match button {
                        PointerButton::Primary => "left",
                        PointerButton::Secondary => "right",
                        PointerButton::Middle => "middle",
                        _ => continue,
                    };
                    if pressed {
                        // On a separator or the pixels beside it: Neovim sees
                        // the press on its cell, and the drag follows from there.
                        let (at, shift) = match self.separator_grab(pos).filter(|_| button == PointerButton::Primary) {
                            Some((_, cell)) => (cell, cell - pos),
                            None => (pos, Vec2::ZERO),
                        };
                        if let Some((grid, r, c)) = self.grid_cell_at(at) {
                            // One button is followed at a time: another
                            // pressed meanwhile ends the first one's drag
                            // under its own name.
                            if let Some(d) = self.drag.take().filter(|d| d.button != name) {
                                let (r, c) = self.cell_in(d.grid, pos + d.shift);
                                self.send_mouse(d.button, "release", modifiers, d.grid, r, c);
                            }
                            self.send_mouse(name, "press", modifiers, grid, r, c);
                            let still = grid == DEFAULT_GRID && usize::try_from(r).ok() == self.last_status_row();
                            self.drag = Some(Drag { button: name, grid, cell: (r, c), shift, still });
                        }
                    } else if self.drag.as_ref().is_some_and(|d| d.button == name) {
                        // (Not the release of a button whose drag has ended already.)
                        if let Some(d) = self.drag.take() {
                            let (r, c) = self.cell_in(d.grid, pos + d.shift);
                            self.send_mouse(d.button, "release", modifiers, d.grid, r, c);
                        }
                    }
                }
                Event::PointerMoved(pos) => {
                    let Some((grid, shift)) = self.drag.as_ref().filter(|d| !d.still).map(|d| (d.grid, d.shift)) else { continue };
                    let cell = self.cell_in(grid, pos + shift);
                    if let Some(d) = self.drag.as_mut().filter(|d| d.cell != cell) {
                        d.cell = cell;
                        let button = d.button;
                        flush_keys(&mut keys, &self.bridge);
                        let mods = ctx.input(|i| i.modifiers);
                        self.send_mouse(button, "drag", mods, grid, cell.0, cell.1);
                    }
                }
                Event::MouseWheel { unit, delta, modifiers, .. } if self.editor.mouse_enabled => {
                    flush_keys(&mut keys, &self.bridge);
                    let m = self.renderer.metrics();
                    let lines = match unit {
                        MouseWheelUnit::Point => Vec2::new(delta.x / m.width, delta.y / m.height),
                        MouseWheelUnit::Line => delta,
                        MouseWheelUnit::Page => delta * self.requested_size.1 as f32,
                    };
                    self.wheel += lines;
                    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { continue };
                    if self.over_popup(ctx, pos) || self.preview_at(pos).is_some() {
                        // egui scrolls the popup (or preview) under the pointer.
                        self.wheel = Vec2::ZERO;
                        continue;
                    }
                    let Some((grid, r, c)) = self.grid_cell_at(pos) else { continue };
                    // Over a file (or a side of the changes diff): by the
                    // scroll bar's path, one row of text per row of wheel (a
                    // mouse's notch: 'mousescroll''s default). Elsewhere
                    // (terminals, lists): Neovim's wheel.
                    let file = self.editor.windows.get(&grid).filter(|w| self.scrolls_by_wheel(w));
                    if let Some(w) = file {
                        let win = w.win;
                        let rows = if unit == MouseWheelUnit::Line { self.wheel.y * WHEEL_NOTCH_ROWS } else { self.wheel.y };
                        self.wheel.y = 0.0;
                        let pending = self.wheel_rows.filter(|&(w, _)| w == win).map_or(0.0, |(_, p)| p);
                        // Down the sheet is up the file.
                        self.wheel_rows = Some((win, pending - rows));
                    }
                    while self.wheel.y.abs() >= 1.0 {
                        let dir = if self.wheel.y > 0.0 { "up" } else { "down" };
                        self.wheel.y -= self.wheel.y.signum();
                        self.send_mouse("wheel", dir, modifiers, grid, r, c);
                    }
                    while self.wheel.x.abs() >= 1.0 {
                        let dir = if self.wheel.x > 0.0 { "left" } else { "right" };
                        self.wheel.x -= self.wheel.x.signum();
                        self.send_mouse("wheel", dir, modifiers, grid, r, c);
                    }
                }
                _ => {}
            }
        }
        flush_keys(&mut keys, &self.bridge);
        if typed || side_typed {
            self.msgs.on_input(&mut self.editor);
        }
        self.pump_wheel(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::{waits_for_keys, wheel_step};

    #[test]
    fn wheel_rows_go_a_few_at_a_time() {
        // Less than a row waits for more.
        assert_eq!(wheel_step(0.6), (0, 0.6));
        // A row or a few: one a frame.
        assert_eq!(wheel_step(3.5), (1, 2.5));
        assert_eq!(wheel_step(-3.0), (-1, -2.0));
        // A long way: a quarter of it.
        assert_eq!(wheel_step(40.0), (10, 30.0));
    }

    /// Esc over an in-place preview leaves it only when Neovim waits for
    /// nothing; else it's Neovim's (an operator, which-key's popup).
    #[test]
    fn neovim_waits_for_keys_in_an_operator_or_with_keys_pending() {
        let mut ed = grid::Editor::new();
        ed.mode = "normal".into();
        assert!(!waits_for_keys(&ed));
        ed.mode = "operator".into();
        assert!(waits_for_keys(&ed), "after `c`: its motion");
        ed.mode = "normal".into();
        ed.showcmd = vec![grid::StyledChunk { style: grid::ChunkStyle::Id(0), text: "\"".into() }];
        assert!(waits_for_keys(&ed), "a register, or a mapping's prefix");
        ed.showcmd[0].text.clear();
        assert!(!waits_for_keys(&ed));
    }
}
