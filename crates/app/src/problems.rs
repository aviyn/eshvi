//! The Problems list's data: the diagnostics Neovim reported (`gui_problems`),
//! which of them the view shows (scope, severities, the filter text), and
//! the rows as tree nodes. No drawing: `tree_view` draws the nodes and the
//! bottom panel's strip has the controls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nvim_bridge::gui::{Problem, TreeNode};

/// Errors, warnings and infos (hints are left out by Neovim's side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Info,
}

impl Severity {
    pub const ALL: [Severity; 3] = [Self::Error, Self::Warn, Self::Info];

    pub fn index(self) -> usize {
        self as usize
    }

    fn from_number(n: u8) -> Self {
        match n {
            2 => Self::Warn,
            3 => Self::Info,
            _ => Self::Error,
        }
    }

    /// The letter the strip's toggles and `:EshviProblems severity` know it by.
    pub fn letter(self) -> char {
        ['E', 'W', 'I'][self.index()]
    }

    pub fn from_letter(c: char) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.letter() == c.to_ascii_uppercase())
    }
}

/// Whose problems are listed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Workspace,
    /// The file being edited.
    File,
}

/// One diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub file: PathBuf,
    pub lnum: usize,
    pub col: usize,
    pub severity: Severity,
    pub message: String,
    pub source: String,
    pub code: String,
}

impl Item {
    fn new(p: &Problem) -> Self {
        Self {
            file: PathBuf::from(&p.file),
            lnum: p.lnum.max(1),
            col: p.col.max(1),
            severity: Severity::from_number(p.severity),
            message: one_line(&p.message),
            source: p.source.clone(),
            code: p.code.clone(),
        }
    }

    /// `source(code)`, whichever of them there is.
    fn origin(&self) -> String {
        match (self.source.is_empty(), self.code.is_empty()) {
            (false, false) => format!("{}({})", self.source, self.code),
            (false, true) => self.source.clone(),
            (true, false) => format!("({})", self.code),
            (true, true) => String::new(),
        }
    }

    /// What a row says after the message: `source(code) [Ln 3, Col 5]`.
    fn detail(&self) -> String {
        let place = format!("[Ln {}, Col {}]", self.lnum, self.col);
        match self.origin() {
            o if o.is_empty() => place,
            o => format!("{o} {place}"),
        }
    }
}

/// A message on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The problems of one file as listed: the file, and the items it has left
/// after the filters (indices into the model's items).
#[derive(Debug, PartialEq, Eq)]
pub struct Group {
    pub file: PathBuf,
    pub name: String,
    /// Its folder, relative to the root when under it ("" for the root itself).
    pub dir: String,
    pub items: Vec<usize>,
}

/// The rows as tree nodes, and what each problem node stands for.
pub struct Rows {
    pub nodes: Vec<TreeNode>,
    /// A problem node's id and its item.
    pub problems: HashMap<String, usize>,
}

#[derive(Debug, Default)]
pub struct Model {
    items: Vec<Item>,
    /// Which severities are listed (errors, warnings, infos).
    pub shown: [bool; 3],
    pub filter: String,
    pub scope: Scope,
    /// The file being edited, for the file scope.
    pub current: Option<PathBuf>,
    /// The workspace's folder, which file folders are shown relative to.
    pub root: Option<PathBuf>,
}

impl Model {
    pub fn new() -> Self {
        Self { shown: [true; 3], ..Default::default() }
    }

