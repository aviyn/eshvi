//! Grep and the preview: the search, the rows it makes, the pane that shows
//! a file, and the box that holds both.

use std::sync::atomic::Ordering;

use nvim_bridge::gui::{PickerPreview, PreviewSpan};

use super::*;
use crate::picker::grep::{self, Grep, Hit, Msg, Scope};
use crate::picker::preview::{self as pv, PreviewPane, Style, Subject, Target};

// ---- The search ----

fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = temp(name);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

/// Everything a search finds, sorted by path then line.
fn found(grep: &Grep) -> (Vec<Hit>, bool) {
    let mut hits = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut capped = false;
    while Instant::now() < deadline {
        match grep.found.recv_timeout(Duration::from_millis(50)) {
            Ok(Msg::Hits(h)) => hits.extend(h),
            Ok(Msg::Done { capped: c }) => {
                capped = c;
                break;
            }
            Err(_) => {}
        }
    }
    hits.sort_by(|a, b| (&a.path, a.lnum).cmp(&(&b.path, b.lnum)));
    (hits, capped)
}

fn search(dir: &std::path::Path, query: &str, literal: bool) -> Vec<(String, usize)> {
    let grep = Grep::start(Scope::Tree(dir.to_path_buf()), query, literal, || {}).expect("a query");
    found(&grep).0.into_iter().map(|h| (h.path, h.lnum)).collect()
}

