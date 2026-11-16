//! The native tree view: a source in Neovim gives nodes (`Eshvi.tree`,
//! `tree.lua`), this draws them as the explorer draws its rows and keeps what
//! the user did to them: which are open, which is selected, how they are
//! sorted. It only asks: what it wants from Neovim comes back as `Action`s
//! (children of a node opened with none yet, a node chosen, a menu), the app
//! sends them. The Outline (`explorer_view.rs`) is the first view.
//!
//! Expansion and selection are kept by node id, across a new set of nodes, so
//! a refresh after typing doesn't fold or move anything. So are the groups of
//! nodes a side view's Filter menu hides: the nodes stay here, only their rows
//! go, so a toggle is instant and asks nothing. The Filter by Name field is the
//! same: it only hides rows, here, and asks Neovim for nothing.

use std::collections::HashSet;

use egui::{Align, Align2, Color32, FontId, Key, Layout, Modifiers, Pos2, Rect, RichText, ScrollArea, Sense, Vec2};
use nvim_bridge::gui::{TreeNode, TreeSet};

use crate::explorer_view::{paint_chevron, paint_row_bg, row_surface, truncated, ExplorerColors, INDENT_PER_PT};
use crate::style;

/// Space before the first and after the last thing in a row, as the explorer's.
const MARGIN: f32 = 10.0;

#[cfg(test)]
mod tests;

/// How a node was chosen: a click or Enter (the keyboard goes to the editor),
/// or Space (it stays in the tree).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Open,
    Peek,
}

impl How {
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Peek => "peek",
        }
    }
}

/// What the view asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// `id` was opened with no children yet: ask for them, under `gen`.
    Expand { id: String, gen: i64 },
    Accept { id: String, how: How },
    /// The right-click menu of `id`.
    Menu { id: String },
    /// Item `index` (from 1) of that menu.
    Run { id: String, index: usize },
    /// A header button of a side view (its place, from 1).
    Button { index: usize, selected: Option<String>, expanded: Vec<String> },
    /// A key the source listed (`keys`) was pressed; `id` is the selected node, if any.
    Key { id: Option<String>, key: String },
    /// A toggle of the Filter menu changed, for Neovim to remember.
    Filter { group: String, shown: bool },
    /// The view was clicked: it takes the keyboard.
    Focus,
    /// Esc: the keyboard goes back to the editor.
    FocusEditor,
}

/// How the children of a node are ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    /// As the source gave them: their place in the file.
    #[default]
    Position,
    /// By name, ignoring case.
    Name,
    /// By `SymbolKind`, then name.
    Kind,
}

impl Sort {
    pub const ALL: [Sort; 3] = [Self::Position, Self::Name, Self::Kind];

    pub fn label(self) -> &'static str {
        match self {
            Self::Position => "Position",
            Self::Name => "Name",
            Self::Kind => "Kind",
        }
    }
}

/// LSP's `SymbolKind`s in their order (by number), for sorting by kind.
const KINDS: [&str; 26] = [
    "File", "Module", "Namespace", "Package", "Class", "Method", "Property", "Field", "Constructor", "Enum", "Interface",
    "Function", "Variable", "Constant", "String", "Number", "Boolean", "Array", "Object", "Key", "Null", "EnumMember",
    "Struct", "Event", "Operator", "TypeParameter",
];

fn kind_order(kind: &str) -> usize {
    KINDS.iter().position(|k| *k == kind).unwrap_or(KINDS.len())
}

/// One visible row: a node (or, with `node` `None`, "Loading…" under its parent).
struct Row<'a> {
    node: Option<&'a TreeNode>,
    depth: usize,
    open: bool,
    /// It has children, or may have.
    branch: bool,
    /// The row of the node it is under.
    parent: Option<usize>,
}

/// The Filter by Name field, open while the user is narrowing the rows.
#[derive(Default)]
struct NameFilter {
    text: String,
    /// Take the keyboard when next drawn.
    focus: bool,
    /// It had the keyboard when last drawn. (egui lets go of a field when Esc comes in, before
    /// the frame is drawn, so asking it then is too late.)
    focused: bool,
}

/// The field's egui id: it keeps the keyboard (the app's other fields do).
pub fn filter_id() -> egui::Id {
    egui::Id::unique("eshvi-tree-filter")
}

/// Whether `label` has `query` (lower case) in it, ignoring case.
fn label_matches(label: &str, query: &str) -> bool {
    label.to_lowercase().contains(query)
}

/// The byte ranges of `label` that are `query` (lower case). (None when lower-casing changes
/// the text's length, which would put them in the wrong places: the label is then not marked.)
fn match_ranges(label: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let lower = label.to_lowercase();
    if lower.len() != label.len() || query.is_empty() {
        return Vec::new();
    }
    lower.match_indices(query).map(|(at, m)| at..at + m.len()).collect()
}

/// A right-click menu being asked for, then shown at `pos`.
struct Menu {
    id: String,
    pos: Pos2,
    items: Option<Vec<String>>,
    /// It has been drawn open (so a later frame without it means it was closed).
    shown: bool,
}

/// What the rows' drawing found, applied once they are done with.
enum Event {
    Click { id: String, chevron: bool },
    Menu { id: String, pos: Pos2 },
}

