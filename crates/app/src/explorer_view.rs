//! Native file explorer panel (VS Code style): folder tree with Material
//! icons, mouse and keyboard navigation, the current file revealed, and
//! file operations (new, rename, move to Trash) from menus or keys.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, FontId, Id, Key, Modifiers, Pos2, Rect, ScrollArea, Sense, Stroke, Vec2};

use crate::decorations::{DecoColors, Decorations};
use crate::explorer::{Row, Tree, TreeWatcher};
use crate::fonts::GridFonts;
use crate::icons::{IconTextures, IconTheme};
use crate::scm_view::{pane_body, title_button};
use crate::tree_view::{self, TreeView};

#[derive(Clone, Copy, Debug)]
pub struct ExplorerColors {
    /// The panel's.
    pub bg: Color32,
    /// The editor's (a text field in the panel).
    pub editor_bg: Color32,
    pub fg: Color32,
    pub dim: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub accent: Color32,
    pub guide: Color32,
}

impl ExplorerColors {
    /// The side bar's colors from the theme's.
    pub fn from_chrome(chrome: &crate::style::Chrome) -> Self {
        use crate::style;
        Self {
            bg: chrome.panel,
            editor_bg: chrome.bg,
            fg: chrome.fg,
            dim: chrome.dim(chrome.fg),
            hover: chrome.shade(chrome.panel, style::SUBTLE),
            selected: crate::popup::mix(chrome.panel, chrome.accent, style::SELECTED),
            accent: chrome.accent,
            guide: chrome.line(),
        }
    }

    /// The same colors as the chrome's.
    pub fn chrome(&self) -> crate::style::Chrome {
        crate::style::Chrome { bg: self.editor_bg, fg: self.fg, panel: self.bg, accent: self.accent }
    }
}

#[derive(Debug, PartialEq)]
pub enum Action {
    /// Open a file in Neovim (and give it focus).
    Open(PathBuf),
    /// Open a file beside the current window.
    OpenSplit(PathBuf),
    /// Give keyboard focus back to Neovim.
    FocusEditor,
    /// Make a folder the explorer root (and Neovim's working directory).
    SetRoot(PathBuf),
    /// Ask for a folder to use as root (native dialog).
    PickRoot,
    /// A file or folder was renamed; open buffers should follow.
    Renamed { from: PathBuf, to: PathBuf },
    /// A file or folder was moved to the Trash.
    Deleted(PathBuf),
    CopyText(String),
    /// Show in Finder (or the platform's file manager).
    Reveal(PathBuf),
    /// Send a file or folder to the agent as context.
    SendToAgent(PathBuf),
    /// Something failed; show it to the user.
    Error(String),
    /// Something to tell the user that is not a failure.
    Note(String),
    /// A tree view (the Outline) asks something of Neovim.
    Tree { view: String, action: tree_view::Action },
    /// The Outline's Follow Cursor was switched.
    OutlineFollow(bool),
}

/// An inline name editor: a new entry in `dir`, or a rename.
#[derive(Clone, Debug, PartialEq)]
enum EditKind {
    NewFile,
    NewFolder,
    Rename(PathBuf),
}

#[derive(Clone, Debug)]
struct Edit {
    kind: EditKind,
    /// Folder the entry is in.
    dir: PathBuf,
    text: String,
    /// Focus the field (and select the name) on the next frame.
    focus: bool,
}

/// Is `name` usable as a new entry name? `allow_nested` permits `a/b`.
fn valid_name(name: &str, allow_nested: bool) -> bool {
    let parts: Vec<&str> = name.split('/').collect();
    !name.is_empty()
        && !name.starts_with('/')
        && (allow_nested || parts.len() == 1)
        && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != "..")
}

pub struct ExplorerView {
    tree: Option<Tree>,
    watcher: Option<TreeWatcher>,
    /// Changed directories waiting to be read again (`poll`).
    settle: crate::explorer::Settle,
    pub visible: bool,
    /// Keyboard input goes to the explorer instead of Neovim.
    pub focused: bool,
    selected: Option<PathBuf>,
    /// The file shown in Neovim's current window.
    active: Option<PathBuf>,
    scroll_to_selected: bool,
    /// Where a file drag from the Finder is (the app asks: `file_drop`).
    pub drop_pos: Option<Pos2>,
    textures: IconTextures,
    show_ignored: bool,
    /// Related files nested under a main one (`[explorer] nesting`).
    pub nesting: bool,
    pub font_size: f32,
    /// Offer "Send to Agent" (the agent panel exists).
    pub agent: bool,
    /// Folders to switch the root to, in the "⋯" menu.
    pub bookmarks: crate::bookmarks::Bookmarks,
    /// What to tell the user about the bookmarks file, once the window shows.
    pub bookmarks_note: Option<String>,
    /// Drops being copied on worker threads report here.
    copies: (std::sync::mpsc::Sender<CopyDone>, std::sync::mpsc::Receiver<CopyDone>),
    edit: Option<Edit>,
    confirm_delete: Option<Vec<PathBuf>>,
    /// More selected rows (Cmd/Shift-click), the anchor (`selected`) among
    /// them; empty for a single selection.
    multi: Vec<PathBuf>,
    /// Neovim's tree views by name; the Outline is "outline".
    pub trees: BTreeMap<String, TreeView>,
    /// The Outline pane exists (`[outline] enabled`).
    pub outline_enabled: bool,
    pub outline_open: bool,
    /// The height the Outline was dragged to.
    outline_height: Option<f32>,
    /// Of the two panes, the Outline has the keyboard (while `focused`).
    outline_focus: bool,
    /// Follow Cursor, as set in the pane's menu or `[outline]`.
    pub outline_follow: bool,
    /// Where the Outline's title bar was drawn.
    #[cfg(test)]
    outline_header: Rect,
    /// Where the selected file's row was drawn (NOTHING: out of sight).
    #[cfg(test)]
    selected_row: Rect,
}

/// Indent per level and icon size, relative to the font size (12pt and
/// 16pt at the default 13pt), so zoom scales them too.
pub const INDENT_PER_PT: f32 = 12.0 / 13.0;
pub const ICON_PER_PT: f32 = 16.0 / 13.0;

impl ExplorerView {
    /// `[explorer] show_ignored` and `nesting`, changed in the config file.
    pub fn set_filters(&mut self, show_ignored: bool, nesting: bool) {
        self.nesting = nesting;
        if let Some(t) = &mut self.tree {
            t.nesting = nesting;
        }
        if self.show_ignored != show_ignored {
            self.show_ignored = show_ignored;
            if let Some(t) = &mut self.tree {
                t.show_ignored = show_ignored;
            }
            self.refresh_all();
        }
    }

    pub fn new(visible: bool, show_ignored: bool, font_size: f32) -> Self {
        Self {
            tree: None,
            watcher: None,
            settle: Default::default(),
            visible,
            focused: false,
            selected: None,
            active: None,
            scroll_to_selected: false,
            drop_pos: None,
            textures: IconTextures::default(),
            show_ignored,
            nesting: true,
            font_size,
            agent: false,
            bookmarks: Default::default(),
            bookmarks_note: None,
            copies: std::sync::mpsc::channel(),
            edit: None,
            confirm_delete: None,
            multi: Vec::new(),
            trees: BTreeMap::new(),
            outline_enabled: true,
            outline_open: true,
            outline_height: None,
            outline_focus: false,
            outline_follow: true,
            #[cfg(test)]
            outline_header: Rect::NOTHING,
            #[cfg(test)]
            selected_row: Rect::NOTHING,
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.tree.as_ref().map(|t| t.root.as_path())
    }

    /// Show a new root (Neovim's working directory).
    pub fn set_root(&mut self, root: PathBuf, ctx: &egui::Context) {
        // Real paths, like Neovim's buffer names (symlinks resolved).
        let root = root.canonicalize().unwrap_or(root);
        if self.root() == Some(root.as_path()) || !root.is_dir() {
            return;
        }
        let ctx = ctx.clone();
        self.watcher = TreeWatcher::new(&root, move || ctx.request_repaint())
            .map_err(|e| log::warn!("explorer: cannot watch {}: {e}", root.display()))
            .ok();
        let mut tree = Tree::new(root, self.show_ignored);
        tree.nesting = self.nesting;
        self.tree = Some(tree);
        if let Some(active) = self.active.clone() {
            self.reveal(&active);
        }
        self.sync_watched();
    }

    /// Neovim switched to another file: reveal and select it. Without a
    /// root yet (launched from `/`, e.g. from Finder), the file's git root
    /// (or its folder) becomes the root.
    pub fn set_active(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.tree.is_none() {
            if let Some(root) = workspace_root_for(&path) {
                self.active = Some(path.clone());
                self.set_root(root, ctx);
                return;
            }
        }
        if self.active.as_ref() == Some(&path) {
            return;
        }
        self.active = Some(path.clone());
        self.reveal(&path);
    }

    pub fn reveal(&mut self, path: &Path) {
        if let Some(tree) = &mut self.tree {
            if tree.reveal(path) {
                self.selected = Some(path.to_path_buf());
                self.scroll_to_selected = true;
            }
        }
        self.sync_watched();
    }

    /// The expanded folders (a session keeps them).
    pub fn expanded_dirs(&self) -> Vec<PathBuf> {
        self.tree.as_ref().map(|t| t.expanded_dirs()).unwrap_or_default()
    }

    /// Exactly these folders expanded (a session's).
    pub fn set_expanded_dirs(&mut self, dirs: &[PathBuf]) {
        if let Some(tree) = &mut self.tree {
            tree.set_expanded_dirs(dirs);
        }
        self.sync_watched();
    }

    fn sync_watched(&mut self) {
        if let (Some(tree), Some(w)) = (&self.tree, &self.watcher) {
            *w.loaded.write().unwrap() = tree.loaded_dirs();
        }
    }

    /// Apply changes reported by the file watcher, a moment after the first
    /// of a burst (`Settle`). Returns true if anything changed on disk.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let (Some(tree), Some(w)) = (&mut self.tree, &self.watcher) else { return false };
        let now = std::time::Instant::now();
        self.settle.add(w.changed.try_iter(), now);
        let dirs = match self.settle.take(now) {
            Ok(dirs) => dirs,
            Err(wait) => {
                ctx.request_repaint_after(wait);
                return false;
            }
        };
        if dirs.is_empty() {
            return false;
        }
        for d in dirs {
            tree.refresh(&d);
        }
        self.sync_watched();
        true
    }

