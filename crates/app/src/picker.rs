//! The native picker: a box at the top center of the editor area with a
//! field and a list, in the style of VS Code's quick open. Neovim says what
//! it knows (`picker.lua`: buffers, recent files, a `vim.ui.select`'s items),
//! the app walks the folder for files, ranks (`rank.rs`) or greps (`grep.rs`),
//! draws and takes the keys, and answers with what was chosen.
//!
//! A source gives rows, the core filters, shows and accepts: a row can
//! carry a `path`, `lnum` and `col`. A picker opened with a preview gets a
//! wider box with the file's text beside the list (`preview.rs`), where the
//! window is wide enough.

mod grep;
mod preview;
mod rank;
#[cfg(test)]
mod tests;
mod walk;

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::text::LayoutJob;
use egui::{Align2, Color32, FontId, Id, Key, Margin, Modifiers, Order, Pos2, Rect, RichText, Sense, Vec2};
use nucleo_matcher::pattern::Pattern;
use nucleo_matcher::Matcher;
use nvim_bridge::gui::{PickerChoice, PickerOpen};

use crate::fonts::GridFonts;
use crate::icons::{IconTextures, IconTheme};
use crate::popup::{mix, PopupTheme};
use crate::popupmenu::{row_height, MAX_VISIBLE};
use crate::renderer::CellMetrics;
use crate::style::{self, Chrome};
pub use preview::CodeTheme;
pub use preview::Subject;
use preview::{PreviewLook, PreviewPane, Target};
use rank::{RankItem, RankWorker, Ranked};
use walk::{Found, Walk};

/// The box's width, window permitting.
const WIDTH: f32 = 600.0;
/// Under the editor area's top.
const TOP: f32 = 8.0;
/// What the popup frame takes of the box's width: its margin and edge, twice.
const FRAME: f32 = 10.0;
/// The box's width with the preview beside the list, window permitting.
const WIDE: f32 = 1600.0;
/// `[picker] preview = "beside"`: an editor area narrower than this has no preview.
const PREVIEW_MIN_AREA: f32 = 700.0;
/// The list's share of a box with the preview beside it.
const LIST_SHARE: f32 = 0.35;
/// How tall a box with the preview beside the list is, of the editor area.
const TALL: f32 = 0.8;
/// `auto`: the preview goes beside the list only if that gives it this many columns.
const PREVIEW_COLUMNS: f32 = 100.0;
/// The box's width with the preview under the list, window permitting.
const UNDER_WIDE: f32 = 1100.0;
/// How tall a box with the preview under the list is, of the editor area.
const UNDER_TALL: f32 = 0.85;
/// The most rows the list has above the preview...
const UNDER_ROWS: usize = 9;
/// ...and its share of the room the two have, when that is less.
const UNDER_LIST_SHARE: f32 = 0.4;
/// A grep row's first part (icon, name:line, folder), of the row.
const GREP_SPLIT: f32 = 0.42;
/// Grep starts this long after the query stops changing.
const GREP_DEBOUNCE: Duration = Duration::from_millis(80);
/// A live picker asks Neovim this long after the query stops changing.
const LIVE_DEBOUNCE: Duration = Duration::from_millis(150);

/// The field's id: it keeps the keyboard while the picker is open.
pub fn field_id() -> Id {
    Id::unique("eshvi-picker-field")
}

/// The field's cursor after character `at`.
fn set_cursor(ctx: &egui::Context, at: usize) {
    let id = field_id();
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
    state.store(ctx, id);
}

/// Where a row came from, which ranks it a little higher.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Group {
    Buffer,
    Recent,
    #[default]
    Other,
}

impl Group {
    fn parse(text: Option<&str>) -> Self {
        match text {
            Some("buffer") => Self::Buffer,
            Some("recent") => Self::Recent,
            _ => Self::Other,
        }
    }

    fn bonus(self) -> i32 {
        match self {
            Self::Buffer => rank::BUFFER_BONUS,
            Self::Recent => rank::RECENT_BONUS,
            Self::Other => 0,
        }
    }
}

/// One row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// Matched and shown first; a file's path relative to the folder.
    pub text: String,
    /// Dim, after `text` (for items; a file shows its folder instead).
    pub desc: String,
    /// The file's full path.
    pub path: Option<String>,
    pub lnum: Option<usize>,
    pub col: Option<usize>,
    /// Its place in the list Neovim sent (from 1): what the answer names it by.
    pub nvim: Option<usize>,
    pub group: Group,
    /// The buffer it is in, whose text the preview shows.
    pub buf: Option<i64>,
    /// A grep match, or a location: the line shown after the file.
    pub grep: Option<Box<Snippet>>,
    /// A symbol's kind ("Function"): drawn as the completion menu's badge.
    pub kind: Option<String>,
    /// How deep in its parents a symbol is.
    pub depth: usize,
    /// A diagnostic's severity, 1 (error) to 4 (hint).
    pub severity: Option<u8>,
    /// What the filter matches when that isn't `text` alone.
    pub search: Option<String>,
    /// Stretches of the target line (bytes) the preview marks, for a row
    /// that has no `grep` line.
    pub marks: Vec<Range<usize>>,
    /// Dim, at the row's right (a shortcut).
    pub hint: String,
    /// Dim, before `text` (a keymap's mode).
    pub prefix: String,
    /// `desc` is drawn in the grid's font (a register's text).
    pub mono: bool,
}

impl Row {
    /// What the filter matches.
    fn search_text(&self) -> &str {
        self.search.as_deref().unwrap_or(&self.text)
    }
}

/// A grep match's line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snippet {
    /// Trimmed and cut around the first match.
    pub text: String,
    /// The matches in `text` (bytes).
    pub ranges: Vec<Range<usize>>,
    /// The matches in the whole line (bytes), which the preview marks.
    pub line_ranges: Vec<Range<usize>>,
    /// `text`'s colors in its file's language, worked out when first drawn.
    pub colors: std::cell::OnceCell<Vec<(Range<usize>, preview::Style)>>,
}

/// All the rows, the ones Neovim sent first and the walk's after them, each
/// file once.
#[derive(Clone, Debug, Default)]
pub struct Items {
    pub rows: Vec<Row>,
    cwd: PathBuf,
    seen: HashSet<String>,
}

impl Items {
    /// Neovim's rows (a file twice in them counts once, the first).
    pub fn new(cwd: PathBuf, exclude: Option<String>, from_nvim: Vec<nvim_bridge::gui::PickerItem>) -> Self {
        Self::gated(cwd, exclude, from_nvim, &|_| true)
    }

    /// As `new`, leaving out the rows `keep` says no to (they still take
    /// their place in Neovim's list, which the answer names them by).
    pub fn gated(
        cwd: PathBuf,
        exclude: Option<String>,
        from_nvim: Vec<nvim_bridge::gui::PickerItem>,
        keep: &dyn Fn(&nvim_bridge::gui::PickerItem) -> bool,
    ) -> Self {
        let mut items = Self { rows: Vec::new(), cwd, seen: exclude.into_iter().collect() };
        for (i, item) in from_nvim.into_iter().enumerate() {
            if !keep(&item) {
                continue;
            }
            // (A file once; but a file's lines are many rows.)
            if let (Some(path), None) = (&item.path, item.lnum) {
                if !items.seen.insert(path.clone()) {
                    continue;
                }
            }
            items.rows.push(Row {
                text: item.text,
                desc: item.desc,
                path: item.path,
                lnum: item.lnum,
                col: item.col,
                nvim: Some(i + 1),
                group: Group::parse(item.group.as_deref()),
                buf: item.buf,
                grep: item.line.map(|l| {
                    let ranges = |r: Vec<(usize, usize)>| r.into_iter().map(|(a, b)| a..b).collect();
                    Box::new(Snippet { text: l.text, ranges: ranges(l.ranges), line_ranges: ranges(l.line_ranges), colors: Default::default() })
                }),
                kind: item.kind,
                depth: item.depth,
                severity: item.severity,
                search: item.search,
                marks: item.marks.into_iter().map(|(a, b)| a..b).collect(),
                hint: item.hint,
                prefix: item.prefix,
                mono: item.mono,
            });
        }
        items
    }

    /// The walk's files (relative to the folder), except those already
    /// there. Returns the rows added.
    pub fn add_walked(&mut self, files: Vec<String>) -> Range<usize> {
        let start = self.rows.len();
        for rel in files {
            let path = self.cwd.join(&rel).to_string_lossy().into_owned();
            if self.seen.insert(path.clone()) {
                self.rows.push(Row { text: rel, path: Some(path), ..Default::default() });
            }
        }
        start..self.rows.len()
    }
}

