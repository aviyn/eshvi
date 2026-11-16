//! The diff model: two texts become hunks (a line diff, then a word diff
//! inside each changed pair of lines), and from them the rows of both
//! layouts, with the runs of unchanged lines folded. Pure: no egui, no I/O.
//!
//! A file's diff is a list of `Op`s over the two sides' lines: stretches that
//! are equal, and stretches that changed (lines removed, lines added, or
//! both). The rows are made from the ops: unified (one line per row, old
//! and new numbers) and side by side (the two sides aligned, blank filler
//! where one has no line). A folded run is a row of its own that stands for
//! the equal lines between two changes (or before the first, or after the
//! last): which ones are shown is `context` lines next to each change, the
//! rest folds, unless the run was expanded.

use std::collections::BTreeSet;
use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{capture_diff_slices_deadline, Algorithm, DiffTag};

/// A file with more lines than this (on either side) isn't diffed here.
pub const MAX_LINES: usize = 20_000;

/// The longest a line pair may be for its words to be compared.
const MAX_WORD_DIFF: usize = 2_000;

/// Give up on a line diff that takes longer than this (the ops found so far
/// are still correct, only longer than they could be).
const DEADLINE: Duration = Duration::from_secs(2);

/// What one side of a file is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    /// Not text (a NUL in its first bytes, as git decides).
    Binary,
    /// The file isn't on this side (a new file's left, a deleted file's right).
    Missing,
}

impl Content {
    /// `bytes` as the side's content: binary if it has a NUL in the first 8000.
    pub fn from_bytes(bytes: &[u8]) -> Content {
        if bytes.iter().take(8000).any(|b| *b == 0) {
            Content::Binary
        } else {
            Content::Text(String::from_utf8_lossy(bytes).into_owned())
        }
    }
}

/// A stretch of the two sides' lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Equal { old: usize, new: usize, len: usize },
    /// Lines `old` of the old side became lines `new` of the new one (either
    /// range may be empty: lines only removed, or only added).
    Change { old: Range<usize>, new: Range<usize> },
}

/// What a file's diff is, to draw.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Text(Diff),
    Binary,
    /// Too many lines (`lines`, on the larger side) to diff here.
    TooLarge { lines: usize },
}

/// A text file's diff.
#[derive(Clone, Debug, PartialEq)]
pub struct Diff {
    pub old: Vec<String>,
    pub new: Vec<String>,
    pub ops: Vec<Op>,
    /// The changed words of a changed line (byte ranges), by line index:
    /// where a line of one side was paired with one of the other.
    pub old_words: Vec<Vec<Range<usize>>>,
    pub new_words: Vec<Vec<Range<usize>>>,
    pub added: usize,
    pub removed: usize,
    /// Whether each side ends with a line end (an empty one does), and which
    /// lines of it had a `\r` before theirs: what a patch of one change
    /// needs to say the lines as they are.
    pub old_eol: bool,
    pub new_eol: bool,
    pub old_cr: Vec<bool>,
    pub new_cr: Vec<bool>,
}

/// Which way a line changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Removed,
    Added,
}

/// A line of the unified layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    /// Line indexes (0-based) in each side, if the line is on that side.
    pub old: Option<usize>,
    pub new: Option<usize>,
    /// The line of the new side this one is at: its own, or (a removed
    /// line) where it was.
    pub at: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Line(Line),
    /// `hidden` unchanged lines, the equal run of op `op`, expanded by a click.
    Fold { op: usize, hidden: usize },
}

/// A line of one side in the side by side layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub kind: Kind,
    /// Line index (0-based) in the cell's side.
    pub line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitRow {
    /// A cell has none where its side has no line to match.
    Pair { left: Option<Cell>, right: Option<Cell>, at: usize },
    Fold { op: usize, hidden: usize },
}

