//! File tree model for the native explorer: lazily loaded directories,
//! folders first, `.gitignore` respected, and a watcher that reports
//! changes in loaded directories.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, RwLock};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Loaded children (directories only; `None` until first expanded).
    pub children: Option<Vec<Entry>>,
    pub expanded: bool,
}

/// One visible line of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// A file with related files nested under it (`Cargo.lock` under
    /// `Cargo.toml`): whether they are shown.
    pub nest: Option<bool>,
}

pub struct Tree {
    pub root: PathBuf,
    entries: Vec<Entry>,
    pub show_ignored: bool,
    /// Nest related files under a main one, like VS Code's file nesting.
    pub nesting: bool,
    /// Files whose nested files are shown.
    nest_open: HashSet<PathBuf>,
}

/// File nesting: (parent, child) name patterns; `*` is the same text in
/// both. A child shows under its parent when both are in one folder.
const NESTING: &[(&str, &str)] = &[
    ("Cargo.toml", "Cargo.lock"),
    ("package.json", "package-lock.json"),
    ("package.json", "yarn.lock"),
    ("package.json", "pnpm-lock.yaml"),
    ("package.json", "bun.lock"),
    ("package.json", "bun.lockb"),
    ("package.json", ".npmrc"),
    ("go.mod", "go.sum"),
    ("pyproject.toml", "poetry.lock"),
    ("pyproject.toml", "uv.lock"),
    ("Gemfile", "Gemfile.lock"),
    ("flake.nix", "flake.lock"),
    ("composer.json", "composer.lock"),
    (".gitignore", ".gitattributes"),
    (".gitignore", ".gitmodules"),
    ("Dockerfile", ".dockerignore"),
    ("*.ts", "*.d.ts"),
    ("*.ts", "*.js"),
    ("*.ts", "*.js.map"),
    ("*.tsx", "*.js"),
    ("*.js", "*.js.map"),
    ("*.js", "*.min.js"),
    ("*.css", "*.css.map"),
    ("*.css", "*.min.css"),
];

/// The file `name` nests under, among the names of its folder.
pub fn nest_parent<'a>(name: &str, siblings: &HashSet<&'a str>) -> Option<&'a str> {
    for (parent, child) in NESTING {
        let candidate = match child.split_once('*') {
            Some((pre, post)) => {
                let Some(stem) = name.strip_prefix(pre).and_then(|r| r.strip_suffix(post)).filter(|s| !s.is_empty()) else {
                    continue;
                };
                parent.replacen('*', stem, 1)
            }
            None if *child == name => (*parent).to_owned(),
            None => continue,
        };
        if candidate != name {
            if let Some(found) = siblings.get(candidate.as_str()) {
                return Some(found);
            }
        }
    }
    None
}

/// List a directory: folders first, then files, case-insensitively by name.
/// `.git` is always hidden; ignored files are hidden unless `show_ignored`.
pub fn list_dir(dir: &Path, show_ignored: bool) -> Vec<Entry> {
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .ignore(!show_ignored)
        .git_ignore(!show_ignored)
        .git_global(!show_ignored)
        .git_exclude(!show_ignored)
        .build();
    let mut entries: Vec<Entry> = walker
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .filter(|e| e.file_name() != ".git")
        .map(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir())
                || (e.path_is_symlink() && e.path().is_dir());
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path().to_path_buf(),
                is_dir,
                children: None,
                expanded: false,
            }
        })
        .collect();
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries
}

fn find_mut<'a>(entries: &'a mut [Entry], path: &Path) -> Option<&'a mut Entry> {
    for e in entries {
        if e.path == path {
            return Some(e);
        }
        if e.is_dir && path.starts_with(&e.path) {
            if let Some(children) = &mut e.children {
                return find_mut(children, path);
            }
        }
    }
    None
}

fn find<'a>(entries: &'a [Entry], path: &Path) -> Option<&'a Entry> {
    for e in entries {
        if e.path == path {
            return Some(e);
        }
        if e.is_dir && path.starts_with(&e.path) {
            if let Some(children) = &e.children {
                return find(children, path);
            }
        }
    }
    None
}

