//! What can be done to a terminal's grid without a process behind it: what
//! the screen holds, selecting, searching and the Vi mode. Functions over
//! alacritty's `Term`, so they are tested by feeding it output directly.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{point_to_viewport, Term, TermMode};
use alacritty_terminal::vi_mode::ViMotion;
use alacritty_terminal::vte::ansi::{ClearMode, CursorShape, Handler};

/// A run of cells in one row of the screen, both ends included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub row: usize,
    pub c0: usize,
    pub c1: usize,
}

/// The terminal's cursor on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorDraw {
    pub row: usize,
    pub col: usize,
    /// On a wide character: two cells.
    pub wide: bool,
    pub shape: CursorShape,
    pub blinking: bool,
}

/// The screen as drawn: a copy taken under the lock, so painting doesn't hold it.
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    /// Row by row.
    pub cells: Vec<Cell>,
    /// `None` while scrolled away from it, or hidden.
    pub cursor: Option<CursorDraw>,
    /// The Vi mode's cursor (in Vi mode), if on the screen.
    pub vi_cursor: Option<CursorDraw>,
    /// How many lines the view is scrolled back, and how many there are.
    pub display_offset: usize,
    pub history: usize,
    pub mode: TermMode,
    pub selection: Vec<Span>,
    /// The search's matches on the screen, and the current one.
    pub matches: Vec<Span>,
    pub current_match: Vec<Span>,
    /// The colors the program set itself (OSC 4, 10, 11).
    pub colors: Colors,
}

impl Snapshot {
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        (row < self.rows && col < self.cols).then(|| &self.cells[row * self.cols + col])
    }

    pub fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }
}