/// The lines of `text` (no line ends, `\r\n` too).
pub fn lines_of(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect();
    // (The text ends with a line end: no empty line after it.)
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Which of `text`'s lines (as `lines_of` cuts them) end in `\r\n`.
fn carriage_returns(text: &str) -> Vec<bool> {
    let mut cr: Vec<bool> = text.split('\n').map(|l| l.ends_with('\r')).collect();
    if text.is_empty() || text.ends_with('\n') {
        cr.pop();
    }
    cr
}

fn line_count(text: &str) -> usize {
    text.bytes().filter(|b| *b == b'\n').count() + usize::from(!text.is_empty() && !text.ends_with('\n'))
}

/// The diff of two sides: the file's `Body`.
pub fn body(old: &Content, new: &Content, max_lines: usize) -> Body {
    let (old, new) = match (old, new) {
        (Content::Binary, _) | (_, Content::Binary) => return Body::Binary,
        (Content::Text(a), Content::Text(b)) => (a.as_str(), b.as_str()),
        (Content::Text(a), Content::Missing) => (a.as_str(), ""),
        (Content::Missing, Content::Text(b)) => ("", b.as_str()),
        (Content::Missing, Content::Missing) => ("", ""),
    };
    let lines = line_count(old).max(line_count(new));
    if lines > max_lines {
        return Body::TooLarge { lines };
    }
    Body::Text(Diff::new(old, new))
}

impl Diff {
    pub fn new(old: &str, new: &str) -> Diff {
        let (a, b) = (lines_of(old), lines_of(new));
        let deadline = Instant::now() + DEADLINE;
        let raw = capture_diff_slices_deadline(Algorithm::Myers, &a, &b, Some(deadline));
        let mut ops = Vec::new();
        let (mut added, mut removed) = (0, 0);
        for op in &raw {
            let (tag, o, n) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => ops.push(Op::Equal { old: o.start, new: n.start, len: o.len() }),
                DiffTag::Delete | DiffTag::Insert | DiffTag::Replace => {
                    removed += o.len();
                    added += n.len();
                    ops.push(Op::Change { old: o, new: n });
                }
            }
        }
        let mut old_words = vec![Vec::new(); a.len()];
        let mut new_words = vec![Vec::new(); b.len()];
        for op in &ops {
            let Op::Change { old: o, new: n } = op else { continue };
            for (i, j) in o.clone().zip(n.clone()) {
                let (ow, nw) = changed_words(&a[i], &b[j]);
                old_words[i] = ow;
                new_words[j] = nw;
            }
        }
        Diff {
            old: a,
            new: b,
            ops,
            old_words,
            new_words,
            added,
            removed,
            old_eol: old.is_empty() || old.ends_with('\n'),
            new_eol: new.is_empty() || new.ends_with('\n'),
            old_cr: carriage_returns(old),
            new_cr: carriage_returns(new),
        }
    }

    /// The op of the change (not an equal run) that holds old line `old` or
    /// new line `new`: what a changed row says it is part of.
    pub fn change_at(&self, old: Option<usize>, new: Option<usize>) -> Option<usize> {
        self.ops.iter().position(|op| match op {
            Op::Change { old: o, new: n } => old.is_some_and(|l| o.contains(&l)) || new.is_some_and(|l| n.contains(&l)),
            Op::Equal { .. } => false,
        })
    }

    /// The `n`th change's ordinal among the changes (not the equal runs): the
    /// number of changes before op `op`.
    pub fn ordinal_of(&self, op: usize) -> usize {
        self.ops[..op.min(self.ops.len())].iter().filter(|o| matches!(o, Op::Change { .. })).count()
    }

    /// The old and new lines of op `op`, if it is a change.
    pub fn change(&self, op: usize) -> Option<(Range<usize>, Range<usize>)> {
        match self.ops.get(op)? {
            Op::Change { old, new } => Some((old.clone(), new.clone())),
            Op::Equal { .. } => None,
        }
    }

    /// Change `op` alone as a unified diff, for `git apply`: its `context`
    /// lines of the unchanged run on each side (never lines of another
    /// change, so the patch fits a file where only this change was made or
    /// undone), a header whose counts and line numbers are the sides' own, and
    /// "\ No newline at end of file" where a side's last line has none. The
    /// patch turns the old side into one with only this change in it.
    /// `path` is the file in both (a renamed file's content is at its new
    /// path where the patch applies); `kind` says whether it is created or removed.
    pub fn change_patch(&self, op: usize, path: &str, kind: FileKind, context: usize) -> Option<String> {
        let (old, new) = self.change(op)?;
        let (mut before, mut after) = (0, 0);
        if let Some(Op::Equal { len, .. }) = op.checked_sub(1).and_then(|i| self.ops.get(i)) {
            before = context.min(*len);
        }
        if let Some(Op::Equal { len, old: at, .. }) = self.ops.get(op + 1) {
            after = context.min(*len);
            // (An equal last line that one side ends and the other doesn't isn't equal.)
            if self.old_eol != self.new_eol && at + len == self.old.len() {
                after = after.min(len - 1);
            }
        }
        let (os, ns) = (old.start - before, new.start - before);
        let (oc, nc) = (before + old.len() + after, before + new.len() + after);
        let start = |at: usize, count: usize| if count == 0 { at } else { at + 1 };
        let (a, b) = (patch_name("a/", path), patch_name("b/", path));
        let mut out = String::new();
        match kind {
            FileKind::Modified => out.push_str(&format!("--- {a}\n+++ {b}\n")),
            FileKind::New => out.push_str(&format!("--- /dev/null\n+++ {b}\n")),
            FileKind::Deleted => out.push_str(&format!("--- {a}\n+++ /dev/null\n")),
        }
        out.push_str(&format!("@@ -{},{oc} +{},{nc} @@\n", start(os, oc), start(ns, nc)));
        let line = |out: &mut String, sign: char, text: &str, cr: bool, last: bool, eol: bool| {
            out.push(sign);
            out.push_str(text);
            if cr {
                out.push('\r');
            }
            out.push('\n');
            if last && !eol {
                out.push_str("\\ No newline at end of file\n");
            }
        };
        for k in 0..before {
            let i = old.start - before + k;
            line(&mut out, ' ', &self.old[i], self.old_cr[i], i + 1 == self.old.len(), self.old_eol);
        }
        for i in old.clone() {
            line(&mut out, '-', &self.old[i], self.old_cr[i], i + 1 == self.old.len(), self.old_eol);
        }
        for j in new.clone() {
            line(&mut out, '+', &self.new[j], self.new_cr[j], j + 1 == self.new.len(), self.new_eol);
        }
        for k in 0..after {
            let i = old.end + k;
            line(&mut out, ' ', &self.old[i], self.old_cr[i], i + 1 == self.old.len(), self.old_eol);
        }
        Some(out)
    }

    /// Nothing differs.
    pub fn is_identical(&self) -> bool {
        self.added == 0 && self.removed == 0
    }

    /// The equal runs' pieces for `context` lines of context: which of each
    /// run shows, and the rest as a fold (unless `expanded` has the run's op).
    fn pieces(&self, context: usize, expanded: &BTreeSet<usize>) -> Vec<Piece> {
        let mut out = Vec::new();
        for (i, op) in self.ops.iter().enumerate() {
            match op {
                Op::Change { old, new } => out.push(Piece::Change { old: old.clone(), new: new.clone() }),
                &Op::Equal { old, new, len } => {
                    let head = if i > 0 { context } else { 0 };
                    let tail = if i + 1 < self.ops.len() { context } else { 0 };
                    let hidden = len.saturating_sub(head + tail);
                    // (A fold of one line is a row for a row: show the line.)
                    if hidden < 2 || expanded.contains(&i) {
                        out.push(Piece::Equal { old, new, len });
                    } else {
                        if head > 0 {
                            out.push(Piece::Equal { old, new, len: head });
                        }
                        out.push(Piece::Fold { op: i, hidden });
                        if tail > 0 {
                            out.push(Piece::Equal { old: old + len - tail, new: new + len - tail, len: tail });
                        }
                    }
                }
            }
        }
        out
    }

    /// The unified layout's rows.
    pub fn unified(&self, context: usize, expanded: &BTreeSet<usize>) -> Vec<Row> {
        let mut rows = Vec::new();
        for piece in self.pieces(context, expanded) {
            match piece {
                Piece::Fold { op, hidden } => rows.push(Row::Fold { op, hidden }),
                Piece::Equal { old, new, len } => {
                    for k in 0..len {
                        rows.push(Row::Line(Line { kind: Kind::Context, old: Some(old + k), new: Some(new + k), at: new + k }));
                    }
                }
                Piece::Change { old, new } => {
                    for i in old.clone() {
                        rows.push(Row::Line(Line { kind: Kind::Removed, old: Some(i), new: None, at: new.start }));
                    }
                    for j in new {
                        rows.push(Row::Line(Line { kind: Kind::Added, old: None, new: Some(j), at: j }));
                    }
                }
            }
        }
        rows
    }

    /// The side by side layout's rows: a change's removed lines on the left
    /// against its added lines on the right, one to one, the shorter side
    /// filled out with blanks.
    pub fn split(&self, context: usize, expanded: &BTreeSet<usize>) -> Vec<SplitRow> {
        let mut rows = Vec::new();
        for piece in self.pieces(context, expanded) {
            match piece {
                Piece::Fold { op, hidden } => rows.push(SplitRow::Fold { op, hidden }),
                Piece::Equal { old, new, len } => {
                    for k in 0..len {
                        rows.push(SplitRow::Pair {
                            left: Some(Cell { kind: Kind::Context, line: old + k }),
                            right: Some(Cell { kind: Kind::Context, line: new + k }),
                            at: new + k,
                        });
                    }
                }
                Piece::Change { old, new } => {
                    for k in 0..old.len().max(new.len()) {
                        rows.push(SplitRow::Pair {
                            left: (k < old.len()).then(|| Cell { kind: Kind::Removed, line: old.start + k }),
                            right: (k < new.len()).then(|| Cell { kind: Kind::Added, line: new.start + k }),
                            at: new.start + k.min(new.len().saturating_sub(1)),
                        });
                    }
                }
            }
        }
        rows
    }
}

