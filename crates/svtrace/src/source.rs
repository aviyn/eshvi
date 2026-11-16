//! The design's source files: their URIs, and LSP positions (0-based lines, UTF-16 columns) for the
//! dump's 1-based byte columns.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::ast::{Pos, Range};

pub struct Sources {
    /// (Shared with the other reader of the same build: see [`Texts`].)
    files: Vec<Arc<SourceFile>>,
    /// The file each URI asked about was found to be (`None`: none), so that a path is resolved
    /// through the file system once.
    lookups: Mutex<HashMap<String, Option<u32>>>,
}

struct SourceFile {
    path: PathBuf,
    uri: String,
    /// None when the file can't be read now; positions then count bytes as characters.
    text: Option<String>,
    /// Byte offset of each line's start.
    line_starts: Vec<usize>,
}

/// The source files read for one build. The trace's graph and the hierarchy each list the files
/// they place things in (each numbers them its own way); a file both list is read, and kept, once.
#[derive(Default)]
pub struct Texts {
    files: Mutex<HashMap<String, Arc<SourceFile>>>,
}

impl Sources {
    /// Reads the files the dump names (relative to `root`, the folder slang ran in).
    pub fn load(root: &Path, names: &[String]) -> Self {
        Self::load_shared(root, names, &Texts::default())
    }

    /// [`Sources::load`], taking the files that `texts` already has from there.
    pub fn load_shared(root: &Path, names: &[String], texts: &Texts) -> Self {
        let mut known = texts.files.lock().unwrap();
        let files = names
            .iter()
            .map(|name| {
                let read = || {
                    let path = absolute(root, name);
                    let text = fs::read_to_string(&path).ok();
                    let line_starts = text.as_deref().map(line_starts).unwrap_or_default();
                    Arc::new(SourceFile { uri: file_uri(&path), path, text, line_starts })
                };
                known.entry(name.clone()).or_insert_with(read).clone()
            })
            .collect();
        Sources { files, lookups: Mutex::default() }
    }

    /// Whether the two hold the same copy of each file both have (for the test of that).
    pub fn shares_files_with(&self, other: &Sources) -> bool {
        let same = |file: &Arc<SourceFile>| other.files.iter().filter(|theirs| theirs.path == file.path).all(|theirs| Arc::ptr_eq(file, theirs));
        self.files.iter().all(same) && self.files.iter().any(|file| other.files.iter().any(|theirs| Arc::ptr_eq(file, theirs)))
    }

    /// The absolute paths of the files.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }

    pub fn uri(&self, file: u32) -> &str {
        &self.files[file as usize].uri
    }

    /// The text of a line (1-based, as in the dump) without its line ending, when the file can be read.
    pub fn line(&self, file: u32, line: u32) -> Option<&str> {
        let source = self.files.get(file as usize)?;
        let text = source.text.as_deref()?;
        let index = (line as usize).checked_sub(1)?;
        let start = *source.line_starts.get(index)?;
        let end = source.line_starts.get(index + 1).copied().unwrap_or(text.len());
        Some(text[start..end].trim_end_matches(['\n', '\r']))
    }

    /// The file whose URI is `uri`: compared by path, with the percent-encoding undone and symbolic
    /// links resolved (the files' own paths are resolved when they are loaded), so that a buffer
    /// opened through a link (`/tmp` for `/private/tmp`) finds its file.
    pub fn file_by_uri(&self, uri: &str) -> Option<u32> {
        if let Some(found) = self.lookups.lock().unwrap().get(uri) {
            return *found;
        }
        let wanted = decode(uri);
        let found = self.files.iter().position(|file| file.uri == uri || decode(&file.uri) == wanted).or_else(|| {
            let path = canonical(&uri_path(uri)?);
            self.files.iter().position(|file| file.path == path)
        });
        let found = found.map(|at| at as u32);
        self.lookups.lock().unwrap().insert(uri.to_string(), found);
        found
    }