/// The columns and lines of a terminal.
#[derive(Clone, Copy, Debug)]
pub struct Dims {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// The spans of the screen that a range of the grid covers.
fn spans(start: Point, end: Point, block: bool, offset: usize, rows: usize, cols: usize) -> Vec<Span> {
    let mut out = Vec::new();
    for row in 0..rows {
        let line = row as i32 - offset as i32;
        if line < start.line.0 || line > end.line.0 {
            continue;
        }
        let (c0, c1) = if block {
            (start.column.0.min(end.column.0), start.column.0.max(end.column.0))
        } else {
            (
                if line == start.line.0 { start.column.0 } else { 0 },
                if line == end.line.0 { end.column.0 } else { cols - 1 },
            )
        };
        out.push(Span { row, c0, c1: c1.min(cols - 1) });
    }
    out
}

/// The grid point under a cell of the screen.
pub fn grid_point<L>(term: &Term<L>, row: usize, col: usize) -> Point {
    let offset = term.grid().display_offset() as i32;
    let col = col.min(term.columns() - 1);
    Point::new(Line((row as i32 - offset).clamp(-(term.history_size() as i32), term.screen_lines() as i32 - 1)), Column(col))
}

pub fn snapshot<L: EventListener>(term: &Term<L>, search: &mut Search) -> Snapshot {
    let (cols, rows) = (term.columns(), term.screen_lines());
    let content = term.renderable_content();
    let offset = content.display_offset;
    let mut cells = vec![Cell::default(); cols * rows];
    for indexed in content.display_iter {
        let row = indexed.point.line.0 + offset as i32;
        if row >= 0 && (row as usize) < rows && indexed.point.column.0 < cols {
            cells[row as usize * cols + indexed.point.column.0] = indexed.cell.clone();
        }
    }
    let vi = term.mode().contains(TermMode::VI);
    let draw = |point: Point, shape: CursorShape, blinking: bool| {
        let p = point_to_viewport(offset, point).filter(|p| p.line < rows)?;
        let wide = cells[p.line * cols + *p.column].flags.contains(Flags::WIDE_CHAR);
        Some(CursorDraw { row: p.line, col: *p.column, wide, shape, blinking })
    };
    let style = term.cursor_style();
    let shape = if term.mode().contains(TermMode::SHOW_CURSOR) { style.shape } else { CursorShape::Hidden };
    // (The cursor sits on a wide character's first cell.)
    let mut cursor_point = term.grid().cursor.point;
    if term.grid()[cursor_point].flags.contains(Flags::WIDE_CHAR_SPACER) {
        cursor_point.column -= 1;
    }
    let cursor = draw(cursor_point, shape, style.blinking).filter(|_| shape != CursorShape::Hidden);
    let vi_cursor = vi.then(|| draw(term.vi_mode_cursor.point, CursorShape::Block, false)).flatten();
    let selection = term
        .selection
        .as_ref()
        .and_then(|s| s.to_range(term))
        .map(|r| spans(r.start, r.end, r.is_block, offset, rows, cols))
        .unwrap_or_default();
    let (matches, current_match) = search.visible(term, offset);
    Snapshot {
        cols,
        rows,
        cells,
        cursor,
        vi_cursor,
        display_offset: offset,
        history: term.history_size(),
        mode: content.mode,
        selection,
        matches,
        current_match,
        colors: *content.colors,
    }
}

// Selecting.

/// How a selection starts: a click's count, and Option for a block.
pub fn select_begin<L>(term: &mut Term<L>, ty: SelectionType, row: usize, col: usize, side: Side) {
    let point = grid_point(term, row, col);
    term.selection = Some(Selection::new(ty, point, side));
}

/// The selection's end moves to a cell (a drag, or Shift-click). Starts a
/// simple one where there is none.
pub fn select_update<L>(term: &mut Term<L>, row: usize, col: usize, side: Side) {
    let point = grid_point(term, row, col);
    match term.selection.as_mut() {
        Some(s) => s.update(point, side),
        None => {
            let mut s = Selection::new(SelectionType::Simple, point, side);
            s.update(point, side);
            term.selection = Some(s);
        }
    }
}

/// Everything, the scrollback too, down to the last line with text (or the
/// cursor's).
pub fn select_all<L>(term: &mut Term<L>) {
    let grid = term.grid();
    let blank = |line: Line| grid[line].into_iter().all(|c| c.c == ' ' && c.extra.is_none());
    let mut last = term.bottommost_line();
    while last > grid.cursor.point.line && last > term.topmost_line() && blank(last) {
        last -= 1;
    }
    let mut s = Selection::new(SelectionType::Simple, Point::new(term.topmost_line(), Column(0)), Side::Left);
    s.update(Point::new(last, term.last_column()), Side::Right);
    term.selection = Some(s);
}

/// The selected text, if any is selected.
pub fn selection_text<L>(term: &Term<L>) -> Option<String> {
    term.selection.as_ref().filter(|s| !s.is_empty())?;
    term.selection_to_string().filter(|t| !t.is_empty())
}

/// Clear the screen and the scrollback (⌘K). On the primary screen the shell is
/// left to redraw its prompt (it is sent ^L).
pub fn clear_history<L: EventListener>(term: &mut Term<L>) {
    term.selection = None;
    term.clear_screen(ClearMode::Saved);
    term.scroll_display(Scroll::Bottom);
}

// Searching.

/// A search in a terminal: the pattern, its regex and the current match.
#[derive(Default)]
pub struct Search {
    pub pattern: String,
    regex: Option<RegexSearch>,
    pub current: Option<Match>,
}

/// Most matches kept for the screen.
const MAX_VISIBLE_MATCHES: usize = 2000;

impl Search {
    pub fn is_active(&self) -> bool {
        self.regex.is_some()
    }

    /// Whether the pattern is a valid regex (an empty one is nothing to find).
    pub fn is_valid(&self) -> bool {
        self.pattern.is_empty() || self.regex.is_some()
    }

    pub fn clear(&mut self) {
        *self = Search::default();
    }

    /// Search for a pattern (a regex, case-insensitive unless it has an
    /// uppercase letter) from the top of the screen, and bring the first
    /// match into view. `false` if the pattern isn't a regex.
    pub fn set<L: EventListener>(&mut self, term: &mut Term<L>, pattern: &str) -> bool {
        self.pattern = pattern.to_owned();
        self.current = None;
        self.regex = None;
        if pattern.is_empty() {
            return true;
        }
        match RegexSearch::new(pattern) {
            Ok(regex) => self.regex = Some(regex),
            Err(_) => return false,
        }
        let top = Point::new(Line(-(term.grid().display_offset() as i32)), Column(0));
        self.go(term, top, Direction::Right, Side::Left);
        true
    }