/// A file row's text split into its folder and its name.
pub fn split_path(text: &str) -> (&str, &str) {
    match text.rfind('/') {
        Some(i) => (&text[..i], &text[i + 1..]),
        None => ("", text),
    }
}

/// The query with the word before character `cursor` taken out (Ctrl-W): the
/// spaces before the cursor, then a run of letters and digits or of the
/// punctuation between them, so a path goes a part at a time. The new text
/// and where the cursor is in it.
pub fn delete_word_back(text: &str, cursor: usize) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = cursor;
    while start > 0 && chars[start - 1].is_whitespace() {
        start -= 1;
    }
    if start > 0 {
        let is_word = word(chars[start - 1]);
        while start > 0 && !chars[start - 1].is_whitespace() && word(chars[start - 1]) == is_word {
            start -= 1;
        }
    }
    let kept: String = chars[..start].iter().chain(&chars[cursor..]).collect();
    (kept, start)
}

/// `text` cut so that it, with a "…", is no wider than `room`: what to
/// draw, and how many of its characters are kept. Whole when it fits; empty
/// when not even the "…" does.
pub fn truncate_to_width(text: &str, room: f32, width_of: &dyn Fn(&str) -> f32) -> (String, usize) {
    let total = text.chars().count();
    if width_of(text) <= room {
        return (text.to_owned(), total);
    }
    let cut = |n: usize| format!("{}…", text.chars().take(n).collect::<String>());
    // The most characters whose cut still fits.
    let (mut lo, mut hi) = (0, total);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width_of(&cut(mid)) <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if width_of(&cut(lo)) > room {
        return (String::new(), 0);
    }
    (cut(lo), lo)
}

/// The selected row after moving `by` rows through `count`, wrapping round
/// the ends (a page, which doesn't wrap, is `step_clamped`).
pub fn step(selected: usize, count: usize, by: isize) -> usize {
    if count == 0 {
        return 0;
    }
    (selected as isize + by).rem_euclid(count as isize) as usize
}

pub fn step_clamped(selected: usize, count: usize, by: isize) -> usize {
    if count == 0 {
        return 0;
    }
    (selected as isize + by).clamp(0, count as isize - 1) as usize
}

/// Input the picker keeps from Neovim while it is open: the keys, text and
/// the mouse (the wheel scrolls its list, a click is its own).
pub fn takes_input(ev: &egui::Event) -> bool {
    use egui::Event;
    matches!(
        ev,
        Event::Key { .. }
            | Event::Text(_)
            | Event::Ime(_)
            | Event::Paste(_)
            | Event::Copy
            | Event::Cut
            | Event::PointerButton { .. }
            | Event::PointerMoved(_)
            | Event::MouseWheel { .. }
    )
}

/// Put held events back at the front of the frame's: they came before what is there.
fn give_back(ctx: &egui::Context, mut held: Vec<egui::Event>) {
    ctx.input_mut(|i| {
        held.append(&mut i.events);
        i.events = held;
    });
}

fn is_keyboard(ev: &egui::Event) -> bool {
    matches!(ev, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Ime(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut)
}

/// What the picker asks the app to do.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// "edit", "split", "vsplit", "tab" or "quickfix" the chosen.
    Accept { id: i64, chosen: Vec<PickerChoice>, how: &'static str },
    Cancel(i64),
    /// Ask Neovim for the preview's text (`Eshvi.picker.preview`).
    Preview { id: i64, request: preview::Request },
    /// A live picker's query, which has been still long enough: Neovim finds
    /// the rows (`Eshvi.picker.query`) and answers with this `gen`.
    Query { id: i64, gen: i64, text: String },
    /// The picker is on screen with its field focused: Neovim may hand over
    /// what was typed meanwhile (`Eshvi.picker.opened`).
    Opened(i64),
    /// The selection has stayed on the row Neovim's list holds at `idx` (from 1).
    Selected { id: i64, idx: usize },
}

/// How long keys are held for what Neovim hands over, if it never does.
const TYPED_PATIENCE: Duration = Duration::from_millis(500);

/// How long the selection stays before a picker that applies it live hears.
const SELECT_DEBOUNCE: Duration = Duration::from_millis(100);

/// A picker that tells Neovim about its selection (the colorschemes).
struct SelectRun {
    /// The row the selection is on, and since when.
    current: Option<usize>,
    since: Instant,
    /// What Neovim was last told, or already has (the row the picker opened on).
    sent: Option<usize>,
    armed: bool,
}

/// The box's measures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxLayout {
    /// The popup frame's width.
    pub width: f32,
    /// The list's (and the field's), inside the frame.
    pub list_width: f32,
    /// The preview's, beside the list; None where there is no room for it.
    pub preview_width: Option<f32>,
    /// The preview is under the list, this tall (beside it, it is as tall as the list).
    pub preview_height: Option<f32>,
    /// The list's rows, fixed (a box with a preview doesn't change height).
    pub rows: Option<usize>,
}

/// Where the preview goes (`[picker] preview`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewPlacement {
    /// Beside the list if it gets `PREVIEW_COLUMNS` there, else under it.
    #[default]
    Auto,
    Beside,
    Under,
}

/// The box with the preview beside the list.
fn beside(area: Rect, row_h: f32) -> BoxLayout {
    let width = WIDE.min(area.width() - 2.0 * style::TAB_PAD).max(0.0);
    let inner = (width - FRAME).max(0.0);
    let list_width = ((inner - style::GAP) * LIST_SHARE).floor();
    // The field and the room around the list take about two rows.
    let rows = ((area.height() * TALL - 2.0 * row_h) / row_h).floor().max(MIN_ROWS as f32) as usize;
    BoxLayout { width, list_width, preview_width: Some(inner - style::GAP - list_width), preview_height: None, rows: Some(rows) }
}

/// The box with the preview under the list, the width theirs alike; none in an
/// area too short to show some of both.
fn under(area: Rect, row_h: f32) -> Option<BoxLayout> {
    let width = UNDER_WIDE.min(area.width() - 2.0 * style::TAB_PAD).max(0.0);
    let inner = (width - FRAME).max(0.0);
    let room = area.height() * UNDER_TALL - 2.0 * row_h;
    let rows = ((room * UNDER_LIST_SHARE / row_h).floor() as usize).clamp(MIN_ROWS, UNDER_ROWS);
    let height = room - rows as f32 * row_h - style::GAP;
    (height >= MIN_ROWS as f32 * row_h).then_some(BoxLayout { width, list_width: inner, preview_width: Some(inner), preview_height: Some(height), rows: Some(rows) })
}

/// The box in an editor area `area` wide and tall, for a picker that does or
/// doesn't want a preview, with rows `row_h` tall and columns `cell_w` wide.
pub fn layout(area: Rect, wants_preview: bool, row_h: f32, cell_w: f32, placement: PreviewPlacement) -> BoxLayout {
    let with_preview = wants_preview
        .then(|| match placement {
            PreviewPlacement::Beside => (area.width() >= PREVIEW_MIN_AREA).then(|| beside(area, row_h)),
            PreviewPlacement::Under => under(area, row_h),
            PreviewPlacement::Auto => Some(beside(area, row_h)).filter(|b| b.preview_width.unwrap_or(0.0) >= PREVIEW_COLUMNS * cell_w).or_else(|| under(area, row_h)),
        })
        .flatten();
    with_preview.unwrap_or_else(|| {
        let width = WIDTH.min(area.width() - 2.0 * style::TAB_PAD).max(0.0);
        BoxLayout { width, list_width: (width - FRAME).max(0.0), preview_width: None, preview_height: None, rows: None }
    })
}

/// The fewest rows a box with a preview has.
const MIN_ROWS: usize = 4;

/// Grep's state.
struct GrepRun {
    scope: grep::Scope,
    literal: bool,
    /// The search running, if any (dropping it stops it).
    search: Option<grep::Grep>,
    /// The query changed at this time: a search starts when it has been still.
    pending: Option<Instant>,
    /// The next matches are a new search's: they replace the rows.
    fresh: bool,
    /// The limit stopped the search.
    capped: bool,
}

