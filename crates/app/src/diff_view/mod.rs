//! The diff view: the changes of one or many files, drawn by the GUI over
//! the editor area until it's closed. Read-only: the lines come from git (or
//! the disk, or the agents' snapshot) and are diffed here, in Rust, off the
//! UI thread; Neovim isn't asked for anything. Files are sections of one
//! scrolling view, each with a header that sticks to the top while you are in
//! it; a file's diff is computed when it scrolls near, so a change of 200
//! files opens at once.
//!
//! This file is the state: sections, the cursor and selection, scrolling
//! and the keys. `model` makes the rows of a diff, `source` reads the files,
//! and `paint` draws.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use serde::Deserialize;

mod edit;
mod find;
pub mod model;
mod paint;
pub mod source;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

pub use edit::{Do, Edit};
pub use find::{field_id as find_id, Find};
pub use paint::{DiffColors, Look};
use model::{Body, Kind, Row, SplitRow};
pub use source::{compared, side_name, Coloring, FileSpec, Files, Mode, Request, Side, Status};
use source::{Done, Job, Loaded, Worker};

/// How the lines are laid out (`[diff] layout`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// One column, as `git diff` prints it.
    #[default]
    Unified,
    /// Old on the left, new on the right.
    Split,
}

impl Layout {
    pub fn other(self) -> Layout {
        match self {
            Layout::Unified => Layout::Split,
            Layout::Split => Layout::Unified,
        }
    }
}

/// What a view is opened with (from the settings).
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub layout: Layout,
    pub context: usize,
    pub max_lines: usize,
    /// Side by side, the left side's share of the width.
    pub split: f32,
}

/// Side by side, the least share of the width either side keeps.
pub const SPLIT_MIN: f32 = 0.15;

impl Default for Options {
    fn default() -> Self {
        Self { layout: Layout::Unified, context: 3, max_lines: model::MAX_LINES, split: 0.5 }
    }
}

/// What the view asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Close,
    /// Open the file in the editor at this line (1-based), and close the view.
    Open { path: PathBuf, line: usize },
    /// A file too large to show here: Neovim's diff of it, in a tab.
    Neovim(FileSpec),
    /// Change the files: a change or a file reverted, accepted, staged,
    /// unstaged or discarded. The app does it.
    Edit(Edit),
    /// `:` was pressed: Neovim's command line.
    Command,
}

/// Where the cursor is: a section's header, or one of its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor {
    pub section: usize,
    /// `None`: the section's header.
    pub row: Option<usize>,
}

/// The rows of a section for the layout it was made for.
#[derive(Clone, Debug)]
pub enum Rows {
    Unified(Vec<Row>),
    Split(Vec<SplitRow>),
}

impl Rows {
    pub fn len(&self) -> usize {
        match self {
            Rows::Unified(r) => r.len(),
            Rows::Split(r) => r.len(),
        }
    }

    fn change_starts(&self) -> Vec<usize> {
        match self {
            Rows::Unified(r) => model::change_starts(r.iter().map(Row::is_change)),
            Rows::Split(r) => model::change_starts(r.iter().map(SplitRow::is_change)),
        }
    }
}

/// One file of the view.
pub struct Section {
    pub spec: FileSpec,
    pub loaded: Option<Loaded>,
    /// A job for it is queued or running.
    requested: bool,
    /// The request whose answer this section takes (`Done::serial`): an
    /// answer to an older one is out of date.
    serial: u64,
    /// What's loaded is out of date (the disk or the colors changed).
    stale: bool,
    pub folded: bool,
    /// The folds (by op) a click opened.
    pub expanded: BTreeSet<usize>,
    /// The rows, for the layout and context they were made with.
    pub rows: Option<(Layout, usize, Rows)>,
    /// Colors are asked for the lines a fold opened.
    colors_asked: bool,
    /// Accept was pressed (at the latest request then, and when): its buttons
    /// are gone until the file is read again after it (or it takes too long).
    accepting: Option<(u64, Instant)>,
}

impl Section {
    fn new(spec: FileSpec) -> Self {
        Self { spec, loaded: None, requested: false, serial: 0, stale: false, folded: false, expanded: BTreeSet::new(), rows: None, colors_asked: false, accepting: None }
    }

