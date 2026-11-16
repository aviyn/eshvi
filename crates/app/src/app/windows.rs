//! External windows (Neovim's `:wincmd T`-style OS windows) and the editor's scrollbars.

use super::*;

impl VimApp {
    /// External windows (`nvim_open_win` with `external = true`), each in an
    /// OS window of its own: its grid and cursor; keys typed there go to
    /// Neovim with that window current; resizing it resizes the grid,
    /// closing it closes the window.
    pub(super) fn show_external_windows(&mut self, ctx: &egui::Context) {
        let externals: Vec<(u64, i64)> = self
            .editor
            .windows
            .values()
            .filter(|w| w.kind == WindowKind::External && !w.hidden && self.editor.grids.contains_key(&w.grid))
            .map(|w| (w.grid, w.win))
            .collect();
        let m = self.renderer.metrics();
        let pad = 6.0;
        for (grid, win) in externals {
            let Some(g) = self.editor.grids.get(&grid) else { continue };
            let size = Vec2::new(g.width() as f32 * m.width, g.height() as f32 * m.height) + Vec2::splat(2.0 * pad);
            let title = format!("{} — Eshvi", self.display_title());
            let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size(size);
            let id = egui::ViewportId::from_hash_of(("eshvi-external", win));
            ctx.show_viewport_immediate(id, builder, |ui, _class| {
                let vctx = ui.ctx().clone();
                let bg = rgb(self.editor.default_colors.bg);
                let rect = ui.max_rect();
                ui.painter().rect_filled(rect, 0.0, bg);
                let origin = rect.min + Vec2::splat(pad);
                self.renderer.paint_grid(ui.painter(), &self.editor, grid, origin, LayerStyle::default());
                if self.editor.cursor.grid == grid {
                    let focused = vctx.input(|i| i.viewport().focused).unwrap_or(false);
                    self.renderer.paint_cursor(ui.painter(), &self.editor, origin, focused);
                }
                // Its size follows the OS window's.
                let cols = ((rect.width() - 2.0 * pad) / m.width).floor().max(1.0) as usize;
                let rows = ((rect.height() - 2.0 * pad) / m.height).floor().max(1.0) as usize;
                let (focused, closing, events) =
                    vctx.input(|i| (i.viewport().focused.unwrap_or(false), i.viewport().close_requested(), i.events.clone()));
                if (cols, rows) != (g.width(), g.height()) && !self.external_resized.contains(&(grid, cols, rows)) {
                    // Only the last size asked for is remembered: going back
                    // to an earlier one is asked for again.
                    self.external_resized.retain(|(g, _, _)| *g != grid);
                    self.external_resized.push((grid, cols, rows));
                    self.bridge.send(UiCommand::ResizeGrid { grid, width: cols, height: rows });
                }
                if closing {
                    self.bridge.send(UiCommand::Command(format!("lua pcall(vim.api.nvim_win_close, {win}, false)")));
                }
                // Typing here types into that window.
                if focused {
                    let mut keys = String::new();
                    for ev in events {
                        match ev {
                            Event::Key { key, physical_key, pressed: true, modifiers, .. } => {
                                if let Some(k) = self.keys.key(key, physical_key, modifiers) {
                                    keys.push_str(&k);
                                }
                            }
                            Event::Text(text) => {
                                if let Some(k) = self.keys.text(&text) {
                                    keys.push_str(&k);
                                }
                            }
                            _ => {}
                        }
                    }
                    if !keys.is_empty() {
                        if self.editor.cursor.grid != grid {
                            self.bridge.send(UiCommand::Command(format!("lua pcall(vim.api.nvim_set_current_win, {win})")));
                        }
                        self.bridge.send(UiCommand::Input(keys));
                    }
                }
            });
        }
        let grids = &self.editor.grids;
        self.external_resized.retain(|(g, ..)| grids.contains_key(g));
    }

    /// Scrollbars on the editor windows (not floats or file trees), over
    /// their text area; while the diff view covers them, on it instead.
    pub(super) fn paint_scrollbars(&mut self, ctx: &egui::Context, painter: &egui::Painter) {
        let m = self.renderer.metrics();
        let (fg, bg) = (rgb(self.editor.default_colors.fg), rgb(self.editor.default_colors.bg));
        let under_diff = self.diff.is_some();
        let mut targets: Vec<crate::scrollbar::Target> = self
            .editor
            .windows
            .values()
            .filter(|w| !under_diff && w.kind == WindowKind::Normal && !w.hidden && !self.sidebar_wins.contains(&w.win))
            .filter_map(|w| {
                // A Markdown preview's bar is over what it shows, not the
                // text behind it.
                let (top, bottom, lines) =
                    self.preview.viewport(w.win).or_else(|| w.viewport.as_ref().map(|v| (v.topline, v.botline, v.line_count)))?;
                let r = self.window_rect(w);
                let area = Rect::from_x_y_ranges(
                    r.x_range(),
                    r.min.y + w.margins.top as f32 * m.height..=r.max.y - w.margins.bottom as f32 * m.height,
                );
                Some(crate::scrollbar::Target { win: w.win, area, top, bottom, lines, fg, bg })
            })
            .collect();
        // Popups (plugins' floats: a completion menu, its documentation, a
        // picker's list) get the same bar, in the popups' colors; a
        // plugin's own bar beside one isn't drawn (`is_plugin_scrollbar`).
        let theme = crate::popup::PopupTheme::from_editor(&self.editor);
        targets.extend(
            self.editor
                .windows
                .values()
                .filter(|w| matches!(w.kind, WindowKind::Float { .. }) && !w.hidden)
                .filter(|w| !self.is_container(w) && !self.is_plugin_scrollbar(w))
                .filter_map(|w| {
                    let v = w.viewport?;
                    let (row, col) = self.editor.grid_origin(w.grid);
                    let min = self.origin
                        + Vec2::new(col as f32 * m.width, (row + w.margins.top as i64) as f32 * m.height)
                        + self.float_offsets.get(&w.grid).copied().unwrap_or(Vec2::ZERO);
                    let rows = w.height.saturating_sub(w.margins.top + w.margins.bottom);
                    let mut area = Rect::from_min_size(min, Vec2::new(w.width as f32 * m.width, rows as f32 * m.height));
                    // Inside a frame: its border cells are outside it (the bar
                    // there would look like a popup of its own).
                    if self.editor.grids.get(&w.grid).is_some_and(crate::renderer::has_frame) {
                        area = area.shrink2(Vec2::new(m.width, m.height));
                    }
                    Some(crate::scrollbar::Target {
                        win: w.win,
                        area,
                        top: v.topline,
                        bottom: v.botline,
                        lines: v.line_count,
                        fg: theme.fg,
                        bg: theme.bg,
                    })
                }),
        );
        if let Some(d) = &self.diff {
            let chrome = crate::style::Chrome::from_editor(&self.editor);
            targets.extend(d.scrollbar_target(chrome.fg, chrome.bg));
        }
        self.scrollbars.paint(ctx, painter, targets);
    }
}
