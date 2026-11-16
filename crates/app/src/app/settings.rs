//! Settings: the config file, opened from Eshvi › Settings… and applied
//! again when it changes.

use super::*;

impl VimApp {
    /// Eshvi › Settings…: the config file, created with a short header the
    /// first time.
    pub(super) fn open_settings(&mut self) {
        let Some(path) = crate::config::path() else { return };
        if !path.exists() {
            let header = "# eshvi settings; every key is optional. All of them are listed at\n\
                          # the top of crates/app/src/config.rs in the eshvi sources.\n";
            let created = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, header));
            if let Err(e) = created {
                self.editor.push_message("emsg", &format!("eshvi: cannot create {}: {e}", path.display()));
                return;
            }
        }
        self.set_focus(Focus::Editor);
        self.bridge.send(UiCommand::OpenFile { path, split: false });
    }

    /// Apply the config file again when it changed on disk (looked at
    /// twice a second while the app is active, and after a save in Neovim).
    pub(super) fn watch_config(&mut self, ctx: &egui::Context) {
        if self.config_checked.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        self.config_checked = Some(Instant::now());
        let modified = crate::config::modified();
        if modified == self.config_modified {
            return;
        }
        self.config_modified = modified;
        match crate::config::read() {
            Ok(config) => self.apply_config(ctx, config),
            Err(e) => self.editor.push_message("emsg", &format!("eshvi: settings not applied: {e}")),
        }
    }

    /// Apply what can change while eshvi runs; say which changes need a
    /// restart.
    fn apply_config(&mut self, ctx: &egui::Context, config: Config) {
        let old = std::mem::replace(&mut self.config, config);
        let c = &self.config;
        crate::popup::set_settings(crate::popup::ThemeSettings {
            colors: c.colors.resolve(),
            corner_radius: c.popup.corner_radius,
            shadow: c.popup.shadow,
            restyle_floats: c.popup.restyle_floats,
        });
        // Fonts ('guifont', when Neovim sets it, wins over `font`).
        if (&old.font, &old.ui_font, &old.sidebar.font, &old.agent.font, &old.terminal.font)
            != (&c.font, &c.ui_font, &c.sidebar.font, &c.agent.font, &c.terminal.font)
        {
            let mut font = c
                .font
                .as_deref()
                .and_then(|f| FontSpec::parse_guifont(f, DEFAULT_FONT_SIZE))
                .unwrap_or(FontSpec::default_with_size(DEFAULT_FONT_SIZE));
            if let Some(size) = self.cli_font_size {
                font.size = size;
            }
            self.ui_font = c.ui_font.as_deref().and_then(|f| FontSpec::parse_guifont(f, font.size - 1.0));
            self.sidebar_font = c.sidebar.font.as_deref().and_then(|f| FontSpec::parse_guifont(f, font.size - 1.0));
            self.agent_font = c.agent.font.as_deref().and_then(|f| FontSpec::parse_guifont(f, font.size));
            self.terminal_font = c.terminal.font.as_deref().and_then(|f| FontSpec::parse_guifont(f, font.size));
            self.base_font = self
                .seen_guifont
                .as_deref()
                .and_then(|gf| FontSpec::parse_guifont(gf, font.size))
                .unwrap_or_else(|| font.clone());
            self.config_font = font;
            self.zoom = 0.0;
            self.pending_fonts =
                Some(fonts::install(
                ctx,
                &self.base_font,
                self.ui_font.as_ref(),
                self.sidebar_font.as_ref(),
                self.agent_font.as_ref(),
                self.terminal_font.as_ref(),
            ));
        }
        self.keys.alt_is_meta = c.alt_is_meta.unwrap_or(KeyTranslator::default().alt_is_meta);
        self.blink.enabled = c.cursor_blink.unwrap_or(true);
        self.smooth_scroll = c.smooth_scroll.unwrap_or(true);
        self.scrollbars.mode = c.scrollbar.unwrap_or_default();
        self.lsp.limits = PopupLimits {
            hover_max_width: c.popup.hover_max_width,
            hover_max_height: c.popup.hover_max_height,
            signature_max_width: c.popup.signature_max_width,
        };
        self.statusline_gui = c.statusline.gui;
        self.status_colors = c.statusline.resolve_colors();
        self.status_font_base = fonts::status_base_size(c.statusline.font_size, self.ui_font.as_ref());
        self.highlight_mode = c.statusline.highlight_mode;
        self.sidebar_hides_statusline = c.sidebar.hide_statusline;
        self.explorer_font_base = fonts::explorer_base_size(c.explorer.font_size, self.ui_font.as_ref());
        self.explorer_width = c.explorer.width.clamp(120.0, 800.0);
        self.problems_proportional = c.terminal.problems_font == crate::config::ListFont::Ui;
        // The explorer's toggles change only when their keys did (they
        // are also switched in the explorer's menu).
        if (old.explorer.show_ignored, old.explorer.nesting) != (c.explorer.show_ignored, c.explorer.nesting) {
            self.explorer.set_filters(c.explorer.show_ignored, c.explorer.nesting);
        }
        // The Outline's, likewise.
        if (old.outline.enabled, old.outline.follow_cursor) != (c.outline.enabled, c.outline.follow_cursor) {
            self.explorer.outline_enabled = c.outline.enabled;
            self.explorer.outline_follow = c.outline.follow_cursor;
            self.send_outline_config();
        }
        if old.outline.sort != c.outline.sort {
            self.explorer.outline().sort = c.outline.sort;
        }
        // Sizes that follow the editor font's zoom.
        self.set_zoom(ctx, self.zoom);
        let restart = crate::config::restart_needed(&old, &self.config);
        let message = if restart.is_empty() {
            "Settings applied".to_owned()
        } else {
            format!("Settings applied; restart Eshvi for {}", restart.join("; "))
        };
        self.editor.push_message(if restart.is_empty() { "" } else { "wmsg" }, &message);
        ctx.request_repaint();
    }
}
