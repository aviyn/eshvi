//! What can be done to the changes the view shows: put a change back (the
//! agents'), accept a file, stage, unstage or discard a change or a file
//! (Source Control's). The view only says what to do (`Edit`); the app does
//! it (Neovim for a revert, so that its buffer and undo stay right; `git` for
//! the rest), and tells the view how it went (`refresh`, `cancel`).

use std::ops::Range;
use std::time::Instant;

use super::model::{FileKind, Row, SplitRow};
use super::{Action, Cursor, DiffView, Mode, Rows, Side, Status};

/// A button of the view, or the key that does the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Do {
    /// Put the change back as it was before the agents.
    Revert,
    Stage,
    Unstage,
    Discard,
    RevertFile,
    Accept,
    StageFile,
    UnstageFile,
    DiscardFile,
}

impl Do {
    pub fn label(self) -> &'static str {
        match self {
            Do::Revert => "Revert",
            Do::Stage => "Stage",
            Do::Unstage => "Unstage",
            Do::Discard => "Discard",
            Do::RevertFile => "Revert file",
            Do::Accept => "Accept",
            Do::StageFile => "Stage file",
            Do::UnstageFile => "Unstage file",
            Do::DiscardFile => "Discard file",
        }
    }

    /// What the button says once it was pressed (a press more does it).
    pub fn confirm_label(self) -> &'static str {
        match self {
            Do::Discard => "Discard? Press again",
            Do::DiscardFile => "Discard file? Press again",
            other => other.label(),
        }
    }

    pub fn key(self) -> Option<&'static str> {
        match self {
            Do::Revert => Some("r"),
            Do::Stage | Do::Unstage => Some("-"),
            Do::Discard => Some("X"),
            _ => None,
        }
    }

    /// Asks twice: what it takes away can't be got back from the view.
    pub fn confirms(self) -> bool {
        matches!(self, Do::Discard | Do::DiscardFile)
    }

    /// For the tooltip.
    pub fn tip(self) -> String {
        let what = match self {
            Do::Revert => "Put this change back as it was before the agents",
            Do::Stage => "Stage this change",
            Do::Unstage => "Unstage this change",
            Do::Discard => "Discard this change from the working tree (asks again)",
            Do::RevertFile => "Put the whole file back as it was before the agents (a file they made is removed)",
            Do::Accept => "Keep this file's changes: it leaves the Changes list",
            Do::StageFile => "Stage the whole file",
            Do::UnstageFile => "Unstage the whole file",
            Do::DiscardFile => "Discard the whole file from the working tree (asks again)",
        };
        match self.key() {
            Some(key) => format!("{what} ({key})"),
            None => what.to_owned(),
        }
    }
}

impl Mode {
    /// The buttons of a change, left to right.
    pub fn change_actions(self) -> &'static [Do] {
        match self {
            Mode::ReadOnly => &[],
            Mode::Agents => &[Do::Revert],
            Mode::Stage => &[Do::Stage, Do::Discard],
            Mode::Unstage => &[Do::Unstage],
        }
    }

    /// The buttons of a file's header, left to right.
    pub fn file_actions(self) -> &'static [Do] {
        match self {
            Mode::ReadOnly => &[],
            Mode::Agents => &[Do::RevertFile, Do::Accept],
            Mode::Stage => &[Do::StageFile, Do::DiscardFile],
            Mode::Unstage => &[Do::UnstageFile],
        }
    }
}

/// How long an accepted file's buttons stay away waiting for it to be read
/// again (an accept that failed leaves them back).
const ACCEPT_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// What git does to whole files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOp {
    Stage,
    Unstage,
    Discard,
}

/// What the app is asked to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// In the file's buffer in Neovim, lines `start..start + expect.len()`
    /// (which must be `expect`, as the view read them) become `put`, and the
    /// file is written. Reverts a change, or (all the lines) a file.
    Revert { path: String, start: usize, expect: Vec<String>, put: Vec<String> },
    /// Keep the file's changes (the Changes popup's Accept).
    Accept { path: String },
    /// One change as a patch, for `git apply` (`cached`: the index; `reverse`:
    /// taken out again). Stage: `cached`; unstage: `cached` and `reverse`;
    /// discard: `reverse`.
    Patch { path: String, patch: String, cached: bool, reverse: bool },
    /// Whole files by git. `untracked`: a file git doesn't have yet (a discard
    /// of it goes to the Trash).
    Files { how: FileOp, paths: Vec<String>, untracked: bool },
}

impl Edit {
    /// The file it is about.
    pub fn path(&self) -> &str {
        match self {
            Edit::Revert { path, .. } | Edit::Accept { path } | Edit::Patch { path, .. } => path,
            Edit::Files { paths, .. } => paths.first().map_or("", String::as_str),
        }
    }