/// A live picker's state: Neovim finds the rows for each query.
struct LiveRun {
    /// The number of the latest query sent (or of the emptied one): an
    /// answer carries the number it was asked with, and an older one is dropped.
    gen: i64,
    /// The query changed at this time: it is sent when it has been still.
    pending: Option<Instant>,
    /// A query was sent and has no answer yet.
    waiting: bool,
    /// What the list says while the query is empty.
    hint: String,
}

/// What is listed: every row in order (an empty query), or those matching.
enum Shown {
    All,
    Matches(Vec<Ranked>),
}

struct State {
    id: i64,
    title: String,
    files: bool,
    multi: bool,
    items: Items,
    query: String,
    /// The query's number; the ranking answers with the number it ranked for.
    version: u64,
    pattern: Pattern,
    shown: Shown,
    /// The query the list is of.
    shown_version: u64,
    worker: Option<RankWorker>,
    walk: Option<Walk>,
    grep: Option<GrepRun>,
    live: Option<LiveRun>,
    /// The preview of the selected row, for a picker that has one.
    preview: Option<PreviewPane>,
    /// It was drawn last frame (the window was wide enough).
    preview_shown: bool,
    /// Rows show their line number, this many digits wide.
    number_digits: usize,
    /// The user has moved the selection (a grep that is still finding rows
    /// then keeps it on its match).
    moved: bool,
    /// Enter was pressed before the rows it means were there: it takes the
    /// first one once they are (`how` it opens).
    pending_accept: Option<&'static str>,
    /// Esc came by `typed`.
    pending_cancel: bool,
    /// The row to select once the rows are there (a resumed picker).
    want: Option<nvim_bridge::gui::PickerWant>,
    /// Neovim hasn't yet been told the box is up.
    announce: bool,
    /// Since when the box waits for what Neovim took out of its typeahead
    /// (`typed`, even if empty). Keys the user types meanwhile come after those,
    /// so they are held and given to the field when it arrives.
    awaiting_typed: Option<Instant>,
    held: Vec<egui::Event>,
    select: Option<SelectRun>,
    selected: usize,
    /// Rows marked with Tab, in the order marked (indexes into the items).
    marked: Vec<usize>,
    /// A row to bring into view.
    scroll_to: Option<usize>,
    /// How far the list is scrolled.
    offset: f32,
    /// The most rows the list has had: the box doesn't shrink below them.
    tallest: usize,
    /// Matched positions of rows past the first few, found when drawn.
    highlights: std::collections::HashMap<usize, Vec<u32>>,
    matchers: (Matcher, Matcher),
    /// Where each listed row was drawn this frame, by list row.
    #[cfg(test)]
    row_rects: Vec<(usize, Rect)>,
}

impl State {
    fn count(&self) -> usize {
        match &self.shown {
            Shown::All => self.items.rows.len(),
            Shown::Matches(m) => m.len(),
        }
    }

    /// The item shown in list row `row`.
    fn item_at(&self, row: usize) -> Option<usize> {
        match &self.shown {
            Shown::All => (row < self.items.rows.len()).then_some(row),
            Shown::Matches(m) => m.get(row).map(|r| r.index),
        }
    }

    /// Whether the list is of the latest query, with the rows that query is
    /// going to have, or as many of them as there will be: Enter pressed
    /// before this takes the first row once it is so.
    fn ready(&self) -> bool {
        if let Some(g) = &self.grep {
            return g.pending.is_none() && !g.fresh;
        }
        if let Some(l) = &self.live {
            return l.pending.is_none() && !l.waiting;
        }
        self.shown_version == self.version && (self.count() > 0 || self.walk.is_none())
    }

    /// Enter (or an open-elsewhere key): now if the rows are there, else when they are.
    fn request_accept(&mut self, how: &'static str, actions: &mut Vec<Action>) {
        self.pending_accept = Some(how);
        self.settle_accept(actions);
    }

    /// An Enter that came early takes the first row once the list is ready; if it ends
    /// with no rows at all there is nothing to open, and the box stays.
    fn settle_accept(&mut self, actions: &mut Vec<Action>) {
        let Some(how) = self.pending_accept else { return };
        if !self.ready() {
            return;
        }
        self.pending_accept = None;
        actions.extend(self.accept(how));
    }

    /// Select the row a resumed picker had, once the rows are in.
    fn apply_want(&mut self) {
        // (Of the list the query makes, not the one before it comes.)
        if self.want.is_none() || !self.ready() {
            return;
        }
        let Some(want) = self.want.as_ref() else { return };
        let found = (0..self.count()).find(|&r| {
            self.item_at(r).is_some_and(|i| {
                let row = &self.items.rows[i];
                row.text == want.text && (want.path.is_none() || row.path == want.path) && (want.lnum.is_none() || row.lnum == want.lnum)
            })
        });
        match found {
            Some(row) => {
                self.selected = row;
                self.scroll_to = Some(row);
                self.want = None;
            }
            None if self.ready() && self.walk.is_none() && self.grep.as_ref().is_none_or(|g| g.search.is_none()) => self.want = None,
            None => {}
        }
    }

    /// The selected row and the query, for a resume.
    fn snapshot(&self) -> nvim_bridge::gui::PickerState {
        let row = self.item_at(self.selected).map(|i| {
            let row = &self.items.rows[i];
            nvim_bridge::gui::PickerWant { text: row.text.clone(), path: row.path.clone(), lnum: row.lnum }
        });
        nvim_bridge::gui::PickerState { query: self.query.clone(), row }
    }

    /// Tell Neovim about a selection that has stayed (the colorschemes).
    fn select_tick(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let current = self.item_at(self.selected).and_then(|i| self.items.rows[i].nvim);
        let Some(run) = self.select.as_mut() else { return };
        if !run.armed {
            // The row it opens on is already what Neovim has.
            if current.is_none() {
                return;
            }
            run.armed = true;
            run.sent = current;
            run.current = current;
            run.since = Instant::now();
            return;
        }
        if current != run.current {
            run.current = current;
            run.since = Instant::now();
        }
        if current == run.sent {
            return;
        }
        let waited = run.since.elapsed();
        match current {
            Some(idx) if waited >= SELECT_DEBOUNCE => {
                run.sent = current;
                actions.push(Action::Selected { id: self.id, idx });
            }
            Some(_) => ctx.request_repaint_after(SELECT_DEBOUNCE - waited),
            None => {}
        }
    }

    fn start_grep(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        let Some(g) = self.grep.as_mut() else { return };
        g.pending = None;
        g.search = grep::Grep::start(g.scope.clone(), &self.query, g.literal, move || ctx.request_repaint());
        g.fresh = true;
        g.capped = false;
    }

    /// A grep that is waiting for the query to settle starts when it has.
    fn grep_tick(&mut self, ctx: &egui::Context) {
        let Some(since) = self.grep.as_ref().and_then(|g| g.pending) else { return };
        let waited = since.elapsed();
        if waited >= GREP_DEBOUNCE {
            self.start_grep(ctx);
        } else {
            ctx.request_repaint_after(GREP_DEBOUNCE - waited);
        }
    }

    /// A live picker whose query has been still sends it to Neovim.
    fn live_tick(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(l) = self.live.as_mut() else { return };
        let Some(since) = l.pending else { return };
        let waited = since.elapsed();
        if waited >= LIVE_DEBOUNCE {
            l.pending = None;
            l.gen += 1;
            l.waiting = true;
            actions.push(Action::Query { id: self.id, gen: l.gen, text: self.query.clone() });
        } else {
            ctx.request_repaint_after(LIVE_DEBOUNCE - waited);
        }
    }

    /// Neovim's rows for the query `gen`. Whether they were taken (an
    /// answer to an older query is dropped).
    fn set_live_items(&mut self, gen: i64, items: Vec<nvim_bridge::gui::PickerItem>) -> bool {
        let Some(l) = self.live.as_mut() else { return false };
        if gen != l.gen || !l.waiting {
            return false;
        }
        l.waiting = false;
        // (The answer names a row by its place in this list, as Neovim keeps the latest.)
        self.items = Items::new(self.items.cwd.clone(), None, items);
        self.clear_rows_keep_items();
        true
    }

    /// Take in the matches a search has found.
    fn poll_grep(&mut self) {
        let mut hits = Vec::new();
        let mut done = None;
        let Some(g) = self.grep.as_mut() else { return };
        if let Some(search) = &g.search {
            while let Ok(msg) = search.found.try_recv() {
                match msg {
                    grep::Msg::Hits(h) => hits.extend(h),
                    grep::Msg::Done { capped } => done = Some(capped),
                }
            }
        }
        if !hits.is_empty() {
            self.add_hits(hits);
        }
        if let Some(capped) = done {
            let g = self.grep.as_mut().expect("grep");
            g.search = None;
            g.capped = capped;
            // (A search that found nothing: the old rows go.)
            if std::mem::take(&mut g.fresh) {
                self.clear_rows();
            }
        }
    }