/// Re-list `entries` from disk, keeping the expanded/loaded state of
/// directories that still exist: their loaded subtrees move over (by path,
/// not searched for and copied: this runs for every change on disk).
fn merge(old: Vec<Entry>, mut fresh: Vec<Entry>) -> Vec<Entry> {
    let mut old: HashMap<PathBuf, (bool, Option<Vec<Entry>>)> =
        old.into_iter().filter(|o| o.is_dir).map(|o| (o.path, (o.expanded, o.children))).collect();
    for f in fresh.iter_mut().filter(|f| f.is_dir) {
        if let Some((expanded, children)) = old.remove(&f.path) {
            f.expanded = expanded;
            f.children = children;
        }
    }
    fresh
}

impl Tree {
    pub fn new(root: PathBuf, show_ignored: bool) -> Self {
        let entries = list_dir(&root, show_ignored);
        Self { root, entries, show_ignored, nesting: true, nest_open: HashSet::new() }
    }

    /// Visible rows, depth first through expanded directories; nested files
    /// under their main file while it is open.
    pub fn rows(&self) -> Vec<Row> {
        fn walk(tree: &Tree, entries: &[Entry], depth: usize, out: &mut Vec<Row>) {
            // Which files nest under which (one level: not under a file that
            // is itself nested).
            let names: HashSet<&str> = entries.iter().filter(|e| !e.is_dir).map(|e| e.name.as_str()).collect();
            let parent_of = |e: &Entry| if tree.nesting && !e.is_dir { nest_parent(&e.name, &names) } else { None };
            let nested: Vec<(&Entry, &str)> = entries
                .iter()
                .filter_map(|e| Some((e, parent_of(e)?)))
                .filter(|(_, p)| entries.iter().find(|e| e.name == *p).and_then(parent_of).is_none())
                .collect();
            for e in entries {
                if nested.iter().any(|(n, _)| n.path == e.path) {
                    continue;
                }
                let kids: Vec<&Entry> = nested.iter().filter(|(_, p)| *p == e.name).map(|(n, _)| *n).collect();
                let open = tree.nest_open.contains(&e.path);
                out.push(Row {
                    path: e.path.clone(),
                    name: e.name.clone(),
                    depth,
                    is_dir: e.is_dir,
                    expanded: e.expanded,
                    nest: (!kids.is_empty()).then_some(open),
                });
                if open {
                    for k in kids {
                        out.push(Row {
                            path: k.path.clone(),
                            name: k.name.clone(),
                            depth: depth + 1,
                            is_dir: false,
                            expanded: false,
                            nest: None,
                        });
                    }
                }
                if e.expanded {
                    if let Some(children) = &e.children {
                        walk(tree, children, depth + 1, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(self, &self.entries, 0, &mut out);
        out
    }

    /// Show or hide the files nested under `path`.
    #[cfg(test)]
    pub fn toggle_nest(&mut self, path: &Path) {
        if !self.nest_open.remove(path) {
            self.nest_open.insert(path.to_path_buf());
        }
    }

    pub fn set_nest(&mut self, path: &Path, open: bool) {
        if open {
            self.nest_open.insert(path.to_path_buf());
        } else {
            self.nest_open.remove(path);
        }
    }

    pub fn set_expanded(&mut self, path: &Path, expanded: bool) {
        let show_ignored = self.show_ignored;
        if let Some(e) = find_mut(&mut self.entries, path).filter(|e| e.is_dir) {
            if expanded && e.children.is_none() {
                e.children = Some(list_dir(&e.path, show_ignored));
            }
            e.expanded = expanded;
        }
    }

    pub fn toggle(&mut self, path: &Path) {
        let expanded = self.is_expanded(path);
        self.set_expanded(path, !expanded);
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        find(&self.entries, path).is_some_and(|e| e.is_dir && e.expanded)
    }

    /// Expand every ancestor of `path` so it becomes visible. Returns false
    /// if `path` is outside the root.
    pub fn reveal(&mut self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else { return false };
        let mut dir = self.root.clone();
        let parts: Vec<_> = rel.components().collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            dir.push(part);
            self.set_expanded(&dir, true);
        }
        // A nested file: its main file's group opens.
        if self.nesting && !self.rows().iter().any(|r| r.path == path) {
            if let (Some(name), Some(parent_dir)) = (path.file_name().and_then(|n| n.to_str()), path.parent()) {
                let siblings: Vec<String> = self
                    .rows()
                    .iter()
                    .filter(|r| r.path.parent() == Some(parent_dir) && !r.is_dir)
                    .map(|r| r.name.clone())
                    .collect();
                let set: HashSet<&str> = siblings.iter().map(String::as_str).chain([name]).collect();
                if let Some(p) = nest_parent(name, &set) {
                    self.nest_open.insert(parent_dir.join(p));
                }
            }
        }
        true
    }

    pub fn collapse_all(&mut self) {
        fn walk(entries: &mut [Entry]) {
            for e in entries {
                e.expanded = false;
                if let Some(c) = &mut e.children {
                    walk(c);
                }
            }
        }
        walk(&mut self.entries);
    }

    /// The expanded folders, parents before their children (a session keeps them).
    pub fn expanded_dirs(&self) -> Vec<PathBuf> {
        fn walk(entries: &[Entry], out: &mut Vec<PathBuf>) {
            for e in entries.iter().filter(|e| e.is_dir && e.expanded) {
                out.push(e.path.clone());
                if let Some(c) = &e.children {
                    walk(c, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.entries, &mut out);
        out
    }

    /// Exactly these folders expanded (a session's), in the order given; those that are
    /// gone or outside the root are skipped.
    pub fn set_expanded_dirs(&mut self, dirs: &[PathBuf]) {
        self.collapse_all();
        for dir in dirs {
            self.set_expanded(dir, true);
        }
    }

    /// Re-read a directory (the root or a loaded one) after a change on disk.
    pub fn refresh(&mut self, dir: &Path) {
        let show_ignored = self.show_ignored;
        if dir == self.root {
            let old = std::mem::take(&mut self.entries);
            self.entries = merge(old, list_dir(&self.root, show_ignored));
        } else if let Some(e) = find_mut(&mut self.entries, dir) {
            if let Some(old) = e.children.take() {
                e.children = Some(merge(old, list_dir(dir, show_ignored)));
            }
        }
    }

    /// Directories whose listing is shown (for the watcher).
    pub fn loaded_dirs(&self) -> HashSet<PathBuf> {
        fn walk(entries: &[Entry], out: &mut HashSet<PathBuf>) {
            for e in entries {
                if let Some(children) = &e.children {
                    out.insert(e.path.clone());
                    walk(children, out);
                }
            }
        }
        let mut out = HashSet::from([self.root.clone()]);
        walk(&self.entries, &mut out);
        out
    }
}

/// How long after the first change of a burst the changed directories are
/// read again.
pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

/// The directories the watcher reported, taken together a moment after the
/// first: a build or a checkout changes one folder hundreds of times in a
/// row, and each used to be a listing of it in the frame.
#[derive(Default)]
pub struct Settle {
    dirs: Vec<PathBuf>,
    since: Option<std::time::Instant>,
}

impl Settle {
    pub fn add(&mut self, dirs: impl IntoIterator<Item = PathBuf>, now: std::time::Instant) {
        self.dirs.extend(dirs);
        if !self.dirs.is_empty() {
            self.since.get_or_insert(now);
        }
    }

    /// The directories to read again now (each once), none when nothing
    /// changed; `Err`: how long the burst is still given.
    pub fn take(&mut self, now: std::time::Instant) -> Result<Vec<PathBuf>, std::time::Duration> {
        let Some(since) = self.since else { return Ok(Vec::new()) };
        if let Some(wait) = SETTLE.checked_sub(now.saturating_duration_since(since)).filter(|w| !w.is_zero()) {
            return Err(wait);
        }
        self.since = None;
        let mut dirs = std::mem::take(&mut self.dirs);
        dirs.sort();
        dirs.dedup();
        Ok(dirs)
    }
}

/// Watches the root recursively and reports directories (among
/// `loaded`) that changed. `wake` is called only for those.
pub struct TreeWatcher {
    _watcher: notify::RecommendedWatcher,
    pub changed: mpsc::Receiver<PathBuf>,
    pub loaded: Arc<RwLock<HashSet<PathBuf>>>,
}

impl TreeWatcher {
    pub fn new(root: &Path, wake: impl Fn() + Send + 'static) -> notify::Result<Self> {
        use notify::Watcher;
        let (tx, rx) = mpsc::channel();
        let loaded: Arc<RwLock<HashSet<PathBuf>>> = Arc::default();
        let loaded2 = loaded.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            let loaded = loaded2.read().unwrap();
            let mut woke = false;
            for path in event.paths {
                let Some(dir) = path.parent() else { continue };
                if loaded.contains(dir) {
                    let _ = tx.send(dir.to_path_buf());
                    woke = true;
                }
            }
            if woke {
                wake();
            }
        })?;
        watcher.watch(root, notify::RecursiveMode::Recursive)?;
        Ok(Self { _watcher: watcher, changed: rx, loaded })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("eshvi-explorer-{}-{}", std::process::id(), rand_suffix()));
        for d in ["src/nested", "docs", ".git", "target"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["README.md", "b.rs", "A.rs", "src/main.rs", "src/nested/deep.rs", "target/out.o", ".env"] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        root
    }

    /// Unique per test in this process (tests run in parallel).
    fn rand_suffix() -> usize {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn a_burst_of_changes_is_one_reading_a_moment_later() {
        let t0 = std::time::Instant::now();
        let at = |ms: u64| t0 + std::time::Duration::from_millis(ms);
        let mut s = Settle::default();
        assert_eq!(s.take(t0), Ok(vec![]));
        s.add([PathBuf::from("/r/b"), PathBuf::from("/r/a")], t0);
        assert_eq!(s.take(at(10)), Err(std::time::Duration::from_millis(90)));
        // More of the burst: the wait counts from its first change.
        s.add([PathBuf::from("/r/a"), PathBuf::from("/r/a")], at(60));
        assert_eq!(s.take(at(70)), Err(std::time::Duration::from_millis(30)));
        assert_eq!(s.take(at(100)), Ok(vec![PathBuf::from("/r/a"), PathBuf::from("/r/b")]));
        assert_eq!(s.take(at(500)), Ok(vec![]));
        // The next burst starts its own wait.
        s.add([PathBuf::from("/r/a")], at(600));
        assert!(s.take(at(650)).is_err());
        assert_eq!(s.take(at(700)), Ok(vec![PathBuf::from("/r/a")]));
    }

    #[test]
    fn refreshing_keeps_loaded_subtrees_and_what_is_open() {
        let root = temp_tree();
        let mut t = Tree::new(root.clone(), true);
        t.set_expanded(&root.join("src"), true);
        t.set_expanded(&root.join("src/nested"), true);
        assert!(t.is_expanded(&root.join("src/nested")));
        assert!(!t.is_expanded(&root.join("docs")));
        assert!(!t.is_expanded(&root.join("b.rs")));
        // The root and a loaded folder change on disk: a file comes, a folder goes.
        std::fs::write(root.join("new.rs"), "x").unwrap();
        std::fs::write(root.join("src/added.rs"), "x").unwrap();
        std::fs::remove_dir_all(root.join("docs")).unwrap();
        t.refresh(&root);
        t.refresh(&root.join("src"));
        assert_eq!(
            names(&t),
            ["src", "  nested", "    deep.rs", "  added.rs", "  main.rs", "target", ".env", ".gitignore", "A.rs", "b.rs", "new.rs", "README.md"]
        );
        assert!(t.is_expanded(&root.join("src/nested")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn expanded_folders_are_listed_and_set_for_a_session() {
        let root = temp_tree();
        let mut t = Tree::new(root.clone(), true);
        t.set_expanded(&root.join("src"), true);
        t.set_expanded(&root.join("src/nested"), true);
        t.set_expanded(&root.join("docs"), true);
        let saved = t.expanded_dirs();
        // Parents come before their children, so they can be set in this order.
        assert_eq!(saved, [root.join("docs"), root.join("src"), root.join("src/nested")]);
        // Another tree of the same root gets them back, and nothing else stays open.
        let mut u = Tree::new(root.clone(), true);
        u.set_expanded(&root.join("target"), true);
        let mut with_gone = saved.clone();
        with_gone.push(root.join("gone"));
        with_gone.push(PathBuf::from("/elsewhere"));
        u.set_expanded_dirs(&with_gone);
        assert_eq!(u.expanded_dirs(), saved);
        assert_eq!(names(&u), names(&t));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn names(t: &Tree) -> Vec<String> {
        t.rows().iter().map(|r| format!("{}{}", "  ".repeat(r.depth), r.name)).collect()
    }

    #[test]
    fn lists_folders_first_and_skips_git() {
        let root = temp_tree();
        let t = Tree::new(root.clone(), true);
        assert_eq!(names(&t), ["docs", "src", "target", ".env", ".gitignore", "A.rs", "b.rs", "README.md"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn respects_gitignore_in_repos() {
        let root = temp_tree();
        // `ignore` only applies .gitignore inside a git repository.
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let t = Tree::new(root.clone(), false);
        assert!(!names(&t).contains(&"target".to_string()), "{:?}", names(&t));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn related_files_nest_under_their_main_file() {
        let names_of = |list: &[&'static str]| list.iter().copied().collect::<HashSet<&str>>();
        let set = names_of(&["Cargo.toml", "Cargo.lock", "app.ts", "app.js", "app.d.ts", "app.js.map", "util.js", "util.min.js", "lone.js"]);
        assert_eq!(nest_parent("Cargo.lock", &set), Some("Cargo.toml"));
        assert_eq!(nest_parent("app.js", &set), Some("app.ts"));
        assert_eq!(nest_parent("app.d.ts", &set), Some("app.ts"));
        assert_eq!(nest_parent("util.min.js", &set), Some("util.js"));
        assert_eq!(nest_parent("lone.js", &set), None, "no lone.ts");
        assert_eq!(nest_parent("Cargo.toml", &set), None);

        let root = temp_tree();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("Cargo.lock"), "").unwrap();
        let mut t = Tree::new(root.clone(), true);
        let n = names(&t);
        assert!(n.contains(&"Cargo.toml".to_string()) && !n.contains(&"Cargo.lock".to_string()), "{n:?}");
        assert_eq!(t.rows().iter().find(|r| r.name == "Cargo.toml").unwrap().nest, Some(false));
        t.toggle_nest(&root.join("Cargo.toml"));
        let n = names(&t);
        let at = n.iter().position(|r| r == "Cargo.toml").unwrap();
        assert_eq!(n[at + 1], "  Cargo.lock");
        // Revealing a nested file opens its group.
        t.toggle_nest(&root.join("Cargo.toml"));
        assert!(t.reveal(&root.join("Cargo.lock")));
        assert!(names(&t).contains(&"  Cargo.lock".to_string()));
        // Off: flat.
        t.nesting = false;
        assert!(names(&t).contains(&"Cargo.lock".to_string()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn expand_reveal_and_refresh_keep_state() {
        let root = temp_tree();
        let mut t = Tree::new(root.clone(), true);
        assert!(t.reveal(&root.join("src/nested/deep.rs")));
        assert_eq!(
            names(&t),
            ["docs", "src", "  nested", "    deep.rs", "  main.rs", "target", ".env", ".gitignore", "A.rs", "b.rs", "README.md"]
        );
        assert!(!t.reveal(Path::new("/elsewhere/file.rs")));

        std::fs::write(root.join("src/new.rs"), "x").unwrap();
        std::fs::write(root.join("c.rs"), "x").unwrap();
        t.refresh(&root.join("src"));
        t.refresh(&root);
        let n = names(&t);
        assert!(n.contains(&"  new.rs".to_string()) && n.contains(&"c.rs".to_string()), "{n:?}");
        assert!(n.contains(&"    deep.rs".to_string()), "expansion kept: {n:?}");

        t.toggle(&root.join("src"));
        assert!(!names(&t).contains(&"  main.rs".to_string()));
        assert!(t.loaded_dirs().contains(&root.join("src/nested")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