pub struct TreeView {
    roots: Vec<TreeNode>,
    gen: i64,
    message: Option<String>,
    /// How many levels are open until the user opens or closes one.
    expand: usize,
    opened: HashSet<String>,
    collapsed: HashSet<String>,
    /// Nodes whose children came with the answer to a request (an empty answer
    /// is still an answer).
    loaded: HashSet<String>,
    /// Children asked for, under this gen.
    asked: HashSet<String>,
    /// Nodes that came with `open` and were opened for it: the next time one comes, the user's
    /// choice stands. (A new set of roots starts again.)
    started_open: HashSet<String>,
    selected: Option<String>,
    pub sort: Sort,
    /// A row's hint keeps its room (up to half the row) and the label gives up its end for
    /// it, so where a problem is stays in view; else the label has the row and the hint what
    /// it leaves.
    pub keep_hints: bool,
    /// The groups whose rows the Filter menu hides. (The selected node and the
    /// nodes it is under are drawn anyway: a reveal must show what it found.)
    hidden: HashSet<String>,
    /// Scroll the selected row into view when drawn.
    reveal: bool,
    /// Nodes have a right-click menu in the source.
    has_menu: bool,
    /// Keys the source asked for (besides the tree's own).
    keys: Vec<String>,
    /// The buffer the tree is of, for the app to match with a Markdown preview.
    pub buf: Option<i64>,
    menu: Option<Menu>,
    /// The Filter by Name field, while it is open. (Not kept across runs, and not by view.)
    name_filter: Option<NameFilter>,
    /// Rows that fit the view, for PageUp and PageDown.
    page: usize,
    /// How far the rows are scrolled, as last drawn.
    #[cfg(test)]
    offset: f32,
}

impl Default for TreeView {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeView {
    pub fn new() -> Self {
        Self {
            roots: Vec::new(),
            gen: 0,
            message: None,
            expand: 1,
            opened: HashSet::new(),
            collapsed: HashSet::new(),
            loaded: HashSet::new(),
            asked: HashSet::new(),
            started_open: HashSet::new(),
            selected: None,
            sort: Sort::Position,
            keep_hints: false,
            hidden: HashSet::new(),
            reveal: false,
            has_menu: false,
            keys: Vec::new(),
            buf: None,
            menu: None,
            name_filter: None,
            page: 10,
            #[cfg(test)]
            offset: 0.0,
        }
    }

    #[cfg(test)]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// What Neovim sent: the roots (a new gen), or the children of a node that
    /// were asked for, which an older gen's answer must not replace.
    pub fn set(&mut self, set: TreeSet) {
        if set.merge {
            return self.merge(set);
        }
        match set.parent {
            None => {
                self.gen = set.gen;
                self.roots = set.nodes;
                self.message = set.message;
                self.expand = set.expand.unwrap_or(1);
                self.buf = set.buf;
                self.has_menu = set.menu;
                self.loaded.clear();
                self.asked.clear();
                self.started_open.clear();
                self.menu = None;
                let roots = std::mem::take(&mut self.roots);
                self.open_started(&roots);
                self.roots = roots;
            }
            Some(parent) => {
                if set.gen < self.gen {
                    return;
                }
                self.open_started(&set.nodes);
                if let Some(node) = find_mut(&mut self.roots, &parent) {
                    node.children = set.nodes;
                    self.loaded.insert(parent.clone());
                }
                self.asked.remove(&parent);
            }
        }
    }

    /// Open the nodes that came with `open`, the first time each is received. (Selection stays where it is.)
    fn open_started(&mut self, nodes: &[TreeNode]) {
        for node in nodes {
            if node.open && self.started_open.insert(node.id.clone()) {
                self.open_node(&node.id, true);
            }
            self.open_started(&node.children);
        }
    }

    /// `Eshvi.tree.update`: the nodes are the children of `parent` (or the roots)
    /// merged by id. A node kept keeps its children unless the update brings
    /// some; nodes missing from it go. What the user opened and selected is by
    /// id, so it stays; nothing is asked again.
    fn merge(&mut self, set: TreeSet) {
        match set.parent {
            None => {
                self.open_started(&set.nodes);
                self.roots = merge_nodes(std::mem::take(&mut self.roots), set.nodes);
                self.message = set.message;
                self.has_menu |= set.menu;
            }
            Some(parent) => {
                if set.gen < self.gen {
                    return;
                }
                self.open_started(&set.nodes);
                if let Some(node) = find_mut(&mut self.roots, &parent) {
                    // (An update with none drops them: the node loads them again when next opened.)
                    if set.nodes.is_empty() {
                        node.children.clear();
                        self.loaded.remove(&parent);
                    } else {
                        node.children = merge_nodes(std::mem::take(&mut node.children), set.nodes);
                        self.loaded.insert(parent.clone());
                    }
                }
                self.asked.remove(&parent);
            }
        }
    }

    /// The keys a source asked for while the view has the keyboard.
    pub fn set_keys(&mut self, keys: Vec<String>) {
        self.keys = keys;
    }

    /// Select `id` and show it: its ancestors (`path`, root first) opened,
    /// scrolled into view.
    pub fn select(&mut self, id: &str, path: &[String]) {
        for ancestor in path.iter().filter(|p| p.as_str() != id) {
            self.opened.insert(ancestor.clone());
            self.collapsed.remove(ancestor);
        }
        self.selected = Some(id.to_owned());
        self.reveal = true;
    }