    fn toggle_dir(&mut self, path: &Path) {
        if let Some(tree) = &mut self.tree {
            tree.toggle(path);
        }
        self.sync_watched();
    }

    fn set_dir(&mut self, path: &Path, expanded: bool) {
        if let Some(tree) = &mut self.tree {
            tree.set_expanded(path, expanded);
        }
        self.sync_watched();
    }

    /// An inline editor or the delete confirmation has the keyboard.
    /// Move `paths` into folder `dir` (dragged there): open buffers follow
    /// (`Renamed`). Not into themselves, not over an existing name.
    fn move_into(&mut self, paths: &[PathBuf], dir: &Path) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut moved = Vec::new();
        for from in paths {
            let Some(name) = from.file_name() else { continue };
            if from.parent() == Some(dir) {
                continue;
            }
            if dir.starts_with(from) {
                actions.push(Action::Error(format!("Can't move {} into itself", from.display())));
                continue;
            }
            let to = dir.join(name);
            match rename_new(from, &to) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    actions.push(Action::Error(format!("{} already exists", to.display())));
                }
                Ok(()) => {
                    if let (Some(tree), Some(parent)) = (&mut self.tree, from.parent()) {
                        tree.refresh(parent);
                    }
                    actions.push(Action::Renamed { from: from.clone(), to: to.clone() });
                    moved.push(to);
                }
                Err(e) => actions.push(Action::Error(format!("Could not move {}: {e}", from.display()))),
            }
        }
        if let Some(last) = moved.last().cloned() {
            self.show_path(dir, &last);
            self.multi = if moved.len() > 1 { moved } else { Vec::new() };
        }
        actions
    }

    /// Copy files dropped from outside into folder `dir` (folders whole).
    /// The copy runs on a worker thread so a big folder doesn't freeze the
    /// window; `drain_copies` reports back.
    fn copy_into(&mut self, paths: &[PathBuf], dir: &Path, ctx: &egui::Context) {
        let (paths, dir, ctx, tx) = (paths.to_vec(), dir.to_path_buf(), ctx.clone(), self.copies.0.clone());
        std::thread::spawn(move || {
            let (last, errors) = copy_paths(&paths, &dir);
            let _ = tx.send(CopyDone { dir, last, errors });
            ctx.request_repaint();
        });
    }

    /// Finished drops: reveal the last copy, report the failures.
    fn drain_copies(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        let done: Vec<CopyDone> = self.copies.1.try_iter().collect();
        for d in done {
            if let Some(last) = d.last {
                self.show_path(&d.dir, &last);
            }
            actions.extend(d.errors.into_iter().map(Action::Error));
        }
        actions
    }

    /// The selected paths: the extra selection, else the selected row.
    fn selection(&self) -> Vec<PathBuf> {
        if self.multi.is_empty() {
            self.selected.iter().cloned().collect()
        } else {
            self.multi.clone()
        }
    }

    fn is_selected(&self, path: &Path) -> bool {
        if self.multi.is_empty() {
            self.selected.as_deref() == Some(path)
        } else {
            self.multi.iter().any(|p| p == path)
        }
    }

    /// What a row's menu acts on: the selection when the row is in it, else
    /// just the row (which becomes the selection).
    fn targets_for(&mut self, row: &Row) -> Vec<PathBuf> {
        if !self.is_selected(&row.path) {
            self.selected = Some(row.path.clone());
            self.multi.clear();
        }
        self.selection()
    }

    fn set_nest(&mut self, path: &Path, open: bool) {
        if let Some(tree) = &mut self.tree {
            tree.set_nest(path, open);
        }
    }

    /// The pane that has the keyboard (while the side bar does) is the Outline.
    pub fn outline_has_keyboard(&self) -> bool {
        self.focused && self.outline_enabled && self.outline_open && self.outline_focus
    }

    /// The files list has the keyboard, or at least the Outline doesn't.
    fn files_focus(&self) -> bool {
        !(self.outline_enabled && self.outline_open && self.outline_focus)
    }

    /// Give the Outline the keyboard (`:EshviOutline focus`, a click in it).
    pub fn focus_outline(&mut self) {
        self.outline_open = true;
        self.focused = true;
        self.outline_focus = true;
    }

    /// The icons drawn from the bundled set (the side bar's header uses them).
    pub fn icons(&mut self) -> &mut IconTextures {
        &mut self.textures
    }

    /// A tree view that has the side bar to itself (`Eshvi.tree.register` with `side`),
    /// below the side bar's `header`.
    pub fn show_tree_side(&mut self, ui: &mut egui::Ui, header: Rect, info: &crate::side_bar::SideInfo, colors: &ExplorerColors, fonts: &GridFonts) -> Vec<Action> {
        let mut font = fonts.sidebar_font_id(false);
        font.size = self.font_size;
        let row_h = crate::side_bar::row_height(self.font_size);
        let look = tree_view::Look { colors, font, font_size: self.font_size, row_h };
        let focused = self.focused;
        let tree = self.trees.entry(info.view.clone()).or_default();
        let asked = tree.show_side(ui, header, info, &look, &mut self.textures, focused);
        let mut actions = Vec::new();
        for action in asked {
            match action {
                tree_view::Action::Focus => self.focused = true,
                tree_view::Action::FocusEditor => actions.push(Action::FocusEditor),
                action => actions.push(Action::Tree { view: info.view.clone(), action }),
            }
        }
        actions
    }

    /// The Outline's tree, made when first asked for.
    pub fn outline(&mut self) -> &mut TreeView {
        self.trees.entry("outline".to_owned()).or_default()
    }

    pub fn wants_keyboard(&self) -> bool {
        self.edit.is_some() || self.confirm_delete.is_some()
    }

    fn selected_row(&self) -> Option<Row> {
        let sel = self.selected.as_ref()?;
        let visible = self.tree.as_ref()?.rows().into_iter().find(|r| &r.path == sel);
        // The selection may be inside a collapsed folder.
        visible.or_else(|| {
            sel.exists().then(|| Row {
                path: sel.clone(),
                name: sel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                depth: 0,
                is_dir: sel.is_dir(),
                expanded: false,
                nest: None,
            })
        })
    }

    /// Folder for a new entry: the selected folder, else the selected
    /// file's folder, else the root.
    fn target_dir(&self, at: Option<&Row>) -> Option<PathBuf> {
        match at {
            Some(r) if r.is_dir => Some(r.path.clone()),
            Some(r) => r.path.parent().map(Path::to_path_buf),
            None => self.root().map(Path::to_path_buf),
        }
    }

    fn start_new(&mut self, folder: bool, at: Option<Row>) {
        let Some(dir) = self.target_dir(at.as_ref()) else { return };
        if self.root() != Some(dir.as_path()) {
            self.set_dir(&dir, true);
        }
        let kind = if folder { EditKind::NewFolder } else { EditKind::NewFile };
        self.edit = Some(Edit { kind, dir, text: String::new(), focus: true });
        self.focused = true;
        self.outline_focus = false;
    }

    fn start_rename(&mut self, path: PathBuf) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else { return };
        let text = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.edit = Some(Edit { kind: EditKind::Rename(path), dir, text, focus: true });
        self.focused = true;
        self.outline_focus = false;
    }

    /// Re-read `dir` and select `path` (after a change made here).
    fn show_path(&mut self, dir: &Path, path: &Path) {
        if let Some(tree) = &mut self.tree {
            tree.refresh(dir);
            tree.reveal(path);
            // A nested new path ("a/b.rs") created folders below `dir`.
            if let Some(parent) = path.parent() {
                tree.refresh(parent);
            }
        }
        self.selected = Some(path.to_path_buf());
        self.scroll_to_selected = true;
        self.sync_watched();
    }

    /// Apply the inline editor's name.
    fn commit_edit(&mut self) -> Vec<Action> {
        let Some(edit) = self.edit.take() else { return vec![] };
        let name = edit.text.trim();
        if name.is_empty() {
            return vec![];
        }
        let nested = !matches!(edit.kind, EditKind::Rename(_));
        if !valid_name(name, nested) {
            return vec![Action::Error(format!("\"{name}\" is not a valid name"))];
        }
        let target = edit.dir.join(name);
        match edit.kind {
            EditKind::Rename(from) => {
                if target == from {
                    return vec![];
                }
                // Only the letters' case changed, on a disk that takes both
                // names for one file (default APFS): the target "exists" and
                // is this very file, so it is renamed as it is.
                let renamed = if same_name_and_file(&from, &target) { std::fs::rename(&from, &target) } else { rename_new(&from, &target) };
                match renamed {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        return vec![Action::Error(format!("{} already exists", target.display()))];
                    }
                    Err(e) => return vec![Action::Error(format!("Rename failed: {e}"))],
                }
                if self.active.as_ref() == Some(&from) {
                    self.active = Some(target.clone());
                }
                self.show_path(&edit.dir, &target);
                vec![Action::Renamed { from, to: target }]
            }
            EditKind::NewFile => {
                if target.exists() {
                    return vec![Action::Error(format!("{} already exists", target.display()))];
                }
                let created = target
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map(|_| ()));
                if let Err(e) = created {
                    return vec![Action::Error(format!("Could not create {}: {e}", target.display()))];
                }
                self.show_path(&edit.dir, &target);
                vec![Action::Open(target)]
            }
            EditKind::NewFolder => {
                if let Err(e) = std::fs::create_dir_all(&target) {
                    return vec![Action::Error(format!("Could not create {}: {e}", target.display()))];
                }
                self.show_path(&edit.dir, &target);
                vec![]
            }
        }
    }

    /// Move a confirmed path to the Trash.
    fn delete_confirmed(&mut self, path: &Path) -> Vec<Action> {
        if let Err(e) = crate::trash_can::move_to_trash(path) {
            return vec![Action::Error(e)];
        }
        // Select the next row (or the previous one at the end).
        if let Some(rows) = self.tree.as_ref().map(Tree::rows) {
            if let Some(i) = rows.iter().position(|r| r.path == path) {
                let after = rows[i + 1..].iter().find(|r| !r.path.starts_with(path));
                let pick = after.or_else(|| rows[..i].last());
                self.selected = pick.map(|r| r.path.clone());
            }
        }
        if let (Some(tree), Some(parent)) = (&mut self.tree, path.parent()) {
            tree.refresh(parent);
        }
        self.sync_watched();
        vec![Action::Deleted(path.to_path_buf())]
    }

    fn relative(&self, path: &Path) -> String {
        self.root()
            .and_then(|r| path.strip_prefix(r).ok())
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    /// Context menu entries for a row (or the root title when `row` is None).
    fn menu(&mut self, ui: &mut egui::Ui, row: Option<&Row>, actions: &mut Vec<Action>) {
        let is_dir = row.is_none_or(|r| r.is_dir);
        if let Some(r) = row.filter(|r| !r.is_dir) {
            if ui.button("Open").clicked() {
                actions.push(Action::Open(r.path.clone()));
            }
            if ui.button("Open to the Side").clicked() {
                actions.push(Action::OpenSplit(r.path.clone()));
            }
            ui.separator();
        }
        if ui.button("New File…").clicked() {
            self.start_new(false, row.cloned());
        }
        if ui.button("New Folder…").clicked() {
            self.start_new(true, row.cloned());
        }
        ui.separator();
        let path = row.map(|r| r.path.clone()).or_else(|| self.root().map(Path::to_path_buf));
        let targets = row.map(|r| self.targets_for(r)).unwrap_or_default();
        let many = targets.len() > 1;
        if let (true, Some(_)) = (self.agent, row) {
            if ui.button("Send to Agent").clicked() {
                actions.extend(targets.iter().cloned().map(Action::SendToAgent));
            }
            ui.separator();
        }
        if let Some(path) = path.clone() {
            if is_dir && row.is_some() && ui.button("Open as Root").clicked() {
                actions.push(Action::SetRoot(path.clone()));
            }
            // (A folder row: that folder; the header: the root.)
            if is_dir {
                let marked = self.bookmarks.contains(&path);
                if ui.button(if marked { "Remove Bookmark" } else { "Bookmark This Folder" }).clicked() {
                    let done = if marked { self.bookmarks.remove(&path) } else { self.bookmarks.add(&path) };
                    actions.extend(done.err().map(Action::Error));
                }
            }
            if ui.button("Reveal in Finder").clicked() {
                actions.push(Action::Reveal(path.clone()));
            }
            // (Several selected: one per line.)
            let all = if many { targets.clone() } else { vec![path.clone()] };
            if ui.button(if many { "Copy Paths" } else { "Copy Path" }).clicked() {
                let text: Vec<String> = all.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                actions.push(Action::CopyText(text.join("\n")));
            }
            if row.is_some() && ui.button(if many { "Copy Relative Paths" } else { "Copy Relative Path" }).clicked() {
                let text: Vec<String> = all.iter().map(|p| self.relative(p)).collect();
                actions.push(Action::CopyText(text.join("\n")));
            }
        }
        if let Some(r) = row {
            ui.separator();
            if !many && ui.button("Rename…").clicked() {
                self.start_rename(r.path.clone());
            }
            if ui.button("Move to Trash").clicked() {
                self.confirm_delete = Some(targets.clone());
            }
        } else {
            ui.separator();
            if ui.button("Refresh").clicked() {
                self.refresh_all();
            }
            if ui.button("Collapse All").clicked() {
                if let Some(t) = &mut self.tree {
                    t.collapse_all();
                }
            }
            ui.separator();
            // Ignored files (.gitignore) shown or not; related files nested.
            let mut ignored = self.show_ignored;
            if ui.checkbox(&mut ignored, "Show Ignored Files").changed() {
                self.show_ignored = ignored;
                if let Some(t) = &mut self.tree {
                    t.show_ignored = ignored;
                }
                self.refresh_all();
            }
            let mut nesting = self.tree.as_ref().is_none_or(|t| t.nesting);
            if ui.checkbox(&mut nesting, "Nest Related Files").changed() {
                self.nesting = nesting;
                if let Some(t) = &mut self.tree {
                    t.nesting = nesting;
                }
            }
            ui.separator();
            if ui.button("Open Folder…").clicked() {
                actions.push(Action::PickRoot);
            }
            if let Some(parent) = self.root().and_then(Path::parent) {
                if ui.button("Go to Parent Folder").clicked() {
                    actions.push(Action::SetRoot(parent.to_path_buf()));
                }
            }
            actions.extend(self.bookmark_items(ui));
        }
    }

    /// The bookmarks by name, the folder beside it dimmed. A folder that is
    /// gone shows dimmed whole, and clicking it removes the bookmark.
    fn bookmark_items(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.bookmarks.folders().is_empty() {
            return actions;
        }
        ui.separator();
        for folder in self.bookmarks.folders().to_vec() {
            let name = folder.file_name().map_or_else(|| folder.display().to_string(), |n| n.to_string_lossy().into_owned());
            let gone = !folder.is_dir();
            let label = if gone { egui::RichText::new(name).weak() } else { egui::RichText::new(name) };
            let button = egui::Button::new(label).shortcut_text(crate::bookmarks::display(&folder));
            if ui.add(button).clicked() {
                if gone {
                    actions.extend(self.bookmarks.remove(&folder).err().map(Action::Error));
                    actions.push(Action::Note(format!("{} is gone; bookmark removed", folder.display())));
                } else {
                    actions.push(Action::SetRoot(folder));
                }
            }
        }
        actions
    }

    fn refresh_all(&mut self) {
        if let Some(tree) = &mut self.tree {
            for dir in tree.loaded_dirs() {
                tree.refresh(&dir);
            }
        }
        self.sync_watched();
    }

    /// Keyboard navigation while the explorer has focus.
    pub fn key(&mut self, key: Key, mods: Modifiers) -> Option<Action> {
        // The Outline has the keyboard: its own keys.
        if self.outline_has_keyboard() {
            let actions = self.trees.get_mut("outline").map(|t| t.key(key, mods)).unwrap_or_else(|| {
                if key == Key::Escape { vec![tree_view::Action::FocusEditor] } else { Vec::new() }
            });
            return actions.into_iter().find_map(|a| match a {
                tree_view::Action::FocusEditor => Some(Action::FocusEditor),
                tree_view::Action::Focus => None,
                action => Some(Action::Tree { view: "outline".into(), action }),
            });
        }
        // File operations, like nvim-tree (and VS Code's F2 / Cmd+Backspace).
        let row = self.selected_row();
        match key {
            Key::A => {
                self.start_new(mods.shift, row);
                return None;
            }
            Key::R if mods.shift => {
                self.refresh_all();
                return None;
            }
            Key::R | Key::F2 => {
                if let Some(r) = row {
                    self.start_rename(r.path);
                }
                return None;
            }
            Key::D | Key::Delete => {
                self.confirm_delete = row.map(|_| self.selection());
                return None;
            }
            Key::Backspace if mods.mac_cmd => {
                self.confirm_delete = row.map(|_| self.selection());
                return None;
            }
            Key::Y => {
                return row.map(|r| {
                    Action::CopyText(if mods.shift { r.path.to_string_lossy().into_owned() } else { self.relative(&r.path) })
                });
            }
            Key::V => return row.filter(|r| !r.is_dir).map(|r| Action::OpenSplit(r.path)),
            _ => {}
        }
        let rows = self.tree.as_ref()?.rows();
        if rows.is_empty() {
            return (key == Key::Escape).then_some(Action::FocusEditor);
        }
        let idx = self.selected.as_ref().and_then(|s| rows.iter().position(|r| &r.path == s));
        let cur = idx.map(|i| rows[i].clone());
        let select = |this: &mut Self, i: usize| {
            this.selected = Some(rows[i.min(rows.len() - 1)].path.clone());
            this.multi.clear();
            this.scroll_to_selected = true;
        };
        match key {
            Key::Escape => return Some(Action::FocusEditor),
            Key::ArrowUp | Key::K => select(self, idx.map_or(0, |i| i.saturating_sub(1))),
            Key::ArrowDown | Key::J => select(self, idx.map_or(0, |i| i + 1)),
            Key::Home => select(self, 0),
            Key::End => select(self, rows.len() - 1),
            Key::ArrowRight | Key::L => match &cur {
                Some(r) if r.is_dir && !r.expanded => self.set_dir(&r.path, true),
                Some(r) if r.nest == Some(false) => self.set_nest(&r.path, true),
                Some(r) if r.is_dir => {
                    let i = idx.unwrap();
                    if rows.get(i + 1).is_some_and(|n| n.depth > r.depth) {
                        select(self, i + 1);
                    }
                }
                _ => {}
            },
            Key::ArrowLeft | Key::H => match &cur {
                Some(r) if r.is_dir && r.expanded => self.set_dir(&r.path, false),
                Some(r) if r.nest == Some(true) => self.set_nest(&r.path, false),
                Some(r) => {
                    let parent = rows[..idx.unwrap()].iter().rposition(|p| p.depth < r.depth);
                    if let Some(p) = parent {
                        select(self, p);
                    }
                }
                None => select(self, 0),
            },
            Key::Enter | Key::Space | Key::O => match cur {
                Some(r) if r.is_dir => self.toggle_dir(&r.path),
                Some(r) => return Some(Action::Open(r.path)),
                None => select(self, 0),
            },
            _ => {}
        }
        None
    }

    /// Problem count and git letter at the right of a file row, or a dot for
    /// a folder that contains problems or changes.
    /// Draws the git letter, problem count or folder dot at the row's right
    /// end; returns where the name must stop.
    fn paint_decoration(ui: &egui::Ui, row: &Row, rect: Rect, font: &FontId, decos: &Decorations, c: &DecoColors) -> f32 {
        let deco = decos.get(&row.path, row.is_dir);
        let mut x = rect.max.x - 12.0;
        let Some(marker) = c.marker(&deco) else { return x };
        let p = ui.painter();
        let small = crate::style::scaled(font, crate::style::SMALL);
        if row.is_dir {
            p.circle_filled(Pos2::new(x - 2.0, rect.center().y), 3.0, marker);
            return x - 13.0;
        }
        if let Some(g) = deco.git {
            let r = p.text(Pos2::new(x, rect.center().y), Align2::RIGHT_CENTER, g.letter(), small.clone(), c.git(g));
            x = r.min.x - 8.0;
        }
        let (count, color) = if deco.problems.errors > 0 {
            (deco.problems.errors, c.error)
        } else {
            (deco.problems.warnings, c.warning)
        };
        if count > 0 {
            let r = p.text(Pos2::new(x, rect.center().y), Align2::RIGHT_CENTER, count.to_string(), small, color);
            x = r.min.x - 8.0;
        }
        x
    }

    /// How much of the header's right end the "⋯" takes.
    pub fn header_reserved(row_h: f32) -> f32 {
        18.0 + row_h / 2.0 + 4.0
    }

    /// Draw the explorer contents into the panel's `ui`, below `header`
    /// (the side bar's: the "⋯" goes at its right end).
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        header: Rect,
        colors: &ExplorerColors,
        theme: &crate::popup::PopupTheme,
        fonts: &GridFonts,
        decos: &Decorations,
        deco_colors: &DecoColors,
    ) -> Vec<Action> {
        let mut actions = self.drain_copies();
        let font = {
            let mut f = fonts.sidebar_font_id(false);
            f.size = self.font_size;
            f
        };
        let row_h = crate::side_bar::row_height(self.font_size);
        let ppp = ui.ctx().pixels_per_point();

        // The "⋯" menu, in the header.
        let btn = Rect::from_center_size(Pos2::new(header.max.x - 18.0, header.center().y), Vec2::splat(row_h));
        let btn_resp = ui.interact(btn, Id::unique("eshvi-explorer-menu"), Sense::CLICK);
        if btn_resp.hovered() {
            ui.painter().rect_filled(btn.shrink(2.0), crate::style::RADIUS, colors.hover);
        }
        let dots_color = if btn_resp.hovered() { colors.fg } else { colors.dim };
        ui.painter().text(btn.center(), Align2::CENTER_CENTER, "⋯", font.clone(), dots_color);
        egui::Popup::menu(&btn_resp).show(|ui| self.menu(ui, None, &mut actions));

        // The project name; right-click for the same menu.
        let root_name = self
            .root()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        let mut bold = fonts.sidebar_font_id(true);
        bold.size = (self.font_size * crate::style::BADGE).round();
        let (title, title_resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::CLICK);
        ui.painter().text(Pos2::new(title.min.x + 10.0, title.center().y), Align2::LEFT_CENTER, root_name, bold, colors.fg);
        title_resp.context_menu(|ui| self.menu(ui, None, &mut actions));

        let Some(rows) = self.tree.as_ref().map(Tree::rows) else { return actions };
        // Where an inline "new entry" row goes: right after its folder.
        let new_at = self.edit.as_ref().and_then(|e| match e.kind {
            EditKind::Rename(_) => None,
            _ if Some(e.dir.as_path()) == self.root() => Some((0, 0)),
            _ => rows.iter().position(|r| r.path == e.dir).map(|i| (i + 1, rows[i].depth + 1)),
        });
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        #[cfg(test)]
        {
            self.selected_row = Rect::NOTHING;
        }
        // Drop targets (row, its folder) for files dragged in from Finder.
        let mut targets: Vec<(Rect, PathBuf)> = Vec::new();
        let dragging_files = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let drop_pos = self.drop_pos.or_else(|| ui.ctx().pointer_hover_pos());
        // The Outline pins itself at the foot, as the Graph does: the files take
        // what is left above it.
        let rest = ui.available_height();
        let body = if self.outline_enabled && self.outline_open { pane_body(rest, row_h, self.outline_height) } else { 0.0 };
        let lists = if self.outline_enabled { (rest - row_h - body).max(row_h * 2.0) } else { rest };
        ScrollArea::vertical().max_height(lists).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for (i, row) in rows.iter().enumerate() {
                if let Some((at, depth)) = new_at {
                    if at == i {
                        actions.extend(self.edit_row(ui, None, depth, row_h, &font, colors, ppp));
                    }
                }
                let renaming = matches!(&self.edit, Some(Edit { kind: EditKind::Rename(p), .. }) if p == &row.path);
                if renaming {
                    actions.extend(self.edit_row(ui, Some(row), row.depth, row_h, &font, colors, ppp));
                    continue;
                }
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click_and_drag());
                // Dragged: the selection if this row is in it, else the row.
                if resp.drag_started() {
                    let paths = if self.is_selected(&row.path) { self.selection() } else { vec![row.path.clone()] };
                    resp.dnd_set_drag_payload(paths);
                }
                // A folder takes drops; a file, for its folder.
                let drop_dir = if row.is_dir { Some(row.path.clone()) } else { row.path.parent().map(Path::to_path_buf) };
                if let Some(dir) = drop_dir.clone() {
                    targets.push((rect, dir));
                }
                if let (Some(dir), Some(paths)) = (drop_dir.clone(), resp.dnd_release_payload::<Vec<PathBuf>>()) {
                    actions.extend(self.move_into(&paths, &dir));
                }
                resp.context_menu(|ui| self.menu(ui, Some(row), &mut actions));
                let selected = self.is_selected(&row.path);
                // (Before the check below: a row out of sight is the one to scroll to.)
                if selected && scroll_to {
                    resp.scroll_to_me(None);
                }
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                #[cfg(test)]
                if selected {
                    self.selected_row = rect;
                }
                let drop_here = resp.dnd_hover_payload::<Vec<PathBuf>>().is_some()
                    || (dragging_files && drop_pos.is_some_and(|p| rect.contains(p)));
                let p = ui.painter();
                paint_row_bg(p, colors, rect, selected, self.focused && self.files_focus(), resp.hovered());
                if drop_here {
                    p.rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(crate::style::HAIRLINE, colors.accent), egui::StrokeKind::Inside);
                }
                // The name keeps the text color; git status and problems show
                // only in the badges after it.
                let name_end = Self::paint_decoration(ui, row, rect, &font, decos, deco_colors);
                self.paint_row(ui, row, rect, name_end, &font, colors, ppp);
                // Cmd-click adds or removes a row; Shift-click selects the
                // rows from the last one clicked.
                let mods = ui.input(|i| i.modifiers);
                if resp.clicked() && (mods.command || mods.shift) {
                    self.focused = true;
                    self.outline_focus = false;
                    if mods.shift {
                        let anchor = self.selected.as_ref().and_then(|s| rows.iter().position(|r| &r.path == s)).unwrap_or(i);
                        let (a, b) = (anchor.min(i), anchor.max(i));
                        self.multi = rows[a..=b].iter().map(|r| r.path.clone()).collect();
                    } else {
                        if self.multi.is_empty() {
                            self.multi.extend(self.selected.clone());
                        }
                        match self.multi.iter().position(|p| p == &row.path) {
                            Some(at) => {
                                self.multi.remove(at);
                            }
                            None => self.multi.push(row.path.clone()),
                        }
                        self.selected = Some(row.path.clone());
                    }
                } else if resp.clicked() {
                    self.multi.clear();
                    self.selected = Some(row.path.clone());
                    self.focused = true;
                    self.outline_focus = false;
                    // A nesting file's chevron shows or hides what's under
                    // it; the rest of its row opens it.
                    let indent = (self.font_size * INDENT_PER_PT).round();
                    let chevron_x = rect.min.x + 10.0 + row.depth as f32 * indent + 12.0;
                    let on_chevron = resp.interact_pointer_pos().is_some_and(|p| p.x < chevron_x);
                    if row.is_dir {
                        self.toggle_dir(&row.path);
                    } else if row.nest.is_some() && on_chevron {
                        self.set_nest(&row.path, row.nest != Some(true));
                    } else {
                        actions.push(Action::Open(row.path.clone()));
                    }
                }
            }
            if let Some((at, depth)) = new_at {
                if at >= rows.len() {
                    actions.extend(self.edit_row(ui, None, depth, row_h, &font, colors, ppp));
                }
            }
            // Below the rows: the root takes drops.
            let rest = ui.available_rect_before_wrap();
            if rest.height() > 0.0 {
                let resp = ui.allocate_rect(rest, Sense::hover());
                if let (Some(root), Some(paths)) = (self.root().map(Path::to_path_buf), resp.dnd_release_payload::<Vec<PathBuf>>()) {
                    actions.extend(self.move_into(&paths, &root));
                }
                if let Some(root) = self.root() {
                    targets.push((rest, root.to_path_buf()));
                }
            }
        });
        if self.outline_enabled {
            self.outline_pane(ui, colors, &font, row_h, rest, body, &mut actions);
        }
        // What's being dragged, by the pointer.
        if let (Some(paths), Some(pos)) =
            (egui::DragAndDrop::payload::<Vec<PathBuf>>(ui.ctx()), ui.ctx().pointer_interact_pos())
        {
            let label = match &paths[..] {
                [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                many => format!("{} items", many.len()),
            };
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("eshvi-explorer-drag")));
            let galley = painter.layout_no_wrap(label, font.clone(), colors.fg);
            let r = Rect::from_min_size(pos + Vec2::new(14.0, 6.0), galley.size() + Vec2::new(14.0, 6.0));
            painter.rect_filled(r, crate::style::RADIUS, colors.selected);
            painter.galley(r.min + Vec2::new(7.0, 3.0), galley, colors.fg);
        }
        // Files dropped in from Finder: copied into the folder under the
        // pointer.
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| p.is_absolute()).collect());
        if !dropped.is_empty() {
            if let Some((_, dir)) = targets.iter().find(|(r, _)| drop_pos.is_some_and(|p| r.contains(p))) {
                let dir = dir.clone();
                self.copy_into(&dropped, &dir, ui.ctx());
            }
        }

        // Move-to-Trash confirmation.
        if let Some(paths) = self.confirm_delete.clone() {
            let what = match &paths[..] {
                [one] => format!("\"{}\"", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                many => format!("{} items", many.len()),
            };
            let modal = crate::style::dialog(ui.ctx(), Id::unique("eshvi-explorer-delete"), theme, 320.0, |ui| {
                crate::style::dialog_title(ui, theme, &format!("Move {what} to the Trash?"));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("You can restore it from the Trash.").small());
                ui.add_space(10.0);
                let mut choice = None;
                ui.horizontal(|ui| {
                    if crate::style::dialog_button(ui, theme, "Move to Trash", true).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        choice = Some(true);
                    }
                    if crate::style::dialog_button(ui, theme, "Cancel", false).clicked() {
                        choice = Some(false);
                    }
                });
                choice
            });
            match modal.inner {
                Some(true) => {
                    self.confirm_delete = None;
                    self.multi.clear();
                    for path in &paths {
                        actions.extend(self.delete_confirmed(path));
                    }
                }
                Some(false) => self.confirm_delete = None,
                None if modal.should_close() => self.confirm_delete = None,
                None => {}
            }
        }
        actions
    }

    /// The Outline, pinned at the panel's foot: its title bar (a click folds it;
    /// Collapse All and "⋯" at the right, shown folded or not), the band along its
    /// top edge that resizes it, and the tree.
    #[allow(clippy::too_many_arguments)]
    fn outline_pane(&mut self, ui: &mut egui::Ui, colors: &ExplorerColors, font: &FontId, row_h: f32, rest: f32, body: f32, actions: &mut Vec<Action>) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::CLICK);
        #[cfg(test)]
        {
            self.outline_header = rect;
        }
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect, 0.0, colors.hover);
        }
        p.hline(rect.x_range(), rect.min.y + 0.5, Stroke::new(crate::style::HAIRLINE, colors.chrome().line()));
        paint_chevron(p, Pos2::new(rect.min.x + 10.0 + 5.0, rect.center().y), self.outline_open, colors.dim);
        p.text(Pos2::new(rect.min.x + 10.0 + 16.0, rect.center().y), Align2::LEFT_CENTER, "Outline", font.clone(), colors.fg);
        if resp.clicked() {
            self.outline_open = !self.outline_open;
            if self.outline_open {
                self.focus_outline();
            }
        }
        // Collapse All, and the "⋯" menu: Follow Cursor and Sort By.
        let (more, more_resp, color) = title_button(ui, colors, row_h, rect, 0.0, "outline-more", "More Actions…", true);
        ui.painter().text(more.center(), Align2::CENTER_CENTER, "⋯", font.clone(), color);
        let (collapse, collapse_resp, color) = title_button(ui, colors, row_h, rect, 1.0, "outline-collapse", "Collapse All", true);
        self.textures.paint_codicon(ui.painter(), "collapse-all", collapse.center(), crate::style::icon_size(row_h), color);
        let tree = self.trees.entry("outline".to_owned()).or_default();
        if collapse_resp.clicked() {
            tree.collapse_all();
        }
        let mut follow = self.outline_follow;
        egui::Popup::menu(&more_resp).show(|ui| {
            if ui.checkbox(&mut follow, "Follow Cursor").changed() {
                actions.push(Action::OutlineFollow(follow));
            }
            ui.separator();
            ui.label(egui::RichText::new("Sort By").small().weak());
            for sort in tree_view::Sort::ALL {
                let mut on = tree.sort == sort;
                if ui.checkbox(&mut on, sort.label()).clicked() {
                    tree.set_sort(sort);
                }
            }
        });
        self.outline_follow = follow;
        if !self.outline_open {
            return;
        }
        // The band along the top edge resizes it (as the Graph's does); a double
        // click goes back to its share.
        let band = Rect::from_x_y_ranges(rect.x_range(), rect.min.y - 3.0..=rect.min.y + 4.0);
        let sash = ui.interact(band, ui.scope_id().with("eshvi-outline-sash"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
        if sash.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            self.outline_height = Some(pane_body(rest, row_h, Some(body - sash.drag_delta().y)));
        }
        if sash.double_clicked() {
            self.outline_height = None;
        }
        let focused = self.outline_has_keyboard();
        let look = tree_view::Look { colors, font: font.clone(), font_size: self.font_size, row_h };
        let asked = self.trees.entry("outline".to_owned()).or_default().show(ui, &look, focused, body);
        for action in asked {
            match action {
                tree_view::Action::Focus => {
                    self.focused = true;
                    self.outline_focus = true;
                }
                tree_view::Action::FocusEditor => actions.push(Action::FocusEditor),
                action => actions.push(Action::Tree { view: "outline".into(), action }),
            }
        }
    }

    /// The inline name field: for a rename (`row`) or a new entry at `depth`.
    #[allow(clippy::too_many_arguments)]
    fn edit_row(
        &mut self,
        ui: &mut egui::Ui,
        row: Option<&Row>,
        depth: usize,
        row_h: f32,
        font: &FontId,
        colors: &ExplorerColors,
        ppp: f32,
    ) -> Vec<Action> {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
        let Some(edit) = &mut self.edit else { return vec![] };
        let is_dir = match &edit.kind {
            EditKind::NewFolder => true,
            EditKind::NewFile => false,
            EditKind::Rename(_) => row.is_some_and(|r| r.is_dir),
        };
        ui.painter().rect_filled(rect, 0.0, colors.selected);
        // Icon for what is being named.
        let indent = (self.font_size * INDENT_PER_PT).round();
        let icon_size = (self.font_size * ICON_PER_PT).round();
        let x = rect.min.x + 10.0 + depth as f32 * indent;
        let theme = IconTheme::get();
        let icon = if is_dir { theme.folder_icon(&edit.text, false) } else { theme.file_icon(&edit.text) };
        let icon_rect = Rect::from_center_size(Pos2::new(x + 14.0 + icon_size / 2.0, rect.center().y), Vec2::splat(icon_size));
        if let Some(tex) = self.textures.get(ui.ctx(), icon, (icon_size * ppp).round() as u32) {
            ui.painter().image(tex.id(), icon_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        let field = Rect::from_min_max(Pos2::new(icon_rect.max.x + 4.0, rect.min.y + 1.0), Pos2::new(rect.max.x - 6.0, rect.max.y - 1.0));
        let id = edit_field_id();
        let text_h = ui.ctx().fonts_mut(|f| f.row_height(font));
        let margin = egui::Margin::symmetric(crate::style::GAP as i8, ((field.height() - text_h) / 2.0).floor().max(0.0) as i8);
        let resp = ui
            .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                crate::style::text_field(ui, &colors.chrome(), id, margin, |ui| {
                    let edit = egui::TextEdit::singleline(&mut edit.text)
                        .id(id)
                        .font(font.clone())
                        .text_color(colors.fg)
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE);
                    ui.add(edit)
                })
                .0
            })
            .inner;
        if std::mem::take(&mut edit.focus) {
            resp.request_focus();
            // Select the name without its extension, like VS Code.
            let stem = match edit.text.rfind('.') {
                Some(i) if i > 0 && !is_dir => edit.text[..i].chars().count(),
                _ => edit.text.chars().count(),
            };
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(stem))));
                state.store(ui.ctx(), id);
            }
            return vec![];
        }
        if resp.lost_focus() {
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.edit = None;
                return vec![];
            }
            return self.commit_edit();
        }
        vec![]
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &mut self,
        ui: &egui::Ui,
        row: &Row,
        rect: Rect,
        name_end: f32,
        font: &FontId,
        colors: &ExplorerColors,
        ppp: f32,
    ) {
        let p = ui.painter();
        let indent = (self.font_size * INDENT_PER_PT).round();
        let icon_size = (self.font_size * ICON_PER_PT).round();
        let x0 = rect.min.x + 10.0;
        // Indent guides.
        for d in 0..row.depth {
            let x = x0 + d as f32 * indent + 5.0;
            p.vline(x, rect.y_range(), Stroke::new(crate::style::HAIRLINE, colors.guide));
        }
        let x = x0 + row.depth as f32 * indent;
        let cy = rect.center().y;
        // Chevron for folders, and for files with others nested under them.
        let open = if row.is_dir { Some(row.expanded) } else { row.nest };
        if let Some(open) = open {
            paint_chevron(p, Pos2::new(x + 5.0, cy), open, colors.dim);
        }
        // Icon.
        let theme = IconTheme::get();
        let icon = if row.is_dir { theme.folder_icon(&row.name, row.expanded) } else { theme.file_icon(&row.name) };
        let icon_rect = Rect::from_center_size(Pos2::new(x + 14.0 + icon_size / 2.0, cy), Vec2::splat(icon_size));
        let px = (icon_size * ppp).round() as u32;
        if let Some(tex) = self.textures.get(ui.ctx(), icon, px) {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(tex.id(), icon_rect, uv, Color32::WHITE);
        }
        let color = if self.active.as_ref() == Some(&row.path) { colors.fg } else { colors.fg.gamma_multiply(0.85) };
        // Too long for the row: cut with "…" before the decorations.
        let left = icon_rect.max.x + 6.0;
        let galley = truncated(ui.painter(), &row.name, font, color, name_end - left);
        let top = cy - galley.size().y / 2.0;
        ui.painter().galley(Pos2::new(left, top), galley, color);
    }
}