    fn clear_rows(&mut self) {
        self.items.rows.clear();
        self.clear_rows_keep_items();
    }

    /// The rows changed: marks and the selection start over.
    fn clear_rows_keep_items(&mut self) {
        self.marked.clear();
        self.highlights.clear();
        self.selected = 0;
        self.moved = false;
        self.scroll_to = Some(0);
    }

    /// Put a search's matches among the rows, by path then line. The selection
    /// stays on its match if the user has moved to one, else on the first row.
    fn add_hits(&mut self, hits: Vec<grep::Hit>) {
        if self.grep.as_mut().is_some_and(|g| std::mem::take(&mut g.fresh)) {
            self.clear_rows();
        }
        let cwd = self.items.cwd.clone();
        let mut new: Vec<Row> = hits
            .into_iter()
            .map(|h| Row {
                path: Some(cwd.join(&h.path).to_string_lossy().into_owned()),
                text: h.path,
                lnum: Some(h.lnum),
                col: Some(h.col),
                grep: Some(Box::new(Snippet { text: h.text, ranges: h.ranges, line_ranges: h.line_ranges, colors: Default::default() })),
                ..Default::default()
            })
            .collect();
        new.sort_by(|a, b| (&a.text, a.lnum, a.col).cmp(&(&b.text, b.lnum, b.col)));
        let old = std::mem::take(&mut self.items.rows);
        let (rows, went) = grep::merge_sorted(old, new, |r| (r.text.as_str(), r.lnum.unwrap_or(0), r.col.unwrap_or(0)));
        self.items.rows = rows;
        self.marked = self.marked.iter().filter_map(|&m| went.get(m).copied()).collect();
        self.selected = if self.moved { went.get(self.selected).copied().unwrap_or(0) } else { 0 };
        if self.moved {
            self.scroll_to = Some(self.selected);
        }
        self.highlights.clear();
    }

    /// What the preview shows: the selected row's file, at its line.
    fn preview_target(&self) -> Option<Target> {
        let row = &self.items.rows[self.item_at(self.selected)?];
        let key = match (row.buf, &row.path) {
            (Some(b), _) => Subject::Buf(b),
            (None, Some(p)) => Subject::Path(p.clone()),
            (None, None) => return None,
        };
        let title = match (&row.grep, &row.path) {
            (Some(_), _) => row.text.clone(),
            (None, Some(p)) => std::path::Path::new(p).strip_prefix(&self.items.cwd).map_or(p.clone(), |r| r.to_string_lossy().into_owned()),
            (None, None) => "[No Name]".to_owned(),
        };
        let ranges = row.grep.as_ref().map_or_else(|| row.marks.clone(), |g| g.line_ranges.clone());
        Some(Target { key, lnum: row.lnum.unwrap_or(1), title, ranges })
    }

    fn rank_item(&self, row: &Row) -> RankItem {
        RankItem { text: row.search_text().to_owned(), bonus: row.group.bonus(), path: self.files }
    }

    /// Take in what the threads have for us.
    fn poll(&mut self) {
        if self.grep.is_some() {
            self.poll_grep();
        }
        while let Some(Ok(done)) = self.worker.as_ref().map(|w| w.results.try_recv()) {
            if done.version != self.version {
                continue;
            }
            // A new query's list starts at its top.
            if self.shown_version != done.version {
                self.shown_version = done.version;
                self.selected = 0;
                self.scroll_to = Some(0);
                self.highlights.clear();
            }
            self.shown = match done.matches {
                None => Shown::All,
                Some(matches) => Shown::Matches(matches),
            };
            self.selected = self.selected.min(self.count().saturating_sub(1));
        }
        let mut added = Vec::new();
        let mut done = false;
        if let Some(walk) = &self.walk {
            while let Ok(found) = walk.found.try_recv() {
                match found {
                    Found::Files(files) => {
                        let range = self.items.add_walked(files);
                        added.extend(self.items.rows[range].iter().map(|r| self.rank_item(r)));
                    }
                    Found::Done => done = true,
                }
            }
        }
        if let (false, Some(worker)) = (added.is_empty(), &self.worker) {
            worker.add(added);
        }
        if done {
            self.walk = None;
        }
    }

    fn set_query(&mut self, query: &str) {
        if query == self.query {
            return;
        }
        self.query = query.to_owned();
        self.version += 1;
        if let Some(l) = self.live.as_mut() {
            // Neovim finds the rows, once the query has been still; nothing is
            // filtered here. An empty query shows the hint and asks for nothing
            // (an answer on its way is dropped).
            self.pattern = rank::pattern(query);
            self.highlights.clear();
            if query.is_empty() {
                l.pending = None;
                l.waiting = false;
                l.gen += 1;
                self.clear_rows();
            } else {
                l.pending = Some(Instant::now());
            }
            return;
        }
        if let Some(g) = self.grep.as_mut() {
            // The search running is for a query that is gone; the next starts
            // once this one stops changing (an empty one searches nothing).
            g.search = None;
            g.capped = false;
            if query.is_empty() {
                g.pending = None;
                g.fresh = false;
                self.clear_rows();
            } else {
                g.pending = Some(Instant::now());
            }
            return;
        }
        self.pattern = rank::pattern(query);
        if let Some(worker) = &self.worker {
            worker.query(self.version, query);
        }
    }

    fn mark(&mut self, item: usize) {
        match self.marked.iter().position(|&m| m == item) {
            Some(at) => {
                self.marked.remove(at);
            }
            None => self.marked.push(item),
        }
    }

    fn choice(&self, item: usize) -> PickerChoice {
        let row = &self.items.rows[item];
        PickerChoice {
            idx: row.nvim,
            // (Neovim knows its own rows by their place; a live list's has moved on
            // by the time an answer lands, so those say what they hold too.)
            path: if row.nvim.is_none() || self.live.is_some() { row.path.clone() } else { None },
            lnum: row.lnum,
            col: row.col,
            // (A grep match's line, for the quickfix list.)
            text: row.grep.as_ref().map(|g| g.text.clone()),
        }
    }

    /// Enter and the open-elsewhere keys: the marked rows go to the
    /// quickfix list, else the selected one is opened as `how` says.
    fn accept(&self, how: &'static str) -> Option<Action> {
        if how == "edit" && !self.marked.is_empty() {
            let chosen = self.marked.iter().map(|&i| self.choice(i)).collect();
            return Some(Action::Accept { id: self.id, chosen, how: "quickfix" });
        }
        let item = self.item_at(self.selected)?;
        Some(Action::Accept { id: self.id, chosen: vec![self.choice(item)], how })
    }