/// What a patch does to the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Modified,
    /// The file is created (the old side is nothing). (The app has git make or
    /// remove whole files; the patch for them is here, and tried against git.)
    #[allow(dead_code)]
    New,
    /// The file is removed (the new side is nothing).
    #[allow(dead_code)]
    Deleted,
}

/// `prefix` + `path` as a patch's header names a file: spaces get a tab after
/// the name (as `git diff` writes them), and a name with quotes, backslashes
/// or control characters is quoted whole, C style.
fn patch_name(prefix: &str, path: &str) -> String {
    if path.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        let mut out = String::from("\"");
        out.push_str(prefix);
        for c in path.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => out.push_str(&format!("\\{:03o}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        return out;
    }
    if path.contains(' ') {
        format!("{prefix}{path}\t")
    } else {
        format!("{prefix}{path}")
    }
}

enum Piece {
    Equal { old: usize, new: usize, len: usize },
    Change { old: Range<usize>, new: Range<usize> },
    Fold { op: usize, hidden: usize },
}

impl Row {
    pub fn is_change(&self) -> bool {
        matches!(self, Row::Line(l) if l.kind != Kind::Context)
    }
}

impl SplitRow {
    pub fn is_change(&self) -> bool {
        matches!(self, SplitRow::Pair { left, right, .. } if [left, right].iter().any(|c| c.is_some_and(|c| c.kind != Kind::Context)))
    }
}

/// The first row of each run of changed rows (`is_change`).
pub fn change_starts(is_change: impl Iterator<Item = bool>) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut prev = false;
    for (i, change) in is_change.enumerate() {
        if change && !prev {
            starts.push(i);
        }
        prev = change;
    }
    starts
}

