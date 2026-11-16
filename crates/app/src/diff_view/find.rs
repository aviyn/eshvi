//! ⌘F in the view: a find field in the bar, the matches drawn in every file
//! read, Enter and ⇧Enter (⌘G, ⇧⌘G) step through them across files, reading
//! the ones not read yet as the stepping comes to them.

use std::ops::Range;

use super::model::{Kind, Row, SplitRow};
use super::{Cursor, DiffView, Rows};

/// The find field's id: the app lets it keep the keyboard.
pub fn field_id() -> egui::Id {
    egui::Id::unique("eshvi-diff-find")
}

/// A match: its section, the row, which side of the row (split rows have two
/// texts), and which match of that text it is.
pub type Pos = (usize, usize, u8, usize);

/// The find field.
#[derive(Clone, Debug, Default)]
pub struct Find {
    pub text: String,
    /// Take the keyboard when next drawn.
    pub focus: bool,
    /// It had the keyboard when last drawn.
    pub focused: bool,
    /// The match stepped to.
    pub current: Option<Pos>,
    /// A whole round found nothing.
    pub none: bool,
    /// Stepping stopped at a file that wasn't read: which way, and from where.
    waiting: Option<(bool, Option<Pos>)>,
}

/// Where `pattern` is in `line`: case blind unless it has an upper case
/// letter (ASCII; other letters match as they are).
pub fn occurrences(line: &str, pattern: &str) -> Vec<Range<usize>> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let blind = !pattern.chars().any(|c| c.is_uppercase());
    let (hay, needle) = if blind { (line.to_ascii_lowercase(), pattern.to_ascii_lowercase()) } else { (line.to_owned(), pattern.to_owned()) };
    hay.match_indices(&needle).map(|(at, m)| at..at + m.len()).collect()
}

impl Find {
    /// Stepping waits for a file to be read.
    #[cfg(test)]
    pub fn waiting(&self) -> bool {
        self.waiting.is_some()
    }
}

impl DiffView {
    /// ⌘F: the field opens (or takes the keyboard again).
    pub fn open_find(&mut self) {
        match &mut self.find {
            Some(f) => f.focus = true,
            None => self.find = Some(Find { focus: true, ..Find::default() }),
        }
    }

    pub fn close_find(&mut self) {
        self.find = None;
    }

    /// The field has the keyboard.
    pub fn find_focused(&self) -> bool {
        self.find.as_ref().is_some_and(|f| f.focused)
    }

    /// The pattern changed: to the first match from the cursor on.
    pub fn find_changed(&mut self) {
        let Some(f) = &mut self.find else { return };
        f.current = None;
        f.none = false;
        f.waiting = None;
        if f.text.is_empty() {
            return;
        }
        self.find_run(true, None);
    }

    /// The next (previous) match.
    pub fn find_step(&mut self, forward: bool) {
        let Some(f) = &mut self.find else { return };
        if f.text.is_empty() {
            return;
        }
        f.waiting = None;
        let from = f.current;
        self.find_run(forward, from);
    }

    /// The texts of a row that are searched: the line's (unified), or each
    /// side's (split).
    fn row_texts(&self, i: usize, row: usize) -> [Option<&str>; 2] {
        let s = &self.sections[i];
        let (Some(d), Some((.., rows))) = (s.diff(), &s.rows) else { return [None, None] };
        match rows {
            Rows::Unified(v) => match v.get(row) {
                Some(Row::Line(l)) => {
                    let text = if l.kind == Kind::Removed { l.old.and_then(|o| d.old.get(o)) } else { l.new.and_then(|n| d.new.get(n)) };
                    [text.map(String::as_str), None]
                }
                _ => [None, None],
            },
            Rows::Split(v) => match v.get(row) {
                Some(SplitRow::Pair { left, right, .. }) => [
                    left.and_then(|c| d.old.get(c.line)).map(String::as_str),
                    right.and_then(|c| d.new.get(c.line)).map(String::as_str),
                ],
                _ => [None, None],
            },
        }
    }

    /// Every match of section `i`, in order (none until it is read).
    pub fn section_matches(&self, i: usize) -> Vec<Pos> {
        let Some(f) = &self.find else { return Vec::new() };
        let rows = self.sections[i].cursor_rows();
        let mut out = Vec::new();
        for row in 0..rows {
            for (side, text) in self.row_texts(i, row).into_iter().enumerate() {
                if let Some(text) = text {
                    out.extend((0..occurrences(text, &f.text).len()).map(|k| (i, row, side as u8, k)));
                }
            }
        }
        out
    }

    /// How many matches the files read so far hold.
    pub fn find_count(&self) -> usize {
        (0..self.sections.len()).map(|i| self.section_matches(i).len()).sum()
    }

    /// The matches in `text`, the text of `side` of row `row` of section `i`,
    /// and which of them is the current one.
    pub fn marks(&self, i: usize, row: usize, side: u8, text: &str) -> (Vec<Range<usize>>, Option<usize>) {
        let Some(f) = self.find.as_ref().filter(|f| !f.text.is_empty()) else { return (Vec::new(), None) };
        let marks = occurrences(text, &f.text);
        let current = f.current.filter(|p| p.0 == i && p.1 == row && p.2 == side).map(|p| p.3);
        (marks, current)
    }

    /// Step from `from` (none: from the cursor, the match at it counting),
    /// reading the files that come up unread.
    fn find_run(&mut self, forward: bool, from: Option<Pos>) {
        let n = self.sections.len();
        if n == 0 {
            return;
        }
        let inclusive = from.is_none();
        let origin: Pos = from.unwrap_or((self.cursor.section, self.cursor.row.unwrap_or(0), 0, 0));
        let start = origin.0.min(n - 1);
        for k in 0..=n {
            let i = if forward { (start + k) % n } else { (start + n - k % n) % n };
            if self.sections[i].loaded.is_none() {
                self.request(i, true);
                if let Some(f) = &mut self.find {
                    f.waiting = Some((forward, from));
                }
                return;
            }
            let list = self.section_matches(i);
            let pick = if k == 0 {
                if forward {
                    list.iter().find(|p| if inclusive { **p >= origin } else { **p > origin })
                } else {
                    list.iter().rev().find(|p| if inclusive { **p <= origin } else { **p < origin })
                }
            } else if k == n {
                // The first file again: what lay before (after) the start.
                if forward { list.iter().find(|p| **p <= origin) } else { list.iter().rev().find(|p| **p >= origin) }
            } else if forward {
                list.first()
            } else {
                list.last()
            };
            if let Some(&p) = pick {
                if let Some(f) = &mut self.find {
                    f.current = Some(p);
                    f.none = false;
                }
                self.anchor = None;
                self.cursor = Cursor { section: p.0, row: Some(p.1) };
                self.reveal(true);
                return;
            }
        }
        if let Some(f) = &mut self.find {
            f.current = None;
            f.none = true;
        }
    }

    /// A file was read: stepping that waited for one goes on.
    pub(super) fn find_after_load(&mut self) {
        let waiting = self.find.as_mut().and_then(|f| f.waiting.take());
        if let Some((forward, from)) = waiting {
            self.find_run(forward, from);
        }
    }
}
