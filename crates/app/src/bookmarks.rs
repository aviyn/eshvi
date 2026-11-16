//! Bookmarked folders for the explorer's root: kept across runs in
//! `$XDG_STATE_HOME/eshvi/bookmarks.json` (default `~/.local/state/eshvi/`;
//! `%APPDATA%\eshvi\` where there is no home, as `config::path` does).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file on disk.
#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    folders: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub struct Bookmarks {
    /// Where it is saved; none keeps the list in memory only (tests).
    file: Option<PathBuf>,
    folders: Vec<PathBuf>,
}

/// The default place for the file.
pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("eshvi").join("bookmarks.json"))
}

impl Bookmarks {
    /// From the default file; a note for the user when it is unreadable
    /// (a missing file is an empty list, not a problem).
    pub fn load() -> (Self, Option<String>) {
        match path() {
            Some(file) => Self::load_from(file),
            None => (Self::default(), None),
        }
    }

    pub fn load_from(file: PathBuf) -> (Self, Option<String>) {
        let read = match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str::<Stored>(&text).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Stored::default()),
            Err(e) => Err(e.to_string()),
        };
        match read {
            Ok(stored) => {
                let mut folders: Vec<PathBuf> = Vec::new();
                for f in stored.folders {
                    if !folders.contains(&f) {
                        folders.push(f);
                    }
                }
                (Self { file: Some(file), folders }, None)
            }
            // Kept as it is until the next change overwrites it.
            Err(e) => (Self { file: Some(file.clone()), folders: Vec::new() }, Some(format!("eshvi: ignoring {}: {e}", file.display()))),
        }
    }

    pub fn folders(&self) -> &[PathBuf] {
        &self.folders
    }

    pub fn contains(&self, folder: &Path) -> bool {
        self.folders.iter().any(|f| f == folder)
    }

    /// Bookmark a folder (once); `Err` says why it was not saved.
    pub fn add(&mut self, folder: &Path) -> Result<(), String> {
        if !self.contains(folder) {
            self.folders.push(folder.to_path_buf());
            self.save()?;
        }
        Ok(())
    }

    pub fn remove(&mut self, folder: &Path) -> Result<(), String> {
        if self.contains(folder) {
            self.folders.retain(|f| f != folder);
            self.save()?;
        }
        Ok(())
    }

    /// Written to a file aside, then renamed over the real one.
    fn save(&self) -> Result<(), String> {
        let Some(file) = &self.file else { return Ok(()) };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = serde_json::to_string_pretty(&Stored { folders: self.folders.clone() }).map_err(std::io::Error::other)?;
            let aside = file.with_extension("json.tmp");
            std::fs::write(&aside, text)?;
            std::fs::rename(&aside, file)
        };
        write().map_err(|e| format!("eshvi: bookmarks not saved to {}: {e}", file.display()))
    }
}

/// The longest a folder is shown beside its name (a menu stays narrow).
const SHOWN: usize = 32;

/// The folder as shown beside its name: `~` for the home folder, a long path
/// cut at its front.
pub fn display(folder: &Path) -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let full = match home.as_deref().and_then(|h| folder.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    };
    let chars: Vec<char> = full.chars().collect();
    if chars.len() <= SHOWN {
        return full;
    }
    format!("…{}", chars[chars.len() - (SHOWN - 1)..].iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eshvi-bookmarks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_missing_file_is_an_empty_list() {
        let dir = temp("missing");
        let (b, note) = Bookmarks::load_from(dir.join("state/eshvi/bookmarks.json"));
        assert!(b.folders().is_empty());
        assert_eq!(note, None);
    }

    #[test]
    fn added_and_removed_folders_are_kept_across_loads() {
        let dir = temp("roundtrip");
        let file = dir.join("state/eshvi/bookmarks.json");
        let (mut b, _) = Bookmarks::load_from(file.clone());
        b.add(Path::new("/a/one")).unwrap();
        b.add(Path::new("/b/two")).unwrap();
        // Once only.
        b.add(Path::new("/a/one")).unwrap();
        assert_eq!(b.folders(), [PathBuf::from("/a/one"), PathBuf::from("/b/two")]);
        // Written aside and renamed: no leftover.
        assert!(file.is_file() && !file.with_extension("json.tmp").exists());

        let (mut again, note) = Bookmarks::load_from(file.clone());
        assert_eq!(note, None);
        assert_eq!(again.folders(), b.folders());
        assert!(again.contains(Path::new("/b/two")));
        again.remove(Path::new("/a/one")).unwrap();
        again.remove(Path::new("/not/there")).unwrap();
        let (last, _) = Bookmarks::load_from(file);
        assert_eq!(last.folders(), [PathBuf::from("/b/two")]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broken_file_is_a_note_and_a_fresh_start() {
        let dir = temp("broken");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bookmarks.json");
        std::fs::write(&file, "{ not json").unwrap();
        let (mut b, note) = Bookmarks::load_from(file.clone());
        assert!(b.folders().is_empty());
        assert!(note.is_some_and(|n| n.contains("bookmarks.json")));
        // The next change replaces it.
        b.add(Path::new("/x")).unwrap();
        assert_eq!(Bookmarks::load_from(file).0.folders(), [PathBuf::from("/x")]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_save_says_so_and_keeps_the_list() {
        let dir = temp("unwritable");
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the folder should be.
        std::fs::write(dir.join("eshvi"), "").unwrap();
        let (mut b, _) = Bookmarks::load_from(dir.join("eshvi/bookmarks.json"));
        assert!(b.add(Path::new("/x")).unwrap_err().contains("not saved"));
        assert!(b.contains(Path::new("/x")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn folders_are_shown_short() {
        assert_eq!(display(Path::new("/srv/a")), "/srv/a");
        let long = display(Path::new("/very/long/path/to/some/deeply/nested/project"));
        assert_eq!(long.chars().count(), SHOWN);
        assert!(long.starts_with('…') && long.ends_with("nested/project"));
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(display(Path::new(&home)), "~");
            assert_eq!(display(&Path::new(&home).join("x")), "~/x");
        }
    }

    #[test]
    fn in_memory_lists_touch_no_disk() {
        let mut b = Bookmarks::default();
        b.add(Path::new("/x")).unwrap();
        assert_eq!(b.folders().len(), 1);
    }
}
