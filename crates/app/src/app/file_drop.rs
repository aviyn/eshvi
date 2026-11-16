//! Files dragged onto the window from the Finder. On the explorer they copy
//! into a folder (`explorer_view`); anywhere else they open in the editor, in
//! the editor window under the pointer if there is one.

use super::*;
use std::path::PathBuf;

impl VimApp {
    /// Where a file drag is, this frame. macOS tells nothing of the pointer
    /// during one (winit has no event for it), so it's asked.
    pub(super) fn track_file_drag(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let dragging = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped = ctx.input(|i| !i.raw.dropped_files.is_empty());
        self.explorer.drop_pos = (dragging || dropped).then(|| drag_position(ctx, frame)).flatten();
        if dragging {
            // Nothing else wakes the app while the pointer moves.
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    /// The editor window file `pos` would open in, if it's over one.
    fn drop_window(&self, pos: Pos2) -> Option<&grid::Window> {
        self.editor.windows.values().find(|w| w.kind == WindowKind::Normal && self.window_rect(w).contains(pos))
    }

    /// Whether `pos` is the explorer's to take.
    fn drop_is_explorers(&self, pos: Option<Pos2>) -> bool {
        self.side == SideView::Explorer && self.explorer_rect.zip(pos).is_some_and(|(r, p)| r.contains(p))
    }

    /// Show where a dragged file would land, and open what was dropped.
    pub(super) fn finish_file_drag(&mut self, ctx: &egui::Context) {
        let pos = self.explorer.drop_pos;
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) && !self.drop_is_explorers(pos) {
            if let Some(rect) = pos.and_then(|p| self.drop_window(p)).map(|w| self.window_rect(w)) {
                let accent = style::Chrome::from_editor(&self.editor).accent;
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("eshvi-file-drop")));
                painter.rect_filled(rect, 0.0, accent.gamma_multiply(0.15));
            }
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| p.is_absolute()).collect());
        if dropped.is_empty() || self.drop_is_explorers(pos) {
            return;
        }
        let (paths, folders) = files_and_folders(dropped);
        if !folders.is_empty() {
            self.editor.push_message("wmsg", "A folder opens with File › Open Folder…");
        }
        if !paths.is_empty() {
            let win = pos.and_then(|p| self.drop_window(p)).map(|w| w.win);
            self.bridge.send(UiCommand::OpenDropped { paths, win });
            self.set_focus(Focus::Editor);
            // It came from another app: the keys are for this file now.
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }
}

/// What was dropped: the files to open, and the rest (folders).
fn files_and_folders(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    paths.into_iter().partition(|p| p.is_file())
}

#[cfg(target_os = "macos")]
fn drag_position(ctx: &egui::Context, frame: &eframe::Frame) -> Option<Pos2> {
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(RawWindowHandle::AppKit(handle)) = frame.window_handle().map(|h| h.as_raw()) else { return None };
    // SAFETY: eframe's content view, alive for this frame, on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let window = view.window()?;
    let height = window.contentView()?.frame().size.height;
    // In the window's coordinates, from its bottom as AppKit counts.
    let at = window.mouseLocationOutsideOfEventStream();
    let zoom = ctx.zoom_factor();
    Some(Pos2::new(at.x as f32 / zoom, (height - at.y) as f32 / zoom))
}

#[cfg(not(target_os = "macos"))]
fn drag_position(ctx: &egui::Context, _frame: &eframe::Frame) -> Option<Pos2> {
    ctx.pointer_hover_pos()
}

#[cfg(test)]
mod tests {
    use super::files_and_folders;

    #[test]
    fn folders_are_kept_apart_from_files() {
        let dir = std::env::temp_dir().join(format!("eshvi-file-drop-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let (files, folders) = files_and_folders(vec![dir.join("a.txt"), dir.join("sub"), dir.join("gone.txt")]);
        assert_eq!(files, vec![dir.join("a.txt")]);
        assert_eq!(folders, vec![dir.join("sub"), dir.join("gone.txt")]);
        std::fs::remove_file(dir.join("a.txt")).unwrap();
        std::fs::remove_dir(dir.join("sub")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
