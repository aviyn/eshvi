//! The Outline and Neovim's other tree views in the side bar: what they send,
//! what they ask for, and `:EshviOutline`.

use nvim_bridge::gui::{TreeRegister, TreeSet};

use super::*;
use crate::tree_view::{self, How};

/// What `:EshviOutline` changes: the side bar shown, its view, the Outline
/// unfolded, and who has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OutlineState {
    pub visible: bool,
    pub view: SideView,
    pub open: bool,
    /// The side bar has the keyboard.
    pub focused: bool,
    /// ...and it is the Outline that has it.
    pub in_outline: bool,
}

impl OutlineState {
    fn shown(self) -> bool {
        self.visible && self.view == SideView::Explorer
    }

    /// The Outline unfolded in the Explorer.
    fn shown_open(self) -> Self {
        Self { visible: true, view: SideView::Explorer, open: true, ..self }
    }

    /// Folded: it has no keyboard either.
    fn folded(self) -> Self {
        Self { open: false, focused: self.focused && !self.in_outline, in_outline: false, ..self }
    }

    /// `:EshviOutline <action>`: `show` unfolds it, `hide` folds it, `focus` gives it
    /// the keyboard (again: back to the editor), and `toggle` folds it if it is
    /// showing, else shows it.
    pub fn command(self, action: &str) -> Self {
        match action {
            "show" => self.shown_open(),
            "hide" => self.folded(),
            "focus" if self.shown() && self.in_outline => Self { focused: false, in_outline: false, ..self },
            "focus" => Self { focused: true, in_outline: true, ..self.shown_open() },
            _ if self.shown() && self.open => self.folded(),
            _ => self.shown_open(),
        }
    }
}

impl VimApp {
    /// Neovim sent a tree's rows (`Eshvi.tree.set`).
    pub(super) fn tree_set(&mut self, ctx: &egui::Context, set: TreeSet) {
        self.explorer.trees.entry(set.view.clone()).or_default().set(set);
        ctx.request_repaint();
    }

    /// A tree view registered with keys, or a side bar view of its own.
    pub(super) fn tree_register(&mut self, ctx: &egui::Context, reg: TreeRegister) {
        let tree = self.explorer.trees.entry(reg.view.clone()).or_default();
        tree.set_keys(reg.keys);
        if let Some(side) = reg.side {
            tree.set_hidden(side.filters.iter().filter(|f| f.hidden).map(|f| f.id.clone()));
            self.side_views.register(&reg.view, side);
            self.session_view_registered(&reg.view);
        }
        ctx.request_repaint();
    }

    /// A tree view was unregistered: its button goes, and the side bar shows the
    /// Explorer if it was showing this.
    pub(super) fn tree_unregister(&mut self, ctx: &egui::Context, view: &str) {
        self.explorer.trees.remove(view);
        let before = self.side;
        self.side = self.side_views.unregister(view, self.side);
        if self.side != before {
            self.set_focus(Focus::Editor);
        }
        ctx.request_repaint();
    }

    /// A key in a side bar tree view: its own, or one its source asked for.
    pub(super) fn tree_side_key(&mut self, id: u32, key: egui::Key, mods: egui::Modifiers) {
        let Some(view) = self.side_views.by_id(id).map(|i| i.view.clone()) else { return };
        let actions = self.explorer.trees.get_mut(&view).map(|t| t.key(key, mods)).unwrap_or_default();
        for action in actions {
            match action {
                tree_view::Action::FocusEditor => self.set_focus(Focus::Editor),
                tree_view::Action::Focus => {}
                action => self.tree_action(view.clone(), action),
            }
        }
    }

    /// Neovim selected a node (the Outline follows the cursor).
    pub(super) fn tree_select(&mut self, ctx: &egui::Context, view: &str, id: &str, path: &[String]) {
        if let Some(tree) = self.explorer.trees.get_mut(view) {
            tree.select(id, path);
            ctx.request_repaint();
        }
    }

    pub(super) fn tree_menu(&mut self, ctx: &egui::Context, view: &str, id: &str, items: Vec<String>) {
        if let Some(tree) = self.explorer.trees.get_mut(view) {
            tree.set_menu(id, items);
            ctx.request_repaint();
        }
    }