    /// Select the last node that starts at or before `line` (the nodes are in
    /// the file's order, whichever way they are sorted), as `select` does.
    /// Whether the selection changed.
    pub fn select_line(&mut self, line: usize) -> bool {
        let mut best: Option<(usize, Vec<String>)> = None;
        let mut path = Vec::new();
        find_line(&self.roots, line, &mut path, &mut best);
        let Some((_, path)) = best else { return false };
        let id = path.last().cloned().unwrap_or_default();
        if self.selected.as_deref() == Some(id.as_str()) {
            return false;
        }
        self.select(&id, &path);
        true
    }

    /// Fold everything, the top level too.
    pub fn collapse_all(&mut self) {
        self.opened.clear();
        fn all(nodes: &[TreeNode], into: &mut HashSet<String>) {
            for n in nodes {
                if !n.children.is_empty() || n.expandable {
                    into.insert(n.id.clone());
                }
                all(&n.children, into);
            }
        }
        let mut ids = HashSet::new();
        all(&self.roots, &mut ids);
        self.collapsed = ids;
    }

    /// A menu's items, answering `Action::Menu`.
    pub fn set_menu(&mut self, id: &str, items: Vec<String>) {
        if let Some(menu) = self.menu.as_mut().filter(|m| m.id == id) {
            if items.is_empty() {
                self.menu = None;
            } else {
                menu.items = Some(items);
            }
        }
    }

    /// The groups the view starts with hidden.
    pub fn set_hidden(&mut self, groups: impl IntoIterator<Item = String>) {
        self.hidden = groups.into_iter().collect();
    }

    pub fn is_shown(&self, group: &str) -> bool {
        !self.hidden.contains(group)
    }

    /// A toggle of the Filter menu. The selected row, hidden by it, gives the
    /// selection to the nearest node still shown above it.
    pub fn set_shown(&mut self, group: &str, shown: bool) {
        if shown {
            self.hidden.remove(group);
        } else {
            self.hidden.insert(group.to_owned());
            let path = self.selected_path();
            if let Some(first) = path.iter().position(|n| self.hidden.contains(&n.group)) {
                self.selected = first.checked_sub(1).map(|at| path[at].id.clone());
            }
        }
        self.reveal = true;
    }

    /// The selected node and those it is under, root first.
    fn selected_path(&self) -> Vec<&TreeNode> {
        fn find<'a>(nodes: &'a [TreeNode], id: &str, path: &mut Vec<&'a TreeNode>) -> bool {
            for n in nodes {
                path.push(n);
                if n.id == id || find(&n.children, id, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        if let Some(id) = &self.selected {
            find(&self.roots, id, &mut path);
        }
        path
    }

    /// Whether the node has a row. `kept`: `selected_path`.
    fn has_row(&self, node: &TreeNode, kept: &[&TreeNode]) -> bool {
        !self.hidden.contains(&node.group) || kept.iter().any(|k| k.id == node.id)
    }

    /// (All its children hidden, it has nothing to open.)
    fn is_branch(&self, node: &TreeNode, kept: &[&TreeNode]) -> bool {
        node.children.iter().any(|c| self.has_row(c, kept)) || (node.children.is_empty() && node.expandable && !self.loaded.contains(&node.id))
    }

    fn is_open(&self, node: &TreeNode, depth: usize) -> bool {
        if self.opened.contains(&node.id) {
            true
        } else if self.collapsed.contains(&node.id) {
            false
        } else {
            depth < self.expand
        }
    }

    fn sorted<'a>(&self, nodes: &'a [TreeNode]) -> Vec<&'a TreeNode> {
        let mut v: Vec<&TreeNode> = nodes.iter().collect();
        match self.sort {
            Sort::Position => {}
            Sort::Name => v.sort_by_cached_key(|n| n.label.to_lowercase()),
            Sort::Kind => v.sort_by_cached_key(|n| (kind_order(&n.kind), n.label.to_lowercase())),
        }
        v
    }

    /// What the name typed in the field asks for, lower case; none while it is empty or closed.
    fn query(&self) -> Option<String> {
        self.name_filter.as_ref().map(|f| f.text.to_lowercase()).filter(|q| !q.is_empty())
    }

    /// The ids of the nodes a name `query` leaves: those whose label has it and the nodes above
    /// them (under the kind toggles: a node they hide has no rows, nor have its children).
    fn name_matches(&self, nodes: &[TreeNode], query: &str, kept: &[&TreeNode], into: &mut HashSet<String>) -> bool {
        let mut any = false;
        for node in nodes.iter().filter(|n| self.has_row(n, kept)) {
            let below = self.name_matches(&node.children, query, kept, into);
            if below || label_matches(&node.label, query) {
                into.insert(node.id.clone());
                any = true;
            }
        }
        any
    }

    /// Some node that could have children has not been asked for them: a name search can't see them.
    fn has_unloaded(&self) -> bool {
        fn any(this: &TreeView, nodes: &[TreeNode]) -> bool {
            nodes.iter().filter(|n| !this.hidden.contains(&n.group)).any(|n| {
                (n.children.is_empty() && n.expandable && !this.loaded.contains(&n.id)) || any(this, &n.children)
            })
        }
        any(self, &self.roots)
    }