    /// The dump's place (1-based line, byte column) for an LSP position (0-based line, UTF-16 column).
    pub fn dump_pos(&self, file: u32, line: u32, character: u32) -> Option<Pos> {
        let source = &self.files[file as usize];
        let text = source.text.as_deref()?;
        let start = *source.line_starts.get(line as usize)?;
        let mut units = 0;
        let mut bytes = 0;
        for c in text[start..].chars() {
            if units >= character || c == '\n' {
                break;
            }
            units += c.len_utf16() as u32;
            bytes += c.len_utf8();
        }
        Some(Pos { line: line + 1, col: bytes as u32 + 1 })
    }

    /// The LSP position of a place in the dump.
    pub fn position(&self, file: u32, pos: Pos) -> (u32, u32) {
        let line = pos.line.saturating_sub(1);
        let col = pos.col.saturating_sub(1);
        let source = &self.files[file as usize];
        let Some(text) = source.text.as_deref() else {
            return (line, col);
        };
        let Some(&start) = source.line_starts.get(line as usize) else {
            return (line, col);
        };
        // (A column past the end of the line is the end of the line, not a place in the next one.)
        let line_end = source.line_starts.get(line as usize + 1).map_or(text.len(), |next| next - 1);
        let mut end = (start + col as usize).min(line_end);
        // A column inside a character (it shouldn't be) counts up to that character.
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        (line, text[start.min(end)..end].encode_utf16().count() as u32)
    }

    /// Where `name` is written in the invocation of the macro at `pos`, if it is one of the
    /// arguments. The dump puts everything that comes out of a macro at the macro's name, while a
    /// signal handed to a macro is written at the argument.
    pub fn macro_argument(&self, file: u32, pos: Pos, name: &str) -> Option<Range> {
        let source = &self.files[file as usize];
        let invocation = self.invocation(source, pos)?;
        let (start, end) = invocation.arguments.into_iter().find(|&(start, end)| {
            source.text.as_deref().is_some_and(|text| &text[start..end] == name)
        })?;
        Some(Range { file, start: self.pos_at(source, start), end: self.pos_at(source, end) })
    }

    /// Whether the cursor at `at` is on `name`, a signal that comes out of the macro invocation that
    /// `range` is: the dump gives every node of a macro's expansion the whole invocation as its
    /// range, so the place of a signal in it is only known by the word written there. `None` when
    /// `range` is not an invocation with arguments (then the range itself is the place).
    pub fn at_name_in_macro(&self, range: Range, at: Pos, name: &str) -> Option<bool> {
        let source = &self.files[range.file as usize];
        let invocation = self.invocation(source, range.start)?;
        if invocation.arguments.is_empty() {
            return None;
        }
        let text = source.text.as_deref()?;
        let offset = |pos: Pos| Some(source.line_starts.get(pos.line.checked_sub(1)? as usize)? + pos.col.checked_sub(1)? as usize);
        let Some(cursor) = offset(at) else { return Some(false) };
        let ident = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$';
        let bytes = text.as_bytes();
        // The word the cursor is in, else the one it is just past.
        let mut from = cursor;
        if !bytes.get(from).is_some_and(|&byte| ident(byte)) {
            from = from.saturating_sub(1);
        }
        if !bytes.get(from).is_some_and(|&byte| ident(byte)) {
            return Some(false);
        }
        let start = (0..=from).rev().take_while(|&i| ident(bytes[i])).last().unwrap_or(from);
        let end = (from..bytes.len()).take_while(|&i| ident(bytes[i])).last().map_or(from, |i| i + 1);
        let inside = start > offset(range.start)? && end <= invocation.end;
        // (A word right after a backtick is a macro's name, not a signal.)
        Some(inside && bytes[start - 1] != b'`' && &text[start..end] == name)
    }