    pub fn diff(&self) -> Option<&model::Diff> {
        match &self.loaded {
            Some(Loaded { body: Body::Text(d), .. }) => Some(d),
            _ => None,
        }
    }

    /// What the change is: as read, once loaded, else as named.
    pub fn status(&self) -> Status {
        self.loaded.as_ref().map_or(self.spec.status, |l| l.status)
    }

    /// Lines added and removed, once loaded (a binary file has none).
    pub fn counts(&self) -> Option<(usize, usize)> {
        match &self.loaded {
            Some(Loaded { body: Body::Text(d), .. }) => Some((d.added, d.removed)),
            Some(_) => Some((0, 0)),
            None => None,
        }
    }

    /// The rows the cursor can be on.
    fn cursor_rows(&self) -> usize {
        match (&self.rows, self.diff()) {
            (Some((.., rows)), Some(d)) if !self.folded && !d.is_identical() => rows.len(),
            _ => 0,
        }
    }

    /// The rows drawn under the header (a message is one; a file too large
    /// for the view has its button on a row of its own).
    pub fn body_rows(&self) -> usize {
        if self.folded {
            0
        } else if matches!(&self.loaded, Some(Loaded { body: Body::TooLarge { .. }, .. })) {
            2
        } else {
            self.cursor_rows().max(1)
        }
    }
}

/// Where `]c` can stop in a section.
enum Stops {
    /// Not loaded yet.
    Unknown,
    /// Nothing in rows to stop at: its header.
    Header,
    Rows(Vec<usize>),
}

/// A move to a change in a section that hadn't loaded, made when it has.
#[derive(Clone, Copy)]
struct Jump {
    section: usize,
    forward: bool,
}

/// Sizes the painter measured, which the scrolling uses.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub row_h: f32,
    pub header_h: f32,
    pub gap: f32,
    pub char_w: f32,
}

impl Default for Geometry {
    fn default() -> Self {
        Self { row_h: 20.0, header_h: 28.0, gap: 10.0, char_w: 8.0 }
    }
}

/// The disk changed: how long to wait for it to settle before reading again.
const REDIFF_AFTER: Duration = Duration::from_millis(300);

pub struct DiffView {
    pub top: PathBuf,
    /// What is compared ("HEAD → working tree").
    pub compared: String,
    pub layout: Layout,
    /// Side by side, the left side's share of the width (the divider is dragged).
    pub split: f32,
    /// The divider is being dragged.
    split_drag: bool,
    pub context: usize,
    max_lines: usize,
    pub sections: Vec<Section>,
    /// A commit's files are being listed.
    awaiting_files: bool,
    /// The file to start at, once the sections are there.
    focus_path: Option<String>,
    worker: Worker,
    /// The last request's serial (sections' requests are numbered across the view).
    serial: u64,
    coloring: Option<Coloring>,
    pub scroll: f32,
    pub scroll_x: f32,
    /// Where the rows are on the screen, as of the last frame.
    body: egui::Rect,
    /// The scroll bar is being dragged: its press and drag are not the rows'.
    pub bar_drag: bool,
    /// A file whose header is to stay at the top while the files around it
    /// are read (their sizes move it): from opening at a file, or jumping to one.
    pin: Option<usize>,
    pub cursor: Cursor,
    pub anchor: Option<Cursor>,
    /// A key that starts a pair (`]`, `[`, `z`, `g`).
    pending: Option<Key>,
    jump: Option<Jump>,
    pub geom: Geometry,
    pub viewport_h: f32,
    disk_dirty: Option<Instant>,
    /// A press on the rows is being dragged.
    pub dragging: bool,
    /// The view has the keyboard (the cursor shows it).
    focused: bool,
    /// What the view can do to the changes.
    pub mode: Mode,
    /// The button that was pressed once: the next press (or its key) does it.
    armed: Option<edit::Armed>,
    /// Where the cursor goes when the file just acted on is read again.
    place: Option<edit::Place>,
    /// Where the pointer is, in the view's coordinates of the screen.
    ptr: Option<egui::Pos2>,
    /// The agents' snapshot the left sides are read from.
    base: Option<String>,
    /// The find field, when open.
    pub find: Option<Find>,
}