/// A line cut into words: runs of letters, digits and `_`, runs of
/// whitespace, and every other character alone.
pub fn words(line: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut prev: Option<Class> = None;
    for (i, c) in line.char_indices() {
        let k = class(c);
        match (out.last_mut(), prev) {
            (Some(last), Some(p)) if p == k && k != Class::Other => last.end = i + c.len_utf8(),
            _ => out.push(i..i + c.len_utf8()),
        }
        prev = Some(k);
    }
    out
}

/// The words that differ between two lines, as byte ranges of each (adjacent
/// ones joined). None when the lines have too little in common for the words
/// to mean anything (the whole line changed), or are too long.
pub fn changed_words(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if old == new || old.len() > MAX_WORD_DIFF || new.len() > MAX_WORD_DIFF {
        return (Vec::new(), Vec::new());
    }
    let (wo, wn) = (words(old), words(new));
    let (so, sn): (Vec<&str>, Vec<&str>) = (wo.iter().map(|r| &old[r.clone()]).collect(), wn.iter().map(|r| &new[r.clone()]).collect());
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &so, &sn, Some(Instant::now() + DEADLINE));
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let mut same = 0;
    let push = |out: &mut Vec<Range<usize>>, r: Range<usize>| match out.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => out.push(r),
    };
    for op in &ops {
        let (tag, o, n) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => same += o.clone().map(|k| wo[k].len()).sum::<usize>(),
            _ => {
                if !o.is_empty() {
                    push(&mut a, wo[o.start].start..wo[o.end - 1].end);
                }
                if !n.is_empty() {
                    push(&mut b, wn[n.start].start..wn[n.end - 1].end);
                }
            }
        }
    }
    // (Less than 40% of the longer line shared: a different line, not an edit.)
    if same * 5 < old.len().max(new.len()) * 2 {
        return (Vec::new(), Vec::new());
    }
    (a, b)
}

/// A piece of a line to draw alike: its byte range, and the index of the
/// syntax span it is in (into the spans given), and whether it's a changed word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub range: Range<usize>,
    pub span: Option<usize>,
    pub word: bool,
    /// A match of the find field: the index into the marks given.
    pub mark: Option<usize>,
}

/// `line` cut where a syntax span or a changed word starts or ends.
/// `spans` and `words` are sorted byte ranges, not overlapping.
#[cfg(test)]
pub fn segments(line: &str, spans: &[Range<usize>], words: &[Range<usize>]) -> Vec<Segment> {
    segments_marked(line, spans, words, &[])
}