/// The fill under a row's buttons and badges: the selected row's (the focused
/// view's brighter), a hovered row's, else the panel's.
pub(crate) fn row_surface(colors: &ExplorerColors, selected: bool, focused: bool, hovered: bool) -> Color32 {
    match (selected, focused, hovered) {
        (true, true, _) => colors.selected,
        (true, false, _) | (_, _, true) => colors.hover,
        _ => colors.bg,
    }
}

/// A row's fill: the explorer's, for selected and for hovered rows. The lists of
/// Source Control, its Graph and the Outline draw theirs with it.
pub(crate) fn paint_row_bg(p: &egui::Painter, colors: &ExplorerColors, rect: Rect, selected: bool, focused: bool, hovered: bool) {
    if selected {
        p.rect_filled(rect, 0.0, row_surface(colors, true, focused, hovered));
        if focused {
            p.rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(crate::style::HAIRLINE, colors.accent), egui::StrokeKind::Inside);
        }
    } else if hovered {
        p.rect_filled(rect, 0.0, colors.hover);
    }
}

/// `text` on one line in `color`, cut with "…" at `width`.
pub(crate) fn truncated(p: &egui::Painter, text: &str, font: &FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font.clone(), color));
    job.wrap = TextWrapping::truncate_at_width(width.max(0.0));
    p.layout_job(job)
}