    /// What Neovim reported: replaces everything, kept in file and position order.
    pub fn set(&mut self, problems: &[Problem]) {
        self.items = problems.iter().map(Item::new).collect();
        self.items.sort_by(|a, b| (&a.file, a.lnum, a.col).cmp(&(&b.file, b.lnum, b.col)));
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn toggle(&mut self, s: Severity) {
        self.shown[s.index()] = !self.shown[s.index()];
    }

    /// Whether an item is in the scope.
    fn in_scope(&self, item: &Item) -> bool {
        match self.scope {
            Scope::Workspace => true,
            Scope::File => self.current.as_deref() == Some(item.file.as_path()),
        }
    }

    /// Errors, warnings and infos in the scope, before the toggles and the
    /// filter (the strip's counts).
    pub fn counts(&self) -> [usize; 3] {
        let mut counts = [0; 3];
        for item in self.items.iter().filter(|i| self.in_scope(i)) {
            counts[item.severity.index()] += 1;
        }
        counts
    }

    /// Errors plus warnings in the whole workspace, whatever the scope (the
    /// tab's badge, and which colour it takes: errors first).
    pub fn badge(&self) -> (usize, usize) {
        let count = |s| self.items.iter().filter(|i| i.severity == s).count();
        (count(Severity::Error), count(Severity::Warn))
    }

    /// "Showing N of M": the current file's errors and warnings of the workspace's.
    pub fn showing(&self) -> (usize, usize) {
        let ew = |i: &&Item| i.severity != Severity::Info;
        let file = self.items.iter().filter(ew).filter(|i| self.current.as_deref() == Some(i.file.as_path())).count();
        (file, self.items.iter().filter(ew).count())
    }

    fn matches(&self, item: &Item) -> bool {
        if !self.shown[item.severity.index()] {
            return false;
        }
        let needle = self.filter.trim().to_lowercase();
        needle.is_empty()
            || format!("{} {} {}", item.message, item.file.display(), item.source).to_lowercase().contains(&needle)
    }

    /// The items listed, by file (in path order), each with its items.
    pub fn groups(&self) -> Vec<Group> {
        let mut groups: Vec<Group> = Vec::new();
        for (i, item) in self.items.iter().enumerate().filter(|(_, it)| self.in_scope(it) && self.matches(it)) {
            match groups.last_mut().filter(|g| g.file == item.file) {
                Some(g) => g.items.push(i),
                None => groups.push(Group {
                    file: item.file.clone(),
                    name: item.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    dir: self.folder(&item.file),
                    items: vec![i],
                }),
            }
        }
        groups
    }

    /// The file's folder as shown dimmed after its name.
    fn folder(&self, file: &Path) -> String {
        let dir = file.parent().unwrap_or(file);
        match self.root.as_deref() {
            Some(root) if dir == root => String::new(),
            Some(root) => match dir.strip_prefix(root) {
                Ok(rel) => rel.to_string_lossy().into_owned(),
                Err(_) => home_relative(dir),
            },
            None => home_relative(dir),
        }
    }

    /// Why nothing is listed, when nothing is.
    pub fn empty_note(&self) -> Option<&'static str> {
        if !self.groups().is_empty() {
            return None;
        }
        let total: usize = self.counts().iter().sum();
        let filtered = !self.filter.trim().is_empty() || self.shown.iter().any(|s| !s);
        Some(if filtered && total > 0 {
            "No problems match the filter."
        } else if self.scope == Scope::File {
            "No problems in this file."
        } else {
            "No problems in the workspace."
        })
    }

    /// The rows: a node per file (open unless folded by the view), with its
    /// problems, in the colors of the severities.
    pub fn rows(&self, colors: [u32; 3]) -> Rows {
        let mut problems = HashMap::new();
        let nodes = self
            .groups()
            .into_iter()
            .map(|g| {
                let children = g
                    .items
                    .iter()
                    .map(|&i| {
                        let item = &self.items[i];
                        // (Two problems alike at one place stay two rows.)
                        let mut id = format!("p:{}:{}:{}:{}", item.file.display(), item.lnum, item.col, item.severity.letter());
                        while problems.contains_key(&id) {
                            id.push('+');
                        }
                        problems.insert(id.clone(), i);
                        TreeNode {
                            id,
                            label: item.message.clone(),
                            hint: item.detail(),
                            icon: "●".into(),
                            fg: Some(colors[item.severity.index()]),
                            tooltip: format!("{} {}", item.message, item.detail()),
                            ..Default::default()
                        }
                    })
                    .collect();
                let count = g.items.len();
                TreeNode {
                    id: format!("f:{}", g.file.display()),
                    label: g.name.clone(),
                    hint: if g.dir.is_empty() { count.to_string() } else { format!("{}  {count}", g.dir) },
                    folder: true,
                    open: true,
                    expandable: true,
                    tooltip: g.file.display().to_string(),
                    children,
                    ..Default::default()
                }
            })
            .collect();
        Rows { nodes, problems }
    }
}