impl DiffView {
    pub fn new(req: Request, opts: Options, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let worker = Worker::new(wake);
        let mut view = Self {
            top: req.top.clone(),
            compared: req.compared,
            layout: opts.layout,
            split: opts.split.clamp(SPLIT_MIN, 1.0 - SPLIT_MIN),
            split_drag: false,
            context: opts.context,
            max_lines: opts.max_lines,
            sections: Vec::new(),
            awaiting_files: false,
            focus_path: req.focus,
            worker,
            serial: 0,
            coloring: None,
            scroll: 0.0,
            scroll_x: 0.0,
            body: egui::Rect::ZERO,
            bar_drag: false,
            pin: None,
            cursor: Cursor { section: 0, row: None },
            anchor: None,
            pending: None,
            jump: None,
            geom: Geometry::default(),
            viewport_h: 800.0,
            disk_dirty: None,
            dragging: false,
            focused: false,
            mode: req.mode,
            armed: None,
            place: None,
            ptr: None,
            base: None,
            find: None,
        };
        match req.files {
            Files::List(files) => view.set_files(files),
            Files::Commit(commit) => {
                view.awaiting_files = true;
                view.worker.send(Job::ListCommit { top: req.top, commit }, true);
            }
        }
        view
    }

    fn set_files(&mut self, files: Vec<FileSpec>) {
        self.awaiting_files = false;
        self.base = files.iter().find_map(|f| if let Side::Ref(r) = &f.left { Some(r.clone()) } else { None }).filter(|_| self.mode == Mode::Agents);
        self.sections = files.into_iter().map(Section::new).collect();
        self.cursor = Cursor { section: 0, row: None };
        self.scroll = 0.0;
        if let Some(path) = self.focus_path.take() {
            if let Some(i) = self.sections.iter().position(|s| s.spec.path == path) {
                self.go_to_file(i);
            }
        }
    }

    /// Opening says what it was: "3 files" for the title, none while a commit's are listed.
    pub fn awaiting_files(&self) -> bool {
        self.awaiting_files
    }

    // Loading.

    /// The colorscheme's syntax theme changed (or came): colors are made again.
    pub fn set_coloring(&mut self, coloring: Option<Coloring>) {
        let same = match (&self.coloring, &coloring) {
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(&a.theme, &b.theme) && a.fg == b.fg,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.coloring = coloring;
            for s in &mut self.sections {
                s.stale |= s.loaded.is_some();
            }
        }
    }

    /// A file changed on disk: the files read from it are diffed again, a
    /// moment from now (a build or an agent writes many times).
    pub fn on_disk_changed(&mut self, now: Instant) {
        if self.sections.iter().any(|s| s.spec.reads_disk()) {
            self.disk_dirty.get_or_insert(now);
        }
    }