/// A folder's chevron centered at `c`: down while open, else right.
pub fn paint_chevron(p: &egui::Painter, c: Pos2, open: bool, color: Color32) {
    let s = 3.5;
    let pts = if open {
        vec![c + Vec2::new(-s, -s * 0.5), c + Vec2::new(0.0, s * 0.5), c + Vec2::new(s, -s * 0.5)]
    } else {
        vec![c + Vec2::new(-s * 0.5, -s), c + Vec2::new(s * 0.5, 0.0), c + Vec2::new(-s * 0.5, s)]
    };
    p.add(egui::Shape::line(pts, Stroke::new(crate::style::STROKE, color)));
}

/// The git repository containing `file`, else its folder.
pub fn workspace_root_for(file: &Path) -> Option<PathBuf> {
    let dir = file.parent()?;
    dir.ancestors().find(|d| d.join(".git").exists()).or(Some(dir)).map(Path::to_path_buf)
}

/// A sensible initial root: the launch directory, unless that is a root
/// (`/`, `C:\`) or the home directory (as when started from Finder).
pub fn initial_root(cwd: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    (cwd.parent().is_some() && home.as_deref() != Some(cwd)).then(|| cwd.to_path_buf())
}

/// The inline name field (new file/folder, rename): the only widget that
/// takes keyboard focus.
pub fn edit_field_id() -> Id {
    Id::unique("eshvi-explorer-edit")
}