    /// What git is asked for: none for the edits Neovim and the Changes
    /// bookkeeping do.
    pub fn git_op(self) -> Option<crate::git::Op> {
        use crate::git::Op;
        Some(match self {
            Edit::Patch { patch, cached, reverse, .. } => Op::Apply { patch, cached, reverse },
            Edit::Files { how: FileOp::Stage, paths, .. } => Op::Stage(paths),
            Edit::Files { how: FileOp::Unstage, paths, .. } => Op::Unstage(paths),
            Edit::Files { how: FileOp::Discard, paths, untracked: true } => Op::Discard { tracked: Vec::new(), untracked: paths },
            Edit::Files { how: FileOp::Discard, paths, untracked: false } => Op::Discard { tracked: paths, untracked: Vec::new() },
            Edit::Revert { .. } | Edit::Accept { .. } => return None,
        })
    }

    /// A discard of the working tree, which unsaved edits of the file in
    /// Neovim would be lost to.
    pub fn discards(&self) -> bool {
        matches!(self, Edit::Patch { cached: false, .. } | Edit::Files { how: FileOp::Discard, .. })
    }
}

/// A button pressed once, to be pressed again: which, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Armed {
    what: Do,
    section: usize,
    /// The change (its op); none for a file's.
    op: Option<usize>,
}

/// Where the cursor goes when the file acted on has been read again: the
/// change with the ordinal the acted-on one had, which is the next change
/// when that one is gone.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    path: String,
    ordinal: usize,
    /// The action worked: the next read of the file is the one to look at.
    armed: bool,
}

/// The rows from the first of the run of changed rows around `row` to its last.
fn run_around(rows: &Rows, row: usize) -> Option<Range<usize>> {
    let is = |r: usize| match rows {
        Rows::Unified(v) => v.get(r).is_some_and(Row::is_change),
        Rows::Split(v) => v.get(r).is_some_and(SplitRow::is_change),
    };
    if !is(row) {
        return None;
    }
    let (mut a, mut b) = (row, row);
    while a > 0 && is(a - 1) {
        a -= 1;
    }
    while is(b + 1) {
        b += 1;
    }
    Some(a..b + 1)
}

impl DiffView {
    /// The change at the cursor's row `c` (on a changed row): its op and the
    /// rows it takes.
    pub fn change_under(&self, c: Cursor) -> Option<(usize, Range<usize>)> {
        let s = self.sections.get(c.section)?;
        let row = c.row?;
        let (old, new) = self.row_identity(c)?;
        let op = s.diff()?.change_at(old, new)?;
        Some((op, run_around(&s.rows.as_ref()?.2, row)?))
    }

    /// The rows of the change the cursor, or the pointer, is on in section `i`.
    pub fn hot_runs(&self, i: usize, pointer_row: Option<usize>) -> Vec<Range<usize>> {
        let Some(rows) = self.sections.get(i).and_then(|s| s.rows.as_ref()).map(|r| &r.2) else { return Vec::new() };
        let mut runs: Vec<Range<usize>> = Vec::new();
        let cursor = (self.cursor.section == i).then_some(self.cursor.row).flatten();
        for r in [cursor, pointer_row].into_iter().flatten() {
            if let Some(run) = run_around(rows, r) {
                if !runs.contains(&run) {
                    runs.push(run);
                }
            }
        }
        runs
    }

    /// Whether the buttons of a change are there at all: the view lets the
    /// changes be edited, and the file is shown as lines.
    pub fn changes_editable(&self, i: usize) -> bool {
        !self.mode.change_actions().is_empty() && self.sections.get(i).is_some_and(|s| s.diff().is_some_and(|d| !d.is_identical()))
    }

    /// Whether button `what` of a file's header is offered for section `i`: the
    /// file is read and has changes (reverting puts lines back through Neovim,
    /// so they have to be shown as lines).
    pub fn file_action_offered(&self, what: Do, i: usize) -> bool {
        let Some(s) = self.sections.get(i) else { return false };
        if s.loaded.is_none() || self.mode.file_actions().is_empty() || s.accepting.is_some_and(|(_, when)| when.elapsed() < ACCEPT_WAIT) {
            return false;
        }
        let identical = s.diff().is_some_and(|d| d.is_identical());
        !identical && (what != Do::RevertFile || s.diff().is_some())
    }

    /// The key of a button: the cursor's change (or file, for none) is acted on.
    pub(super) fn act_key(&mut self, what: Do, armed: Option<Armed>) -> Option<Action> {
        self.armed = armed;
        let c = self.cursor;
        self.do_action(what, c)
    }

    /// Press button `what` at `c` (a change's row, or a header). Returns the
    /// edit to make. Discarding asks twice: the first press only says so.
    pub fn do_action(&mut self, what: Do, c: Cursor) -> Option<Action> {
        self.sections.get(c.section)?;
        let file_level = matches!(what, Do::RevertFile | Do::Accept | Do::StageFile | Do::UnstageFile | Do::DiscardFile);
        let change = if file_level { None } else { Some(self.change_under(c)?) };
        let op = change.as_ref().map(|(op, _)| *op);
        let here = Armed { what, section: c.section, op };
        if what.confirms() && self.armed != Some(here) {
            self.armed = Some(here);
            return None;
        }
        self.armed = None;
        let edit = if file_level { self.file_edit(what, c.section)? } else { self.change_edit(what, c.section, op?)? };
        if what == Do::Accept {
            // (It takes git a moment: the buttons go now.)
            self.sections[c.section].accepting = Some((self.serial, Instant::now()));
        }
        let ordinal = op.and_then(|op| self.sections[c.section].diff().map(|d| d.ordinal_of(op)));
        self.place = ordinal.map(|ordinal| Place { path: edit.path().to_owned(), ordinal, armed: false });
        Some(Action::Edit(edit))
    }