/// `segments`, cut at the `marks` (find matches, not overlapping) too.
pub fn segments_marked(line: &str, spans: &[Range<usize>], words: &[Range<usize>], marks: &[Range<usize>]) -> Vec<Segment> {
    let mut cuts: Vec<usize> = vec![0, line.len()];
    for r in spans.iter().chain(words).chain(marks) {
        cuts.extend([r.start.min(line.len()), r.end.min(line.len())]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start == end || !line.is_char_boundary(start) || !line.is_char_boundary(end) {
            continue;
        }
        let span = spans.iter().position(|s| s.start <= start && end <= s.end);
        let word = words.iter().any(|w| w.start <= start && end <= w.end);
        let mark = marks.iter().position(|m| m.start <= start && end <= m.end);
        out.push(Segment { range: start..end, span, word, mark });
    }
    out
}

/// `text` with tabs as spaces to the next multiple of `tab` columns, from
/// column `col`.
pub fn expand_tabs(text: &str, col: usize, tab: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = col;
    for c in text.chars() {
        if c == '\t' {
            let n = tab - at % tab;
            out.extend(std::iter::repeat_n(' ', n));
            at += n;
        } else {
            out.push(c);
            at += 1;
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn text(lines: &[&str]) -> String {
        lines.iter().map(|l| format!("{l}\n")).collect()
    }

    fn diff(old: &[&str], new: &[&str]) -> Diff {
        Diff::new(&text(old), &text(new))
    }

    fn none() -> BTreeSet<usize> {
        BTreeSet::new()
    }

    #[test]
    fn lines_split_on_either_line_end() {
        assert_eq!(lines_of("a\r\nb\n"), ["a", "b"]);
        assert_eq!(lines_of("a\n\nb"), ["a", "", "b"]);
        assert!(lines_of("").is_empty());
        assert_eq!(lines_of("\n"), [""]);
        assert_eq!((line_count("a\nb"), line_count("a\nb\n"), line_count("")), (2, 2, 0));
    }

    #[test]
    fn a_changed_line_is_a_removed_and_an_added_one() {
        let d = diff(&["a", "b", "c"], &["a", "B", "c"]);
        assert_eq!(
            d.ops,
            [Op::Equal { old: 0, new: 0, len: 1 }, Op::Change { old: 1..2, new: 1..2 }, Op::Equal { old: 2, new: 2, len: 1 }]
        );
        assert_eq!((d.added, d.removed), (1, 1));
        assert!(!d.is_identical());
        assert!(diff(&["a"], &["a"]).is_identical());
    }

    #[test]
    fn counts_added_and_removed_lines() {
        let d = diff(&["a", "b", "c", "d"], &["a", "x", "y", "z", "d"]);
        assert_eq!((d.added, d.removed), (3, 2));
        let d = diff(&[], &["a", "b"]);
        assert_eq!((d.added, d.removed), (2, 0));
        assert_eq!(d.ops, [Op::Change { old: 0..0, new: 0..2 }]);
    }

    #[test]
    fn words_split_at_classes() {
        fn w(s: &str) -> Vec<&str> {
            words(s).into_iter().map(|r| &s[r]).collect()
        }
        assert_eq!(w("let foo_1 = bar(x);"), ["let", " ", "foo_1", " ", "=", " ", "bar", "(", "x", ")", ";"]);
        assert_eq!(w("a  b"), ["a", "  ", "b"]);
        assert_eq!(w("日本 x"), ["日本", " ", "x"]);
        assert!(w("").is_empty());
    }

    #[test]
    fn changed_words_are_only_the_words_that_differ() {
        let (old, new) = ("let total = count + 1;", "let total = count + step;");
        let (a, b) = changed_words(old, new);
        assert_eq!(a.iter().map(|r| &old[r.clone()]).collect::<Vec<_>>(), ["1"]);
        assert_eq!(b.iter().map(|r| &new[r.clone()]).collect::<Vec<_>>(), ["step"]);
        // Adjacent changed words are one range.
        let (old, new) = ("call(a, b)", "call(x, y)");
        let (a, b) = changed_words(old, new);
        assert_eq!(a.iter().map(|r| &old[r.clone()]).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(b.iter().map(|r| &new[r.clone()]).collect::<Vec<_>>(), ["x", "y"]);
        // Lines with little in common, equal ones and a deletion at the end.
        assert_eq!(changed_words("alpha beta", "gamma delta"), (vec![], vec![]));
        assert_eq!(changed_words("same", "same"), (vec![], vec![]));
        let (a, b) = changed_words("fn main() {}", "fn main() { x }");
        assert_eq!((a.len(), b.len()), (0, 1));
    }

    #[test]
    fn paired_lines_of_a_change_get_their_words() {
        let d = diff(&["keep", "let x = 1;", "keep2"], &["keep", "let x = 2;", "keep2"]);
        assert_eq!(d.old_words[1], [8..9]);
        assert_eq!(d.new_words[1], [8..9]);
        assert!(d.old_words[0].is_empty() && d.new_words[2].is_empty());
        // Unpaired lines (more added than removed) have none.
        let d = diff(&["let x = 1;"], &["let x = 2;", "extra line here"]);
        assert_eq!(d.new_words[0], [8..9]);
        assert!(d.new_words[1].is_empty());
    }

    fn numbered(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    #[test]
    fn unchanged_runs_fold_to_the_context() {
        let old = numbered(30);
        let mut new = old.clone();
        new[14] = "changed".into();
        let d = Diff::new(&old.join("\n"), &new.join("\n"));
        let rows = d.unified(3, &none());
        // 14 equal lines before: 11 fold, 3 shown; the change; 3 after, 12 fold.
        assert_eq!(rows.len(), 1 + 3 + 2 + 3 + 1);
        assert_eq!(rows[0], Row::Fold { op: 0, hidden: 11 });
        assert_eq!(*rows.last().unwrap(), Row::Fold { op: 2, hidden: 12 });
        let line = |r: &Row| match r {
            Row::Line(l) => *l,
            _ => panic!("{r:?}"),
        };
        assert_eq!(line(&rows[1]).old, Some(11));
        assert_eq!(line(&rows[4]).kind, Kind::Removed);
        assert_eq!(line(&rows[5]).kind, Kind::Added);
        assert_eq!(line(&rows[5]).new, Some(14));
        assert_eq!(line(&rows[6]).old, Some(15));
        // Context 0: only the change.
        assert_eq!(d.unified(0, &none()).len(), 1 + 2 + 1);
    }

    #[test]
    fn a_run_expands_when_asked_and_a_fold_of_one_line_is_not_made() {
        let old = numbered(30);
        let mut new = old.clone();
        new[14] = "changed".into();
        let d = Diff::new(&old.join("\n"), &new.join("\n"));
        let rows = d.unified(3, &BTreeSet::from([0]));
        assert_eq!(rows.len(), 14 + 2 + 3 + 1);
        assert!(matches!(rows[0], Row::Line(_)));
        assert!(matches!(rows.last(), Some(Row::Fold { op: 2, .. })));
        // Two changes with one line between, and with five.
        let one = diff(&["a", "b", "c", "d", "e"], &["A", "b", "C", "d", "E"]);
        assert!(one.unified(0, &none()).iter().all(|r| !matches!(r, Row::Fold { .. })));
        let mut old: Vec<String> = numbered(20);
        let mut new = old.clone();
        old[2] = "x".into();
        new[2] = "y".into();
        old[18] = "x".into();
        new[18] = "y".into();
        let d = Diff::new(&old.join("\n"), &new.join("\n"));
        let rows = d.unified(3, &none());
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Fold { .. })).count(), 1, "{rows:?}");
        assert_eq!(rows.iter().find_map(|r| if let Row::Fold { hidden, .. } = r { Some(*hidden) } else { None }), Some(9));
    }

    #[test]
    fn unified_rows_keep_both_numbers_and_where_a_removed_line_was() {
        let d = diff(&["a", "b", "c"], &["a", "x", "y", "c"]);
        let rows = d.unified(3, &none());
        let lines: Vec<Line> = rows.iter().map(|r| if let Row::Line(l) = r { *l } else { panic!() }).collect();
        assert_eq!(lines.len(), 5);
        assert_eq!((lines[0].old, lines[0].new), (Some(0), Some(0)));
        assert_eq!((lines[1].kind, lines[1].old, lines[1].new, lines[1].at), (Kind::Removed, Some(1), None, 1));
        assert_eq!((lines[2].kind, lines[2].new), (Kind::Added, Some(1)));
        assert_eq!((lines[4].old, lines[4].new), (Some(2), Some(3)));
        // Lines only removed: they point at the line after.
        let d = diff(&["a", "b", "c"], &["a", "c"]);
        let removed = d.unified(3, &none()).into_iter().find_map(|r| match r {
            Row::Line(l) if l.kind == Kind::Removed => Some(l),
            _ => None,
        });
        assert_eq!(removed.map(|l| l.at), Some(1));
    }

    #[test]
    fn split_rows_align_the_sides_with_blank_filler() {
        let d = diff(&["a", "b", "c", "d"], &["a", "x", "y", "z", "d"]);
        let rows = d.split(3, &none());
        let cell = |c: Option<Cell>| c.map(|c| (c.kind, c.line));
        let shape: Vec<_> = rows
            .iter()
            .map(|r| match r {
                SplitRow::Pair { left, right, .. } => (cell(*left), cell(*right)),
                SplitRow::Fold { .. } => panic!(),
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some((Kind::Context, 0)), Some((Kind::Context, 0))),
                (Some((Kind::Removed, 1)), Some((Kind::Added, 1))),
                (Some((Kind::Removed, 2)), Some((Kind::Added, 2))),
                // The left side has no third line: filler.
                (None, Some((Kind::Added, 3))),
                (Some((Kind::Context, 3)), Some((Kind::Context, 4))),
            ]
        );
        // Lines only removed: filler on the right.
        let d = diff(&["a", "b"], &["a"]);
        let rows = d.split(3, &none());
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[1], SplitRow::Pair { left: Some(_), right: None, at: 1 }));
    }

    #[test]
    fn split_and_unified_fold_alike() {
        let old = numbered(40);
        let mut new = old.clone();
        new[20] = "changed".into();
        let d = Diff::new(&old.join("\n"), &new.join("\n"));
        let folds = |n: &[usize]| n.to_vec();
        let u: Vec<usize> = d.unified(3, &none()).iter().enumerate().filter(|(_, r)| matches!(r, Row::Fold { .. })).map(|(i, _)| i).collect();
        // A changed line is two rows unified and one split: the last fold is a row later unified.
        let s: Vec<usize> = d.split(3, &none()).iter().enumerate().filter(|(_, r)| matches!(r, SplitRow::Fold { .. })).map(|(i, _)| i).collect();
        assert_eq!(folds(&u), [0, 9]);
        assert_eq!(folds(&s), [0, 8]);
    }

    #[test]
    fn change_starts_are_the_first_rows_of_runs() {
        let d = diff(&["a", "b", "c", "d", "e", "f", "g", "h"], &["a", "B", "c", "d", "e", "f", "G", "H"]);
        let rows = d.unified(1, &none());
        let starts = change_starts(rows.iter().map(Row::is_change));
        assert_eq!(starts.len(), 2);
        let split = d.split(1, &none());
        assert_eq!(change_starts(split.iter().map(SplitRow::is_change)).len(), 2);
    }

    #[test]
    fn a_new_file_is_all_added_and_a_deleted_one_all_removed() {
        let new = Content::Text("a\nb\n".into());
        let Body::Text(d) = body(&Content::Missing, &new, MAX_LINES) else { panic!() };
        assert_eq!((d.added, d.removed), (2, 0));
        assert!(d.unified(3, &none()).iter().all(|r| matches!(r, Row::Line(l) if l.kind == Kind::Added)));
        let Body::Text(d) = body(&new, &Content::Missing, MAX_LINES) else { panic!() };
        assert_eq!((d.added, d.removed), (0, 2));
        assert!(d.unified(3, &none()).iter().all(|r| matches!(r, Row::Line(l) if l.kind == Kind::Removed)));
        let Body::Text(d) = body(&Content::Missing, &Content::Missing, MAX_LINES) else { panic!() };
        assert!(d.is_identical() && d.unified(3, &none()).is_empty());
    }

    #[test]
    fn binary_and_big_files_are_not_diffed() {
        assert_eq!(Content::from_bytes(b"ab\0cd"), Content::Binary);
        assert_eq!(Content::from_bytes(b"abc"), Content::Text("abc".into()));
        let text = Content::Text("a\n".into());
        assert_eq!(body(&text, &Content::Binary, MAX_LINES), Body::Binary);
        assert_eq!(body(&Content::Binary, &Content::Missing, MAX_LINES), Body::Binary);
        let big = Content::Text("x\n".repeat(30));
        assert_eq!(body(&text, &big, 20), Body::TooLarge { lines: 30 });
        assert_eq!(body(&big, &text, 20), Body::TooLarge { lines: 30 });
        assert!(matches!(body(&text, &big, 30), Body::Text(_)));
    }

    #[test]
    fn segments_cut_at_syntax_and_word_edges() {
        let line = "let x = 12;";
        let spans = [0..3, 4..5, 8..10];
        let words = [8..10];
        let segs = segments(line, &spans, &words);
        let cut: Vec<(&str, Option<usize>, bool)> = segs.iter().map(|s| (&line[s.range.clone()], s.span, s.word)).collect();
        assert_eq!(
            cut,
            [("let", Some(0), false), (" ", None, false), ("x", Some(1), false), (" = ", None, false), ("12", Some(2), true), (";", None, false)]
        );
        assert!(segments("", &[], &[]).is_empty());
        // A range past the line's end is cut to it.
        assert_eq!(segments("ab", &[0..9], &[]).len(), 1);
    }

    #[test]
    fn tabs_expand_to_stops() {
        assert_eq!(expand_tabs("\tx", 0, 4), "    x");
        assert_eq!(expand_tabs("ab\tx", 0, 4), "ab  x");
        assert_eq!(expand_tabs("x", 0, 4), "x");
        assert_eq!(expand_tabs("\t", 2, 4), "  ");
    }

    fn patch(old: &str, new: &str, n: usize) -> String {
        Diff::new(old, new).change_patch(n, "a.txt", FileKind::Modified, 3).unwrap()
    }

    fn nth(d: &Diff, n: usize) -> usize {
        d.ops.iter().enumerate().filter(|(_, o)| matches!(o, Op::Change { .. })).nth(n).map(|(i, _)| i).unwrap()
    }

    #[test]
    fn a_patch_of_one_change_has_its_own_counts_and_context() {
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 5\n", "five\nand a half\n").replace("line 15\n", "");
        let d = Diff::new(&old, &new);
        assert_eq!(d.ops.iter().filter(|o| matches!(o, Op::Change { .. })).count(), 2);
        // The first change: 1 line became 2, with 3 lines of context each side.
        let first = d.change_patch(nth(&d, 0), "a.txt", FileKind::Modified, 3).unwrap();
        assert_eq!(
            first,
            "--- a/a.txt\n+++ b/a.txt\n@@ -2,7 +2,8 @@\n line 2\n line 3\n line 4\n-line 5\n+five\n+and a half\n line 6\n line 7\n line 8\n"
        );
        // The second: a line gone. The new side is a line further on (the first change added one).
        let second = d.change_patch(nth(&d, 1), "a.txt", FileKind::Modified, 3).unwrap();
        assert!(second.contains("@@ -12,7 +13,6 @@\n"), "{second}");
        assert!(second.contains("-line 15\n line 16\n"));
        // Not a change: none.
        assert!(d.change_patch(0, "a.txt", FileKind::Modified, 3).is_none());
        // No context at all, and a line only added (the old count is 0: the line before).
        let p = Diff::new("a\nb\n", "a\nx\nb\n").change_patch(1, "f", FileKind::Modified, 0).unwrap();
        assert!(p.ends_with("@@ -1,0 +2,1 @@\n+x\n"), "{p}");
    }

    #[test]
    fn context_stops_at_the_file_ends_and_at_the_next_change() {
        // A change on the first line, and one on the last: no context before the first, none after the last.
        let d = Diff::new("a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n");
        let first = d.change_patch(nth(&d, 0), "f", FileKind::Modified, 3).unwrap();
        assert!(first.contains("@@ -1,4 +1,4 @@\n-a\n+A\n b\n c\n d\n"), "{first}");
        let last = d.change_patch(nth(&d, 1), "f", FileKind::Modified, 3).unwrap();
        assert!(last.contains("@@ -2,4 +2,4 @@\n b\n c\n d\n-e\n+E\n"), "{last}");
        // Two changes with one unchanged line between: each patch's context is that line only, never the other change's.
        let d = Diff::new("a\nb\nc\nd\ne\n", "a\nB\nc\nD\ne\n");
        let p = d.change_patch(nth(&d, 0), "f", FileKind::Modified, 3).unwrap();
        assert_eq!(p, "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n");
    }

    #[test]
    fn a_missing_last_line_end_is_said_on_the_line_it_belongs_to() {
        // The new side has no newline at the end.
        let p = patch("a\nb\n", "a\nB", 1);
        assert_eq!(p, "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n\\ No newline at end of file\n");
        // The old side has none.
        let p = patch("a\nb", "a\nB\n", 1);
        assert_eq!(p, "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+B\n");
        // Neither has, and the last line is context.
        let d = Diff::new("a\nb\nc", "A\nb\nc");
        let p = d.change_patch(nth(&d, 0), "a.txt", FileKind::Modified, 3).unwrap();
        assert!(p.ends_with("-a\n+A\n b\n c\n\\ No newline at end of file\n"), "{p}");
        // Only the end of the file differs: the last line is no context.
        let d = Diff::new("a\nb\nc\n", "A\nb\nc");
        let p = d.change_patch(nth(&d, 0), "a.txt", FileKind::Modified, 3).unwrap();
        assert!(p.ends_with("-a\n+A\n b\n") && p.contains("@@ -1,2 +1,2 @@"), "{p}");
    }

    #[test]
    fn a_file_made_or_removed_has_a_header_to_match() {
        let d = Diff::new("", "one\ntwo\n");
        assert_eq!(d.change_patch(0, "n.txt", FileKind::New, 3).unwrap(), "--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n");
        let d = Diff::new("one\ntwo\n", "");
        assert_eq!(d.change_patch(0, "n.txt", FileKind::Deleted, 3).unwrap(), "--- a/n.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n");
    }

    #[test]
    fn carriage_returns_and_odd_names_are_kept() {
        let d = Diff::new("a\r\nb\r\n", "a\r\nB\r\n");
        assert_eq!(d.change_patch(1, "f", FileKind::Modified, 3).unwrap(), "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\r\n-b\r\n+B\r\n");
        assert_eq!(patch_name("a/", "my file.txt"), "a/my file.txt\t");
        assert_eq!(patch_name("b/", "q\"x\\y"), "\"b/q\\\"x\\\\y\"");
        assert_eq!(patch_name("a/", "src/é.rs"), "a/src/é.rs");
    }

    #[test]
    fn a_changed_row_knows_its_change() {
        let d = diff(&["a", "b", "c", "d", "e"], &["a", "B", "c", "d", "X", "Y"]);
        assert_eq!(d.change_at(Some(1), None), Some(1));
        assert_eq!(d.change_at(None, Some(1)), Some(1));
        assert_eq!(d.change_at(Some(0), Some(0)), None);
        assert_eq!(d.change_at(None, Some(5)), Some(3));
        assert_eq!((d.ordinal_of(1), d.ordinal_of(3)), (0, 1));
        assert_eq!(d.change(3), Some((4..5, 4..6)));
        assert_eq!(d.change(0), None);
    }

    #[test]
    fn matches_in_a_segment_are_cut_out() {
        let segs = segments_marked("let x = 1;", &[], &[], &[4..5]);
        let cut: Vec<(Range<usize>, Option<usize>)> = segs.iter().map(|s| (s.range.clone(), s.mark)).collect();
        assert_eq!(cut, [(0..4, None), (4..5, Some(0)), (5..10, None)]);
    }

    #[test]
    fn a_diff_again_after_an_edit_is_a_new_diff() {
        let before = diff(&["a", "b"], &["a", "c"]);
        let after = Diff::new(&text(&["a", "b"]), &text(&["a", "c", "d"]));
        assert_ne!(before, after);
        assert_eq!((after.added, after.removed), (2, 1));
    }
}