/// What a worker thread found after copying dropped files.
struct CopyDone {
    dir: PathBuf,
    /// The last thing copied.
    last: Option<PathBuf>,
    errors: Vec<String>,
}

/// Copy `from` to `to`, which must not exist. Symlinks are copied as links:
/// following one to an ancestor would recurse for ever.
/// `a` and `b` name one file by names that differ in their letters' case only.
fn same_name_and_file(a: &Path, b: &Path) -> bool {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    if a.parent() != b.parent() || name(a) != name(b) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!((a.symlink_metadata(), b.symlink_metadata()), (Ok(x), Ok(y)) if x.dev() == y.dev() && x.ino() == y.ino())
    }
    #[cfg(not(unix))]
    {
        matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
    }
}

/// Rename `from` to `to` unless something is there (`AlreadyExists`). The
/// rename itself refuses where the system can (macOS, Linux): checking first
/// and renaming after would replace a file made in between.
fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput));
        let (f, t) = (c(from)?, c(to)?);
        // SAFETY: both are NUL-terminated strings that live across the call.
        #[cfg(target_os = "macos")]
        let rc = unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) };
        // SAFETY: as above; `renameat2` by number, for C libraries without the wrapper.
        #[cfg(target_os = "linux")]
        let rc = unsafe { libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), 1 as libc::c_uint) };
        if rc == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        // A file system or kernel without it: checked first, below.
        if !matches!(e.raw_os_error(), Some(libc::ENOTSUP | libc::EINVAL | libc::ENOSYS)) {
            return Err(e);
        }
    }
    if to.symlink_metadata().is_ok() {
        return Err(std::io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(std::fs::read_link(from)?, to);
        #[cfg(not(unix))]
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "links can't be copied here"));
    }
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        // Made here first, or not at all: `fs::copy` alone would replace a
        // file that appeared since the caller looked.
        std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Copy `paths` into `dir`: the last one copied and the errors. Not into
