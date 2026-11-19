//! Files on the clipboard (the macOS pasteboard), as the Finder puts them
//! there: ⌘C in the explorer copies its selection so, and ⌘V in a terminal
//! pastes copied files as their paths, as a drop does (the text that comes
//! with them is only their names).

use std::path::PathBuf;

/// A path as Terminal pastes a dropped file's: one shell word, its special
/// characters behind backslashes.
pub fn shell_escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii() && !(c.is_ascii_alphanumeric() || "-_./+,:@%^=".contains(c)) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Paths as a terminal is given them: escaped, between spaces.
pub fn shell_words(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| shell_escape(&p.to_string_lossy())).collect::<Vec<_>>().join(" ")
}

/// What ⌘V gives a terminal when the clipboard holds files: their paths.
pub fn pasted_paths() -> Option<String> {
    let files = files();
    (!files.is_empty()).then(|| shell_words(&files))
}

/// The files on the clipboard, if it holds any.
#[cfg(target_os = "macos")]
pub fn files() -> Vec<PathBuf> {
    files_on(&objc2_app_kit::NSPasteboard::generalPasteboard())
}

#[cfg(target_os = "macos")]
fn files_on(board: &objc2_app_kit::NSPasteboard) -> Vec<PathBuf> {
    use objc2_app_kit::NSPasteboardTypeFileURL;
    use objc2_foundation::NSURL;
    let Some(items) = board.pasteboardItems() else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            // SAFETY: an AppKit constant, there for the program's life.
            let url = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
            // (The Finder may give a file reference, `file:///.file/id=…`.)
            let url = NSURL::URLWithString(&url)?;
            let path = url.filePathURL().unwrap_or(url).path()?;
            Some(PathBuf::from(path.to_string()))
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn files() -> Vec<PathBuf> {
    Vec::new()
}

/// Put `paths` on the clipboard as files, with their paths as the text
/// (one a line) for what takes text.
#[cfg(target_os = "macos")]
pub fn set_files(paths: &[PathBuf]) {
    set_files_on(&objc2_app_kit::NSPasteboard::generalPasteboard(), paths);
}

#[cfg(target_os = "macos")]
fn set_files_on(board: &objc2_app_kit::NSPasteboard, paths: &[PathBuf]) {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeString, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};
    let text = paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n");
    let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let item = NSPasteboardItem::new();
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            // SAFETY: AppKit constants, there for the program's life.
            if let Some(url) = url.absoluteString() {
                item.setString_forType(&url, unsafe { NSPasteboardTypeFileURL });
            }
            if i == 0 {
                item.setString_forType(&NSString::from_str(&text), unsafe { NSPasteboardTypeString });
            }
            ProtocolObject::from_retained(item)
        })
        .collect();
    board.clearContents();
    board.writeObjects(&NSArray::from_retained_slice(&items));
}

#[cfg(not(target_os = "macos"))]
pub fn set_files(paths: &[PathBuf]) {
    let text = paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n");
    if let Ok(mut c) = arboard::Clipboard::new() {
        let _ = c.set_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_escaped_as_terminal_does() {
        assert_eq!(shell_escape("/Users/me/Desktop/shot.png"), "/Users/me/Desktop/shot.png");
        assert_eq!(shell_escape("/tmp/Screenshot 2026-11-19 at 9.06.59 PM.png"), r"/tmp/Screenshot\ 2026-11-19\ at\ 9.06.59\ PM.png");
        assert_eq!(shell_escape("/tmp/a'b(c)&$d"), r"/tmp/a\'b\(c\)\&\$d");
        assert_eq!(shell_escape("/tmp/café"), "/tmp/café");
        assert_eq!(shell_words(&["/a b".into(), "/c".into()]), r"/a\ b /c");
    }

    /// Copied as files, read back as files (their names with spaces too), with
    /// their paths as the text. (On a pasteboard of its own, not the clipboard.)
    #[cfg(target_os = "macos")]
    #[test]
    fn files_go_on_the_pasteboard_and_come_back() {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        let board = NSPasteboard::pasteboardWithUniqueName();
        let paths = [PathBuf::from("/tmp/a b.png"), PathBuf::from("/Users/me/notes.md")];
        set_files_on(&board, &paths);
        assert_eq!(files_on(&board), paths);
        // SAFETY: an AppKit constant.
        let text = board.stringForType(unsafe { NSPasteboardTypeString }).map(|s| s.to_string());
        assert_eq!(text.as_deref(), Some("/tmp/a b.png\n/Users/me/notes.md"));
    }
}