    /// Take the keys the picker has for itself, so the field never sees them.
    fn keys(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let pressed = |ctx: &egui::Context, mods: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(mods, key));
        let count = self.count();
        let page = self.tallest.max(1) as isize;
        let mut moved = None;
        let (none, ctrl, shift) = (Modifiers::NONE, Modifiers::CTRL, Modifiers::SHIFT);
        if pressed(ctx, none, Key::Escape) {
            actions.push(Action::Cancel(self.id));
        }
        if pressed(ctx, none, Key::Enter) {
            self.request_accept("edit", actions);
        }
        for (mods, key, how) in [(ctrl, Key::S, "split"), (ctrl, Key::V, "vsplit"), (ctrl, Key::T, "tab")] {
            if pressed(ctx, mods, key) {
                self.request_accept(how, actions);
            }
        }
        for (mods, key, by) in
            [(none, Key::ArrowDown, 1), (ctrl, Key::N, 1), (ctrl, Key::J, 1), (none, Key::ArrowUp, -1), (ctrl, Key::P, -1), (ctrl, Key::K, -1)]
        {
            while pressed(ctx, mods, key) {
                self.selected = step(self.selected, count, by);
                moved = Some(self.selected);
            }
        }
        for (key, by) in [(Key::PageDown, page), (Key::PageUp, -page)] {
            while pressed(ctx, none, key) {
                self.selected = step_clamped(self.selected, count, by);
                moved = Some(self.selected);
            }
        }
        // Tab marks the row and moves on (Shift-Tab, up), where marking is on. (Shift-Tab
        // first: a plain Tab matches it too.)
        for (mods, by) in [(shift, -1), (none, 1)] {
            while pressed(ctx, mods, Key::Tab) {
                if self.multi {
                    if let Some(item) = self.item_at(self.selected) {
                        self.mark(item);
                    }
                    self.selected = step(self.selected, count, by);
                    moved = Some(self.selected);
                }
            }
        }
        if moved.is_some() {
            self.scroll_to = moved;
            self.moved = true;
            self.want = None;
        }
        // Ctrl-F and Ctrl-B page the preview, as they page the completion docs.
        if self.preview_shown {
            if let Some(pane) = self.preview.as_mut() {
                for (key, pages) in [(Key::F, 1.0), (Key::B, -1.0)] {
                    while pressed(ctx, ctrl, key) {
                        pane.page(pages);
                    }
                }
            }
        }
        // Clear the query, or take a word off its end (at the cursor).
        let edit = |ctx: &egui::Context, text: &str| -> Option<String> {
            let id = field_id();
            let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
            let at = state.cursor.char_range().map_or(text.chars().count(), |r| r.primary.index.into());
            let (kept, at) = if pressed(ctx, ctrl, Key::U) {
                (String::new(), 0)
            } else if pressed(ctx, ctrl, Key::W) {
                delete_word_back(text, at)
            } else {
                return None;
            };
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
            state.store(ctx, id);
            Some(kept)
        };
        if let Some(kept) = edit(ctx, &self.query) {
            self.set_query(&kept);
        }
    }
}

/// What the picker is drawn with.
pub struct Look<'a> {
    pub theme: &'a PopupTheme,
    pub chrome: &'a Chrome,
    pub fonts: &'a GridFonts,
    pub metrics: CellMetrics,
    /// The editor area: the box sits at its top center.
    pub area: Rect,
    /// The editor's colors for the preview's code.
    pub code: &'a CodeTheme,
    /// Errors', warnings' and infos' colors: the Problems panel's.
    pub problems: [Color32; 3],
    /// `[picker] preview`.
    pub preview: PreviewPlacement,
    /// The syntax colors grep's lines are drawn in (the diff view's).
    pub syntax: Option<&'a syntect::highlighting::Theme>,
}

/// A grep match's line colored in `path`'s language (by its extension), as
/// the diff view colors code; none for a language syntect doesn't know.
pub fn snippet_colors(text: &str, path: &str, theme: &syntect::highlighting::Theme, fg: Color32) -> Vec<(Range<usize>, preview::Style)> {
    let lang = crate::diff_view::source::language(path);
    let spans = crate::markdown::spans(&[text.to_owned()], std::slice::from_ref(&(0..1)), lang, theme, fg);
    let line = spans.and_then(|lines| lines.into_iter().next().flatten()).unwrap_or_default();
    line.into_iter().map(|s| (s.start..s.end, preview::Style { fg: Some(s.color), bold: s.bold, italic: s.italic })).collect()
}

#[derive(Default)]
pub struct PickerView {
    state: Option<State>,
    /// The box as drawn last frame.
    rect: Option<Rect>,
    /// It was just opened: put it above the other popups.
    fresh: bool,
}

impl PickerView {
    pub fn is_open(&self) -> bool {
        self.state.is_some()
    }

    pub fn id(&self) -> Option<i64> {
        self.state.as_ref().map(|s| s.id)
    }

    /// The box, as drawn last frame.
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    /// Open the picker (instead of the open one, which Neovim has cancelled).
    #[cfg(test)]
    pub fn open(&mut self, ctx: &egui::Context, open: PickerOpen) {
        self.open_gated(ctx, open, &|_| true);
    }

    /// As `open`, leaving out the rows `keep` says no to.
    pub fn open_gated(&mut self, ctx: &egui::Context, open: PickerOpen, keep: &dyn Fn(&nvim_bridge::gui::PickerItem) -> bool) {
        let cwd = PathBuf::from(&open.cwd);
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let items = Items::gated(cwd.clone(), open.exclude, open.items, keep);
        let walk = match open.source.as_str() {
            "files" | "smart" => Some(Walk::files(cwd.clone(), wake.clone())),
            "git_files" => Some(Walk::git(cwd.clone(), wake.clone())),
            _ => None,
        };
        let grepping = open.source == "grep";
        let worker = (!grepping && !open.live).then(|| RankWorker::new(wake));
        let scope = match (open.grep_files, open.extensions) {
            (Some(files), _) => grep::Scope::Files { cwd, files: files.into_iter().map(PathBuf::from).collect() },
            (None, Some(extensions)) => grep::Scope::Extensions { cwd, extensions },
            (None, None) => grep::Scope::Tree(cwd),
        };
        let number_digits = items.rows.iter().filter_map(|r| r.lnum).max().map_or(0, |n| n.to_string().len());
        let state = State {
            id: open.id,
            title: open.title,
            files: open.files,
            multi: open.multi,
            items,
            query: open.query.clone(),
            // (A query the box opens with is ranked as one typed.)
            version: u64::from(!open.query.is_empty() && !grepping && !open.live),
            pattern: rank::pattern(&open.query),
            shown: Shown::All,
            shown_version: 0,
            worker,
            walk,
            grep: grepping.then(|| GrepRun { scope, literal: open.literal, search: None, pending: None, fresh: false, capped: false }),
            live: open.live.then(|| LiveRun {
                gen: 0,
                pending: (!open.query.is_empty()).then(Instant::now),
                waiting: false,
                hint: open.hint.clone(),
            }),
            preview: open.preview.then(PreviewPane::default),
            preview_shown: false,
            number_digits: if open.numbered { number_digits } else { 0 },
            moved: false,
            pending_accept: None,
            pending_cancel: false,
            want: open.want,
            announce: true,
            awaiting_typed: Some(Instant::now()),
            held: Vec::new(),
            select: open.on_select.then(|| SelectRun { current: None, since: Instant::now(), sent: None, armed: false }),
            selected: open.selected.saturating_sub(1),
            marked: Vec::new(),
            scroll_to: (open.selected > 0).then(|| open.selected - 1),
            offset: 0.0,
            tallest: 0,
            highlights: Default::default(),
            matchers: (rank::matcher(true), rank::matcher(false)),
            #[cfg(test)]
            row_rects: Vec::new(),
        };
        let mut state = state;
        if let Some(worker) = &state.worker {
            let rows: Vec<RankItem> = state.items.rows.iter().map(|r| state.rank_item(r)).collect();
            worker.add(rows);
            if state.version > 0 {
                worker.query(state.version, &state.query);
            }
        }
        if !state.query.is_empty() {
            set_cursor(ctx, state.query.chars().count());
        }
        // A grep that opens with a query (the word under the cursor) searches at once.
        if grepping && !state.query.is_empty() {
            state.start_grep(ctx);
        }
        self.state = Some(state);
        self.rect = None;
        self.fresh = true;
    }

    /// What it holds now, for `Eshvi.picker.resume`: the query and the selected row.
    pub fn state(&self) -> nvim_bridge::gui::PickerState {
        self.state.as_ref().map(State::snapshot).unwrap_or_default()
    }

    /// Keys Neovim took out of its typeahead while the box was opening, as if
    /// typed in the field: text, Backspace, Enter, Esc (the rest is dropped).
    pub fn typed(&mut self, ctx: &egui::Context, id: i64, keys: &[String]) {
        let Some(st) = self.state.as_mut().filter(|s| s.id == id) else { return };
        // The keys held meanwhile come after these.
        let held = std::mem::take(&mut st.held);
        st.awaiting_typed = None;
        give_back(ctx, held);
        let mut chars: Vec<char> = st.query.chars().collect();
        let mut at = egui::TextEdit::load_state(ctx, field_id())
            .and_then(|s| s.cursor.char_range())
            .map_or(chars.len(), |r| usize::from(r.primary.index))
            .min(chars.len());
        for key in keys {
            match key.as_str() {
                "<CR>" | "<Enter>" => {
                    st.pending_accept = Some("edit");
                    break;
                }
                "<Esc>" => {
                    st.pending_cancel = true;
                    break;
                }
                "<BS>" => {
                    if at > 0 {
                        chars.remove(at - 1);
                        at -= 1;
                    }
                }
                other => {
                    let text = match other {
                        "<Space>" => " ",
                        "<lt>" => "<",
                        "<Bar>" => "|",
                        t if t.starts_with('<') && t.chars().count() > 1 => continue,
                        t => t,
                    };
                    for c in text.chars().filter(|c| !c.is_control()) {
                        chars.insert(at, c);
                        at += 1;
                    }
                }
            }
        }
        let query: String = chars.into_iter().collect();
        st.set_query(&query);
        set_cursor(ctx, at);
        ctx.request_repaint();
    }

