//! Links in a terminal's text (the agent panel): file references like
//! `src/main.rs:42` or `@src/main.rs#L10-20`, and web URLs. A link may wrap
//! onto the next rows, as the agent hard-wraps its output to the width.

use grid::Grid;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Url(String),
    /// A path as written (relative to where the agent runs) and a 1-based
    /// line and column, when given.
    File { path: String, line: Option<u32>, col: Option<u32> },
}

/// A link and the cells it covers: `(row, first col, last col)` per row.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub target: Target,
    pub spans: Vec<(usize, usize, usize)>,
}

/// The link under a cell, if any. `exists` decides whether a file path is
/// real (so words like `and.` aren't links).
pub fn link_at(grid: &Grid, row: usize, col: usize, exists: &dyn Fn(&str) -> bool) -> Option<Link> {
    // The cell's logical line: rows joined across breaks with text on both
    // sides (a word wrapped at the edge).
    let joined = |r: usize| {
        let last = grid.cell(r, grid.width().checked_sub(1)?)?;
        let next = grid.cell(r + 1, 0)?;
        Some(!last.text.trim().is_empty() && !next.text.trim().is_empty())
    };
    let mut first = row;
    while first > 0 && joined(first - 1) == Some(true) {
        first -= 1;
    }
    let mut last = row;
    while joined(last) == Some(true) {
        last += 1;
    }
    // Characters with the cell each came from (wide chars' second cells
    // hold no text).
    let mut text = String::new();
    let mut cells: Vec<(usize, usize)> = Vec::new();
    for r in first..=last {
        for c in 0..grid.width() {
            let t = &grid.cell(r, c)?.text;
            for ch in t.chars() {
                text.push(ch);
                cells.push((r, c));
            }
        }
    }
    let chars: Vec<char> = text.chars().collect();
    let at = cells.iter().position(|&(r, c)| r == row && c >= col)?;
    let (start, end) = token(&chars, at)?;
    let word: String = chars[start..end].iter().collect();
    let target = parse(&word, exists)?;
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    for &(r, c) in &cells[start..end] {
        match spans.last_mut() {
            Some(s) if s.0 == r => s.2 = c,
            _ => spans.push((r, c, c)),
        }
    }
    Some(Link { target, spans })
}

/// The run of link characters around `at`, without trailing punctuation.
fn token(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let ok = |c: char| !c.is_whitespace() && !"\"'`<>()[]{}|│,".contains(c);
    if !ok(*chars.get(at)?) {
        return None;
    }
    let mut start = at;
    while start > 0 && ok(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && ok(chars[end]) {
        end += 1;
    }
    // A sentence's full stop or colon isn't part of the link.
    while end > start && ".:;!?".contains(chars[end - 1]) {
        end -= 1;
    }
    if chars[start] == '@' {
        start += 1;
    }
    (at >= start && at < end.max(start + 1) && end > start).then_some((start, end))
}

fn parse(word: &str, exists: &dyn Fn(&str) -> bool) -> Option<Target> {
    if word.starts_with("https://") || word.starts_with("http://") {
        return Some(Target::Url(word.to_owned()));
    }
    let word = word.strip_prefix("file://").unwrap_or(word);
    // path#L10-20 (Claude's mentions), path#L10, path:10:5, path:10-20.
    let (path, line, col) = if let Some((path, frag)) = word.split_once("#L") {
        let line = frag.split(['-', ':']).next().and_then(|n| n.parse().ok());
        (path, line, None)
    } else {
        let mut parts = word.splitn(3, ':');
        let path = parts.next()?;
        let line = parts.next().and_then(|l| l.split('-').next()?.parse().ok());
        let col = parts.next().and_then(|c| c.parse().ok());
        (path, line, col)
    };
    if path.is_empty() || !(path.contains('/') || path.contains('.')) || !exists(path) {
        return None;
    }
    Some(Target::File { path: path.to_owned(), line, col })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&str]) -> Grid {
        let width = rows.iter().map(|r| r.chars().count()).max().unwrap();
        let mut g = Grid::new(width, rows.len());
        for (r, text) in rows.iter().enumerate() {
            let cells: Vec<_> = text.chars().map(|c| grid::LineCell { text: c.to_string(), hl_id: 0, repeat: 1 }).collect();
            g.put_line(r, 0, &cells);
        }
        g
    }

    fn file(path: &str, line: Option<u32>, col: Option<u32>) -> Target {
        Target::File { path: path.into(), line, col }
    }

    #[test]
    fn finds_files_with_lines_and_urls() {
        let real = |p: &str| p != "src/missing.rs";
        let g = grid(&["See src/app.rs:1580, @NOTES.md#L40-52 and", "https://github.com/x/y. src/missing.rs:3  "]);
        let at = |r, c| link_at(&g, r, c, &real).map(|l| l.target);
        assert_eq!(at(0, 6), Some(file("src/app.rs", Some(1580), None)));
        assert_eq!(at(0, 25), Some(file("NOTES.md", Some(40), None)));
        assert_eq!(at(0, 0), None, "a word without a dot or slash");
        assert_eq!(at(0, 39), None, "'and' is no file");
        assert_eq!(at(1, 3), Some(Target::Url("https://github.com/x/y".into())));
        assert_eq!(at(1, 26), None, "the file doesn't exist");
    }

    #[test]
    fn follows_a_link_wrapped_at_the_edge() {
        let g = grid(&["edit crates/app/src/ap", "p.rs:12:5 now         "]);
        let link = link_at(&g, 1, 2, &|_| true).unwrap();
        assert_eq!(link.target, file("crates/app/src/app.rs", Some(12), Some(5)));
        assert_eq!(link.spans, [(0, 5, 21), (1, 0, 8)]);
        assert_eq!(link_at(&g, 0, 8, &|_| true).unwrap().spans, link.spans);
    }
}