    /// The next (or previous) match from the current one, wrapping around;
    /// scrolls it into view.
    pub fn step<L: EventListener>(&mut self, term: &mut Term<L>, forward: bool) -> Option<Match> {
        let (origin, direction, side) = match (&self.current, forward) {
            (Some(m), true) => (m.end().add(term, Boundary::None, 1), Direction::Right, Side::Left),
            (Some(m), false) => (m.start().sub(term, Boundary::None, 1), Direction::Left, Side::Right),
            (None, true) => (Point::new(Line(-(term.grid().display_offset() as i32)), Column(0)), Direction::Right, Side::Left),
            (None, false) => (
                Point::new(Line(-(term.grid().display_offset() as i32) + term.screen_lines() as i32 - 1), term.last_column()),
                Direction::Left,
                Side::Right,
            ),
        };
        self.go(term, origin, direction, side)
    }

    fn go<L: EventListener>(&mut self, term: &mut Term<L>, origin: Point, direction: Direction, side: Side) -> Option<Match> {
        let regex = self.regex.as_mut()?;
        let found = term.search_next(regex, origin, direction, side, None);
        if let Some(m) = &found {
            term.scroll_to_point(*m.start());
        }
        self.current = found.clone();
        found
    }

    /// The matches on the screen, and the current one.
    fn visible<L: EventListener>(&mut self, term: &Term<L>, offset: usize) -> (Vec<Span>, Vec<Span>) {
        let (rows, cols) = (term.screen_lines(), term.columns());
        let Some(regex) = self.regex.as_mut() else { return (Vec::new(), Vec::new()) };
        let start = Point::new(Line(-(offset as i32)), Column(0));
        let end = Point::new(Line(rows as i32 - 1 - offset as i32), Column(cols - 1));
        let mut all = Vec::new();
        for m in RegexIter::new(start, end, Direction::Right, term, regex).take(MAX_VISIBLE_MATCHES) {
            all.extend(spans(*m.start(), *m.end(), false, offset, rows, cols));
        }
        let current = self.current.as_ref().map(|m| spans(*m.start(), *m.end(), false, offset, rows, cols)).unwrap_or_default();
        (all, current)
    }
}

// Vi mode.

/// A key in Vi mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViKey {
    Char(char),
    /// Ctrl with a letter.
    Ctrl(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Escape,
}

/// What a Vi key did, for the UI to finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViOutcome {
    /// Nothing more to do (a motion, a selection started or ended).
    Handled,
    /// Vi mode ended.
    Left,
    /// `/` or `?`: the find bar, searching this way.
    Search(bool),
    /// `n` or `N`: the next match this way (forward is `true`).
    Step(bool),
    /// `y`: copy this text.
    Copy(String),
    /// Not a Vi key.
    Ignored,
}

/// The Vi mode's state between keys: a `g` waiting for the second.
#[derive(Default)]
pub struct ViState {
    pending_g: bool,
}

pub fn toggle_vi<L: EventListener>(term: &mut Term<L>) {
    term.toggle_vi_mode();
    term.selection = None;
}

/// Move the Vi cursor to a point and show it.
pub fn vi_goto<L: EventListener>(term: &mut Term<L>, point: Point) {
    term.vi_goto_point(point);
}

fn toggle_selection<L: EventListener>(term: &mut Term<L>, ty: SelectionType) {
    match term.selection.as_mut() {
        Some(s) if s.ty == ty && !s.is_empty() => term.selection = None,
        Some(s) if !s.is_empty() => s.ty = ty,
        _ => term.selection = Some(Selection::new(ty, term.vi_mode_cursor.point, Side::Left)),
    }
    if let Some(s) = term.selection.as_mut() {
        s.include_all();
    }
}