    /// The whole invocation of the macro at `pos`, from its backtick to its closing parenthesis.
    /// What a macro expands to is attributed to this range.
    pub fn macro_invocation(&self, file: u32, pos: Pos) -> Option<Range> {
        let source = &self.files[file as usize];
        let invocation = self.invocation(source, pos)?;
        Some(Range { file, start: pos, end: self.pos_at(source, invocation.end) })
    }

    /// Reads the macro invocation that starts at `pos`, if the text there is one.
    fn invocation(&self, source: &SourceFile, pos: Pos) -> Option<Invocation> {
        let text = source.text.as_deref()?;
        let at = source.line_starts.get(pos.line.checked_sub(1)? as usize)? + pos.col.checked_sub(1)? as usize;
        let bytes = text.as_bytes();
        if bytes.get(at) != Some(&b'`') {
            return None;
        }
        let mut i = at + 1;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$') {
            i += 1;
        }
        let name_end = i;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'(') {
            return Some(Invocation { end: name_end, arguments: Vec::new() });
        }
        let mut depth = 0;
        let mut in_string = false;
        let mut arg_start = i + 1;
        let mut arguments = Vec::new();
        for (offset, &byte) in bytes.iter().enumerate().skip(i) {
            match byte {
                b'"' => in_string = !in_string,
                _ if in_string => {}
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                _ => {}
            }
            let closes = depth == 0 && !in_string && byte == b')';
            if (byte == b',' && depth == 1 && !in_string) || closes {
                let arg = &text[arg_start..offset];
                let lead = arg.len() - arg.trim_start().len();
                arguments.push((arg_start + lead, arg_start + lead + arg.trim().len()));
                arg_start = offset + 1;
            }
            if closes {
                return Some(Invocation { end: offset + 1, arguments });
            }
        }
        None
    }

    fn pos_at(&self, source: &SourceFile, offset: usize) -> Pos {
        let line = source.line_starts.partition_point(|&start| start <= offset) - 1;
        Pos { line: line as u32 + 1, col: (offset - source.line_starts[line]) as u32 + 1 }
    }
}

/// A macro invocation in the source text, as byte offsets.
struct Invocation {
    /// One past the closing parenthesis, or the macro's name when it has no arguments.
    end: usize,
    /// The start and end of each argument, without the space around it.
    arguments: Vec<(usize, usize)>,
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    starts
}

/// The absolute path of a file named in the dump. Symbolic links are resolved the way slang does,
/// so the URI matches the one slang-server reports.
fn absolute(root: &Path, name: &str) -> PathBuf {
    canonical(&root.join(name))
}

/// The path with symbolic links resolved. For a file that doesn't exist, the folder it would be in
/// is resolved (the nearest one that does exist) and the rest is just tidied.
pub fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }
    let path = normalize(path);
    let mut rest = Vec::new();
    let mut folder = path.as_path();
    while let Some(parent) = folder.parent() {
        rest.extend(folder.file_name());
        if let Ok(resolved) = fs::canonicalize(parent) {
            return rest.iter().rev().fold(resolved, |resolved, name| resolved.join(name));
        }
        folder = parent;
    }
    path
}

/// Removes `.` and `..` components without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// The path of a `file:` URI, with its percent-encoding undone. A host (`file://localhost/tmp/x`)
/// is not part of the path.
pub fn uri_path(uri: &str) -> Option<PathBuf> {
    let decoded = decode(uri);
    let rest = decoded.strip_prefix("file://")?;
    let path = if rest.starts_with('/') { rest } else { &rest[rest.find('/')?..] };
    Some(PathBuf::from(path))
}

