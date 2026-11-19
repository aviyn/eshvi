//! The native picker in the window: opening it, drawing it over the editor
//! area, and what it asks for.

use super::*;
use crate::picker::{Action, Look, Subject};

impl VimApp {
    /// Neovim opened a picker (`picker.lua`).
    pub(super) fn open_picker(&mut self, ctx: &egui::Context, open: nvim_bridge::gui::PickerOpen) {
        self.set_focus(Focus::Editor);
        // No popup of the editor's stays over the box.
        self.lsp.close_all();
        // The palette lists the menu's items that are not greyed out now.
        let greyed: std::collections::HashSet<String> = {
            let state = self.menu_item_state(ctx);
            open.items
                .iter()
                .filter_map(|i| i.gate.clone())
                .filter(|action| state(action).is_some_and(|s| !s.enabled))
                .collect()
        };
        let enabled = |item: &nvim_bridge::gui::PickerItem| item.gate.as_ref().is_none_or(|a| !greyed.contains(a));
        self.picker.open_gated(ctx, open, &enabled);
    }

    /// Draw the picker; answer Neovim with what was chosen, or that nothing was.
    pub(super) fn show_picker(&mut self, ctx: &egui::Context, area: Rect, metrics: crate::renderer::CellMetrics) {
        if !self.picker.is_open() {
            return;
        }
        // A prompt (a question for the keyboard, `:messages`) comes first.
        if MessageView::dialog_open(&self.editor) {
            self.cancel_picker();
            return;
        }
        let theme = crate::popup::PopupTheme::from_editor(&self.editor);
        let chrome = Chrome::from_editor(&self.editor);
        let code = crate::picker::CodeTheme::from_editor(&self.editor, theme.accent);
        let syntax = self.lsp.syntax();
        let look = Look {
            theme: &theme,
            chrome: &chrome,
            fonts: &self.grid_fonts,
            metrics,
            area,
            code: &code,
            problems: self.problem_colors(),
            preview: self.config.picker.preview,
            syntax: syntax.as_deref(),
        };
        let actions = self.picker.show(ctx, &look, &mut self.icons);
        if let Some(rect) = self.picker.rect() {
            self.popup_rects.push(rect);
        }
        let mut answered = false;
        for action in actions {
            match action {
                Action::Opened(id) => {
                    if self.picker.id() == Some(id) {
                        self.bridge.send(UiCommand::PickerOpened(id));
                    }
                    continue;
                }
                Action::Selected { id, idx } => {
                    if !answered && self.picker.id() == Some(id) {
                        self.bridge.send(UiCommand::PickerSelected { id, idx });
                    }
                    continue;
                }
                Action::Query { id, gen, text } => {
                    if !answered && self.picker.id() == Some(id) {
                        self.bridge.send(UiCommand::PickerQuery { id, gen, text });
                    }
                    continue;
                }
                Action::Preview { id, request } => {
                    // (Not while a choice is being made: the picker is closing.)
                    if !answered && self.picker.id() == Some(id) {
                        let (path, buf) = match request.key {
                            Subject::Path(p) => (Some(p), None),
                            Subject::Buf(b) => (None, Some(b)),
                        };
                        self.bridge.send(UiCommand::PickerPreview { req: request.req, path, buf, lnum: request.lnum });
                    }
                    continue;
                }
                // (Enter and Esc together in a frame: the first wins.)
                _ if answered => continue,
                Action::Accept { id, chosen, how } => {
                    let state = self.picker.state();
                    self.picker.close();
                    self.bridge.send(UiCommand::PickerAccept { id, chosen, how: how.into(), state });
                }
                Action::Cancel(_) => self.cancel_picker(),
            }
            answered = true;
        }
    }

    /// Close the picker; Neovim hears that nothing was chosen.
    pub(super) fn cancel_picker(&mut self) {
        if let Some(id) = self.picker.id() {
            let state = self.picker.state();
            self.picker.close();
            self.bridge.send(UiCommand::PickerCancel { id, state });
        }
    }
}

/// Whether the editor's popups (hover, signature, a problem) are drawn this
/// frame. Not while the picker is open: Neovim sits resting under it and a
/// `CursorHold` would bring one over the box. What arrives meanwhile is
/// dropped, so nothing stale comes back when it closes.
pub(super) fn editor_popups_shown(picker_open: bool, lsp: &mut crate::lsp_popups::LspPopups) -> bool {
    if picker_open {
        lsp.close_all();
        return false;
    }
    !lsp.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_popups::LspPopups;
    use nvim_bridge::gui::{Diagnostic, GuiEvent, PopupKind, Severity};

    fn hover() -> GuiEvent {
        GuiEvent::Hover { contents: "docs".into(), kind: "markdown".into() }
    }

    fn problem() -> GuiEvent {
        GuiEvent::Diagnostic(vec![Diagnostic { message: "bad".into(), severity: Severity::Error, source: String::new(), code: String::new() }])
    }

    #[test]
    fn no_popup_is_drawn_over_an_open_picker_and_none_comes_back_stale() {
        let mut lsp = LspPopups::default();
        lsp.handle(problem());
        assert!(editor_popups_shown(false, &mut lsp), "a resting problem shows");
        // The picker opens with the popup up: it goes.
        assert!(!editor_popups_shown(true, &mut lsp));
        assert!(lsp.is_empty());
        // A `CursorHold` while it is open brings a new one: not drawn, not kept.
        lsp.handle(problem());
        lsp.handle(hover());
        assert!(!editor_popups_shown(true, &mut lsp));
        // Closed: nothing is left from then; the next rest brings its own.
        assert!(!editor_popups_shown(false, &mut lsp));
        lsp.handle(problem());
        assert!(editor_popups_shown(false, &mut lsp));
        lsp.handle(GuiEvent::PopupClose(Some(PopupKind::Diagnostic)));
    }
}