    /// Close it. The answer, if any, is the caller's.
    pub fn close(&mut self) {
        self.state = None;
        self.rect = None;
    }

    /// The query typed, how many rows match and how many there are, and the
    /// selected row.
    #[cfg(test)]
    pub fn summary(&self) -> Option<(&str, usize, usize, usize)> {
        self.state.as_ref().map(|s| (s.query.as_str(), s.count(), s.items.rows.len(), s.selected))
    }

    /// Where list row `row` was drawn.
    #[cfg(test)]
    pub fn row_rect(&self, row: usize) -> Option<Rect> {
        self.state.as_ref()?.row_rects.iter().find(|(r, _)| *r == row).map(|(_, rect)| *rect)
    }

    /// The rows marked, as the texts of the items.
    #[cfg(test)]
    pub fn marked(&self) -> Vec<String> {
        self.state.as_ref().map_or(Vec::new(), |s| s.marked.iter().map(|&i| s.items.rows[i].text.clone()).collect())
    }

    /// The file line at the top of the preview (from 1), if there is one.
    #[cfg(test)]
    pub fn preview_top(&self) -> Option<usize> {
        self.state.as_ref()?.preview.as_ref().map(|p| p.top_line())
    }

    /// The stretches (bytes) the preview marks on its target line.
    #[cfg(test)]
    pub fn preview_marks(&self) -> Vec<Range<usize>> {
        self.state.as_ref().and_then(|s| s.preview_target()).map_or(Vec::new(), |t| t.ranges)
    }

    /// A live picker has asked for rows that haven't come.
    #[cfg(test)]
    pub fn live_waiting(&self) -> bool {
        self.state.as_ref().and_then(|s| s.live.as_ref()).is_some_and(|l| l.waiting)
    }

    /// How many rows the preview showed last frame.
    #[cfg(test)]
    pub fn preview_rows(&self) -> f32 {
        self.state.as_ref().and_then(|s| s.preview.as_ref()).map_or(0.0, |p| p.rows())
    }