    /// `shown`: with a name query, the nodes it leaves. They are drawn open when something under
    /// them matches, whatever the user has opened (which is left as it is).
    fn walk<'a>(&'a self, nodes: &'a [TreeNode], depth: usize, parent: Option<usize>, kept: &[&TreeNode], shown: Option<&HashSet<String>>, out: &mut Vec<Row<'a>>) {
        for node in self.sorted(nodes).into_iter().filter(|n| self.has_row(n, kept) && shown.is_none_or(|s| s.contains(&n.id))) {
            let (branch, open) = match shown {
                None => {
                    let branch = self.is_branch(node, kept);
                    (branch, branch && self.is_open(node, depth))
                }
                Some(s) => {
                    let below = node.children.iter().any(|c| s.contains(&c.id));
                    let unloaded = node.children.is_empty() && node.expandable && !self.loaded.contains(&node.id);
                    (below || unloaded, below || (unloaded && self.is_open(node, depth)))
                }
            };
            let at = out.len();
            out.push(Row { node: Some(node), depth, open, branch, parent });
            if !open {
                continue;
            }
            if node.children.is_empty() {
                // Asked for, not here yet.
                out.push(Row { node: None, depth: depth + 1, open: false, branch: false, parent: Some(at) });
            } else {
                self.walk(&node.children, depth + 1, Some(at), kept, shown, out);
            }
        }
    }

    fn rows(&self) -> Vec<Row<'_>> {
        self.rows_by(true)
    }

    /// The rows; `named`: narrowed by the name typed in the field.
    fn rows_by(&self, named: bool) -> Vec<Row<'_>> {
        let mut out = Vec::new();
        let kept = if self.hidden.is_empty() { Vec::new() } else { self.selected_path() };
        let shown = self.query().filter(|_| named).map(|q| {
            let mut set = HashSet::new();
            self.name_matches(&self.roots, &q, &kept, &mut set);
            set
        });
        self.walk(&self.roots, 0, None, &kept, shown.as_ref(), &mut out);
        out
    }

    /// Whether the row only holds others (`folder`).
    fn is_folder(&self, id: &str) -> bool {
        self.rows().iter().any(|r| r.node.is_some_and(|n| n.id == id && n.folder))
    }

    /// The selected row stopped being shown by the name (or there is none): the first row shown has it.
    fn fit_selection(&mut self) {
        if self.query().is_none() {
            return;
        }
        let rows = self.rows();
        let ids = rows.iter().filter_map(|r| r.node.map(|n| n.id.as_str()));
        if self.selected.as_deref().is_some_and(|s| ids.clone().any(|i| i == s)) {
            return;
        }
        let first = ids.clone().next().map(str::to_owned);
        drop(rows);
        self.selected = first;
        self.reveal = true;
    }

    /// Open the Filter by Name field and give it the keyboard (`f`, the Filter menu).
    fn open_filter(&mut self) {
        self.name_filter.get_or_insert_with(NameFilter::default).focus = true;
    }

    /// Close the field and forget what was typed: the rows are as they were, the selected one in view.
    fn close_filter(&mut self) {
        self.name_filter = None;
        self.reveal = true;
    }

    /// Ids of the nodes that are open as drawn (shown, and with their children showing).
    pub fn expanded_ids(&self) -> Vec<String> {
        // (As the user left them: the name filter draws some open that aren't.)
        self.rows_by(false).iter().filter(|r| r.open).filter_map(|r| r.node.map(|n| n.id.clone())).collect()
    }

    /// Ids of the rows shown, top to bottom.
    #[cfg(test)]
    pub fn visible_ids(&self) -> Vec<String> {
        self.rows().iter().filter_map(|r| r.node.map(|n| n.id.clone())).collect()
    }

    /// Ask for the children of every open node that has none yet, once each.
    fn request_missing(&mut self) -> Vec<Action> {
        let wanted: Vec<String> = self
            .rows()
            .iter()
            .filter_map(|r| r.node.filter(|n| r.open && n.children.is_empty() && n.expandable))
            .map(|n| n.id.clone())
            .filter(|id| !self.asked.contains(id))
            .collect();
        wanted
            .into_iter()
            .map(|id| {
                self.asked.insert(id.clone());
                Action::Expand { id, gen: self.gen }
            })
            .collect()
    }

    fn open_node(&mut self, id: &str, open: bool) {
        if open {
            self.opened.insert(id.to_owned());
            self.collapsed.remove(id);
        } else {
            self.collapsed.insert(id.to_owned());
            self.opened.remove(id);
        }
    }

    /// Sort the children differently, keeping the selected row in view.
    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        self.reveal = true;
    }

    fn select_row(&mut self, id: String) {
        self.selected = Some(id);
        self.reveal = true;
    }

    /// Keys while the view has the keyboard: the explorer's, and Enter and
    /// Space choosing the node.
    pub fn key(&mut self, key: Key, mods: Modifiers) -> Vec<Action> {
        if mods.any() && !mods.shift {
            return Vec::new();
        }
        let mut actions = Vec::new();
        let page = self.page;
        let (ids, parents, branches, opens): (Vec<String>, Vec<Option<usize>>, Vec<bool>, Vec<bool>) = {
            let rows = self.rows();
            let real: Vec<&Row> = rows.iter().filter(|r| r.node.is_some()).collect();
            // (Parents as places among the nodes, the loading lines left out.)
            let place = |at: usize| rows[..at].iter().filter(|r| r.node.is_some()).count();
            (
                real.iter().map(|r| r.node.map(|n| n.id.clone()).unwrap_or_default()).collect(),
                real.iter().map(|r| r.parent.map(place)).collect(),
                real.iter().map(|r| r.branch).collect(),
                real.iter().map(|r| r.open).collect(),
            )
        };
        // The Filter by Name field: `f` opens it, Esc closes it before it gives the keyboard back.
        if key == Key::F && !mods.shift {
            self.open_filter();
            return actions;
        }
        if key == Key::Escape {
            if self.name_filter.is_some() {
                self.close_filter();
                return actions;
            }
            return vec![Action::FocusEditor];
        }
        // A key the source asked for (the tree's own are not given away).
        if !is_own_key(key) {
            if let Some(name) = key_name(key, mods.shift).filter(|n| self.keys.contains(n)) {
                let id = self.selected.clone().filter(|s| ids.contains(s));
                return vec![Action::Key { id, key: name }];
            }
        }
        if ids.is_empty() {
            return actions;
        }
        let at = self.selected.as_ref().and_then(|s| ids.iter().position(|i| i == s));
        let last = ids.len() - 1;
        let go = |this: &mut Self, i: usize| this.select_row(ids[i.min(last)].clone());
        match key {
            Key::ArrowUp | Key::K => go(self, at.map_or(0, |i| i.saturating_sub(1))),
            Key::ArrowDown | Key::J => go(self, at.map_or(0, |i| i + 1)),
            Key::Home => go(self, 0),
            Key::End => go(self, last),
            Key::PageUp => go(self, at.map_or(0, |i| i.saturating_sub(page))),
            Key::PageDown => go(self, at.map_or(0, |i| i + page)),
            Key::ArrowRight | Key::L => match at {
                None => go(self, 0),
                Some(i) if branches[i] && !opens[i] => self.open_node(&ids[i], true),
                // Open already: to the first child, which comes next in the rows.
                Some(i) if branches[i] && ids.get(i + 1).is_some_and(|_| parents.get(i + 1) == Some(&Some(i))) => go(self, i + 1),
                _ => {}
            },
            Key::ArrowLeft | Key::H => match at {
                None => go(self, 0),
                Some(i) if branches[i] && opens[i] => self.open_node(&ids[i], false),
                Some(i) => {
                    if let Some(Some(p)) = parents.get(i) {
                        go(self, *p);
                    }
                }
            },
            Key::Enter | Key::Space => match at {
                None => go(self, 0),
                Some(i) if self.is_folder(&ids[i]) => self.open_node(&ids[i], !opens[i]),
                Some(i) => {
                    let how = if key == Key::Enter { How::Open } else { How::Peek };
                    actions.push(Action::Accept { id: ids[i].clone(), how });
                }
            },
            _ => {}
        }
        actions
    }
}