    /// What a tree view asked of Neovim.
    pub(super) fn tree_action(&mut self, view: String, action: tree_view::Action) {
        use tree_view::Action;
        match action {
            Action::Expand { id, gen } => self.bridge.send(UiCommand::TreeExpand { view, id, gen }),
            Action::Accept { id, how } => {
                self.bridge.send(UiCommand::TreeAccept { view, id, how: how.name().to_owned() });
                // Opened: the keyboard goes to the editor; peeked, it stays.
                if how == How::Open {
                    self.set_focus(Focus::Editor);
                }
            }
            Action::Menu { id } => self.bridge.send(UiCommand::TreeMenu { view, id }),
            Action::Run { id, index } => self.bridge.send(UiCommand::TreeRun { view, id, index }),
            Action::Button { index, selected, expanded } => self.bridge.send(UiCommand::TreeButton { view, index, selected, expanded }),
            Action::Key { id, key } => self.bridge.send(UiCommand::TreeKey { view, id, key }),
            Action::Filter { group, shown } => self.bridge.send(UiCommand::TreeFilter { view, group, shown }),
            Action::Focus | Action::FocusEditor => {}
        }
    }

    /// `[outline]` as Neovim should have it (the pane, Follow Cursor).
    pub(super) fn send_outline_config(&self) {
        let (enabled, follow) = (self.explorer.outline_enabled, self.explorer.outline_follow);
        self.bridge.send(UiCommand::Command(format!("lua Eshvi.outline.configure({{ enabled = {enabled}, follow_cursor = {follow} }})")));
    }

    /// `:EshviOutline <action>`, View › Outline.
    pub(super) fn outline_command(&mut self, action: &str) {
        if !self.explorer_enabled || !self.explorer.outline_enabled {
            return;
        }
        let e = &self.explorer;
        let state = OutlineState { visible: e.visible, view: self.side, open: e.outline_open, focused: e.focused, in_outline: e.outline_has_keyboard() }
            .command(action);
        self.explorer.visible = state.visible;
        self.side = state.view;
        self.explorer.outline_open = state.open;
        if state.in_outline {
            self.explorer.focus_outline();
        } else if !state.focused {
            self.set_focus(Focus::Editor);
        }
    }

    /// The wheel scrolled a Markdown preview of the Outline's file: the
    /// selection follows the heading at its top (the cursor, moving, takes it
    /// back).
    pub(super) fn follow_preview_scroll(&mut self, ctx: &egui::Context) {
        let Some((buf, line)) = self.preview.take_scrolled() else { return };
        if !self.explorer.outline_follow {
            return;
        }
        if let Some(tree) = self.explorer.trees.get_mut("outline") {
            if tree.buf == Some(buf) && tree.select_line(line) {
                ctx.request_repaint();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIDDEN: OutlineState = OutlineState { visible: false, view: SideView::Explorer, open: true, focused: false, in_outline: false };

    #[test]
    fn focus_shows_the_explorer_unfolds_the_pane_and_takes_the_keyboard_and_again_gives_it_back() {
        // From Source Control with the side bar hidden and the pane folded.
        let s = OutlineState { view: SideView::Git, open: false, ..HIDDEN };
        let on = s.command("focus");
        assert_eq!(on, OutlineState { visible: true, view: SideView::Explorer, open: true, focused: true, in_outline: true });
        let back = on.command("focus");
        assert_eq!((back.visible, back.focused, back.in_outline, back.open), (true, false, false, true));
        // The files list having the keyboard: focus moves it to the Outline.
        let files = OutlineState { visible: true, focused: true, ..HIDDEN };
        assert!(files.command("focus").in_outline);
    }

    #[test]
    fn toggle_hide_and_show_fold_and_unfold_without_taking_the_keyboard() {
        let shown = OutlineState { visible: true, ..HIDDEN };
        let folded = shown.command("toggle");
        assert!(!folded.open && folded.visible);
        assert_eq!(folded.command("toggle"), shown, "again: back");
        assert_eq!(shown.command("hide"), folded);
        assert_eq!(folded.command("show"), shown);
        // Not showing (the side bar hidden): toggle shows it, open.
        assert_eq!(HIDDEN.command("toggle"), OutlineState { visible: true, ..HIDDEN });
        // Folding it while it has the keyboard gives the keyboard back.
        let held = OutlineState { visible: true, focused: true, in_outline: true, ..HIDDEN };
        let folded = held.command("hide");
        assert!(!folded.focused && !folded.in_outline);
        // (The files having it stay having it.)
        let files = OutlineState { visible: true, focused: true, ..HIDDEN };
        assert!(files.command("hide").focused);
    }
}