/// themselves, not over an existing name; a failed copy leaves nothing behind.
fn copy_paths(paths: &[PathBuf], dir: &Path) -> (Option<PathBuf>, Vec<String>) {
    let mut errors = Vec::new();
    let mut last = None;
    for from in paths {
        let Some(name) = from.file_name() else { continue };
        let to = dir.join(name);
        // Canonical, so a link or `..` can't hide that `dir` is inside `from`.
        // A link itself is copied as a link, so it can't recurse.
        let is_link = from.symlink_metadata().is_ok_and(|m| m.is_symlink());
        if !is_link {
            if let (Ok(f), Ok(d)) = (from.canonicalize(), dir.canonicalize()) {
                if d.starts_with(&f) {
                    errors.push(format!("Can't copy {} into itself", from.display()));
                    continue;
                }
            }
        }
        if to.symlink_metadata().is_ok() {
            errors.push(format!("{} already exists", to.display()));
            continue;
        }
        match copy_tree(from, &to) {
            Ok(()) => last = Some(to),
            // (Made by someone else since the check above: not ours to remove.)
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && !to.is_dir() => {
                errors.push(format!("{} already exists", to.display()));
            }
            Err(e) => {
                // `to` didn't exist, so whatever is there is this copy's.
                let _ = match to.symlink_metadata() {
                    Ok(m) if m.is_dir() => std::fs::remove_dir_all(&to),
                    Ok(_) => std::fs::remove_file(&to),
                    Err(_) => Ok(()),
                };
                errors.push(format!("Could not copy {}: {e}", from.display()));
            }
        }
    }
    (last, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots() {
        assert_eq!(initial_root(Path::new("/")), None);
        assert_eq!(initial_root(Path::new("/tmp/project")), Some(PathBuf::from("/tmp/project")));
        let repo = std::env::temp_dir().join(format!("eshvi-root-{}", std::process::id()));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("a/b")).unwrap();
        assert_eq!(workspace_root_for(&repo.join("a/b/c.rs")), Some(repo.clone()));
        std::fs::remove_dir_all(&repo).unwrap();
        assert_eq!(workspace_root_for(Path::new("/nonexistent-dir/x.rs")), Some(PathBuf::from("/nonexistent-dir")));
    }

    fn view_with(root: &Path) -> ExplorerView {
        let mut v = ExplorerView::new(true, true, 13.0);
        v.tree = Some(Tree::new(root.to_path_buf(), true));
        v
    }

    #[test]
    fn dragging_moves_and_dropping_copies() {
        let root = std::env::temp_dir().join(format!("eshvi-explorer-dnd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("a.rs"), "a").unwrap();
        std::fs::write(root.join("b.rs"), "b").unwrap();
        std::fs::write(root.join("src/b.rs"), "taken").unwrap();
        let mut v = view_with(&root);

        // a.rs into src: moved; open buffers follow.
        let acts = v.move_into(&[root.join("a.rs")], &root.join("src"));
        assert_eq!(acts, vec![Action::Renamed { from: root.join("a.rs"), to: root.join("src/a.rs") }]);
        assert!(root.join("src/a.rs").is_file() && !root.join("a.rs").exists());
        // Not over an existing name, not a folder into itself.
        assert!(matches!(v.move_into(&[root.join("b.rs")], &root.join("src"))[..], [Action::Error(_)]));
        assert!(root.join("b.rs").exists());
        assert!(matches!(v.move_into(&[root.join("src")], &root.join("src/deep"))[..], [Action::Error(_)]));

        // From outside: copied in, the original stays.
        let outside = std::env::temp_dir().join(format!("eshvi-explorer-dnd-out-{}", std::process::id()));
        std::fs::create_dir_all(outside.join("dir")).unwrap();
        std::fs::write(outside.join("dir/x.txt"), "x").unwrap();
        v.copy_into(&[outside.join("dir")], &root.join("src/deep"), &egui::Context::default());
        let acts = loop {
            let acts = v.drain_copies();
            if root.join("src/deep/dir/x.txt").exists() || !acts.is_empty() {
                break acts;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(acts.is_empty());
        assert_eq!(std::fs::read_to_string(root.join("src/deep/dir/x.txt")).unwrap(), "x");
        assert!(outside.join("dir/x.txt").exists());
        let _ = std::fs::remove_dir_all(outside);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn copying_refuses_itself_keeps_links_and_cleans_up() {
        let root = std::env::temp_dir().join(format!("eshvi-explorer-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("proj/sub")).unwrap();
        std::fs::write(root.join("proj/a.txt"), "a").unwrap();
        // Into itself, and into its own subfolder.
        let (last, errs) = copy_paths(&[root.join("proj")], &root.join("proj"));
        assert!(last.is_none() && errs.len() == 1, "{errs:?}");
        let (last, errs) = copy_paths(&[root.join("proj")], &root.join("proj/sub"));
        assert!(last.is_none() && errs.len() == 1, "{errs:?}");
        assert!(!root.join("proj/sub/proj").exists() && !root.join("proj/proj").exists());

        #[cfg(unix)]
        {
            // A link to an ancestor is copied as a link, not followed.
            std::os::unix::fs::symlink(&root, root.join("proj/up")).unwrap();
            let out = root.join("out");
            std::fs::create_dir(&out).unwrap();
            let (last, errs) = copy_paths(&[root.join("proj")], &out);
            assert!(errs.is_empty(), "{errs:?}");
            assert_eq!(last, Some(out.join("proj")));
            assert!(out.join("proj/up").symlink_metadata().unwrap().is_symlink());
            assert_eq!(std::fs::read_link(out.join("proj/up")).unwrap(), root);
            assert_eq!(std::fs::read_to_string(out.join("proj/a.txt")).unwrap(), "a");

            // A failure removes the partial copy, and only that.
            let bad = root.join("bad");
            std::fs::create_dir(&bad).unwrap();
            std::fs::write(bad.join("ok.txt"), "ok").unwrap();
            std::fs::write(bad.join("zz-unreadable"), "x").unwrap();
            std::fs::set_permissions(bad.join("zz-unreadable"), std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
            let dest = root.join("dest");
            std::fs::create_dir(&dest).unwrap();
            std::fs::write(dest.join("keep.txt"), "keep").unwrap();
            let (last, errs) = copy_paths(&[bad.clone()], &dest);
            std::fs::set_permissions(bad.join("zz-unreadable"), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
            // Running as root can read it; nothing to check then.
            if !errs.is_empty() {
                assert!(last.is_none() && !dest.join("bad").exists());
                assert!(dest.join("keep.txt").exists());
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_rows_menu_acts_on_the_selection_it_is_in() {
        let root = std::env::temp_dir().join(format!("eshvi-explorer-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for f in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        let mut v = view_with(&root);
        let row = |name: &str| v.tree.as_ref().unwrap().rows().into_iter().find(|r| r.name == name).unwrap();
        let (a, b, c) = (row("a.rs"), row("b.rs"), row("c.rs"));
        v.selected = Some(a.path.clone());
        v.multi = vec![a.path.clone(), b.path.clone()];
        assert!(v.is_selected(&b.path) && !v.is_selected(&c.path));
        assert_eq!(v.targets_for(&b), vec![a.path.clone(), b.path.clone()]);
        // A row outside the selection: just it, and it becomes the selection.
        assert_eq!(v.targets_for(&c), vec![c.path.clone()]);
        assert!(v.multi.is_empty() && v.selected == Some(c.path.clone()));
        // Delete confirms the whole selection.
        v.multi = vec![a.path.clone(), c.path.clone()];
        v.key(Key::D, Modifiers::NONE);
        assert_eq!(v.confirm_delete, Some(vec![a.path, c.path]));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn names() {
        assert!(valid_name("a.rs", false));
        assert!(valid_name("src/new.rs", true));
        assert!(!valid_name("src/new.rs", false));
        assert!(!valid_name("../x", true));
        assert!(!valid_name("/abs", true));
        assert!(!valid_name("a//b", true));
        assert!(!valid_name("", true));
    }

    #[test]
    fn a_rename_or_a_copy_never_replaces_what_is_there() {
        let root = std::env::temp_dir().join(format!("eshvi-explorer-noreplace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();
        // The rename itself refuses (no look first that a file could slip in after).
        let e = rename_new(&root.join("a.txt"), &root.join("b.txt")).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(rename_new(&root.join("a.txt"), &root.join("d")).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "b");
        rename_new(&root.join("a.txt"), &root.join("c.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("c.txt")).unwrap(), "a");
        // A copy onto a file that is there fails and leaves it.
        let e = copy_tree(&root.join("c.txt"), &root.join("b.txt")).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "b");
        // Different files whose names differ only in case are not "the same".
        assert!(!same_name_and_file(&root.join("b.txt"), &root.join("c.txt")));
        assert!(!same_name_and_file(&root.join("b.txt"), &root.join("d/B.txt")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_rename_and_errors() {
        let root = std::env::temp_dir().join(format!("eshvi-explorer-ops-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        let mut v = view_with(&root);

        // New file in the selected file's folder, opened afterwards.
        v.selected = Some(root.join("src/main.rs"));
        v.key(Key::A, Modifiers::NONE);
        v.edit.as_mut().unwrap().text = "lib.rs".into();
        assert_eq!(v.commit_edit(), vec![Action::Open(root.join("src/lib.rs"))]);
        assert!(root.join("src/lib.rs").is_file());
        assert_eq!(v.selected.as_deref(), Some(root.join("src/lib.rs").as_path()));

        // Nested path creates folders.
        v.selected = None;
        v.start_new(false, None);
        v.edit.as_mut().unwrap().text = "docs/guide/intro.md".into();
        v.commit_edit();
        assert!(root.join("docs/guide/intro.md").is_file());

        // New folder (Shift+A).
        v.selected = Some(root.join("src"));
        v.key(Key::A, Modifiers::SHIFT);
        assert!(v.wants_keyboard());
        v.edit.as_mut().unwrap().text = "util".into();
        v.commit_edit();
        assert!(root.join("src/util").is_dir());

        // Rename, and the selection follows.
        v.selected = Some(root.join("src/lib.rs"));
        v.key(Key::F2, Modifiers::NONE);
        v.edit.as_mut().unwrap().text = "core.rs".into();
        assert_eq!(
            v.commit_edit(),
            vec![Action::Renamed { from: root.join("src/lib.rs"), to: root.join("src/core.rs") }]
        );
        assert!(root.join("src/core.rs").is_file() && !root.join("src/lib.rs").exists());

        // Errors: existing target, invalid name.
        v.selected = Some(root.join("src/core.rs"));
        v.key(Key::R, Modifiers::NONE);
        v.edit.as_mut().unwrap().text = "main.rs".into();
        assert!(matches!(v.commit_edit()[..], [Action::Error(_)]));
        assert!(root.join("src/core.rs").is_file() && root.join("src/main.rs").is_file(), "neither is replaced");
        v.start_new(false, None);
        v.edit.as_mut().unwrap().text = "../escape".into();
        assert!(matches!(v.commit_edit()[..], [Action::Error(_)]));
        assert!(!v.wants_keyboard());

        // Only the case of the letters changes: where both names are one
        // file (default APFS) that is a rename too, not "already exists".
        v.selected = Some(root.join("src/core.rs"));
        v.key(Key::R, Modifiers::NONE);
        v.edit.as_mut().unwrap().text = "Core.rs".into();
        assert_eq!(v.commit_edit(), vec![Action::Renamed { from: root.join("src/core.rs"), to: root.join("src/Core.rs") }]);
        let names: Vec<String> = std::fs::read_dir(root.join("src")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"Core.rs".to_owned()) && !names.contains(&"core.rs".to_owned()), "{names:?}");
        v.selected = Some(root.join("src/Core.rs"));
        v.key(Key::R, Modifiers::NONE);
        v.edit.as_mut().unwrap().text = "core.rs".into();
        v.commit_edit();

        // Delete asks for confirmation first.
        v.key(Key::D, Modifiers::NONE);
        assert_eq!(v.confirm_delete, Some(vec![root.join("src/core.rs")]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn keyboard_navigation() {
        let root = std::env::temp_dir().join(format!("eshvi-explorer-view-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        let mut v = view_with(&root);

        assert_eq!(v.key(Key::ArrowDown, Modifiers::NONE), None); // selects first row: src
        assert_eq!(v.selected.as_deref(), Some(root.join("src").as_path()));
        v.key(Key::L, Modifiers::NONE); // expand src
        v.key(Key::L, Modifiers::NONE); // into src/main.rs
        assert_eq!(v.selected.as_deref(), Some(root.join("src/main.rs").as_path()));
        assert_eq!(v.key(Key::Enter, Modifiers::NONE), Some(Action::Open(root.join("src/main.rs"))));
        v.key(Key::H, Modifiers::NONE); // back to parent folder
        assert_eq!(v.selected.as_deref(), Some(root.join("src").as_path()));
        v.key(Key::H, Modifiers::NONE); // collapse
        v.key(Key::J, Modifiers::NONE);
        assert_eq!(v.selected.as_deref(), Some(root.join("README.md").as_path()));
        assert_eq!(v.key(Key::Escape, Modifiers::NONE), Some(Action::FocusEditor));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Tab and Enter typed for Neovim (completing and running `:e file`)
    /// must not focus and "click" an explorer widget, as egui does for
    /// focusable ones: that opened the "⋯" menu.
    #[test]
    fn tab_and_enter_do_not_click_explorer_widgets() {
        fn clicked(sense: Sense) -> bool {
            let ctx = egui::Context::default();
            let key = |key, pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            };
            let frames = [
                vec![],
                vec![key(Key::Tab, true)],
                vec![key(Key::Tab, false)],
                vec![],
                vec![key(Key::Enter, true)],
                vec![key(Key::Enter, false)],
            ];
            let mut any = false;
            for events in frames {
                let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(20.0));
                    any |= ui.interact(rect, Id::unique("w"), sense).clicked();
                });
                out.textures_delta.clear();
            }
            any
        }
        assert!(clicked(Sense::click()), "egui's default: Tab focuses, Enter clicks");
        assert!(!clicked(Sense::CLICK));
    }

    // ---- The Outline pane ----

    use crate::popup::PopupTheme;
    use crate::scm_view::pane_body;
    use crate::style::tests::themes;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;
    use nvim_bridge::gui::{TreeNode, TreeSet};
    use std::{cell::RefCell, rc::Rc};

    struct Pane {
        /// The temporary folder the explorer shows, taken away with it.
        dir: PathBuf,
        view: ExplorerView,
        chrome: crate::style::Chrome,
        theme: PopupTheme,
        fonts: Rc<RefCell<Option<GridFonts>>>,
        actions: Vec<Action>,
    }

    impl Drop for Pane {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn node(id: &str, label: &str, kind: &str, line: usize) -> TreeNode {
        TreeNode { id: id.into(), label: label.into(), kind: kind.into(), line: Some(line), ..Default::default() }
    }

    /// A module with a function and a method, and two more symbols.
    fn symbols() -> Vec<TreeNode> {
        let mut m = node("M", "M", "Module", 1);
        m.children = vec![node("M/foo", "foo", "Function", 2), node("M/bar", "bar", "Method", 3)];
        vec![m, node("tail", "tail", "Variable", 6), node("Alpha", "Alpha", "Class", 8)]
    }

    /// An explorer with `files` files, and the Outline showing `symbols()`.
    fn pane(name: &str, width: f32, height: f32, files: usize) -> Harness<'static, Pane> {
        let (_, chrome, theme) = themes().into_iter().find(|(n, ..)| *n == name).unwrap();
        // (The folder's name is the title in the pictures.)
        // One folder per call: tests running at once with the same arguments
        // would otherwise remove each other's (`Pane`'s drop).
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("eshvi-explorer-pane-{}-{name}-{files}-{n}", std::process::id()));
        let root = dir.join("project");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        for i in 0..files {
            std::fs::write(root.join(format!("file_{i:02}.rs")), "").unwrap();
        }
        let mut view = view_with(&root);
        view.outline().set(TreeSet { view: "outline".into(), gen: 1, nodes: symbols(), ..Default::default() });
        let fonts = Rc::new(RefCell::new(None));
        let setup = fonts.clone();
        let state = Pane { dir, view, chrome, theme, fonts, actions: Vec::new() };
        Harness::builder()
            .with_size(Vec2::new(width, height))
            .with_pixels_per_point(2.0)
            .with_accessibility_check(false)
            .wgpu()
            .with_context_setup(move |ctx| {
                *setup.borrow_mut() = Some(crate::fonts::install(ctx, &crate::fonts::FontSpec::default_with_size(13.0), None, None, None, None));
            })
            .build_ui_state(
                |ui, s: &mut Pane| {
                    ui.ctx().set_theme(if s.chrome.bg.r() < 128 { egui::Theme::Dark } else { egui::Theme::Light });
                    crate::style::apply_to_egui(ui.ctx(), &s.theme);
                    let colors = ExplorerColors::from_chrome(&s.chrome);
                    let deco_colors = DecoColors::from_editor(&grid::Editor::new());
                    let fonts = s.fonts.borrow().clone().unwrap();
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(colors.bg)).show(ui, |ui| {
                        let row_h = crate::side_bar::row_height(13.0);
                        let font = fonts.sidebar_font_id(false);
                        let (header, _) = crate::side_bar::header(ui, crate::side_bar::SideView::Explorer, 0, ExplorerView::header_reserved(row_h), row_h, &font, &colors, &[], &mut crate::icons::IconTextures::default());
                        let actions = s.view.show(ui, header, &colors, &s.theme, &fonts, &Decorations::default(), &deco_colors);
                        s.actions.extend(actions);
                    });
                },
                state,
            )
    }

    /// The room under the project title for a pane of `height` (the side bar's
    /// header is 1.2 rows, the project title one), and the Outline's share of it.
    fn outline_room(height: f32) -> (f32, f32) {
        let row_h = crate::side_bar::row_height(13.0);
        let rest = height - row_h * 2.2;
        (rest, pane_body(rest, row_h, None))
    }

    /// Where the Outline's title bar starts, as drawn.
    fn outline_top(h: &Harness<'static, Pane>) -> f32 {
        h.state().view.outline_header.min.y
    }

    fn mouse(h: &mut Harness<'static, Pane>, pos: Pos2, button: egui::PointerButton) {
        h.hover_at(pos);
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
        }
        h.run();
    }

    fn click(h: &mut Harness<'static, Pane>, pos: Pos2) {
        mouse(h, pos, egui::PointerButton::Primary);
    }

    /// A file opened in Neovim (`:e`) far down the list is scrolled to.
    #[test]
    fn a_file_switched_to_out_of_sight_is_scrolled_into_view() {
        let mut h = pane("dark", 280.0, 420.0, 40);
        h.run();
        assert_eq!(h.state().view.selected_row, Rect::NOTHING);
        let file = h.state().dir.join("project/file_39.rs");
        let ctx = h.ctx.clone();
        h.state_mut().view.set_active(file, &ctx);
        h.run();
        h.run();
        let row = h.state().view.selected_row;
        assert!(row.is_positive() && row.max.y <= outline_top(&h) + 0.5, "row {row:?}, list ends at {}", outline_top(&h));
    }

    #[test]
    fn the_outline_pane_snapshots() {
        let mut results = egui_kittest::SnapshotResults::new();
        for (theme, width, files) in [("dark", 280.0, 14), ("light", 280.0, 14), ("dark", 170.0, 14)] {
            let mut h = pane(theme, width, 420.0, files);
            h.state_mut().view.focus_outline();
            h.state_mut().view.outline().select("M/foo", &["M".to_owned()]);
            h.run();
            h.run();
            h.snapshot(format!("explorer_outline_{theme}_{}", width as u32));
            results.extend_harness(&mut h);
        }
        // Folded: the files take the room, the title bar's buttons stay.
        let mut h = pane("dark", 280.0, 300.0, 14);
        h.state_mut().view.outline_open = false;
        h.run();
        h.snapshot("explorer_outline_folded_dark");
        results.extend_harness(&mut h);
        results.unwrap();
    }

    #[test]
    fn the_outline_folds_resizes_and_keeps_the_files_their_rows_like_the_graph() {
        let mut h = pane("dark", 280.0, 420.0, 30);
        h.run();
        let (rest, body) = outline_room(420.0);
        let top = outline_top(&h);
        assert!(h.state().view.outline_open && h.state().view.outline_height.is_none());
        let drag = |h: &mut Harness<'static, Pane>, from: f32, to: f32| {
            let (a, b) = (Pos2::new(140.0, from), Pos2::new(140.0, to));
            h.hover_at(a);
            h.step();
            h.event(egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
            h.step();
            h.hover_at(b);
            h.step();
            h.event(egui::Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            h.step();
            h.step();
        };
        // The sash is the band along the header's top edge: up by 40 makes it 40 taller.
        drag(&mut h, top, top - 40.0);
        let bigger = h.state().view.outline_height.expect("the band wasn't dragged");
        // (Not exactly 40: the drag starts a few points in.)
        assert!(bigger > body + 20.0 && bigger <= body + 40.5, "{bigger} from {body}");
        // As far as it goes: the files keep five rows.
        let top2 = outline_top(&h);
        drag(&mut h, top2, 0.0);
        let most = h.state().view.outline_height.unwrap();
        let row_h = crate::side_bar::row_height(13.0);
        assert!(most <= rest - row_h * 5.0 + 0.5, "the files list would have less than 5 rows: {most} of {rest}");
        assert!(most > bigger, "{most} vs {bigger}, top2 {top2}");
        // Dragging elsewhere (the title's gap in the files' rows) doesn't resize it.
        drag(&mut h, 100.0, 60.0);
        assert_eq!(h.state().view.outline_height, Some(most));
        // The double click goes back to its share.
        let pos = Pos2::new(140.0, outline_top(&h));
        h.hover_at(pos);
        h.step();
        h.ctx.options_mut(|o| o.input_options.max_double_click_delay = 10.0);
        for _ in 0..2 {
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
                h.step();
            }
        }
        h.step();
        assert_eq!(h.state().view.outline_height, None);
        // A click on its title folds it, and again unfolds it and gives it the keyboard.
        let top = outline_top(&h);
        click(&mut h, Pos2::new(60.0, top + row_h / 2.0));
        assert!(!h.state().view.outline_open);
        assert!(!h.state().view.focused);
        let top = outline_top(&h);
        click(&mut h, Pos2::new(60.0, top + row_h / 2.0));
        assert!(h.state().view.outline_open);
        assert!(h.state().view.outline_has_keyboard());
    }

    #[test]
    fn a_click_in_a_pane_gives_it_the_keyboard_and_the_keys_go_there() {
        let mut h = pane("dark", 280.0, 420.0, 6);
        h.run();
        let row_h = crate::side_bar::row_height(13.0);
        let top = outline_top(&h);
        // A symbol: chosen (the app takes the keyboard off to the editor), the Outline has it meanwhile.
        click(&mut h, Pos2::new(120.0, top + row_h * 2.5));
        assert_eq!(
            std::mem::take(&mut h.state_mut().actions),
            vec![Action::Tree { view: "outline".into(), action: tree_view::Action::Accept { id: "M/foo".into(), how: tree_view::How::Open } }]
        );
        assert!(h.state().view.outline_has_keyboard());
        // Keys now are the tree's: Space peeks, Esc leaves.
        assert_eq!(
            h.state_mut().view.key(Key::Space, Modifiers::NONE),
            Some(Action::Tree { view: "outline".into(), action: tree_view::Action::Accept { id: "M/foo".into(), how: tree_view::How::Peek } })
        );
        assert_eq!(h.state_mut().view.key(Key::Escape, Modifiers::NONE), Some(Action::FocusEditor));
        // A file in the list: the files have it, and a key there is the explorer's.
        let first_file = Pos2::new(120.0, row_h * 2.2 + row_h * 1.5);
        click(&mut h, first_file);
        assert!(!h.state().view.outline_has_keyboard() && h.state().view.focused);
        assert!(matches!(h.state_mut().view.key(Key::Enter, Modifiers::NONE), Some(Action::Open(_) | Action::Tree { .. }) | None));
        assert!(!matches!(h.state_mut().view.key(Key::Space, Modifiers::NONE), Some(Action::Tree { .. })));
        // Folded, the pane can't have the keys even if it was last.
        h.state_mut().view.focus_outline();
        h.state_mut().view.outline_open = false;
        assert!(!h.state().view.outline_has_keyboard());
    }

    #[test]
    fn collapse_all_and_the_menu_change_the_tree_follow_cursor_and_the_sort() {
        let mut h = pane("dark", 280.0, 420.0, 6);
        h.run();
        let row_h = crate::side_bar::row_height(13.0);
        let top = outline_top(&h);
        assert_eq!(h.state_mut().view.outline().visible_ids().len(), 5);
        // Collapse All is the button left of "⋯".
        click(&mut h, Pos2::new(280.0 - 18.0 - row_h, top + row_h / 2.0));
        assert_eq!(h.state_mut().view.outline().visible_ids(), ["M", "tail", "Alpha"]);
        // The menu: Follow Cursor off, then Sort By Name.
        click(&mut h, Pos2::new(280.0 - 18.0, top + row_h / 2.0));
        h.run();
        h.get_by_label("Follow Cursor").click();
        h.run();
        h.run();
        assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::OutlineFollow(false)]);
        assert!(!h.state().view.outline_follow);
        click(&mut h, Pos2::new(280.0 - 18.0, top + row_h / 2.0));
        h.run();
        h.get_by_label("Name").click();
        h.run();
        h.run();
        assert_eq!(h.state_mut().view.outline().sort, tree_view::Sort::Name);
        assert_eq!(h.state_mut().view.outline().visible_ids(), ["Alpha", "M", "tail"]);
    }

    /// Opens the header's "⋯" menu.
    fn open_menu(h: &mut Harness<'static, Pane>) {
        let row_h = crate::side_bar::row_height(13.0);
        click(h, Pos2::new(400.0 - 18.0, row_h * 0.6));
        h.run();
        h.run();
    }

    #[test]
    fn the_menu_bookmarks_the_root_lists_bookmarks_and_drops_the_gone_ones() {
        let mut h = pane("dark", 400.0, 700.0, 2);
        h.run();
        let root = h.state().view.root().unwrap().to_path_buf();
        let other = root.parent().unwrap().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let gone = root.parent().unwrap().join("gone");
        // In memory: the real state file is never touched.
        h.state_mut().view.bookmarks.add(&other).unwrap();
        h.state_mut().view.bookmarks.add(&gone).unwrap();

        // Bookmark This Folder: the root, once; then the menu offers to remove it.
        open_menu(&mut h);
        h.get_by_label("Bookmark This Folder").click();
        h.run();
        h.run();
        assert!(h.state().view.bookmarks.contains(&root));
        open_menu(&mut h);
        assert!(h.query_by_label("Bookmark This Folder").is_none());
        // The bookmarks by name, the folder beside it.
        h.get_by_label_contains("other");
        h.get_by_label_contains("gone");
        h.get_by_label("Remove Bookmark").click();
        h.run();
        h.run();
        assert!(!h.state().view.bookmarks.contains(&root));

        // One click on a bookmark makes it the root.
        open_menu(&mut h);
        h.get_by_label_contains("other").click();
        h.run();
        h.run();
        assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::SetRoot(other.clone())]);

        // A folder that is gone: removed when clicked, with a note.
        open_menu(&mut h);
        h.get_by_label_contains("gone").click();
        h.run();
        h.run();
        assert_eq!(h.state().view.bookmarks.folders(), [other.clone()]);
        let acts = std::mem::take(&mut h.state_mut().actions);
        assert!(matches!(&acts[..], [Action::Note(n)] if n.contains("gone") && n.contains("removed")), "{acts:?}");
    }

    #[test]
    fn a_folder_rows_menu_bookmarks_that_folder() {
        let mut h = pane("dark", 400.0, 420.0, 2);
        h.run();
        let src = h.state().view.root().unwrap().join("src");
        h.state_mut().view.bookmarks = Default::default();
        let row_h = crate::side_bar::row_height(13.0);
        // The first row under the title is the `src` folder.
        mouse(&mut h, Pos2::new(100.0, row_h * 3.2), egui::PointerButton::Secondary);
        h.run();
        h.run();
        h.get_by_label("Bookmark This Folder").click();
        h.run();
        h.run();
        assert_eq!(h.state().view.bookmarks.folders(), [src]);
    }
}