/// `new` as the children of a node that had `old`: a node in both keeps the
/// children it had unless the new one brings some (merged the same way).
fn merge_nodes(old: Vec<TreeNode>, new: Vec<TreeNode>) -> Vec<TreeNode> {
    let mut old: std::collections::HashMap<String, TreeNode> = old.into_iter().map(|n| (n.id.clone(), n)).collect();
    new.into_iter()
        .map(|mut n| {
            if let Some(o) = old.remove(&n.id) {
                n.children = if n.children.is_empty() { o.children } else { merge_nodes(o.children, std::mem::take(&mut n.children)) };
            }
            n
        })
        .collect()
}

/// The keys the tree uses itself: a source's can't take them.
fn is_own_key(key: Key) -> bool {
    matches!(
        key,
        Key::ArrowUp
            | Key::ArrowDown
            | Key::ArrowLeft
            | Key::ArrowRight
            | Key::J
            | Key::K
            | Key::H
            | Key::L
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::Enter
            | Key::Space
            | Key::Escape
    )
}

/// A key as a source names it (`keys`): the character it types, upper case with
/// Shift. Only plain characters.
fn key_name(key: Key, shift: bool) -> Option<String> {
    let symbol = key.symbol_or_name();
    let mut chars = symbol.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    if c.is_ascii_alphabetic() {
        return Some(if shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() }.to_string());
    }
    if shift {
        return (key == Key::Slash).then(|| "?".to_owned());
    }
    c.is_ascii_graphic().then(|| c.to_string())
}

/// A hint that only says the label again (an instance named like its module) isn't drawn.
fn shows_hint(node: &TreeNode) -> bool {
    !node.hint.is_empty() && node.hint != node.label
}

fn find_mut<'a>(nodes: &'a mut [TreeNode], id: &str) -> Option<&'a mut TreeNode> {
    for n in nodes {
        if n.id == id {
            return Some(n);
        }
        if let Some(found) = find_mut(&mut n.children, id) {
            return Some(found);
        }
    }
    None
}

/// The last node (in the order given) with `line <= line`, and the ids from
/// the root down to it; of equal lines the later one.
fn find_line(nodes: &[TreeNode], line: usize, path: &mut Vec<String>, best: &mut Option<(usize, Vec<String>)>) {
    for n in nodes {
        path.push(n.id.clone());
        if let Some(l) = n.line.filter(|l| *l <= line) {
            if best.as_ref().is_none_or(|(b, _)| l >= *b) {
                *best = Some((l, path.clone()));
            }
        }
        find_line(&n.children, line, path, best);
        path.pop();
    }
}

