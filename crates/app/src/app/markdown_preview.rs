//! Markdown previews over their windows (`crate::preview` renders them).

use super::*;

impl VimApp {
    /// Where window `w`'s Markdown preview goes, if it shows one: its text
    /// area, under its file tab. At the editor's edges it also covers the
    /// margin before the first column and the sliver after the last, which
    /// would otherwise show the colors of the text under the preview (a
    /// cursorline), beside the side bar or the agent panel.
    pub(super) fn preview_rect(&self, w: &grid::Window) -> Option<Rect> {
        if w.kind != WindowKind::Normal || self.preview.win(w.win).is_none() {
            return None;
        }
        let mut r = self.window_rect(w);
        if w.col == 0 {
            r.min.x = self.grid_area.min.x;
        }
        r.max.x += self.right_sliver(w);
        r.min.y += w.margins.top as f32 * self.renderer.metrics().height;
        r.max.y -= w.margins.bottom as f32 * self.renderer.metrics().height;
        Some(r)
    }

    /// The preview window under `pos` (as last drawn), unless a float is
    /// drawn over it there (the changes diff, a picker): the wheel and
    /// presses are that float's, not the preview's behind it.
    pub(super) fn preview_at(&self, pos: Pos2) -> Option<i64> {
        let (_, win) = self.preview_rects.iter().find(|(r, _)| r.contains(pos))?;
        self.top_window_at(pos).is_none_or(|top| top == *win).then_some(*win)
    }

    /// The window drawn topmost at `pos`.
    pub(super) fn top_window_at(&self, pos: Pos2) -> Option<i64> {
        let (grid, ..) = self.grid_cell_at(pos)?;
        self.editor.windows.get(&grid).map(|w| w.win)
    }