    /// Whether `what` was pressed once at this change (or this header, when `row` is none).
    pub fn is_armed(&self, what: Do, section: usize, row: Option<usize>) -> bool {
        let Some(a) = self.armed else { return false };
        if a.what != what || a.section != section {
            return false;
        }
        match row {
            None => a.op.is_none(),
            Some(r) => self.change_under(Cursor { section, row: Some(r) }).map(|(op, _)| op) == a.op && a.op.is_some(),
        }
    }

    fn file_edit(&self, what: Do, section: usize) -> Option<Edit> {
        let s = &self.sections[section];
        let path = s.spec.path.clone();
        match what {
            Do::Accept => Some(Edit::Accept { path }),
            Do::RevertFile => {
                let d = s.diff().filter(|d| !d.is_identical())?;
                Some(Edit::Revert { path, start: 0, expect: d.new.clone(), put: d.old.clone() })
            }
            Do::StageFile => Some(Edit::Files { how: FileOp::Stage, paths: vec![path], untracked: false }),
            Do::UnstageFile => {
                let mut paths = vec![path];
                paths.extend(s.spec.old_path.clone().filter(|o| *o != paths[0]));
                Some(Edit::Files { how: FileOp::Unstage, paths, untracked: false })
            }
            Do::DiscardFile => Some(Edit::Files { how: FileOp::Discard, paths: vec![path], untracked: s.spec.left == Side::Missing }),
            _ => None,
        }
    }

    fn change_edit(&self, what: Do, section: usize, op: usize) -> Option<Edit> {
        let s = &self.sections[section];
        let d = s.diff()?;
        let path = s.spec.path.clone();
        if what == Do::Revert {
            let (old, new) = d.change(op)?;
            return Some(Edit::Revert { path, start: new.start, expect: d.new[new].to_vec(), put: d.old[old].to_vec() });
        }
        // A file made or removed is one change: the file's own operation.
        let whole = s.spec.left == Side::Missing || s.status() != Status::Modified && s.status() != Status::Renamed;
        if whole {
            let file = match what {
                Do::Stage => Do::StageFile,
                Do::Unstage => Do::UnstageFile,
                _ => Do::DiscardFile,
            };
            return self.file_edit(file, section);
        }
        let patch = d.change_patch(op, &path, FileKind::Modified, self.context.max(3))?;
        let (cached, reverse) = match what {
            Do::Stage => (true, false),
            Do::Unstage => (true, true),
            _ => (false, true),
        };
        Some(Edit::Patch { path, patch, cached, reverse })
    }

    /// The edit worked (`path`: its file; none: whichever files): the files are
    /// read again, and the cursor goes to where the change that is gone was.
    pub fn refresh(&mut self, path: Option<&str>) {
        for s in &mut self.sections {
            if s.loaded.is_some() && path.is_none_or(|p| p == s.spec.path || s.spec.old_path.as_deref() == Some(p)) {
                s.stale = true;
            }
        }
        if let Some(p) = &mut self.place {
            p.armed = path.is_none_or(|q| q == p.path);
        }
    }

    /// The edit failed, or didn't happen.
    pub fn cancel_edit(&mut self) {
        self.place = None;
        self.armed = None;
    }

    /// Section `index` was read: if the cursor waits for it, put it where it goes.
    pub(super) fn after_load(&mut self, index: usize) {
        let fresh = self.sections.get(index).is_some_and(|s| !s.stale);
        if fresh {
            if let Some(p) = self.place.take_if(|p| p.armed && self.sections.get(index).is_some_and(|s| s.spec.path == p.path)) {
                self.place_cursor(index, p.ordinal);
            }
        }
        self.find_after_load();
    }

    /// The cursor on the change of section `i` with this ordinal; past the
    /// last, on the next change (in the files after), else the last one here.
    fn place_cursor(&mut self, i: usize, ordinal: usize) {
        let starts = match &self.sections[i].rows {
            Some((.., rows)) => rows.change_starts(),
            None => return,
        };
        self.anchor = None;
        match starts.get(ordinal) {
            Some(&r) => {
                self.cursor = Cursor { section: i, row: Some(r) };
                self.reveal(false);
            }
            None => {
                self.cursor = Cursor { section: i, row: starts.last().copied() };
                self.goto_change(true);
            }
        }
    }

    /// The agents' snapshot changed (a file was accepted): the files are read
    /// against the new one.
    pub fn rebase(&mut self, base: &str) {
        if self.mode != Mode::Agents || self.base.as_deref() == Some(base) {
            return;
        }
        self.base = Some(base.to_owned());
        for s in &mut self.sections {
            if matches!(s.spec.left, Side::Ref(_)) {
                s.spec.left = Side::Ref(base.to_owned());
                s.stale |= s.loaded.is_some();
            }
        }
    }
}