    /// Whether the walk is still going.
    #[cfg(test)]
    pub fn walking(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.walk.is_some())
    }

    /// Whether the list shown is of the latest query.
    #[cfg(test)]
    pub fn settled(&self) -> bool {
        self.state.as_ref().is_some_and(|s| match &s.grep {
            Some(g) => g.pending.is_none() && g.search.is_none(),
            None if s.live.is_some() => s.live.as_ref().is_some_and(|l| l.pending.is_none()),
            None => s.shown_version == s.version,
        })
    }

    /// The rows as listed, best first (the text of each).
    #[cfg(test)]
    pub fn listed(&self) -> Vec<String> {
        let Some(s) = &self.state else { return Vec::new() };
        (0..s.count()).filter_map(|r| s.item_at(r)).map(|i| s.items.rows[i].text.clone()).collect()
    }

    /// Draw the picker and take its keys. What it asks of the app.
    pub fn show(&mut self, ctx: &egui::Context, look: &Look, icons: &mut IconTextures) -> Vec<Action> {
        let Some(st) = self.state.as_mut() else {
            self.rect = None;
            return Vec::new();
        };
        if let Some(since) = st.awaiting_typed {
            // Typed keys wait for what Neovim hands over; if it never does, they go on.
            if since.elapsed() >= TYPED_PATIENCE {
                st.awaiting_typed = None;
                let held = std::mem::take(&mut st.held);
                give_back(ctx, held);
            } else {
                let held: Vec<egui::Event> = ctx.input_mut(|i| {
                    let (keys, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut i.events).into_iter().partition(is_keyboard);
                    i.events = rest;
                    keys
                });
                st.held.extend(held);
                ctx.request_repaint_after(TYPED_PATIENCE - since.elapsed());
            }
        }
        st.poll();
        st.grep_tick(ctx);
        let mut actions = Vec::new();
        st.live_tick(ctx, &mut actions);
        if std::mem::take(&mut st.pending_cancel) {
            actions.push(Action::Cancel(st.id));
        }
        // The field has the keyboard (so text and IME arrive in it), and keeps Tab,
        // Esc and the arrows for the picker.
        ctx.memory_mut(|m| {
            m.request_focus(field_id());
            m.set_focus_lock_filter(
                field_id(),
                egui::EventFilter { tab: true, horizontal_arrows: false, vertical_arrows: true, escape: true },
            );
        });
        st.keys(ctx, &mut actions);

        let theme = look.theme;
        let (area, m) = (look.area, look.metrics);
        let row_h = row_height(m);
        let font = look.fonts.sidebar_font_id(false);
        let small = style::scaled(&font, style::SMALL);
        let lay = layout(area, st.preview.is_some(), row_h, m.width, look.preview);
        let (width, list_w) = (lay.width, lay.list_width);
        let inner = (width - FRAME).max(0.0);
        let pos = Pos2::new(area.center().x - width / 2.0, area.min.y + TOP);
        st.preview_shown = lay.preview_width.is_some();

        // The preview wants what is selected; it asks Neovim once that has been still a moment.
        let mut pane = st.preview.take();
        if let Some(p) = pane.as_mut() {
            if st.preview_shown {
                p.set_target(st.preview_target());
                let due = p.due(Instant::now());
                actions.extend(due.request.map(|request| Action::Preview { id: st.id, request }));
                if let Some(wait) = due.wait {
                    ctx.request_repaint_after(wait);
                }
            }
        }

        let count = st.count();
        st.tallest = match lay.rows {
            Some(rows) => rows,
            None => st.tallest.max(count.min(MAX_VISIBLE)).max(1),
        };
        let list_h = st.tallest as f32 * row_h;
        let walking = st.walk.is_some();
        let searching = st.grep.as_ref().is_some_and(|g| g.search.is_some() || g.pending.is_some());
        let asking = st.live.as_ref().is_some_and(|l| l.waiting || l.pending.is_some());
        let counts = match (&st.grep, count, st.items.rows.len(), walking) {
            _ if st.live.is_some() => match (st.query.is_empty(), count) {
                (true, _) => String::new(),
                (false, n) => format!("{n}{}", if asking { " …" } else { "" }),
            },
            (Some(_), ..) if st.query.is_empty() => String::new(),
            (Some(g), n, ..) => format!("{n}{}{}", if g.capped { "+" } else { "" }, if searching { " …" } else { "" }),
            (None, _, 0, true) => "…".to_owned(),
            (None, matches, total, walking) => format!("{matches}/{total}{}", if walking { " …" } else { "" }),
        };
        let count_w = ctx.fonts_mut(|f| f.layout_no_wrap(counts.clone(), small.clone(), theme.dim_fg).size().x);
        let number_w = (st.number_digits > 0).then(|| ctx.fonts_mut(|f| f.layout_no_wrap("9".repeat(st.number_digits), small.clone(), theme.dim_fg).size().x));
        let grep_hint = st.grep.is_some().then(|| st.query.is_empty());
        // A live picker's list says what to do until there is a query.
        let live_hint = st.live.as_ref().filter(|_| st.query.is_empty()).map(|l| l.hint.clone());

        let mut query = st.query.clone();
        let mut clicked = None;
        let response = egui::Area::new(Id::unique("eshvi-picker"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .show(ctx, |ui| {
                theme.frame().show(ui, |ui| {
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let mut left = |ui: &mut egui::Ui| {
                        // The field, with how many rows match at its right.
                        let pad = style::GAP;
                        let margin = Margin::symmetric(pad as i8, (pad / 2.0) as i8);
                        let (_, field) = style::text_field(ui, look.chrome, field_id(), margin, |ui| {
                            let v = ui.visuals_mut();
                            v.override_text_color = Some(theme.fg);
                            v.selection.stroke = egui::Stroke::new(style::HAIRLINE, theme.accent);
                            let room = ui.available_width() - count_w - pad;
                            // (The count's room is the field's, and kept empty.)
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    Vec2::new(room, ctx.fonts_mut(|f| f.row_height(&font))),
                                    egui::TextEdit::singleline(&mut query)
                                        .id(field_id())
                                        .hint_text(RichText::new(&st.title).font(font.clone()).color(theme.dim_fg))
                                        .font(font.clone())
                                        .frame(egui::Frame::NONE),
                                );
                                ui.add_space(count_w + pad);
                            })
                        });
                        ui.painter().text(
                            Pos2::new(field.max.x - pad - 1.0, field.center().y),
                            Align2::RIGHT_CENTER,
                            &counts,
                            small.clone(),
                            theme.dim_fg,
                        );
                        ui.add_space(pad / 2.0);

                        // The list: always as tall as it has been, so the box stays put.
                        if count == 0 {
                            let (rect, _) = ui.allocate_exact_size(Vec2::new(list_w, list_h), Sense::hover());
                            let text = match grep_hint {
                                Some(true) => Some("Type to search"),
                                Some(false) if searching => None,
                                _ if live_hint.is_some() => live_hint.as_deref().filter(|h| !h.is_empty()).or(Some("Type to search")),
                                _ if asking => None,
                                _ if walking && st.items.rows.is_empty() => None,
                                _ => Some("No results"),
                            };
                            if let Some(text) = text {
                                ui.painter().text(
                                    Pos2::new(rect.min.x + pad, rect.min.y + row_h / 2.0),
                                    Align2::LEFT_CENTER,
                                    text,
                                    font.clone(),
                                    theme.dim_fg,
                                );
                            }
                            return;
                        }
                        crate::popup::thin_scrollbars(ui, theme);
                        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).min_scrolled_height(list_h);
                        if let Some(row) = st.scroll_to.take() {
                            let (top, bottom) = (row as f32 * row_h, (row + 1) as f32 * row_h);
                            if top < st.offset {
                                scroll = scroll.vertical_scroll_offset(top);
                            } else if bottom > st.offset + list_h {
                                scroll = scroll.vertical_scroll_offset(bottom - list_h);
                            }
                        }
                        #[cfg(test)]
                        st.row_rects.clear();
                        let State {
                            items,
                            shown,
                            selected,
                            marked,
                            highlights,
                            matchers,
                            pattern,
                            files,
                            multi,
                            live: live_run,
                            query: typed,
                            #[cfg(test)]
                            row_rects,
                            ..
                        } = &mut *st;
                        let live = live_run.is_some();
                        let out = ui.allocate_ui(Vec2::new(list_w, list_h), |ui| scroll.show_rows(ui, row_h, count, |ui, range| {
                            for row in range {
                                let item = match shown {
                                    Shown::All => row,
                                    Shown::Matches(m) => m[row].index,
                                };
                                let (rect, resp) = ui.allocate_exact_size(Vec2::new(list_w, row_h), Sense::click());
                                #[cfg(test)]
                                row_rects.push((row, rect));
                                if resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if resp.clicked() {
                                    clicked = Some(row);
                                }
                                let matched: &[u32] = match shown {
                                    Shown::Matches(m) if !m[row].indices.is_empty() => &m[row].indices,
                                    Shown::Matches(_) if !typed.trim().is_empty() => {
                                        let text = items.rows[item].search_text();
                                        let matcher = if *files { &mut matchers.0 } else { &mut matchers.1 };
                                        highlights.entry(item).or_insert_with(|| rank::highlight(pattern, matcher, text))
                                    }
                                    // (A live picker's rows aren't ranked here, but the query's
                                    // letters are still shown in them.)
                                    Shown::All if live && !typed.trim().is_empty() => {
                                        let text = items.rows[item].search_text();
                                        highlights.entry(item).or_insert_with(|| rank::highlight(pattern, &mut matchers.1, text))
                                    }
                                    _ => &[],
                                };
                                let paint = RowPaint {
                                    row: &items.rows[item],
                                    matched,
                                    selected: row == *selected,
                                    marked: marked.contains(&item),
                                    files: *files,
                                    multi: *multi,
                                    number_w,
                                };
                                paint_row(ui, rect, &paint, look, &font, &small, icons);
                            }
                        })).inner;
                        st.offset = out.state.clamped_offset().y;
                    };
                    let plook = PreviewLook { theme, code: look.code, fonts: look.fonts, metrics: m };
                    match (lay.preview_width, lay.preview_height, pane.as_mut()) {
                        (Some(preview_w), Some(preview_h), Some(pane)) => {
                            left(ui);
                            ui.add_space(style::GAP);
                            pane.show(ui, Vec2::new(preview_w, preview_h), &plook);
                        }
                        (Some(preview_w), None, Some(pane)) => {
                            ui.horizontal_top(|ui| {
                                let column = ui.vertical(|ui| {
                                    ui.set_width(list_w);
                                    left(ui);
                                });
                                let left_h = column.response.rect.height();
                                ui.add_space(style::GAP);
                                pane.show(ui, Vec2::new(preview_w, left_h), &plook);
                            });
                        }
                        _ => left(ui),
                    }
                });
            });
        self.rect = Some(response.response.rect);
        if std::mem::take(&mut self.fresh) {
            ctx.move_to_top(response.response.layer_id);
        }

        let st = self.state.as_mut().expect("still open");
        st.preview = pane;
        st.set_query(&query);
        // (After the rows and the query are this frame's: an Enter that came early, a
        // resumed picker's row, the selection a live colorscheme follows.)
        st.settle_accept(&mut actions);
        st.apply_want();
        st.select_tick(ctx, &mut actions);
        if std::mem::take(&mut st.announce) {
            actions.push(Action::Opened(st.id));
        }
        if let Some(row) = clicked {
            st.selected = row;
            st.moved = true;
            // (A click opens the row; marks wait for Enter.)
            if let Some(item) = st.item_at(row) {
                actions.push(Action::Accept { id: st.id, chosen: vec![st.choice(item)], how: "edit" });
            }
        }
        actions
    }

    /// Neovim's rows for a live picker's query `gen`. Whether they were taken.
    pub fn set_items(&mut self, id: i64, gen: i64, items: Vec<nvim_bridge::gui::PickerItem>) -> bool {
        self.state.as_mut().filter(|s| s.id == id).is_some_and(|s| s.set_live_items(gen, items))
    }

    /// Neovim's answer to a preview request. Whether it was shown.
    pub fn set_preview(&mut self, answer: nvim_bridge::gui::PickerPreview) -> bool {
        self.state.as_mut().and_then(|s| s.preview.as_mut()).is_some_and(|p| p.answer(answer))
    }
}

struct RowPaint<'a> {
    row: &'a Row,
    /// Positions in `row.text` that matched.
    matched: &'a [u32],
    selected: bool,
    marked: bool,
    files: bool,
    multi: bool,
    /// The width of a line number before the text (the buffer's lines).
    number_w: Option<f32>,
}

/// `text` in `color`, the characters at `matched` (positions in the
/// original, counted from `base`) in `accent`.
fn highlighted(text: &str, matched: &[u32], base: usize, font: &FontId, color: Color32, accent: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut run = String::new();
    let mut lit = false;
    let flush = |run: &mut String, lit: bool, job: &mut LayoutJob| {
        if !run.is_empty() {
            job.append(run, 0.0, egui::TextFormat { font_id: font.clone(), color: if lit { accent } else { color }, ..Default::default() });
            run.clear();
        }
    };
    for (i, c) in text.chars().enumerate() {
        let is = matched.binary_search(&((base + i) as u32)).is_ok();
        if is != lit {
            flush(&mut run, lit, &mut job);
            lit = is;
        }
        run.push(c);
    }
    flush(&mut run, lit, &mut job);
    job
}

/// How far a symbol is moved in for each level it is down, in cells.
const INDENT: f32 = 2.0;

/// The completion menu's badge for a symbol's kind: its kinds, and the others
/// LSP has that it hasn't, as the kinds nearest them.
pub(crate) fn symbol_badge(kind: &str) -> (&'static str, Color32) {
    let kind = match kind {
        "Namespace" | "Package" => "Module",
        "String" | "Number" | "Boolean" | "Array" | "Object" | "Key" | "Null" => "Value",
        // A Markdown heading in the Outline: its level.
        "H1" | "H2" | "H3" | "H4" | "H5" | "H6" => {
            let label = ["H1", "H2", "H3", "H4", "H5", "H6"][kind.as_bytes()[1] as usize - b'1' as usize];
            return (label, crate::popupmenu::kind_badge("Value").map_or(Color32::GRAY, |(_, c)| c));
        }
        other => other,
    };
    crate::popupmenu::kind_badge(kind).unwrap_or(("?", Color32::GRAY))
}