    /// Draw `grid`'s window's preview in `rect`, over its text, in the
    /// editor's colors and the hover popups' Markdown look. Links to files
    /// open them in this window, one to a file that isn't there says so and
    /// stays on the page; others go to the browser.
    pub(super) fn show_preview(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, grid: u64, rect: Rect) {
        let Some(w) = self.editor.windows.get(&grid).cloned() else { return };
        let bg = self.window_bg(&w).unwrap_or(rgb(self.editor.default_colors.bg));
        let fg = rgb(self.editor.default_colors.fg);
        painter.rect_filled(rect, 0.0, bg);
        let mut theme = crate::popup::PopupTheme::from_editor(&self.editor);
        theme.bg = bg;
        theme.fg = fg;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layer_id(painter.layer_id()));
        child.set_clip_rect(rect.intersect(painter.clip_rect()));
        LspPopups::style_popup_ui(&mut child, &theme);
        // Prose in the UI font, headings bold and up to 1.6 times its size
        // (egui_commonmark sizes them between Body and Heading), bold text
        // brighter than the rest.
        // `[preview] font_size` (that of the editor font by default) follows
        // zoom as the rest of the UI does.
        let zoom = self.grid_fonts.size / self.base_font.size;
        let size = self.config.preview.font_size.map_or(self.grid_fonts.size, |base| (base * zoom).clamp(6.0, 72.0));
        let (mut body, mut heading) = (self.grid_fonts.sidebar_font_id(false), self.grid_fonts.sidebar_font_id(true));
        (body.size, heading.size) = (size, size * 1.6);
        let style = child.style_mut();
        style.text_styles.insert(egui::TextStyle::Body, body);
        style.text_styles.insert(egui::TextStyle::Heading, heading);
        // (Code nested in a list or quote is egui_commonmark's: Monospace.)
        style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(size));
        style.visuals.override_text_color = None;
        style.visuals.widgets.noninteractive.fg_stroke.color = fg;
        style.visuals.widgets.inactive.fg_stroke.color = fg;
        style.visuals.widgets.active.fg_stroke.color = crate::popup::mix(fg, egui::Color32::WHITE, 0.35);
        // While Neovim has the mouse (a separator dragged: its reach is a few
        // pixels into the preview, and the drag crosses it), no text is
        // selected under it.
        if self.drag.is_some() {
            style.interaction.selectable_labels = false;
        }
        let syntax = self.lsp.syntax();
        // (egui scrolls what is under the pointer, whatever is drawn over it.)
        let covered = ui.ctx().input(|i| i.pointer.latest_pos()).is_some_and(|p| rect.contains(p) && self.top_window_at(p).is_some_and(|top| top != w.win));
        let scrolls = !covered;
        let fonts = if size == self.grid_fonts.size { self.grid_fonts.clone() } else { self.grid_fonts.with_size(size) };
        let chrome = crate::style::Chrome::from_editor(&self.editor);
        let first_shape = ui.ctx().graphics(|g| g.get(painter.layer_id()).map_or(0, |l| l.all_entries().len()));
        self.preview.show(&mut child, w.win, syntax.as_ref(), &fonts, fg, scrolls, &chrome, &mut self.icons);
        // The search, marked where its matches are in the page.
        if let Some(search) = self.preview.win(w.win).and_then(|p| p.search.clone()) {
            use crate::preview::{CurrentMark, MarkColors};
            let colors = MarkColors { bg: search.bg.map_or(crate::popup::mix(bg, egui::Color32::YELLOW, 0.5), rgb), fg: search.fg.map(rgb) };
            // The one under the cursor: CurSearch's colors (`n` goes to it).
            let cur = MarkColors { bg: search.cur_bg.map_or(crate::popup::mix(bg, egui::Color32::from_rgb(0xff, 0x8c, 0x00), 0.8), rgb), fg: search.cur_fg.map(rgb) };
            let current = self.preview.current_match(w.win);
            let current = current.as_ref().map(|(shapes, nth, text)| CurrentMark { shapes: shapes.clone(), nth: *nth, text, colors: cur });
            let marks = crate::preview::search_marks(ui.ctx(), painter.layer_id(), first_shape, &search.texts, colors, current);
            painter.with_clip_rect(rect.intersect(painter.clip_rect())).extend(marks);
        }
        self.follow_preview_scroll(ui.ctx());
        self.preview_rects.push((rect, w.win));
        // Links: egui opens URLs; a file link (relative, or file:) opens
        // the file instead.
        let Some(dir) = self.preview.win(w.win).and_then(|p| std::path::Path::new(&p.path).parent().map(|d| d.to_path_buf()))
        else {
            return;
        };
        let mut files = Vec::new();
        ui.ctx().output_mut(|o| {
            o.commands.retain(|c| match c {
                egui::OutputCommand::OpenUrl(open) if link_kind(&open.url) == LinkKind::File => {
                    files.extend(link_file(&dir, &open.url));
                    false
                }
                _ => true,
            })
        });
        for file in files {
            match file {
                Ok(path) => self.bridge.send(UiCommand::OpenFile { path, split: false }),
                Err(message) => self.editor.push_message("emsg", &message),
            }
        }
    }
}