#[test]
fn a_query_is_a_regex_and_text_when_it_does_not_compile() {
    let dir = tree("g-regex", &[("a.txt", "fn main()\nlet x = 1;\nfoo(baz\n"), ("b.txt", "foo.bar\nfooxbar\n")]);
    let hits = |q: &str, literal| search(&dir, q, literal);
    // A pattern.
    assert_eq!(hits("fo+.bar", false), [("b.txt".to_string(), 1), ("b.txt".to_string(), 2)]);
    assert_eq!(hits("^let ", false), [("a.txt".to_string(), 2)]);
    // "foo(" is no regex (an open group): it is the text, and finds the line that has it.
    assert_eq!(hits("foo(", false), [("a.txt".to_string(), 3)]);
    // Asked for as text, "o.b" is not a pattern: only the dot matches.
    assert_eq!(hits("o.b", true), [("b.txt".to_string(), 1)]);
    assert_eq!(hits("o.b", false).len(), 3, "as a pattern the dot is any character");
    // Nothing to search for.
    assert!(Grep::start(Scope::Tree(dir.clone()), "", false, || {}).is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn smart_case_matches_any_case_until_the_query_has_a_capital() {
    let dir = tree("g-case", &[("a.txt", "Needle\nneedle\nNEEDLE\n")]);
    assert_eq!(search(&dir, "needle", false).len(), 3);
    assert_eq!(search(&dir, "Needle", false), [("a.txt".to_string(), 1)]);
    assert_eq!(search(&dir, "NEEDLE", true), [("a.txt".to_string(), 3)]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ignored_hidden_binary_and_big_files_are_left_out() {
    let big = format!("needle\n{}", "x".repeat(grep::MAX_FILE as usize));
    let dir = tree(
        "g-skip",
        &[
            (".gitignore", "ignored/\n*.log\n"),
            ("seen.txt", "needle\n"),
            ("sub/deep.txt", "a needle\n"),
            ("ignored/x.txt", "needle\n"),
            ("noise.log", "needle\n"),
            (".hidden.txt", "needle\n"),
            (".hidden_dir/in.txt", "needle\n"),
            ("big.txt", &big),
            ("blob.bin", "needle\0binary\n"),
        ],
    );
    // A NUL late in the file counts too.
    std::fs::write(dir.join("late.bin"), format!("needle\n{}\0", "y".repeat(100_000))).unwrap();
    assert_eq!(search(&dir, "needle", false), [("seen.txt".to_string(), 1), ("sub/deep.txt".to_string(), 1)]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_search_stops_at_ten_thousand_matches_and_does_not_read_on() {
    // 100 files of 1000 matching lines: 100,000 matches.
    let line = "needle\n".repeat(1000);
    let dir = temp("g-cap");
    for i in 0..100 {
        std::fs::write(dir.join(format!("f{i:03}.txt")), &line).unwrap();
    }
    let grep = Grep::start(Scope::Tree(dir.clone()), "needle", false, || {}).unwrap();
    let (hits, capped) = found(&grep);
    assert_eq!(hits.len(), grep::MAX_MATCHES);
    assert!(capped);
    // (A few files were already being read when the limit came.)
    assert!(grep.searched.load(Ordering::Relaxed) < 40, "{} files searched", grep.searched.load(Ordering::Relaxed));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dropped_search_really_stops() {
    let dir = temp("g-cancel");
    for d in 0..30 {
        let sub = dir.join(format!("d{d:02}"));
        std::fs::create_dir_all(&sub).unwrap();
        for f in 0..100 {
            std::fs::write(sub.join(format!("f{f:03}.txt")), "nothing here\n".repeat(50)).unwrap();
        }
    }
    let grep = Grep::start(Scope::Tree(dir.clone()), "needle", false, || {}).unwrap();
    let searched = grep.searched.clone();
    // Dropped at once: the files that are left are not read.
    drop(grep);
    std::thread::sleep(Duration::from_millis(300));
    let then = searched.load(Ordering::Relaxed);
    assert!(then < 3000 / 2, "{then} of 3000 files searched");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(searched.load(Ordering::Relaxed), then, "it carried on");
    // The same search, left alone, reads them all.
    let grep = Grep::start(Scope::Tree(dir.clone()), "needle", false, || {}).unwrap();
    let all = grep.searched.clone();
    let (hits, _) = found(&grep);
    assert!(hits.is_empty() && all.load(Ordering::Relaxed) == 3000, "{} files searched", all.load(Ordering::Relaxed));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_search_by_extension_reads_only_those_files() {
    let dir = tree(
        "g-ext",
        &[
            ("rtl/core.sv", "needle\n"),
            ("rtl/old.v", "needle\n"),
            ("CLAUDE.md", "needle\n"),
            ("sv", "needle\n"),
            ("rtl/core.sv.bak", "needle\n"),
            (".hidden.sv", "needle\n"),
            ("skipped.sv", "needle\n"),
            (".ignore", "skipped.sv\n"),
        ],
    );
    let scope = Scope::Extensions { cwd: dir.clone(), extensions: vec!["sv".into(), "v".into()] };
    let (hits, _) = found(&Grep::start(scope, "needle", false, || {}).unwrap());
    let mut paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["rtl/core.sv", "rtl/old.v"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_search_of_listed_files_reads_only_those() {
    let dir = tree("g-files", &[("a.txt", "needle\n"), ("b.txt", "needle\n"), ("c.txt", "needle\n")]);
    let scope = Scope::Files { cwd: dir.clone(), files: vec![dir.join("a.txt"), dir.join("c.txt"), dir.join("gone.txt")] };
    let (hits, _) = found(&Grep::start(scope, "needle", false, || {}).unwrap());
    assert_eq!(hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(), ["a.txt", "c.txt"]);
    // A file outside the folder is shown by its path.
    let outside = tree("g-files-out", &[("o.txt", "needle\n")]);
    let scope = Scope::Files { cwd: dir.clone(), files: vec![outside.join("o.txt")] };
    let (hits, _) = found(&Grep::start(scope, "needle", false, || {}).unwrap());
    assert_eq!(hits[0].path, outside.join("o.txt").to_string_lossy());
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn a_line_that_is_not_utf8_is_read_lossily() {
    let dir = temp("g-utf8");
    std::fs::write(dir.join("a.txt"), b"ok line\nbad \xff\xfe bytes needle\ntail\n").unwrap();
    let (hits, _) = found(&Grep::start(Scope::Tree(dir.clone()), "needle", false, || {}).unwrap());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].text, "bad \u{fffd}\u{fffd} bytes needle");
    let r = hits[0].ranges[0].clone();
    assert_eq!(&hits[0].text[r], "needle");
    // The column is in the line the file has (the bytes before it), and
    // the whole-line range is in the lossy text, which is what the match was found in.
    assert!(hits[0].col > 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_hit_has_its_trimmed_line_the_matches_and_the_column() {
    let m = grep::matcher("ab", false).unwrap();
    let h = grep::hit("src/a.rs", 7, "    xx ab yy ab  \n", &m);
    assert_eq!((h.lnum, h.col, h.text.as_str()), (7, 8, "xx ab yy ab"));
    assert_eq!(h.ranges, [3..5, 9..11]);
    assert_eq!(h.line_ranges, [7..9, 13..15]);
    // Characters of more than a byte: the ranges are bytes of the text.
    let m = grep::matcher("שלום", false).unwrap();
    let h = grep::hit("a", 1, "x שלום y", &m);
    assert_eq!(&h.text[h.ranges[0].clone()], "שלום");
    assert_eq!(h.col, 3);
}

#[test]
fn a_long_line_is_cut_around_its_first_match() {
    let m = grep::matcher("needle", false).unwrap();
    let line = format!("{}needle{}", "a".repeat(5000), "b".repeat(5000));
    let h = grep::hit("a", 1, &line, &m);
    assert!(h.text.len() <= grep::CUT + 6, "{}", h.text.len());
    assert!(h.text.starts_with('…') && h.text.ends_with('…'));
    assert_eq!(h.ranges.len(), 1);
    assert_eq!(&h.text[h.ranges[0].clone()], "needle");
    // The whole line's range and column are where the match is in the file's line.
    assert_eq!((h.col, &line[h.line_ranges[0].clone()]), (5001, "needle"));
    // Multibyte text is cut at characters.
    let line = format!("{}needle{}", "ש".repeat(900), "ל".repeat(900));
    let h = grep::hit("a", 1, &line, &m);
    assert_eq!(&h.text[h.ranges[0].clone()], "needle");
    // A line that fits is not cut, and one that has its match first keeps the start.
    let h = grep::hit("a", 1, &format!("needle{}", "z".repeat(1000)), &m);
    assert!(h.text.starts_with("needle") && h.text.ends_with('…') && h.text.len() <= grep::CUT + 3);
    // Cutting to a budget of characters, as the row does for the room it has.
    let (text, ranges) = grep::cut_window("0123456789 needle tail of the line", &[11..17], 12, |_| 1);
    assert_eq!(text.chars().count(), 12 + 2);
    assert_eq!(&text[ranges[0].clone()], "needle");
}

#[test]
fn sorted_batches_merge_into_the_rows_and_say_where_the_old_ones_went() {
    fn key<'a>(r: &'a (&'static str, usize)) -> (&'a str, usize, usize) {
        (r.0, r.1, 0)
    }
    let (merged, went) = grep::merge_sorted(vec![("b", 1), ("b", 5), ("d", 2)], vec![("a", 9), ("b", 3), ("c", 1), ("e", 1)], key);
    assert_eq!(merged, [("a", 9), ("b", 1), ("b", 3), ("b", 5), ("c", 1), ("d", 2), ("e", 1)]);
    assert_eq!(went, [1, 3, 5]);
    let (merged, went) = grep::merge_sorted(Vec::<(&str, usize)>::new(), vec![("a", 1)], key);
    assert_eq!((merged, went), (vec![("a", 1)], vec![]));
}

// ---- The rows ----

fn grep_open(id: i64, dir: &std::path::Path, query: &str) -> PickerOpen {
    PickerOpen {
        id,
        title: "Grep".into(),
        source: "grep".into(),
        cwd: dir.to_string_lossy().into_owned(),
        multi: true,
        preview: true,
        query: query.into(),
        ..Default::default()
    }
}

fn hit(path: &str, lnum: usize) -> Hit {
    Hit { path: path.into(), lnum, col: 1, text: format!("{path}:{lnum}"), ..Default::default() }
}

fn rows_of(view: &PickerView) -> Vec<(String, usize)> {
    view.state.as_ref().unwrap().items.rows.iter().map(|r| (r.text.clone(), r.lnum.unwrap())).collect()
}

#[test]
fn rows_arrive_sorted_and_the_selection_stays_put_or_follows_its_match() {
    let ctx = egui::Context::default();
    let mut view = PickerView::default();
    view.open(&ctx, grep_open(1, std::path::Path::new("/w"), ""));
    fn st(v: &mut PickerView) -> &mut State {
        v.state.as_mut().unwrap()
    }

    // Batches arrive in any order; the rows are sorted by path then line.
    st(&mut view).add_hits(vec![hit("c.rs", 4), hit("c.rs", 2)]);
    st(&mut view).add_hits(vec![hit("a.rs", 9), hit("b.rs", 1)]);
    assert_eq!(rows_of(&view), [("a.rs".into(), 9), ("b.rs".into(), 1), ("c.rs".into(), 2), ("c.rs".into(), 4)]);

    // The user has not moved: the selection stays on the first row, whatever comes before it.
    st(&mut view).add_hits(vec![hit("0.rs", 1)]);
    assert_eq!(st(&mut view).selected, 0);
    assert_eq!(rows_of(&view)[0], ("0.rs".into(), 1));

    // Moved onto c.rs:2: rows that sort before it don't take the selection with them.
    st(&mut view).selected = 3;
    st(&mut view).moved = true;
    st(&mut view).mark(3);
    st(&mut view).add_hits(vec![hit("a.rs", 1), hit("a.rs", 2), hit("b.rs", 0)]);
    let at = st(&mut view).selected;
    assert_eq!(rows_of(&view)[at], ("c.rs".into(), 2));
    assert_eq!(st(&mut view).marked, [at], "a mark stays on its row too");
    // And one after it changes nothing.
    st(&mut view).add_hits(vec![hit("z.rs", 1)]);
    assert_eq!(rows_of(&view)[st(&mut view).selected], ("c.rs".into(), 2));
}

#[test]
fn a_new_search_replaces_the_rows_when_its_first_matches_come() {
    let ctx = egui::Context::default();
    let mut view = PickerView::default();
    view.open(&ctx, grep_open(1, std::path::Path::new("/w"), ""));
    let st = view.state.as_mut().unwrap();
    st.add_hits(vec![hit("a.rs", 1), hit("b.rs", 1)]);
    st.selected = 1;
    st.moved = true;
    // The next search's first batch replaces them, and the selection goes back to the top.
    st.grep.as_mut().unwrap().fresh = true;
    st.add_hits(vec![hit("x.rs", 3)]);
    assert_eq!(rows_of(&view), [("x.rs".into(), 3)]);
    let st = view.state.as_ref().unwrap();
    assert_eq!((st.selected, st.moved), (0, false));
}

// ---- The box ----

#[test]
fn the_preview_goes_where_it_gets_the_room() {
    use PreviewPlacement::*;
    let area = |w: f32, h: f32| Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
    let (row_h, cell) = (24.0, 8.0);
    let auto = |w, h| layout(area(w, h), true, row_h, cell, Auto);
    // Wide: beside the list, which has about 35%, with 100 columns or more, and tall.
    let wide = auto(2000.0, 1000.0);
    assert_eq!((wide.width, wide.preview_height), (WIDE, None));
    let inner = wide.width - FRAME;
    let preview = wide.preview_width.expect("a preview");
    assert!((wide.list_width / inner - 0.35).abs() < 0.02, "{wide:?}");
    assert!((wide.list_width + preview + style::GAP - inner).abs() < 1.0);
    assert!(preview >= 100.0 * cell, "{wide:?}");
    let height = wide.rows.expect("rows") as f32 * row_h;
    assert!((height - 1000.0 * 0.8).abs() < 3.0 * row_h, "{wide:?}");
    // Where beside wouldn't give it 100 columns: under, the same width as the list.
    let mid = auto(1200.0, 800.0);
    assert_eq!((mid.width, mid.list_width, mid.preview_width), (UNDER_WIDE, UNDER_WIDE - FRAME, Some(UNDER_WIDE - FRAME)));
    assert_eq!(mid.rows, Some(UNDER_ROWS));
    let below = mid.preview_height.expect("under");
    let total = 2.0 * row_h + UNDER_ROWS as f32 * row_h + style::GAP + below;
    assert!((total - 800.0 * 0.85).abs() < 1.0, "{mid:?}");
    assert!(below > UNDER_ROWS as f32 * row_h, "the source gets the larger part: {mid:?}");
    // The switch follows the font: a wider column needs a wider window.
    assert!(layout(area(1400.0, 800.0), true, row_h, cell, Auto).preview_height.is_none());
    assert!(layout(area(1400.0, 800.0), true, row_h, 12.0, Auto).preview_height.is_some());
    // A narrow window keeps the source, under; it needs height, not width.
    let narrow = auto(500.0, 700.0);
    assert_eq!(narrow.width, 500.0 - 2.0 * style::TAB_PAD);
    assert!(narrow.preview_height.is_some());
    // A shorter one gives the list less, but no fewer than the minimum; too short, no preview at all.
    assert!(auto(1000.0, 420.0).rows.unwrap() < UNDER_ROWS);
    assert_eq!(auto(1000.0, 200.0).preview_width, None);
    // A picker that didn't ask for one is the box it always was, however wide the window.
    let plain = layout(area(1500.0, 900.0), false, row_h, cell, Auto);
    assert_eq!((plain.width, plain.preview_width, plain.rows), (WIDTH, None, None));
    // Asked for by name, whatever the width. Beside keeps its minimum window and rows.
    assert!(layout(area(2000.0, 900.0), true, row_h, cell, Under).preview_height.is_some());
    let side = layout(area(900.0, 700.0), true, row_h, cell, Beside);
    assert_eq!((side.width, side.preview_height), (900.0 - 2.0 * style::TAB_PAD, None));
    assert_eq!(layout(area(650.0, 700.0), true, row_h, cell, Beside).preview_width, None);
    assert!(layout(area(1000.0, 200.0), true, row_h, cell, Beside).rows.unwrap() >= MIN_ROWS);
}

// ---- The preview pane ----

fn target(path: &str, lnum: usize) -> Option<Target> {
    Some(Target { key: Subject::Path(path.into()), lnum, title: path.into(), ranges: vec![] })
}

/// Ask for `path` at `lnum` and return the request's number.
fn pane_req(pane: &mut PreviewPane, path: &str, lnum: usize, t0: Instant, at_ms: u64) -> i64 {
    pane.set_target(target(path, lnum));
    pane.due(t0 + Duration::from_millis(at_ms));
    pane.due(t0 + Duration::from_millis(at_ms + 70)).request.expect("a request").req
}

fn answer(req: i64, path: &str, first: usize, count: usize, total: usize) -> PickerPreview {
    PickerPreview {
        req,
        path: path.into(),
        first_line: first,
        lines: (first..first + count).map(|n| format!("line {n}")).collect(),
        lnum: first,
        total,
        ..Default::default()
    }
}

#[test]
fn a_request_waits_for_the_selection_to_stop_moving() {
    let mut pane = PreviewPane::default();
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    pane.set_target(target("a", 10));
    let due = pane.due(t0);
    assert!(due.request.is_none() && due.wait == Some(pv::DEBOUNCE));
    assert!(pane.due(t0 + ms(30)).request.is_none());
    // It moves on at 40 ms: the wait starts over, and A is never asked for.
    pane.set_target(target("b", 20));
    assert!(pane.due(t0 + ms(40)).request.is_none());
    assert!(pane.due(t0 + ms(90)).request.is_none(), "50 ms after the last move");
    let due = pane.due(t0 + ms(101));
    let request = due.request.expect("asked once it has been still");
    assert_eq!((request.key.clone(), request.lnum), (Subject::Path("b".into()), 20));
    // Not asked twice while the answer is on its way.
    assert!(pane.due(t0 + ms(200)).request.is_none());
    assert!(pane.answer(answer(request.req, "b", 1, 100, 100)));
    // Nothing more to ask for: the line is in the window.
    assert_eq!(pane.due(t0 + ms(300)), pv::Due::default());
}

#[test]
fn an_answer_to_an_older_request_is_dropped() {
    let mut pane = PreviewPane::default();
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    pane.set_target(target("a", 10));
    pane.due(t0);
    let first = pane.due(t0 + ms(70)).request.unwrap();
    // Moved to B before A's answer came; B is asked for too.
    pane.set_target(target("b", 5));
    pane.due(t0 + ms(80));
    let second = pane.due(t0 + ms(150)).request.unwrap();
    assert!(second.req > first.req);
    assert!(!pane.answer(answer(first.req, "a", 1, 50, 50)), "A's answer arrives late");
    assert!(pane.window().is_none(), "and shows nothing");
    // Not even the right file, if it is not the current request.
    assert!(!pane.answer(answer(second.req + 5, "b", 1, 50, 50)));
    assert!(pane.answer(answer(second.req, "b", 1, 50, 50)));
    assert_eq!(pane.window().unwrap().0, 1);
    // The same answer twice: the second is no longer awaited.
    assert!(!pane.answer(answer(second.req, "b", 1, 50, 50)));
}

#[test]
fn moving_within_the_same_file_only_moves_and_outside_its_window_asks_again() {
    let mut pane = PreviewPane::default();
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    pane.set_target(target("a", 2000));
    pane.due(t0);
    let r = pane.due(t0 + ms(70)).request.unwrap();
    // A window of 1000 lines around 2000, in a file of 5000.
    assert!(pane.answer(answer(r.req, "a", 1500, 1000, 5000)));
    // Another line in the window: no request at any time.
    pane.set_target(target("a", 2300));
    assert_eq!(pane.due(t0 + ms(80)), pv::Due::default());
    assert_eq!(pane.due(t0 + ms(500)), pv::Due::default());
    // A line outside it: asked for, after the wait, centered on the new line.
    pane.set_target(target("a", 4000));
    assert!(pane.due(t0 + ms(600)).request.is_none());
    let again = pane.due(t0 + ms(670)).request.unwrap();
    assert_eq!((again.key.clone(), again.lnum), (Subject::Path("a".into()), 4000));
    // Too near the window's edge to show the surroundings counts as outside.
    assert!(pane.answer(answer(again.req, "a", 3500, 1000, 5000)));
    pane.set_target(target("a", 4490));
    pane.due(t0 + ms(700));
    assert!(pane.due(t0 + ms(770)).request.is_some());
    // The same near the window's start.
    let mut pane = PreviewPane::default();
    let req = pane_req(&mut pane, "a", 4000, t0, 0);
    assert!(pane.answer(answer(req, "a", 3500, 1000, 5000)));
    pane.set_target(target("a", 3510));
    pane.due(t0 + ms(1500));
    assert!(pane.due(t0 + ms(1570)).request.is_some(), "too near the window's start");
    // But the end of the file is the end: nothing more to show beyond it.
    let mut pane = PreviewPane::default();
    pane.set_target(target("e", 100));
    pane.due(t0);
    let r = pane.due(t0 + ms(70)).request.unwrap();
    assert!(pane.answer(answer(r.req, "e", 1, 100, 100)));
    pane.set_target(target("e", 99));
    assert_eq!(pane.due(t0 + ms(80)), pv::Due::default());
    // A note is the answer for the file whatever line is asked for.
    let mut pane = PreviewPane::default();
    pane.set_target(target("bin", 1));
    pane.due(t0);
    let r = pane.due(t0 + ms(70)).request.unwrap();
    assert!(pane.answer(PickerPreview { req: r.req, path: "bin".into(), note: "Binary file".into(), ..Default::default() }));
    pane.set_target(target("bin", 7));
    assert_eq!(pane.due(t0 + ms(80)), pv::Due::default());
}

#[test]
fn colors_are_applied_in_order_by_bytes_in_hebrew_and_other_wide_text() {
    let (red, green, blue) = (Color32::RED, Color32::GREEN, Color32::BLUE);
    let style = |c| Style { fg: Some(c), ..Default::default() };
    // "שלום" is 8 bytes (two each), then a space, then "foo".
    let text = "שלום foo";
    assert_eq!(text.len(), 12);
    let spans = [(0..8, style(blue)), (9..12, style(red)), (10..12, style(green))];
    let runs = pv::runs(text, &spans, &[]);
    let shown: Vec<(&str, Option<Color32>)> = runs.iter().map(|(r, s, _)| (&text[r.clone()], s.fg)).collect();
    // The later span wins over the earlier where they overlap.
    assert_eq!(shown, [("שלום", Some(blue)), (" ", None), ("f", Some(red)), ("oo", Some(green))]);
    // A span that ends inside a character belongs to the character it starts.
    let runs = pv::runs("éa", &[(0..1, style(red))], &[]);
    assert_eq!(runs.iter().map(|(r, s, _)| (r.clone(), s.fg)).collect::<Vec<_>>(), [(0..2, Some(red)), (2..3, None)]);
    // A span past the end of the line is clipped, not a panic.
    assert_eq!(pv::runs("ab", &[(1..99, style(red))], &[]).len(), 2);
    // Nor is one that ends before it starts, or a mark that does: they cover nothing.
    #[allow(clippy::reversed_empty_ranges)]
    let backwards = pv::runs("abc", &[(2..1, style(red)), (9..4, style(red))], &[3..0]);
    assert_eq!(backwards.iter().map(|(r, s, m)| (r.clone(), s.fg, *m)).collect::<Vec<_>>(), [(0..3, None, false)]);
    // The marked stretch is its own run, in the style under it.
    let runs = pv::runs("foo bar", &[(0..7, style(blue))], &[4..7]);
    assert_eq!(runs.iter().map(|(r, s, m)| (r.clone(), s.fg, *m)).collect::<Vec<_>>(), [(0..4, Some(blue), false), (4..7, Some(blue), true)]);
}

#[test]
fn a_line_becomes_a_layout_job_with_tabs_as_spaces_and_the_match_marked() {
    let (_, chrome, popup) = themes().into_iter().next().unwrap();
    let code = CodeTheme::new(chrome.bg, chrome.fg, popup.accent);
    let fonts = GridFonts::uninstalled(14.0);
    let look = PreviewLook { theme: &popup, code: &code, fonts: &fonts, metrics: CellMetrics { width: 8.0, height: 20.0 } };
    let kw = Style { fg: Some(Color32::RED), ..Default::default() };
    let job = pv::line_job("\tif שלום", &[(1..3, kw)], &[4..12], &look);
    let sections: Vec<(&str, Color32, bool)> =
        job.sections.iter().map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.color, s.format.background != Color32::TRANSPARENT)).collect();
    // The tab is four spaces; the keyword is red; the Hebrew word (bytes 4..12) is marked.
    assert_eq!(sections.iter().map(|s| s.0).collect::<String>(), "    if שלום");
    assert_eq!(sections[1], ("if", Color32::RED, false));
    assert!(sections.last().unwrap().2 && sections.last().unwrap().0 == "שלום");
    assert_eq!(sections.last().unwrap().1, code.search_fg);
}

// ---- Typing and keys, in the window ----

pub(super) fn wait_for_request(h: &mut Harness<'static, Env>) -> Action {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.run();
        let at = h.state().actions.iter().position(|a| matches!(a, Action::Preview { .. }));
        if let Some(at) = at {
            return h.state_mut().actions.remove(at);
        }
        assert!(Instant::now() < deadline, "no preview was asked for");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn five_files(name: &str) -> PathBuf {
    let long: String = (1..=300).map(|n| if n == 150 { "    let found = needle(); // שלום\n".to_string() } else { format!("// filler line {n}\n") }).collect();
    tree(
        name,
        &[
            ("a.rs", "fn one() {}\nlet x = needle;\n"),
            ("b/c.rs", "needle at the start\nand more\nneedle again\n"),
            ("long.rs", &long),
            (".git_ignored_not", "needle\n"),
            ("z.txt", "no match here\n"),
        ],
    )
}

fn open_answer(h: &mut Harness<'static, Env>, req: &pv::Request, first: usize, count: usize, total: usize, spans: Vec<PreviewSpan>, note: &str) {
    let path = match &req.key {
        Subject::Path(p) => p.clone(),
        Subject::Buf(_) => unreachable!(),
    };
    let lines: Vec<String> = (first..first + count)
        .map(|n| if n == 150 { "    let found = needle(); // שלום".to_string() } else { format!("// filler line {n}") })
        .collect();
    let a = PickerPreview { req: req.req, path, first_line: first, lines, spans, lnum: req.lnum, total, note: note.into(), ..Default::default() };
    assert!(h.state_mut().view.set_preview(a), "the answer was taken");
    h.run();
}

#[test]
fn typing_runs_a_search_and_rows_arrive_sorted_with_the_matches_found() {
    let dir = five_files("g-typing");
    let mut h = harness("dark", 1200.0, 700.0, grep_open(1, &dir, ""));
    settle(&mut h);
    // Nothing to search for yet: a hint, no rows.
    assert_eq!(h.state().view.listed(), Vec::<String>::new());
    type_text(&mut h, "needle");
    // Sorted by path then line (the order the threads finished in doesn't matter).
    assert_eq!(h.state().view.listed(), ["a.rs", "b/c.rs", "b/c.rs", "long.rs"]);
    let (_, count, total, selected) = h.state().view.summary().unwrap();
    assert_eq!((count, total, selected), (4, 4, 0));
    // The rows know their line and column, and the quickfix text.
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::Enter);
    let Some(Action::Accept { chosen, how, .. }) = accepted(&mut h).into_iter().find(|a| matches!(a, Action::Accept { .. })) else { panic!("no accept") };
    assert_eq!(how, "edit");
    assert_eq!(chosen.len(), 1);
    assert_eq!((chosen[0].lnum, chosen[0].col), (Some(3), Some(1)));
    assert_eq!(chosen[0].path.as_deref(), Some(dir.join("b/c.rs").to_str().unwrap()));
    assert_eq!(chosen[0].text.as_deref(), Some("needle again"));
    // Changing the query searches again; a query nothing matches says so.
    let mut h = harness("dark", 1200.0, 700.0, grep_open(2, &dir, ""));
    type_text(&mut h, "needle");
    type_text(&mut h, " at");
    assert_eq!(h.state().view.listed(), ["b/c.rs"]);
    type_text(&mut h, "xyz");
    assert_eq!(h.state().view.listed(), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tab_marks_matches_and_enter_sends_them_with_their_text() {
    let dir = five_files("g-marks");
    let mut h = harness("dark", 1200.0, 700.0, grep_open(1, &dir, "needle"));
    settle(&mut h);
    assert_eq!(h.state().view.listed().len(), 4, "a query it opened with is searched at once");
    press(&mut h, Modifiers::NONE, Key::Tab);
    press(&mut h, Modifiers::NONE, Key::Tab);
    press(&mut h, Modifiers::NONE, Key::Enter);
    let Some(Action::Accept { chosen, how, .. }) = accepted(&mut h).into_iter().find(|a| matches!(a, Action::Accept { .. })) else { panic!("no accept") };
    assert_eq!(how, "quickfix");
    let texts: Vec<_> = chosen.iter().map(|c| c.text.clone().unwrap()).collect();
    assert_eq!(texts, ["let x = needle;", "needle at the start"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_selected_match_is_asked_for_and_ctrl_f_pages_the_preview_only() {
    let dir = five_files("g-preview");
    let mut h = harness("dark", 1200.0, 700.0, grep_open(1, &dir, "needle\\(\\)"));
    settle(&mut h);
    // The one match, in long.rs at line 150.
    assert_eq!(h.state().view.listed(), ["long.rs"]);
    let Action::Preview { id, request } = wait_for_request(&mut h) else { unreachable!() };
    assert_eq!(id, 1);
    assert_eq!((request.key.clone(), request.lnum), (Subject::Path(dir.join("long.rs").to_string_lossy().into_owned()), 150));
    open_answer(&mut h, &request, 1, 300, 300, vec![], "");
    h.run();
    let rows = h.state().view.preview_rows();
    assert!(rows > 8.0, "{rows}");
    // The target line a third of the way down.
    let top = h.state().view.preview_top().unwrap();
    assert_eq!(top, 150 - (rows / 3.0).floor() as usize);
    // Ctrl-F: a page on, less one line; the query and the selection stay.
    let before = h.state().view.summary().map(|(q, c, t, s)| (q.to_owned(), c, t, s)).unwrap();
    press(&mut h, Modifiers::CTRL, Key::F);
    assert_eq!(h.state().view.preview_top().unwrap(), top + rows as usize - 1);
    press(&mut h, Modifiers::CTRL, Key::B);
    press(&mut h, Modifiers::CTRL, Key::B);
    // Ctrl-B twice: back where it was, then a page (less one line) before that.
    assert_eq!(h.state().view.preview_top().unwrap(), top.saturating_sub(rows as usize - 1));
    let after = h.state().view.summary().map(|(q, c, t, s)| (q.to_owned(), c, t, s)).unwrap();
    assert_eq!(before, after);
    assert!(accepted(&mut h).iter().all(|a| !matches!(a, Action::Accept { .. } | Action::Cancel(_))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_wheel_scrolls_the_preview_when_the_pointer_is_over_it() {
    let dir = five_files("g-wheel");
    let mut h = harness("dark", 1200.0, 700.0, grep_open(1, &dir, "needle\\(\\)"));
    settle(&mut h);
    let Action::Preview { request, .. } = wait_for_request(&mut h) else { unreachable!() };
    open_answer(&mut h, &request, 1, 300, 300, vec![], "");
    let top = h.state().view.preview_top().unwrap();
    let rect = h.state().view.rect().unwrap();
    let over = Pos2::new(rect.max.x - 80.0, rect.center().y);
    h.input_mut().events.push(Event::PointerMoved(over));
    h.run();
    h.input_mut().events.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, -100.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown });
    for _ in 0..10 {
        h.step();
    }
    let scrolled = h.state().view.preview_top().unwrap();
    assert!(scrolled > top + 2, "{top} -> {scrolled}");
    // Over the list, the wheel does not move the preview.
    let list = Pos2::new(rect.min.x + 100.0, rect.min.y + 60.0);
    h.input_mut().events.push(Event::PointerMoved(list));
    h.run();
    h.input_mut().events.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, -100.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown });
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(h.state().view.preview_top().unwrap(), scrolled);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_window_too_short_has_no_preview_and_asks_for_none() {
    let dir = five_files("g-narrow");
    let mut h = harness("dark", 600.0, 200.0, grep_open(1, &dir, "needle"));
    settle(&mut h);
    std::thread::sleep(Duration::from_millis(150));
    h.run();
    assert_eq!(h.state().view.listed().len(), 4);
    assert!(!h.state().actions.iter().any(|a| matches!(a, Action::Preview { .. })));
    // Ctrl-F isn't taken for a preview that is not there.
    assert!(h.state().view.preview_top().is_some_and(|t| t == 1));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Pictures ----

pub(super) const CODE: &str = "use std::fmt;\n\n/// A thing.\nfn needle(count: usize) -> String {\n    let found = format!(\"{count} needles\"); // שלום\n    found\n}\n";

pub(super) fn code_answer(h: &mut Harness<'static, Env>, request: &pv::Request, theme_colors: [Color32; 3]) {
    let [kw, string, comment] = theme_colors;
    let c = |color: Color32| Some(((color.r() as u32) << 16) | ((color.g() as u32) << 8) | color.b() as u32);
    let span = |line, start, end, fg: Option<u32>| PreviewSpan { line, start, end, fg, bold: false, italic: false };
    let lines: Vec<String> = CODE.lines().map(str::to_owned).collect();
    let at = |line: usize, what: &str| {
        let start = lines[line].find(what).unwrap();
        (start, start + what.len())
    };
    let (string_a, string_b) = at(4, "\"{count} needles\"");
    let (comment_a, comment_b) = at(4, "// שלום");
    let spans = vec![
        span(0, 0, 3, c(kw)),
        span(2, 0, 10, c(comment)),
        span(3, 0, 2, c(kw)),
        span(4, 4, 7, c(kw)),
        span(4, string_a, string_b, c(string)),
        span(4, comment_a, comment_b, c(comment)),
    ];
    let (path, buf) = match &request.key {
        Subject::Path(p) => (p.clone(), None),
        Subject::Buf(b) => (String::new(), Some(*b)),
    };
    let a = PickerPreview { req: request.req, path, buf, first_line: 1, lines, spans, lnum: request.lnum, total: lines_len(), ..Default::default() };
    assert!(h.state_mut().view.set_preview(a));
    h.run();
}

fn lines_len() -> usize {
    CODE.lines().count()
}

fn grep_snapshot(name: &str, width: f32, height: f32, note: Option<&str>) {
    let dir = tree(
        &format!("g-snap-{name}"),
        &[("src/lib.rs", CODE), ("src/other/deep_module.rs", "let needle = 1;\n"), ("docs/guide.md", "Use the needle tool.\n"), ("Cargo.toml", "name = \"needle\"\n")],
    );
    let mut results = egui_kittest::SnapshotResults::new();
    for (theme, ..) in themes() {
        let mut h = harness(theme, width, height, grep_open(1, &dir, "needle"));
        settle(&mut h);
        {
            // The selected row is the first, Cargo.toml: go to src/lib.rs. (Asked for
            // since, and answered: not what the preview wants now.)
            accepted(&mut h);
            press(&mut h, Modifiers::NONE, Key::ArrowDown);
            press(&mut h, Modifiers::NONE, Key::ArrowDown);
            press(&mut h, Modifiers::NONE, Key::ArrowDown);
            let Action::Preview { request, .. } = wait_for_request(&mut h) else { unreachable!() };
            match note {
                Some(note) => {
                    let path = match &request.key {
                        Subject::Path(p) => p.clone(),
                        Subject::Buf(_) => unreachable!(),
                    };
                    let a = PickerPreview { req: request.req, path, note: note.into(), ..Default::default() };
                    assert!(h.state_mut().view.set_preview(a));
                }
                None => {
                    let dark = theme == "dark";
                    let colors = if dark {
                        [Color32::from_rgb(0xc6, 0x78, 0xdd), Color32::from_rgb(0x98, 0xc3, 0x79), Color32::from_rgb(0x5c, 0x63, 0x70)]
                    } else {
                        [Color32::from_rgb(0xa6, 0x26, 0xa4), Color32::from_rgb(0x50, 0xa1, 0x4f), Color32::from_rgb(0xa0, 0xa1, 0xa7)]
                    };
                    code_answer(&mut h, &request, colors);
                }
            }
        }
        h.run();
        h.snapshot(format!("picker_{name}_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

/// A window wide enough for the source to go beside the list.
#[test]
fn grep_with_the_preview_beside_the_list() {
    grep_snapshot("grep_beside", 1800.0, 900.0, None);
}

#[test]
fn grep_with_a_preview_of_colored_code() {
    grep_snapshot("grep_preview", 1100.0, 480.0, None);
}

#[test]
fn grep_with_a_preview_that_has_only_a_note() {
    grep_snapshot("grep_note", 1100.0, 480.0, Some("Binary file"));
}

#[test]
fn grep_in_a_narrow_window_has_the_preview_under_the_list() {
    grep_snapshot("grep_narrow", 560.0, 400.0, None);
}

// ---- Speed (run by hand: `cargo test --release -- --ignored --nocapture a_big_tree_is_grepped`) ----

#[test]
#[ignore]
fn a_big_tree_is_grepped_fast_enough_to_type_into() {
    let dir = temp("g-big");
    let mut made = 0;
    for d in 0..100 {
        for s in 0..10 {
            let folder = dir.join(format!("pkg{d:03}/sub{s}"));
            std::fs::create_dir_all(&folder).unwrap();
            for f in 0..100 {
                let text: String = (0..60).map(|l| format!("fn item_{d}_{s}_{f}_{l}() {{ let value = {l}; helper(value); }}\n")).collect();
                std::fs::write(folder.join(format!("file_{f:03}.rs")), text).unwrap();
                made += 1;
            }
        }
    }
    eprintln!("{made} files of 60 lines");
    for query in ["helper", "item_7_3_", "zzz_nothing", "value = 59", "fn item_\\d+_9_99_0"] {
        let started = Instant::now();
        let grep = Grep::start(Scope::Tree(dir.clone()), query, false, || {}).unwrap();
        let (mut first, mut n) = (None, 0);
        loop {
            match grep.found.recv_timeout(Duration::from_secs(60)) {
                Ok(Msg::Hits(h)) => {
                    first.get_or_insert(started.elapsed());
                    n += h.len();
                }
                Ok(Msg::Done { capped }) => {
                    eprintln!("{query:?}: {n} matches (capped: {capped}), first after {first:?}, all after {:?}, {} files read", started.elapsed(), grep.searched.load(Ordering::Relaxed));
                    break;
                }
                Err(_) => break,
            }
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}