fn scroll_by<L: EventListener>(term: &mut Term<L>, lines: i32) {
    term.vi_mode_cursor = term.vi_mode_cursor.scroll(term, lines);
    term.scroll_display(Scroll::Delta(lines));
}

/// One key in Vi mode: hjkl and the arrows, w/b/e (and W/B/E), 0 ^ $, H/M/L,
/// gg and G, ⌃u/⌃d/⌃b/⌃f, v/V/⌃v selecting, `y` copying, `/` `?` `n` `N`
/// searching; Esc or `i` leaves.
pub fn vi_key<L: EventListener>(term: &mut Term<L>, state: &mut ViState, key: ViKey) -> ViOutcome {
    if !term.mode().contains(TermMode::VI) {
        return ViOutcome::Ignored;
    }
    let was_g = std::mem::take(&mut state.pending_g);
    let motion = |term: &mut Term<L>, m: ViMotion| {
        term.vi_motion(m);
        ViOutcome::Handled
    };
    let half = (term.screen_lines() / 2).max(1) as i32;
    let page = term.screen_lines().max(1) as i32;
    match key {
        ViKey::Escape | ViKey::Char('i') => {
            toggle_vi(term);
            ViOutcome::Left
        }
        ViKey::Char('h') | ViKey::Left => motion(term, ViMotion::Left),
        ViKey::Char('j') | ViKey::Down => motion(term, ViMotion::Down),
        ViKey::Char('k') | ViKey::Up => motion(term, ViMotion::Up),
        ViKey::Char('l') | ViKey::Right => motion(term, ViMotion::Right),
        ViKey::Char('w') => motion(term, ViMotion::SemanticRight),
        ViKey::Char('b') => motion(term, ViMotion::SemanticLeft),
        ViKey::Char('e') => motion(term, ViMotion::SemanticRightEnd),
        ViKey::Char('W') => motion(term, ViMotion::WordRight),
        ViKey::Char('B') => motion(term, ViMotion::WordLeft),
        ViKey::Char('E') => motion(term, ViMotion::WordRightEnd),
        ViKey::Char('0') | ViKey::Home => motion(term, ViMotion::First),
        ViKey::Char('$') | ViKey::End => motion(term, ViMotion::Last),
        ViKey::Char('^') => motion(term, ViMotion::FirstOccupied),
        ViKey::Char('H') => motion(term, ViMotion::High),
        ViKey::Char('M') => motion(term, ViMotion::Middle),
        ViKey::Char('L') => motion(term, ViMotion::Low),
        ViKey::Char('%') => motion(term, ViMotion::Bracket),
        ViKey::Char('{') => motion(term, ViMotion::ParagraphUp),
        ViKey::Char('}') => motion(term, ViMotion::ParagraphDown),
        ViKey::Char('g') if was_g => {
            term.scroll_display(Scroll::Top);
            let top = Point::new(term.topmost_line(), Column(0));
            vi_goto(term, top);
            ViOutcome::Handled
        }
        ViKey::Char('g') => {
            state.pending_g = true;
            ViOutcome::Handled
        }
        ViKey::Char('G') => {
            term.scroll_display(Scroll::Bottom);
            let bottom = Point::new(term.bottommost_line(), Column(0));
            vi_goto(term, bottom);
            ViOutcome::Handled
        }
        ViKey::Ctrl('u') => {
            scroll_by(term, half);
            ViOutcome::Handled
        }
        ViKey::Ctrl('d') => {
            scroll_by(term, -half);
            ViOutcome::Handled
        }
        ViKey::Ctrl('b') | ViKey::PageUp => {
            scroll_by(term, page);
            ViOutcome::Handled
        }
        ViKey::Ctrl('f') | ViKey::PageDown => {
            scroll_by(term, -page);
            ViOutcome::Handled
        }
        ViKey::Char('v') => {
            toggle_selection(term, SelectionType::Simple);
            ViOutcome::Handled
        }
        ViKey::Char('V') => {
            toggle_selection(term, SelectionType::Lines);
            ViOutcome::Handled
        }
        ViKey::Ctrl('v') => {
            toggle_selection(term, SelectionType::Block);
            ViOutcome::Handled
        }
        ViKey::Char('y') => {
            let text = selection_text(term);
            term.selection = None;
            text.map_or(ViOutcome::Handled, ViOutcome::Copy)
        }
        ViKey::Char('/') => ViOutcome::Search(true),
        ViKey::Char('?') => ViOutcome::Search(false),
        ViKey::Char('n') => ViOutcome::Step(true),
        ViKey::Char('N') => ViOutcome::Step(false),
        // The key is the mode's: nothing else goes to the program.
        _ => ViOutcome::Handled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    fn term(cols: usize, rows: usize, history: usize) -> Term<VoidListener> {
        Term::new(Config { scrolling_history: history, ..Config::default() }, &Dims { cols, rows }, VoidListener)
    }

    fn feed(term: &mut Term<VoidListener>, text: &str) {
        let mut parser: Processor = Processor::new();
        parser.advance(term, text.as_bytes());
    }

    fn select(term: &mut Term<VoidListener>, ty: SelectionType, from: (usize, usize), to: (usize, usize)) {
        select_begin(term, ty, from.0, from.1, Side::Left);
        select_update(term, to.0, to.1, Side::Right);
    }

    fn row_text(snap: &Snapshot, row: usize) -> String {
        snap.row(row)
            .iter()
            .filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER))
            .map(|c| c.c)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn the_snapshot_holds_the_screen_and_the_cursor() {
        let mut t = term(10, 3, 100);
        feed(&mut t, "hi\r\nthere");
        let snap = snapshot(&t, &mut Search::default());
        assert_eq!((snap.cols, snap.rows), (10, 3));
        assert_eq!((row_text(&snap, 0), row_text(&snap, 1), row_text(&snap, 2)), ("hi".into(), "there".into(), String::new()));
        let c = snap.cursor.unwrap();
        assert_eq!((c.row, c.col, c.shape), (1, 5, CursorShape::Block));
        // The cursor is hidden by ESC [ ? 25 l.
        feed(&mut t, "\x1b[?25l");
        assert!(snapshot(&t, &mut Search::default()).cursor.is_none());
    }

    #[test]
    fn the_resize_reflows_wrapped_lines() {
        let mut t = term(20, 4, 100);
        feed(&mut t, "0123456789abcdefghij\r\nend");
        t.resize(Dims { cols: 10, rows: 4 });
        // (The rows below the text stay, so the first row goes into the scrollback.)
        t.scroll_display(Scroll::Top);
        let snap = snapshot(&t, &mut Search::default());
        let rows: Vec<String> = (0..snap.rows).map(|r| row_text(&snap, r)).collect();
        assert_eq!(&rows[..3], ["0123456789", "abcdefghij", "end"]);
        // And back: the two halves are one line again.
        t.resize(Dims { cols: 20, rows: 4 });
        select_all(&mut t);
        assert_eq!(selection_text(&t).as_deref(), Some("0123456789abcdefghij\nend"));
    }

    #[test]
    fn selecting_text_across_wrapped_lines_and_wide_characters() {
        let mut t = term(10, 4, 100);
        // A line that wraps (no newline in the text), then a wide-character line.
        feed(&mut t, "0123456789abcde\r\n日本語 x");
        select(&mut t, SelectionType::Simple, (0, 2), (1, 2));
        assert_eq!(selection_text(&t).as_deref(), Some("23456789abc"), "a wrapped line is one line");
        select(&mut t, SelectionType::Simple, (2, 0), (2, 6));
        assert_eq!(selection_text(&t).as_deref(), Some("日本語 "), "wide characters once, not their spacers");
        // Ending on the second half of a wide character takes the whole of it.
        select(&mut t, SelectionType::Simple, (2, 0), (2, 3));
        assert_eq!(selection_text(&t).as_deref(), Some("日本"));
        select(&mut t, SelectionType::Simple, (2, 1), (2, 2));
        assert_eq!(selection_text(&t).as_deref(), Some("日本"), "from the second half of one to the first of the next");
        // Across lines: a line break where the line ends.
        select(&mut t, SelectionType::Simple, (1, 3), (2, 5));
        assert_eq!(selection_text(&t).as_deref(), Some("de\n日本語"));
    }

    #[test]
    fn double_click_selects_a_word_and_triple_click_a_line() {
        let mut t = term(30, 3, 0);
        feed(&mut t, "echo hello_world.txt and more");
        select_begin(&mut t, SelectionType::Semantic, 0, 7, Side::Left);
        assert_eq!(selection_text(&t).as_deref(), Some("hello_world.txt"));
        select_begin(&mut t, SelectionType::Lines, 0, 7, Side::Left);
        assert_eq!(selection_text(&t).as_deref(), Some("echo hello_world.txt and more\n"));
    }

    #[test]
    fn a_block_selection_takes_a_rectangle() {
        let mut t = term(10, 3, 0);
        feed(&mut t, "abcdef\r\nghijkl\r\nmnopqr");
        select(&mut t, SelectionType::Block, (0, 1), (2, 3));
        assert_eq!(selection_text(&t).as_deref(), Some("bcd\nhij\nnop"));
        let snap = snapshot(&t, &mut Search::default());
        assert_eq!(snap.selection, [Span { row: 0, c0: 1, c1: 3 }, Span { row: 1, c0: 1, c1: 3 }, Span { row: 2, c0: 1, c1: 3 }]);
    }

    #[test]
    fn an_empty_selection_has_no_text_and_select_all_takes_everything() {
        let mut t = term(10, 2, 10);
        feed(&mut t, "one\r\ntwo\r\nthree");
        select_begin(&mut t, SelectionType::Simple, 0, 0, Side::Left);
        assert_eq!(selection_text(&t), None);
        select_all(&mut t);
        assert_eq!(selection_text(&t).as_deref(), Some("one\ntwo\nthree"), "the scrollback too");
    }

    #[test]
    fn a_selection_stays_on_its_text_while_output_scrolls() {
        let mut t = term(10, 3, 100);
        feed(&mut t, "aaa\r\nbbb\r\nccc");
        select(&mut t, SelectionType::Simple, (1, 0), (1, 2));
        feed(&mut t, "\r\nddd\r\neee");
        assert_eq!(selection_text(&t).as_deref(), Some("bbb"));
        let snap = snapshot(&t, &mut Search::default());
        assert!(snap.selection.is_empty(), "scrolled out of the screen");
        t.scroll_display(Scroll::Delta(2));
        let snap = snapshot(&t, &mut Search::default());
        assert_eq!(snap.selection, [Span { row: 1, c0: 0, c1: 2 }]);
    }

    #[test]
    fn searching_finds_wraps_and_marks_matches() {
        let mut t = term(12, 5, 100);
        feed(&mut t, "foo bar\r\nbaz foo\r\nFoo end\r\n");
        let mut search = Search::default();
        assert!(search.set(&mut t, "foo"));
        let snap = snapshot(&t, &mut search);
        // Case-insensitive with a lowercase pattern: all three.
        assert_eq!(snap.matches.len(), 3);
        assert_eq!(snap.current_match, [Span { row: 0, c0: 0, c1: 2 }]);
        let m = search.step(&mut t, true).unwrap();
        assert_eq!((m.start().line.0, m.start().column.0), (1, 4));
        search.step(&mut t, true);
        let m = search.step(&mut t, true).unwrap();
        assert_eq!((m.start().line.0, m.start().column.0), (0, 0), "wraps around");
        let m = search.step(&mut t, false).unwrap();
        assert_eq!((m.start().line.0, m.start().column.0), (2, 0), "and backwards");
        // An uppercase letter makes it case-sensitive.
        assert!(search.set(&mut t, "Foo"));
        assert_eq!(snapshot(&t, &mut search).matches.len(), 1);
        // No such text, a bad regex, and clearing.
        assert!(search.set(&mut t, "zzz"));
        assert!(snapshot(&t, &mut search).matches.is_empty() && search.current.is_none());
        assert!(!search.set(&mut t, "(") && !search.is_valid());
        search.clear();
        assert!(!search.is_active());
    }

    #[test]
    fn searching_scrolls_a_match_in_the_scrollback_into_view() {
        let mut t = term(10, 3, 100);
        feed(&mut t, "needle\r\n1\r\n2\r\n3\r\n4\r\n5");
        assert_eq!(snapshot(&t, &mut Search::default()).display_offset, 0);
        let mut search = Search::default();
        search.set(&mut t, "needle");
        let snap = snapshot(&t, &mut search);
        assert!(snap.display_offset > 0);
        assert_eq!(snap.current_match.len(), 1);
    }

    #[test]
    fn vi_mode_moves_selects_and_copies() {
        let mut t = term(20, 4, 100);
        feed(&mut t, "one two three\r\nsecond line\r\nthird");
        let mut vi = ViState::default();
        assert_eq!(vi_key(&mut t, &mut vi, ViKey::Char('h')), ViOutcome::Ignored, "not in Vi mode");
        toggle_vi(&mut t);
        let snap = snapshot(&t, &mut Search::default());
        assert_eq!(snap.vi_cursor.map(|c| (c.row, c.col)), Some((2, 5)), "starts at the cursor");
        for k in [ViKey::Char('g'), ViKey::Char('g'), ViKey::Char('0')] {
            vi_key(&mut t, &mut vi, k);
        }
        assert_eq!(t.vi_mode_cursor.point, Point::new(Line(0), Column(0)));
        for k in [ViKey::Char('w'), ViKey::Char('v'), ViKey::Char('e'), ViKey::Char('l')] {
            vi_key(&mut t, &mut vi, k);
        }
        let out = vi_key(&mut t, &mut vi, ViKey::Char('y'));
        assert_eq!(out, ViOutcome::Copy("two ".into()));
        assert!(t.selection.is_none());
        // Down, a line selection.
        vi_key(&mut t, &mut vi, ViKey::Char('j'));
        vi_key(&mut t, &mut vi, ViKey::Char('V'));
        assert_eq!(vi_key(&mut t, &mut vi, ViKey::Char('y')), ViOutcome::Copy("second line\n".into()));
        assert_eq!(vi_key(&mut t, &mut vi, ViKey::Char('/')), ViOutcome::Search(true));
        assert_eq!(vi_key(&mut t, &mut vi, ViKey::Char('N')), ViOutcome::Step(false));
        // Esc and `i` leave.
        assert_eq!(vi_key(&mut t, &mut vi, ViKey::Escape), ViOutcome::Left);
        assert!(!t.mode().contains(TermMode::VI));
        toggle_vi(&mut t);
        assert_eq!(vi_key(&mut t, &mut vi, ViKey::Char('i')), ViOutcome::Left);
    }

    #[test]
    fn vi_mode_scrolls_through_the_history() {
        let mut t = term(10, 3, 100);
        for i in 0..20 {
            feed(&mut t, &format!("line{i}\r\n"));
        }
        toggle_vi(&mut t);
        let mut vi = ViState::default();
        vi_key(&mut t, &mut vi, ViKey::Char('g'));
        vi_key(&mut t, &mut vi, ViKey::Char('g'));
        let snap = snapshot(&t, &mut Search::default());
        assert_eq!(snap.display_offset, snap.history, "the top");
        assert_eq!(snap.vi_cursor.map(|c| c.row), Some(0));
        vi_key(&mut t, &mut vi, ViKey::Char('G'));
        assert_eq!(snapshot(&t, &mut Search::default()).display_offset, 0);
        vi_key(&mut t, &mut vi, ViKey::Ctrl('u'));
        assert_eq!(snapshot(&t, &mut Search::default()).display_offset, 1);
    }

    #[test]
    fn clearing_removes_the_scrollback() {
        let mut t = term(10, 3, 100);
        for i in 0..20 {
            feed(&mut t, &format!("line{i}\r\n"));
        }
        assert!(t.history_size() > 0);
        clear_history(&mut t);
        assert_eq!(t.history_size(), 0);
    }
}