fn paint_row(ui: &mut egui::Ui, rect: Rect, p: &RowPaint, look: &Look, font: &FontId, small: &FontId, icons: &mut IconTextures) {
    let theme = look.theme;
    let painter = ui.painter().clone();
    // (The one highlight: the mouse over another row doesn't get a second. Its hand says a click opens it.)
    if p.selected {
        painter.rect_filled(rect, style::RADIUS, theme.sel_bg);
    }
    let (fg, dim) = if p.selected { (theme.sel_fg, mix(theme.sel_fg, theme.sel_bg, style::DIM)) } else { (theme.fg, theme.dim_fg) };
    let (pad, gap) = (style::GAP, style::GAP);
    let cy = rect.center().y;
    let mut x = rect.min.x + pad;
    if p.multi {
        let side = style::icon_size(rect.height());
        if p.marked {
            style::paint_icon(&painter, style::Icon::Check, Pos2::new(x + side / 2.0, cy), side, look.theme.accent);
        }
        x += side + gap;
    }
    let draw = |x: f32, job: LayoutJob| -> f32 {
        let galley = painter.layout_job(job);
        let w = galley.size().x;
        painter.galley(Pos2::new(x, cy - galley.size().y / 2.0), galley, fg);
        w
    };
    let mut right = rect.max.x - pad;
    let row = p.row;
    let width_of = |text: &str, f: &FontId| painter.layout_no_wrap(text.to_owned(), f.clone(), fg).size().x;
    if !row.hint.is_empty() {
        // A shortcut, dim, at the right: the rest gives way to it.
        let galley = painter.layout_no_wrap(row.hint.clone(), small.clone(), dim);
        painter.galley(Pos2::new(right - galley.size().x, cy - galley.size().y / 2.0), galley.clone(), dim);
        right -= galley.size().x + gap;
    }
    if let Some(g) = &row.grep {
        // The file as a file row has it (icon, name and the line, the folder dim),
        // in a first part of fixed proportion; then the line, in the grid font.
        let (dir, name) = split_path(&row.text);
        let side = style::file_icon_size(font);
        let first_end = x + (right - x) * GREP_SPLIT;
        let icon = Rect::from_center_size(Pos2::new(x + side / 2.0, cy), Vec2::splat(side));
        let px = (side * ui.ctx().pixels_per_point()).round() as u32;
        if let Some(tex) = icons.get(ui.ctx(), IconTheme::get().file_icon(name), px) {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(tex.id(), icon, uv, Color32::WHITE);
        }
        x += side + gap;
        // The name gives way, not the line number.
        let line_part = format!(":{}", row.lnum.unwrap_or(0));
        let (shown, kept) = truncate_to_width(name, first_end - x - gap - width_of(&line_part, font), &|t| width_of(t, font));
        let name_line = if kept == name.chars().count() { format!("{name}{line_part}") } else { format!("{}{line_part}", shown) };
        x += draw(x, highlighted(&name_line, &[], 0, font, fg, theme.accent)) + gap;
        let (shown, kept) = truncate_to_width(dir, first_end - x - gap, &|t| width_of(t, small));
        if !shown.is_empty() {
            let text: String = shown.chars().take(kept).collect();
            let mut job = highlighted(&text, &[], 0, small, dim, theme.accent);
            if kept < dir.chars().count() {
                job.append("…", 0.0, egui::TextFormat { font_id: small.clone(), color: dim, ..Default::default() });
            }
            draw(x, job);
        }
        // The line in its language's colors (as the preview's code), the
        // matches marked as there.
        let room = right - (first_end + gap);
        let chars = (room / look.metrics.width).floor().max(0.0) as usize;
        let cut = grep::Cut::of(&g.text, &g.ranges, chars, |_| 1);
        let colors = look.syntax.map_or(&[][..], |theme| g.colors.get_or_init(|| snippet_colors(&g.text, &row.text, theme, look.code.fg)).as_slice());
        let (ranges, spans): (Vec<_>, Vec<_>) = colors.iter().map(|(r, s)| (r.clone(), *s)).unzip();
        let spans: Vec<_> = cut.moved(&ranges).into_iter().zip(spans).collect();
        let code = CodeTheme { fg, ..*look.code };
        let plook = PreviewLook { theme, code: &code, fonts: look.fonts, metrics: look.metrics };
        draw(first_end + gap, preview::line_job(&cut.text(&g.text), &spans, &cut.moved(&g.ranges), &plook));
        return;
    }
    if let (Some(w), Some(n)) = (p.number_w, row.lnum) {
        // (The buffer's lines: their numbers, dim, right-aligned.)
        let galley = painter.layout_no_wrap(n.to_string(), small.clone(), dim);
        painter.galley(Pos2::new(x + w - galley.size().x, cy - galley.size().y / 2.0), galley, dim);
        x += w + gap;
    }
    if p.files {
        let (dir, name) = split_path(&row.text);
        let side = style::file_icon_size(font);
        let icon = Rect::from_center_size(Pos2::new(x + side / 2.0, cy), Vec2::splat(side));
        let px = (side * ui.ctx().pixels_per_point()).round() as u32;
        if let Some(tex) = icons.get(ui.ctx(), IconTheme::get().file_icon(name), px) {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(tex.id(), icon, uv, Color32::WHITE);
        }
        x += side + gap;
        let dir_chars = if dir.is_empty() { 0 } else { dir.chars().count() + 1 };
        let name = highlighted(name, p.matched, dir_chars, font, fg, theme.accent);
        x += draw(x, name) + gap;
        let (shown, kept) = truncate_to_width(dir, right - x, &|t| width_of(t, small));
        if !shown.is_empty() {
            let cut = kept < dir.chars().count();
            let text: String = shown.chars().take(kept).collect();
            let mut job = highlighted(&text, p.matched, 0, small, dim, theme.accent);
            if cut {
                job.append("…", 0.0, egui::TextFormat { font_id: small.clone(), color: dim, ..Default::default() });
            }
            draw(x, job);
        }
    } else {
        if let Some(severity) = row.severity {
            // The Problems panel's and the status bar's icons and colors (a hint is
            // counted with the infos there).
            let (icon, color) = match severity {
                1 => ("error", look.problems[0]),
                2 => ("warning", look.problems[1]),
                3 => ("info", look.problems[2]),
                _ => ("info", dim),
            };
            let side = style::file_icon_size(font);
            icons.paint_codicon(&painter, icon, Pos2::new(x + side / 2.0, cy), side, color);
            x += side + gap;
        }
        if let Some(kind) = &row.kind {
            // The completion menu's badge for the kind, then the name a little in
            // for each level it is down.
            let (label, color) = symbol_badge(kind);
            let m = look.metrics;
            let badge = Rect::from_center_size(Pos2::new(x + m.width * 1.5, cy), Vec2::new(m.width * 3.0, m.height - 2.0));
            crate::popupmenu::paint_badge(&painter, look.theme, badge, label, color, p.selected, small);
            x += m.width * 3.0 + gap + row.depth as f32 * m.width * INDENT;
        }
        if !row.prefix.is_empty() {
            // A keymap's mode: dim, in a column of its own so the keys line up. A
            // longer prefix (a commit's hash) is set in the grid's font, so the
            // rows' texts start alike.
            let font_id = if row.prefix.chars().count() > 1 { look.fonts.font_id(false, false).0 } else { small.clone() };
            let column = width_of("nxo", small).max(width_of(&row.prefix, &font_id));
            draw(x, highlighted(&row.prefix, &[], 0, &font_id, dim, theme.accent));
            x += column + gap;
        }
        let (shown, kept) = truncate_to_width(&row.text, right - x, &|t| width_of(t, font));
        let cut = kept < row.text.chars().count();
        let text: String = shown.chars().take(kept).collect();
        let mut job = highlighted(&text, p.matched, 0, font, fg, theme.accent);
        if cut {
            job.append("…", 0.0, egui::TextFormat { font_id: font.clone(), color: fg, ..Default::default() });
        }
        x += draw(x, job) + gap;
        if !row.desc.is_empty() {
            let desc_font = if row.mono { look.fonts.font_id(false, false).0 } else { small.clone() };
            let (shown, _) = truncate_to_width(&row.desc, right - x, &|t| width_of(t, &desc_font));
            if !shown.is_empty() {
                draw(x, highlighted(&shown, &[], 0, &desc_font, dim, theme.accent));
            }
        }
    }
}