    /// Take what the worker finished. Whether anything changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(done) = self.worker.done.try_recv() {
            changed = true;
            self.take(done);
        }
        changed
    }

    /// Apply one answer of the worker.
    fn take(&mut self, done: Done) {
        match done {
            Done::Files { files, .. } => self.set_files(files),
            Done::Loaded { index, serial, loaded } => {
                // (The file was asked for again, or the files are others now.)
                let Some(s) = self.sections.get_mut(index).filter(|s| s.serial == serial) else { return };
                s.loaded = Some(loaded);
                s.requested = false;
                s.accepting = s.accepting.filter(|(at, _)| serial <= *at);
                s.colors_asked = false;
                s.rows = None;
                // The folds the click opened belong to the old diff.
                s.expanded.clear();
                self.prepare();
                if let Some(j) = self.jump.filter(|j| j.section == index) {
                    self.jump = None;
                    self.goto_in_section(j.section, j.forward);
                }
                self.after_load(index);
            }
            Done::Colors { index, serial, colors } => {
                // Only for the diff it was made from.
                let Some(s) = self.sections.get_mut(index).filter(|s| s.serial == serial) else { return };
                s.colors_asked = false;
                if let (Some(l), Some(colors)) = (s.loaded.as_mut(), colors) {
                    match &mut l.colors {
                        Some(have) => have.merge(colors),
                        None => l.colors = Some(colors),
                    }
                }
                // (A fold opened while they were made.)
                self.color_shown(index);
            }
        }
    }

    /// Make the rows of the loaded sections (again, where the layout or
    /// context changed), and keep the cursor on a row there is.
    pub fn prepare(&mut self) {
        let (layout, context) = (self.layout, self.context);
        let mut remade = Vec::new();
        for (i, s) in self.sections.iter_mut().enumerate() {
            let fresh = matches!(&s.rows, Some((l, c, _)) if *l == layout && *c == context);
            if fresh {
                continue;
            }
            remade.push(i);
            s.rows = match &s.loaded {
                Some(Loaded { body: Body::Text(d), .. }) => Some((
                    layout,
                    context,
                    match layout {
                        Layout::Unified => Rows::Unified(d.unified(context, &s.expanded)),
                        Layout::Split => Rows::Split(d.split(context, &s.expanded)),
                    },
                )),
                _ => None,
            };
        }
        // Lines a fold or more context shows now are colored.
        for i in remade {
            self.color_shown(i);
        }
        if let Some(i) = self.pin.filter(|i| *i < self.sections.len()) {
            // The file stays at the top while those near it are read; then
            // the scroll is as far as the view goes.
            let top = self.tops()[i];
            self.scroll = top;
            if self.near().iter().all(|&j| self.sections[j].loaded.is_some()) {
                self.pin = None;
                self.scroll = top.clamp(0.0, self.max_scroll());
            }
        }
        let n = self.sections.len();
        if n > 0 {
            self.cursor.section = self.cursor.section.min(n - 1);
            if let Some(r) = self.cursor.row {
                let rows = self.sections[self.cursor.section].cursor_rows();
                if rows == 0 {
                    self.cursor.row = None;
                } else if r >= rows {
                    self.cursor.row = Some(rows - 1);
                }
            }
        }
    }

    /// The sections to read now: those within a screen of the viewport.
    pub fn near(&self) -> Vec<usize> {
        let tops = self.tops();
        let (lo, hi) = (self.scroll - self.viewport_h, self.scroll + self.viewport_h * 2.0);
        (0..self.sections.len()).filter(|&i| tops[i] + self.section_height(i) >= lo && tops[i] <= hi).collect()
    }

    /// Ask for the sections that are near and not read (or out of date). Run
    /// each frame; returns when to wake for a re-diff waiting to settle.
    pub fn tick(&mut self, now: Instant) -> Option<Duration> {
        let mut wake = None;
        if let Some(since) = self.disk_dirty {
            match REDIFF_AFTER.checked_sub(now.duration_since(since)) {
                Some(left) if !left.is_zero() => wake = Some(left),
                _ => {
                    self.disk_dirty = None;
                    for s in &mut self.sections {
                        s.stale |= s.spec.reads_disk() && s.loaded.is_some();
                    }
                }
            }
        }
        self.prepare();
        // Those in view first.
        let near = self.near();
        let tops = self.tops();
        let in_view = |i: usize| tops[i] + self.section_height(i) >= self.scroll && tops[i] <= self.scroll + self.viewport_h;
        let (urgent, rest): (Vec<usize>, Vec<usize>) = near.into_iter().partition(|&i| in_view(i));
        for i in rest {
            self.request(i, false);
        }
        for i in urgent.into_iter().rev() {
            self.request(i, true);
        }
        wake
    }

    /// Ask for the colors of section `i`'s lines that are shown and not colored (only once
    /// its first colors are in: those cover what it showed then).
    fn color_shown(&mut self, i: usize) {
        let (Some(coloring), Some(s)) = (&self.coloring, self.sections.get(i)) else { return };
        let Some(Loaded { body: Body::Text(d), colors: Some(colors), .. }) = &s.loaded else { return };
        if s.colors_asked || s.folded {
            return;
        }
        let Some(shown) = colors.missing(&d.shown(self.context, &s.expanded)) else { return };
        let job = Job::Colors { index: i, serial: s.serial, path: s.spec.path.clone(), old: d.old.clone(), new: d.new.clone(), coloring: coloring.clone(), shown };
        self.sections[i].colors_asked = true;
        self.worker.send(job, true);
    }

    fn request(&mut self, i: usize, urgent: bool) {
        let s = &mut self.sections[i];
        if s.requested || (s.loaded.is_some() && !s.stale) {
            return;
        }
        s.requested = true;
        s.stale = false;
        self.serial += 1;
        s.serial = self.serial;
        self.worker.send(
            Job::Load { index: i, serial: s.serial, top: self.top.clone(), spec: s.spec.clone(), coloring: self.coloring.clone(), max_lines: self.max_lines, context: self.context },
            urgent,
        );
    }

    /// The sections' diffs read so far, and how many there are.
    pub fn loaded(&self) -> (usize, usize) {
        (self.sections.iter().filter(|s| s.loaded.is_some()).count(), self.sections.len())
    }

    /// Lines added and removed over the files read so far.
    pub fn totals(&self) -> (usize, usize) {
        self.sections.iter().filter_map(Section::counts).fold((0, 0), |(a, r), (sa, sr)| (a + sa, r + sr))
    }

    // Geometry.

    pub fn section_height(&self, i: usize) -> f32 {
        self.geom.header_h + self.sections[i].body_rows() as f32 * self.geom.row_h
    }

    /// The top of each section, in the view's own coordinates.
    pub fn tops(&self) -> Vec<f32> {
        let mut y = 0.0;
        self.sections
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let top = y;
                y += self.section_height(i) + self.geom.gap;
                top
            })
            .collect()
    }

    pub fn content_height(&self) -> f32 {
        self.tops().last().map_or(0.0, |t| t + self.section_height(self.sections.len() - 1))
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content_height() - self.viewport_h).max(0.0)
    }

    pub fn scroll_by(&mut self, dy: f32) {
        self.pin = None;
        self.scroll = (self.scroll + dy).clamp(0.0, self.max_scroll());
    }

    /// Scroll to `y` (the scroll bar's).
    pub fn scroll_to(&mut self, y: f32) {
        self.scroll_by(y - self.scroll);
    }

    /// What the scroll bar (`Bar::Diff`) shows: all the sections' height,
    /// in pixels, over the rows.
    pub fn scrollbar_target(&self, fg: egui::Color32, bg: egui::Color32) -> Option<crate::scrollbar::Target> {
        (!self.sections.is_empty()).then(|| crate::scrollbar::Target {
            bar: crate::scrollbar::Bar::Diff,
            area: self.body,
            top: self.scroll.round() as i64,
            bottom: (self.scroll + self.viewport_h).round() as i64,
            lines: self.content_height().round() as i64,
            fg,
            bg,
        })
    }

    /// Where the cursor's line is: its top and height.
    fn y_of(&self, c: Cursor) -> (f32, f32) {
        let tops = self.tops();
        let top = tops.get(c.section).copied().unwrap_or(0.0);
        match c.row {
            None => (top, self.geom.header_h),
            Some(r) => (top + self.geom.header_h + r as f32 * self.geom.row_h, self.geom.row_h),
        }
    }

    /// Scroll so that the cursor is in view (a jump puts it a third of the
    /// way down; a header at the top).
    pub fn reveal(&mut self, jump: bool) {
        let (y, h) = self.y_of(self.cursor);
        let (top, bottom) = (self.scroll, self.scroll + self.viewport_h);
        self.pin = None;
        if self.cursor.row.is_none() {
            self.scroll = y;
        } else if y < top + self.geom.header_h || y + h > bottom {
            self.scroll = if jump { y - self.viewport_h / 3.0 } else if y < top + self.geom.header_h { y - self.geom.header_h } else { y + h - self.viewport_h };
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// The section under a y of the view's coordinates, and the line in it
    /// (`None`: its header).
    pub fn locate(&self, y: f32) -> Option<Cursor> {
        let tops = self.tops();
        let i = tops.partition_point(|t| *t <= y).checked_sub(1)?;
        let rel = y - tops[i];
        if rel < self.geom.header_h {
            return Some(Cursor { section: i, row: None });
        }
        let row = ((rel - self.geom.header_h) / self.geom.row_h) as usize;
        (row < self.sections[i].cursor_rows()).then_some(Cursor { section: i, row: Some(row) })
    }

    // Moving.

    fn step(&self, c: Cursor, down: bool) -> Cursor {
        let n = self.sections.len();
        let rows = |i: usize| self.sections[i].cursor_rows();
        match (c.row, down) {
            (None, true) if rows(c.section) > 0 => Cursor { row: Some(0), ..c },
            (Some(r), true) if r + 1 < rows(c.section) => Cursor { row: Some(r + 1), ..c },
            (_, true) if c.section + 1 < n => Cursor { section: c.section + 1, row: None },
            (Some(r), false) => Cursor { row: r.checked_sub(1), ..c },
            (None, false) if c.section > 0 => {
                let i = c.section - 1;
                Cursor { section: i, row: rows(i).checked_sub(1) }
            }
            _ => c,
        }
    }

    /// Move the cursor `n` lines (negative: up), extending the selection
    /// with `extend`.
    pub fn move_by(&mut self, n: isize, extend: bool) {
        if self.sections.is_empty() {
            return;
        }
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        for _ in 0..n.unsigned_abs() {
            self.cursor = self.step(self.cursor, n > 0);
        }
        self.reveal(false);
    }

    /// Lines to a page.
    fn page(&self) -> isize {
        ((self.viewport_h / self.geom.row_h) as isize - 2).max(1)
    }

    pub fn go_to_file(&mut self, i: usize) {
        if i >= self.sections.len() {
            return;
        }
        self.anchor = None;
        self.cursor = Cursor { section: i, row: None };
        self.scroll = self.tops()[i].clamp(0.0, self.max_scroll());
        self.pin = Some(i);
    }

    fn stops(&self, i: usize) -> Stops {
        let s = &self.sections[i];
        if s.loaded.is_none() {
            return Stops::Unknown;
        }
        if s.folded {
            return Stops::Header;
        }
        match &s.rows {
            Some((.., rows)) if !s.diff().is_some_and(model::Diff::is_identical) => Stops::Rows(rows.change_starts()),
            _ => Stops::Header,
        }
    }

    /// The first (or last) stop of section `i`, which is loaded.
    fn goto_in_section(&mut self, i: usize, first: bool) {
        let at = match self.stops(i) {
            Stops::Rows(r) if !r.is_empty() => Cursor { section: i, row: if first { r.first().copied() } else { r.last().copied() } },
            _ => Cursor { section: i, row: None },
        };
        self.anchor = None;
        self.cursor = at;
        self.reveal(true);
    }

    /// The next (previous) change, across files; waits for a file that
    /// hasn't loaded.
    pub fn goto_change(&mut self, forward: bool) {
        let from = self.cursor;
        let n = self.sections.len();
        let order: Vec<usize> = if forward { (from.section..n).collect() } else { (0..=from.section.min(n.saturating_sub(1))).rev().collect() };
        for i in order {
            let found = match self.stops(i) {
                Stops::Unknown => {
                    self.jump = Some(Jump { section: i, forward });
                    self.request(i, true);
                    return;
                }
                Stops::Header => Some(Cursor { section: i, row: None }).filter(|c| if forward { *c > from } else { *c < from }),
                Stops::Rows(rows) => {
                    let at = |r: &usize| Cursor { section: i, row: Some(*r) };
                    if forward {
                        rows.iter().find(|r| at(r) > from).map(at)
                    } else {
                        rows.iter().rev().find(|r| at(r) < from).map(at)
                    }
                }
            };
            if let Some(c) = found {
                self.anchor = None;
                self.cursor = c;
                self.reveal(true);
                return;
            }
        }
    }

    /// The next file's header (the previous: this file's own, or the one before).
    pub fn goto_file(&mut self, forward: bool) {
        let i = self.cursor.section;
        let target = match (forward, self.cursor.row) {
            (true, _) => (i + 1).min(self.sections.len().saturating_sub(1)),
            (false, Some(_)) => i,
            (false, None) => i.saturating_sub(1),
        };
        self.go_to_file(target);
    }

    // Folding.

    pub fn toggle_fold(&mut self, i: usize) {
        if let Some(s) = self.sections.get_mut(i) {
            s.folded = !s.folded;
            if s.folded && self.cursor.section == i {
                self.cursor.row = None;
            }
        }
        self.anchor = None;
    }

    /// Open the run of unchanged lines folded as `op` in section `i`.
    pub fn expand(&mut self, i: usize, op: usize) {
        if let Some(s) = self.sections.get_mut(i) {
            s.expanded.insert(op);
            s.rows = None;
        }
        self.prepare();
    }

    pub fn toggle_layout(&mut self) {
        // The cursor's line stays the cursor's line in the other layout.
        let line = self.row_identity(self.cursor);
        self.layout = self.layout.other();
        self.anchor = None;
        self.prepare();
        if let (Some((old, new)), Some(r)) = (line, self.cursor.row) {
            let _ = r;
            let rows = self.sections[self.cursor.section].rows.as_ref().map(|(.., r)| r);
            let found = rows.and_then(|rows| match rows {
                Rows::Unified(v) => v.iter().position(|row| matches!(row, Row::Line(l) if (old.is_some() && l.old == old) || (new.is_some() && l.new == new))),
                Rows::Split(v) => v.iter().position(|row| {
                    matches!(row, SplitRow::Pair { left, right, .. }
                        if (old.is_some() && left.map(|c| c.line) == old) || (new.is_some() && right.map(|c| c.line) == new))
                }),
            });
            self.cursor.row = found.or(Some(0));
        }
        self.reveal(false);
    }

    /// The old and new line indexes of the cursor's line.
    fn row_identity(&self, c: Cursor) -> Option<(Option<usize>, Option<usize>)> {
        let rows = &self.sections.get(c.section)?.rows.as_ref()?.2;
        let r = c.row?;
        match rows {
            Rows::Unified(v) => match v.get(r)? {
                Row::Line(l) => Some((l.old, l.new)),
                _ => None,
            },
            Rows::Split(v) => match v.get(r)? {
                SplitRow::Pair { left, right, .. } => Some((left.map(|c| c.line), right.map(|c| c.line))),
                _ => None,
            },
        }
    }

    // Acting on the cursor.

    /// The line of the new side that a click or Enter on `c` opens.
    pub fn open_target(&self, c: Cursor) -> Option<Action> {
        let s = self.sections.get(c.section)?;
        if s.spec.right == Side::Missing || s.status() == Status::Deleted {
            return None;
        }
        let rows = &s.rows.as_ref()?.2;
        let at = match (rows, c.row?) {
            (Rows::Unified(v), r) => match v.get(r)? {
                Row::Line(l) => l.at,
                Row::Fold { .. } => return None,
            },
            (Rows::Split(v), r) => match v.get(r)? {
                SplitRow::Pair { at, .. } => *at,
                SplitRow::Fold { .. } => return None,
            },
        };
        Some(Action::Open { path: self.top.join(&s.spec.path), line: at + 1 })
    }

    /// Enter: fold a header, open a fold, or open the file at the line.
    pub fn activate(&mut self, c: Cursor) -> Option<Action> {
        let Some(row) = c.row else {
            self.toggle_fold(c.section);
            return None;
        };
        let fold = {
            let s = self.sections.get(c.section)?;
            match &s.rows.as_ref()?.2 {
                Rows::Unified(v) => match v.get(row)? {
                    Row::Fold { op, .. } => Some(*op),
                    _ => None,
                },
                Rows::Split(v) => match v.get(row)? {
                    SplitRow::Fold { op, .. } => Some(*op),
                    _ => None,
                },
            }
        };
        match fold {
            Some(op) => {
                self.expand(c.section, op);
                None
            }
            None => self.open_target(c),
        }
    }

    /// The text of the selected lines (the cursor's line when none are
    /// selected), without the `+` and `-`.
    pub fn selected_text(&self) -> Option<String> {
        let anchor = self.anchor.unwrap_or(self.cursor);
        let (a, b) = if anchor <= self.cursor { (anchor, self.cursor) } else { (self.cursor, anchor) };
        let mut lines = Vec::new();
        for i in a.section..=b.section.min(self.sections.len().saturating_sub(1)) {
            let s = &self.sections[i];
            let (Some(d), Some((.., rows))) = (s.diff(), &s.rows) else { continue };
            let first = if i == a.section { a.row.unwrap_or(0) } else { 0 };
            let last = if i == b.section { b.row } else { Some(rows.len().saturating_sub(1)) };
            let Some(last) = last else { continue };
            if a.row.is_none() && i == a.section && a.section == b.section && b.row.is_none() {
                continue;
            }
            for r in first..=last.min(rows.len().saturating_sub(1)) {
                match rows {
                    Rows::Unified(v) => {
                        if let Some(Row::Line(l)) = v.get(r) {
                            let text = if l.kind == Kind::Removed { l.old.and_then(|o| d.old.get(o)) } else { l.new.and_then(|n| d.new.get(n)) };
                            lines.extend(text.cloned());
                        }
                    }
                    Rows::Split(v) => {
                        if let Some(SplitRow::Pair { left, right, .. }) = v.get(r) {
                            let text = match (right, left) {
                                (Some(c), _) => d.new.get(c.line),
                                (None, Some(c)) => d.old.get(c.line),
                                _ => None,
                            };
                            lines.extend(text.cloned());
                        }
                    }
                }
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    // Keys.

    /// A key pressed while the view has the keyboard.
    pub fn key(&mut self, key: Key, m: Modifiers) -> Option<Action> {
        self.prepare();
        // (A button that was pressed once is asked again by its own key only.)
        let armed = self.armed.take();
        if key == Key::Escape && !m.any() && self.find.is_some() {
            self.find = None;
            return None;
        }
        if let Some(first) = self.pending.take() {
            if !m.ctrl && !m.mac_cmd && !m.alt {
                match (first, key) {
                    (Key::CloseBracket, Key::C) => self.goto_change(true),
                    (Key::OpenBracket, Key::C) => self.goto_change(false),
                    (Key::CloseBracket, Key::F) => self.goto_file(true),
                    (Key::OpenBracket, Key::F) => self.goto_file(false),
                    (Key::Z, Key::A) => self.toggle_fold(self.cursor.section),
                    (Key::G, Key::G) => {
                        self.go_to_top();
                    }
                    _ => {}
                }
                return None;
            }
        }
        let page = self.page();
        match key {
            Key::Escape | Key::Q if !m.any() => return Some(Action::Close),
            Key::Colon if !m.ctrl && !m.mac_cmd && !m.alt => return Some(Action::Command),
            Key::Semicolon if m.shift && !m.ctrl && !m.mac_cmd && !m.alt => return Some(Action::Command),
            Key::R if !m.any() && self.mode == Mode::Agents => return self.act_key(Do::Revert, armed),
            Key::Minus if !m.any() && matches!(self.mode, Mode::Stage | Mode::Unstage) => {
                let what = if self.mode == Mode::Stage { Do::Stage } else { Do::Unstage };
                return self.act_key(what, armed);
            }
            Key::X if m.shift && !m.ctrl && !m.mac_cmd && !m.alt && self.mode == Mode::Stage => return self.act_key(Do::Discard, armed),
            Key::J | Key::ArrowDown => self.move_by(1, m.shift),
            Key::K | Key::ArrowUp => self.move_by(-1, m.shift),
            Key::D if m.ctrl => self.move_by(page / 2, false),
            Key::U if m.ctrl => self.move_by(-(page / 2), false),
            Key::PageDown | Key::Space if !m.shift => self.move_by(page, false),
            Key::PageUp => self.move_by(-page, false),
            Key::Space if m.shift => self.move_by(-page, false),
            Key::Home => self.go_to_top(),
            Key::End => self.go_to_bottom(),
            Key::G if m.shift => self.go_to_bottom(),
            Key::G => self.pending = Some(Key::G),
            Key::H | Key::ArrowLeft => self.scroll_x = (self.scroll_x - self.geom.char_w * 4.0).max(0.0),
            Key::L | Key::ArrowRight => self.scroll_x += self.geom.char_w * 4.0,
            Key::N => self.goto_change(!m.shift),
            Key::S if !m.any() => {
                self.toggle_layout();
            }
            Key::CloseBracket | Key::OpenBracket | Key::Z => self.pending = Some(key),
            Key::Enter => {
                let c = self.cursor;
                return self.activate(c);
            }
            _ => {}
        }
        None
    }

    fn go_to_top(&mut self) {
        self.pin = None;
        self.anchor = None;
        self.cursor = Cursor { section: 0, row: None };
        self.scroll = 0.0;
    }

    fn go_to_bottom(&mut self) {
        self.pin = None;
        self.anchor = None;
        let Some(last) = self.sections.len().checked_sub(1) else { return };
        self.cursor = Cursor { section: last, row: self.sections[last].cursor_rows().checked_sub(1) };
        self.scroll = self.max_scroll();
    }

    /// The Enter or click on a file's line number: open it there.
    pub fn open_at(&self, c: Cursor) -> Option<Action> {
        self.open_target(c)
    }
}