impl VimApp {
    /// Links clicked this frame that are left for the OS to open (in a
    /// preview, an LSP popup, a message): only web pages and mail go there.
    /// A `file:` link opens in the editor; any other scheme (`vscode://`,
    /// `smb://`, an app's own) is not opened, and said so: a document must
    /// not be able to start whatever handles it with a click.
    pub(super) fn filter_links(&mut self, ctx: &egui::Context) {
        let mut others = Vec::new();
        ctx.output_mut(|o| {
            o.commands.retain(|c| match c {
                egui::OutputCommand::OpenUrl(open) if link_kind(&open.url) != LinkKind::Web => {
                    others.push(open.url.clone());
                    false
                }
                _ => true,
            })
        });
        for url in others {
            match (link_kind(&url), link_file(std::path::Path::new("/"), &url)) {
                (LinkKind::File, Some(Ok(path))) => self.bridge.send(UiCommand::OpenFile { path, split: false }),
                (LinkKind::File, Some(Err(message))) => self.editor.push_message("emsg", &message),
                (LinkKind::File, None) => {}
                _ => self.editor.push_message("wmsg", &format!("Not opened: {url} (only http, https and mailto links are)")),
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum LinkKind {
    /// http, https, mailto: the OS opens it.
    Web,
    /// No scheme (a path), or `file:`.
    File,
    /// Any other scheme.
    Other,
}

pub(super) fn link_kind(url: &str) -> LinkKind {
    let scheme = url.split_once(':').map(|(s, _)| s).filter(|s| {
        // (One letter is a Windows drive, not a scheme.)
        s.len() > 1 && s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    match scheme.map(str::to_ascii_lowercase).as_deref() {
        None | Some("file") => LinkKind::File,
        Some("http" | "https" | "mailto") => LinkKind::Web,
        Some(_) => LinkKind::Other,
    }
}

/// `%20` and the like as the bytes they stand for.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| text.get(i + 1..i + 3)).flatten().and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The file a preview's link to a file (relative, or `file:`) opens, or what to
/// say if it isn't there; nothing for a link within the page.
fn link_file(dir: &std::path::Path, url: &str) -> Option<Result<std::path::PathBuf, String>> {
    let target = url.strip_prefix("file://").or_else(|| url.strip_prefix("file:")).unwrap_or(url);
    let target = target.split('#').next().unwrap_or(target);
    if target.is_empty() {
        return None;
    }
    // As written first (a name may hold a real `%`), then with its escapes read.
    let path = dir.join(target);
    if path.exists() {
        return Some(Ok(path));
    }
    let decoded = percent_decoded(target);
    let path = dir.join(&decoded);
    Some(if path.exists() { Ok(path) } else { Err(format!("File not found: {decoded}")) })
}

#[cfg(test)]
mod tests {
    use super::{link_file, link_kind, LinkKind};

    #[test]
    fn only_web_and_mail_links_go_to_the_os() {
        for url in ["https://example.com/a", "HTTP://example.com", "mailto:x@y.org"] {
            assert_eq!(link_kind(url), LinkKind::Web, "{url}");
        }
        for url in ["b.md", "../a/b.md#part", "file:///tmp/a.md", "#part", "C:\\notes\\a.md", "notes/12:30.md"] {
            assert_eq!(link_kind(url), LinkKind::File, "{url}");
        }
        for url in ["vscode://file/x", "smb://host/share", "javascript:alert(1)", "x-apple.systempreferences:com.apple"] {
            assert_eq!(link_kind(url), LinkKind::Other, "{url}");
        }
    }

    #[test]
    fn escapes_in_a_file_link_are_read() {
        let dir = std::env::temp_dir().join(format!("eshvi-preview-escapes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("my notes.md"), "# N\n").unwrap();
        std::fs::write(dir.join("100%25.md"), "# P\n").unwrap();
        assert_eq!(link_file(&dir, "my%20notes.md#top"), Some(Ok(dir.join("my notes.md"))));
        // A name that really holds "%25" is itself.
        assert_eq!(link_file(&dir, "100%25.md"), Some(Ok(dir.join("100%25.md"))));
        assert_eq!(link_file(&dir, "no%20such.md"), Some(Err("File not found: no such.md".to_owned())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_link_says_so_instead_of_opening() {
        let dir = std::env::temp_dir().join(format!("eshvi-preview-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.md"), "# B\n").unwrap();
        assert_eq!(link_file(&dir, "b.md"), Some(Ok(dir.join("b.md"))));
        assert_eq!(link_file(&dir, "b.md#part"), Some(Ok(dir.join("b.md"))));
        let absolute = format!("file://{}", dir.join("b.md").display());
        assert_eq!(link_file(&dir, &absolute), Some(Ok(dir.join("b.md"))));
        assert_eq!(link_file(&dir, "nope.md#part"), Some(Err("File not found: nope.md".to_owned())));
        assert_eq!(link_file(&dir, "#part"), None);
        std::fs::remove_file(dir.join("b.md")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