/// `text` on one line in `color`, the byte `ranges` of it in `lit`, cut with "…" at `width`
/// (`truncated` with the matches of a name filter marked).
fn marked(p: &egui::Painter, text: &str, ranges: &[std::ops::Range<usize>], font: &FontId, color: Color32, lit: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    let mut at = 0;
    let mut add = |piece: &str, color: Color32| {
        if !piece.is_empty() {
            job.append(piece, 0.0, egui::TextFormat::simple(font.clone(), color));
        }
    };
    for r in ranges {
        add(&text[at..r.start], color);
        add(&text[r.clone()], lit);
        at = r.end;
    }
    add(&text[at..], color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    p.layout_job(job)
}

/// How the view is drawn: the explorer's colors and sizes.
pub struct Look<'a> {
    pub colors: &'a ExplorerColors,
    pub font: FontId,
    pub font_size: f32,
    pub row_h: f32,
}

impl TreeView {
    /// Draw the rows in `ui`, in at most `max_height` (scrolling), `focused`
    /// when the view has the keyboard. Returns what it asks for.
    pub fn show(&mut self, ui: &mut egui::Ui, look: &Look, focused: bool, max_height: f32) -> Vec<Action> {
        let mut actions = self.request_missing();
        let colors = look.colors;
        let small = style::scaled(&look.font, style::SMALL);
        let indent = (look.font_size * INDENT_PER_PT).round();
        let icon_size = (look.font_size * crate::explorer_view::ICON_PER_PT).round();
        let reveal = std::mem::take(&mut self.reveal);
        let mut events = Vec::new();
        let mut viewport = max_height;
        // The Filter by Name field is the first thing in the tree; the rows have what it leaves.
        let mut max_height = max_height;
        if self.name_filter.is_some() {
            let (asked, used) = self.filter_field(ui, look);
            actions.extend(asked);
            max_height = (max_height - used).max(0.0);
            viewport = max_height;
        }
        self.fit_selection();
        if let (Some(message), true) = (&self.message, self.roots.is_empty()) {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::hover());
            let at = Pos2::new(rect.min.x + MARGIN, rect.center().y);
            ui.painter().text(at, Align2::LEFT_CENTER, message, small.clone(), colors.dim);
        } else {
            let rows = self.rows();
            if rows.is_empty() && self.query().is_some() {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::hover());
                ui.painter().text(Pos2::new(rect.min.x + MARGIN, rect.center().y), Align2::LEFT_CENTER, "No matches", small.clone(), colors.dim);
            }
            let query = self.query();
            // The kind badges are as wide as their widest label.
            let badge_w = ui.painter().layout_no_wrap("mod".into(), small.clone(), Color32::WHITE).size().x + style::GAP;
            let badge_h = (small.size * 1.3).round();
            let shown = ScrollArea::vertical().id_salt("eshvi-tree").max_height(max_height).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                for row in &rows {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::CLICK);
                    // (Before the rows out of sight are skipped: that is where it scrolls from.)
                    if reveal && row.node.is_some_and(|n| self.selected.as_deref() == Some(n.id.as_str())) {
                        resp.scroll_to_me(None);
                    }
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let x = rect.min.x + MARGIN + row.depth as f32 * indent;
                    let cy = rect.center().y;
                    let Some(node) = row.node else {
                        // A node's children on their way.
                        let at = Pos2::new(x + 14.0, cy);
                        ui.painter().text(at, Align2::LEFT_CENTER, "Loading…", small.clone(), colors.dim);
                        continue;
                    };
                    let selected = self.selected.as_deref() == Some(node.id.as_str());
                    let hovered = resp.hovered();
                    paint_row_bg(ui.painter(), colors, rect, selected, focused, hovered);
                    for d in 0..row.depth {
                        let gx = rect.min.x + MARGIN + d as f32 * indent + 5.0;
                        ui.painter().vline(gx, rect.y_range(), egui::Stroke::new(style::HAIRLINE, colors.guide));
                    }
                    if row.branch {
                        paint_chevron(ui.painter(), Pos2::new(x + 5.0, cy), row.open, colors.dim);
                    }
                    // The icon (a glyph in its color) or the kind's badge, then the label.
                    let mut left = x + 14.0;
                    let surface = row_surface(colors, selected, focused, hovered);
                    if !node.icon.is_empty() {
                        // (The group's color, kept readable on a light panel as the badges' are.)
                        let color = node.fg.map_or(colors.fg, |c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8));
                        let color = crate::popup::with_contrast(color, surface, colors.fg, crate::popup::MIN_CONTRAST);
                        let at = Pos2::new(left + badge_w / 2.0, cy);
                        ui.painter().text(at, Align2::CENTER_CENTER, &node.icon, look.font.clone(), color);
                        left += badge_w + style::GAP;
                    } else if let Some((label, color)) = (!node.kind.is_empty()).then(|| crate::picker::symbol_badge(&node.kind)) {
                        let badge = Rect::from_center_size(Pos2::new(left + badge_w / 2.0, cy), Vec2::new(badge_w, badge_h));
                        // (The kinds' colors are the completion menu's: kept readable on a light panel.)
                        let color = crate::popup::with_contrast(color, surface, colors.fg, crate::popup::MIN_CONTRAST);
                        crate::popupmenu::paint_badge_on(ui.painter(), surface, badge, label, color, &small);
                        left += badge_w + style::GAP;
                    }
                    let right = rect.max.x - MARGIN;
                    let kept = (self.keep_hints && shows_hint(node)).then(|| truncated(ui.painter(), &node.hint, &small, colors.dim, (right - left) / 2.0));
                    let label_room = right - left - kept.as_ref().map_or(0.0, |h| h.size().x + style::GAP);
                    let label = match query.as_deref() {
                        Some(q) => {
                            let lit = crate::popup::with_contrast(colors.accent, surface, colors.fg, crate::popup::MIN_CONTRAST);
                            marked(ui.painter(), &node.label, &match_ranges(&node.label, q), &look.font, colors.fg, lit, label_room)
                        }
                        None => truncated(ui.painter(), &node.label, &look.font, colors.fg, label_room),
                    };
                    let label_w = label.size().x;
                    ui.painter().galley(Pos2::new(left, cy - label.size().y / 2.0), label.clone(), colors.fg);
                    // The hint, dim, in what the label leaves.
                    let room = right - left - label_w - style::GAP;
                    if let Some(hint) = kept {
                        let at = Pos2::new(left + label_w + style::GAP, cy - hint.size().y / 2.0);
                        ui.painter().galley(at, hint, colors.dim);
                    } else if shows_hint(node) && room > icon_size {
                        let hint = truncated(ui.painter(), &node.hint, &small, colors.dim, room);
                        let at = Pos2::new(left + label_w + style::GAP, cy - hint.size().y / 2.0);
                        ui.painter().galley(at, hint, colors.dim);
                    }
                    let resp = if node.tooltip.is_empty() { resp } else { resp.on_hover_text(&node.tooltip) };
                    // A double click opens once: its first click did.
                    if resp.clicked() && !resp.double_clicked() {
                        let chevron = resp.interact_pointer_pos().is_some_and(|p| p.x < x + 12.0);
                        events.push(Event::Click { id: node.id.clone(), chevron: chevron && row.branch });
                    } else if resp.secondary_clicked() && self.has_menu {
                        events.push(Event::Menu { id: node.id.clone(), pos: resp.interact_pointer_pos().unwrap_or(rect.center()) });
                    }
                }
            });
            viewport = shown.inner_rect.height();
            #[cfg(test)]
            {
                self.offset = shown.state.clamped_offset().y;
            }
        }
        self.page = ((viewport / look.row_h) as usize).max(1);
        for event in events {
            match event {
                Event::Click { id, chevron } => {
                    actions.push(Action::Focus);
                    if chevron || self.is_folder(&id) {
                        let open = self.rows().iter().find(|r| r.node.is_some_and(|n| n.id == id)).is_some_and(|r| r.open);
                        self.open_node(&id, !open);
                        self.selected = Some(id);
                    } else {
                        self.selected = Some(id.clone());
                        actions.push(Action::Accept { id, how: How::Open });
                    }
                }
                Event::Menu { id, pos } => {
                    actions.push(Action::Focus);
                    self.selected = Some(id.clone());
                    self.menu = Some(Menu { id: id.clone(), pos, items: None, shown: false });
                    actions.push(Action::Menu { id });
                }
            }
        }
        actions.extend(self.show_menu(ui));
        actions
    }

    /// A tree that has the side bar to itself: the source's buttons and Collapse All
    /// at the right end of `header` (the side bar's row), then the rows in what is left.
    pub fn show_side(&mut self, ui: &mut egui::Ui, header: Rect, info: &crate::side_bar::SideInfo, look: &Look, icons: &mut crate::icons::IconTextures, focused: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        let size = style::icon_size(look.row_h);
        // Collapse All last, as VS Code has it; the source's before it, the first at the left.
        let (rect, resp, color) = crate::scm_view::title_button(ui, look.colors, look.row_h, header, 0.0, "tree-collapse", "Collapse All", true);
        icons.paint_codicon(ui.painter(), "collapse-all", rect.center(), size, color);
        if resp.clicked() {
            self.collapse_all();
        }
        for (i, button) in info.buttons.iter().enumerate() {
            let n = (info.buttons.len() - i) as f32;
            let (rect, resp, color) = crate::scm_view::title_button(ui, look.colors, look.row_h, header, n, &format!("tree-button-{}-{i}", info.view), &button.tooltip, true);
            icons.paint_codicon(ui.painter(), &button.icon, rect.center(), size, color);
            if resp.clicked() {
                actions.push(Action::Button { index: i + 1, selected: self.selected.clone(), expanded: self.expanded_ids() });
            }
        }
        if !info.filters.is_empty() {
            actions.extend(self.filter_button(ui, header, info, look, icons));
        }
        let height = ui.available_height();
        // (Its own scroll position, apart from the Outline's, which draws the same way.)
        actions.extend(ui.push_id(("eshvi-tree-side", &info.view), |ui| self.show(ui, look, focused, height)).inner);
        actions
    }

    /// The Filter by Name field and, under it, the line saying not everything is searched. Its keys
    /// while it has the keyboard: Up and Down move the selection, Enter chooses it (and the editor
    /// gets the keyboard), Esc closes the field. Returns what it asks for and the height it took.
    fn filter_field(&mut self, ui: &mut egui::Ui, look: &Look) -> (Vec<Action>, f32) {
        let id = filter_id();
        let colors = look.colors;
        let mut actions = Vec::new();
        if self.name_filter.as_ref().is_some_and(|f| f.focused) {
            // (Before the field is added: it would take Enter and Esc to let go of the keyboard itself.)
            for key in [Key::ArrowUp, Key::ArrowDown, Key::Enter, Key::Escape] {
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, key)) {
                    let before = actions.len();
                    actions.extend(self.key(key, Modifiers::NONE));
                    // (Enter on a row that only opens leaves the keyboard here.)
                    if key == Key::Escape || actions[before..].iter().any(|a| matches!(a, Action::Accept { .. })) {
                        ui.memory_mut(|m| m.surrender_focus(id));
                    }
                }
            }
        }
        let Some(filter) = &mut self.name_filter else { return (actions, 0.0) };
        // As the explorer's name field: in the flow, a pixel in from the row's edges.
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::hover());
        let field = Rect::from_min_max(Pos2::new(rect.min.x + 6.0, rect.min.y + 1.0), Pos2::new(rect.max.x - 6.0, rect.max.y - 1.0));
        let text_h = ui.ctx().fonts_mut(|f| f.row_height(&look.font));
        let margin = egui::Margin::symmetric(style::GAP as i8, ((field.height() - text_h) / 2.0).floor().max(0.0) as i8);
        let resp = ui
            .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                style::text_field(ui, &colors.chrome(), id, margin, |ui| {
                    let edit = egui::TextEdit::singleline(&mut filter.text)
                        .id(id)
                        .font(look.font.clone())
                        .text_color(colors.fg)
                        .hint_text(RichText::new("Filter").font(look.font.clone()).color(colors.dim))
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE);
                    ui.add(edit)
                })
                .0
            })
            .inner;
        if std::mem::take(&mut filter.focus) {
            resp.request_focus();
        }
        filter.focused = resp.has_focus();
        // (A click into the field takes the keyboard for the view too, so Esc ends here.)
        if resp.gained_focus() {
            actions.push(Action::Focus);
        }
        let mut used = rect.height();
        if self.query().is_some() && self.has_unloaded() {
            let small = style::scaled(&look.font, style::SMALL);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h * 0.8), Sense::hover());
            ui.painter().text(Pos2::new(rect.min.x + MARGIN, rect.center().y), Align2::LEFT_CENTER, "Only searching opened items", small, colors.dim);
            used += rect.height();
        }
        (actions, used)
    }

    /// The Filter button, left of the source's, filled while something is hidden, and
    /// its menu of toggles, which stays open for the next one.
    fn filter_button(&mut self, ui: &mut egui::Ui, header: Rect, info: &crate::side_bar::SideInfo, look: &Look, icons: &mut crate::icons::IconTextures) -> Vec<Action> {
        let mut actions = Vec::new();
        let (rect, resp, color) = crate::scm_view::title_button(ui, look.colors, look.row_h, header, info.buttons.len() as f32 + 1.0, &format!("tree-filter-{}", info.view), "Filter", true);
        let active = info.filters.iter().any(|f| !self.is_shown(&f.id));
        icons.paint_codicon(ui.painter(), if active { "filter-filled" } else { "filter" }, rect.center(), style::icon_size(look.row_h), color);
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            if ui.button("Filter by Name…").clicked() {
                self.open_filter();
                // (The view takes the keyboard, so Esc gives it back when the field is closed.)
                actions.push(Action::Focus);
                ui.close();
            }
            ui.separator();
            ui.label(egui::RichText::new("Show").small().weak());
            for filter in &info.filters {
                let mut on = self.is_shown(&filter.id);
                if ui.checkbox(&mut on, &filter.label).changed() {
                    self.set_shown(&filter.id, on);
                    actions.push(Action::Filter { group: filter.id.clone(), shown: on });
                }
            }
        });
        actions
    }

    /// The right-click menu, once Neovim has said what is in it.
    fn show_menu(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let Some(menu) = &mut self.menu else { return Vec::new() };
        let Some(items) = &menu.items else { return Vec::new() };
        let popup_id = ui.scope_id().with("eshvi-tree-menu");
        let first = !menu.shown;
        let open = first.then_some(egui::SetOpenCommand::Bool(true));
        let mut chosen = None;
        let shown = egui::Popup::new(popup_id, ui.ctx().clone(), egui::PopupAnchor::Position(menu.pos), ui.layer_id())
            .kind(egui::PopupKind::Menu)
            .layout(Layout::top_down_justified(Align::Min))
            .style(egui::containers::menu::menu_style)
            .gap(0.0)
            .open_memory(open)
            .show(|ui| {
                for (i, label) in items.iter().enumerate() {
                    if ui.button(label).clicked() {
                        chosen = Some(i + 1);
                    }
                }
            });
        menu.shown = true;
        let id = menu.id.clone();
        if shown.is_none() && !first {
            self.menu = None;
        }
        chosen.map(|index| Action::Run { id, index }).into_iter().collect()
    }
}