fn home_relative(dir: &Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from).and_then(|h| dir.strip_prefix(h).ok().map(|r| r.to_path_buf())) {
        Some(rel) => format!("~/{}", rel.display()).trim_end_matches('/').to_owned(),
        None => dir.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(file: &str, lnum: usize, col: usize, severity: u8, message: &str, source: &str, code: &str) -> Problem {
        Problem {
            file: file.into(),
            lnum,
            col,
            end_lnum: lnum,
            end_col: col + 1,
            severity,
            message: message.into(),
            source: source.into(),
            code: code.into(),
        }
    }

    fn sample() -> Model {
        let mut m = Model::new();
        m.root = Some("/p".into());
        m.set(&[
            problem("/p/src/b.rs", 9, 1, 2, "unused variable", "rustc", "unused"),
            problem("/p/src/a.rs", 3, 5, 1, "mismatched types\n  expected `u8`", "rust-analyzer", "E0308"),
            problem("/p/a.lua", 1, 1, 3, "global x", "", ""),
            problem("/p/src/a.rs", 1, 2, 1, "cannot find value", "rust-analyzer", ""),
            problem("/q/other.rs", 2, 2, 2, "far away", "", ""),
        ]);
        m.current = Some("/p/src/a.rs".into());
        m
    }

    #[test]
    fn problems_are_grouped_by_file_in_path_order_and_by_position() {
        let m = sample();
        let groups = m.groups();
        let files: Vec<_> = groups.iter().map(|g| g.file.to_string_lossy().into_owned()).collect();
        assert_eq!(files, ["/p/a.lua", "/p/src/a.rs", "/p/src/b.rs", "/q/other.rs"]);
        let a = &groups[1];
        assert_eq!((a.name.as_str(), a.dir.as_str()), ("a.rs", "src"));
        let places: Vec<_> = a.items.iter().map(|&i| (m.items()[i].lnum, m.items()[i].col)).collect();
        assert_eq!(places, [(1, 2), (3, 5)]);
        // The workspace's own folder is empty; another folder is shown whole.
        assert_eq!(groups[0].dir, "");
        assert_eq!(groups[3].dir, "/q");
    }

    #[test]
    fn a_message_is_one_line_and_a_row_says_where_it_comes_from() {
        let m = sample();
        let rows = m.rows([1, 2, 3]);
        let file = &rows.nodes[1];
        assert_eq!((file.label.as_str(), file.hint.as_str()), ("a.rs", "src  2"));
        assert!(file.folder && file.open);
        let p = &file.children[1];
        assert_eq!(p.label, "mismatched types expected `u8`");
        assert_eq!(p.hint, "rust-analyzer(E0308) [Ln 3, Col 5]");
        assert_eq!((p.icon.as_str(), p.fg), ("●", Some(1)));
        // No code: the source alone; neither: the place alone.
        assert_eq!(file.children[0].hint, "rust-analyzer [Ln 1, Col 2]");
        assert_eq!(rows.nodes[0].children[0].hint, "[Ln 1, Col 1]");
        assert_eq!(rows.nodes[0].hint, "1");
        // Rows are found again by id.
        let item = rows.problems[&p.id];
        assert_eq!(m.items()[item].message, "mismatched types expected `u8`");
    }

    #[test]
    fn severity_toggles_hide_problems_but_not_the_counts() {
        let mut m = sample();
        assert_eq!(m.counts(), [2, 2, 1]);
        m.toggle(Severity::Warn);
        assert_eq!(m.counts(), [2, 2, 1]);
        let shown: usize = m.groups().iter().map(|g| g.items.len()).sum();
        assert_eq!(shown, 3);
        m.toggle(Severity::Error);
        m.toggle(Severity::Info);
        assert_eq!(m.groups().len(), 0);
        assert_eq!(m.empty_note(), Some("No problems match the filter."));
        m.toggle(Severity::Warn);
        assert_eq!(m.groups().len(), 2);
    }

    #[test]
    fn the_filter_looks_in_the_message_the_file_and_the_source() {
        let mut m = sample();
        let listed = |m: &Model| m.groups().iter().map(|g| g.items.len()).sum::<usize>();
        m.filter = "MISMATCHED".into();
        assert_eq!(listed(&m), 1);
        m.filter = "b.rs".into();
        assert_eq!(listed(&m), 1);
        m.filter = "rust-analyzer".into();
        assert_eq!(listed(&m), 2);
        m.filter = "  ".into();
        assert_eq!(listed(&m), 5);
        m.filter = "nothing like it".into();
        assert_eq!(m.empty_note(), Some("No problems match the filter."));
    }

    #[test]
    fn the_file_scope_lists_the_current_file_and_counts_it() {
        let mut m = sample();
        m.scope = Scope::File;
        assert_eq!(m.groups().len(), 1);
        assert_eq!(m.counts(), [2, 0, 0]);
        // "Showing 2 of 4": the file's errors and warnings of the workspace's.
        assert_eq!(m.showing(), (2, 4));
        m.current = Some("/p/src/c.rs".into());
        assert_eq!(m.empty_note(), Some("No problems in this file."));
        m.current = None;
        assert_eq!(m.showing(), (0, 4));
    }

    #[test]
    fn the_badge_counts_errors_and_warnings_of_the_whole_workspace() {
        let mut m = sample();
        m.scope = Scope::File;
        assert_eq!(m.badge(), (2, 2));
        m.toggle(Severity::Error);
        assert_eq!(m.badge(), (2, 2));
        m.set(&[]);
        assert_eq!(m.badge(), (0, 0));
        assert_eq!(m.empty_note(), Some("No problems in this file."));
        m.scope = Scope::Workspace;
        assert_eq!(m.empty_note(), Some("No problems in the workspace."));
    }

    #[test]
    fn problems_alike_at_one_place_keep_their_own_rows() {
        let mut m = Model::new();
        m.set(&[problem("/p/a.rs", 1, 1, 1, "same", "", ""), problem("/p/a.rs", 1, 1, 1, "same", "", "")]);
        let rows = m.rows([0; 3]);
        assert_eq!(rows.problems.len(), 2);
        assert_eq!(rows.nodes[0].children.len(), 2);
    }

    #[test]
    fn severities_are_named_by_a_letter() {
        for s in Severity::ALL {
            assert_eq!(Severity::from_letter(s.letter()), Some(s));
        }
        assert_eq!(Severity::from_letter('w'), Some(Severity::Warn));
        assert_eq!(Severity::from_letter('x'), None);
    }
}