fn decode(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|hex| std::str::from_utf8(hex).ok());
        match (bytes[i], hex.and_then(|hex| u8::from_str_radix(hex, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file://` and the path with everything but unreserved characters percent-encoded.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.to_string_lossy().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(text: &str) -> Sources {
        Sources {
            files: vec![Arc::new(SourceFile {
                path: PathBuf::new(),
                uri: String::new(),
                text: Some(text.to_string()),
                line_starts: line_starts(text),
            })],
            lookups: Mutex::default(),
        }
    }

    #[test]
    fn columns_count_utf16_units() {
        // "é" is two bytes and one unit, "😀" four bytes and two units.
        let sources = sources("a\n// é😀 x\n");
        assert_eq!(sources.position(0, Pos { line: 2, col: 1 }), (1, 0));
        assert_eq!(sources.position(0, Pos { line: 2, col: 4 }), (1, 3));
        assert_eq!(sources.position(0, Pos { line: 2, col: 10 }), (1, 6));
    }

    #[test]
    fn macro_arguments_are_found_by_name() {
        let sources = sources("`ASSIGN( dst , src );\n");
        let at = Pos { line: 1, col: 1 };
        let src = sources.macro_argument(0, at, "src").unwrap();
        assert_eq!((src.start.col, src.end.col), (16, 19));
        assert!(sources.macro_argument(0, at, "other").is_none());
        assert!(sources.macro_argument(0, Pos { line: 1, col: 2 }, "src").is_none());
        let all = sources.macro_invocation(0, at).unwrap();
        assert_eq!((all.start.col, all.end.col), (1, 21));
    }

    #[test]
    fn a_uri_through_a_symbolic_link_finds_the_file() {
        let dir = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("svtrace-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("real")).unwrap();
        fs::write(dir.join("real/top.sv"), "module top; endmodule\n").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        let sources = Sources::load(&dir, &["link/top.sv".to_string(), "real/other.sv".to_string()]);
        let real = file_uri(&dir.join("real/top.sv"));
        assert_eq!(sources.file_by_uri(&real), Some(0));
        assert_eq!(sources.file_by_uri(&file_uri(&dir.join("link/top.sv"))), Some(0));
        assert_eq!(sources.file_by_uri(&file_uri(&dir.join("real/../link/top.sv"))), Some(0));
        assert_eq!(sources.file_by_uri(&file_uri(&dir.join("link/none.sv"))), None);
        // (A file that doesn't exist is found as it is written.)
        assert_eq!(sources.file_by_uri(&file_uri(&dir.join("real/other.sv"))), Some(1));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(file_uri(Path::new("/a b/c.sv")), "file:///a%20b/c.sv");
    }

    #[test]
    fn a_uris_path_leaves_out_the_host() {
        assert_eq!(uri_path("file:///a%20b/c.sv"), Some(PathBuf::from("/a b/c.sv")));
        assert_eq!(uri_path("file://localhost/tmp/c.sv"), Some(PathBuf::from("/tmp/c.sv")));
        assert_eq!(uri_path("file://host"), None);
        assert_eq!(uri_path("https://example.org/c.sv"), None);
    }

    #[test]
    fn a_column_past_the_end_of_a_line_stays_on_the_line() {
        let sources = sources("ab\ncdef\n");
        assert_eq!(sources.position(0, Pos { line: 1, col: 3 }), (0, 2));
        assert_eq!(sources.position(0, Pos { line: 1, col: 9 }), (0, 2));
        assert_eq!(sources.position(0, Pos { line: 2, col: 9 }), (1, 4));
    }

    #[test]
    fn a_file_two_readers_name_is_read_once() {
        let dir = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("svtrace-texts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.sv"), "module a; endmodule\n").unwrap();
        fs::write(dir.join("b.sv"), "module b; endmodule\n").unwrap();
        let texts = Texts::default();
        let one = Sources::load_shared(&dir, &["a.sv".to_string(), "b.sv".to_string()], &texts);
        let other = Sources::load_shared(&dir, &["b.sv".to_string()], &texts);
        assert!(one.shares_files_with(&other));
        assert_eq!(other.line(0, 1), Some("module b; endmodule"));
        // (Without the shared texts each reads its own.)
        assert!(!one.shares_files_with(&Sources::load(&dir, &["b.sv".to_string()])));
        let _ = fs::remove_dir_all(dir);
    }
}
