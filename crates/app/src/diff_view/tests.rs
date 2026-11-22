use std::time::{Duration, Instant};

use egui::{Key, Modifiers};

use super::testing::Repo;
use super::*;

const NONE: Modifiers = Modifiers::NONE;
const SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::NONE };

/// `n` files `f0.txt` … of 30 lines each, committed, then those in `changed`
/// with their line 15 (and for `big`, 10 lines more) changed on disk.
fn repo(name: &str, n: usize, changed: &[usize]) -> Repo {
    let repo = Repo::new(name);
    for i in 0..n {
        let text: String = (1..=30).map(|l| format!("file {i} line {l}\n")).collect();
        repo.write(&format!("f{i}.txt"), &text);
    }
    repo.commit("first");
    for &i in changed {
        let text: String = (1..=30).map(|l| if l == 15 { format!("file {i} CHANGED\n") } else { format!("file {i} line {l}\n") }).collect();
        repo.write(&format!("f{i}.txt"), &text);
    }
    repo
}

fn spec(i: usize) -> FileSpec {
    FileSpec { path: format!("f{i}.txt"), old_path: None, status: Status::Modified, left: Side::Ref("HEAD".into()), right: Side::Disk }
}

fn request(repo: &Repo, n: usize, focus: Option<&str>) -> Request {
    Request {
        top: repo.dir.clone(),
        compared: "HEAD → working tree".into(),
        files: Files::List((0..n).map(spec).collect()),
        focus: focus.map(str::to_owned),
        mode: Mode::ReadOnly,
    }
}

fn view(repo: &Repo, n: usize) -> DiffView {
    DiffView::new(request(repo, n, None), Options::default(), || {})
}

/// Run the view's frames until every section it wants now is read.
fn settle(v: &mut DiffView) {
    let start = Instant::now();
    loop {
        v.poll();
        v.tick(Instant::now());
        let waiting = v.near().into_iter().any(|i| v.sections[i].loaded.is_none() || v.sections[i].requested);
        if !waiting && v.jump_idle() {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "the files didn't load");
        std::thread::sleep(Duration::from_millis(5));
    }
}

impl DiffView {
    fn jump_idle(&self) -> bool {
        self.jump.is_none() && self.find.as_ref().is_none_or(|f| !f.waiting())
    }

    fn at(&self) -> (usize, Option<usize>) {
        (self.cursor.section, self.cursor.row)
    }
}

#[test]
fn only_the_files_that_are_near_are_read() {
    let r = repo("lazy", 200, &[]);
    let mut v = view(&r, 200);
    assert_eq!(v.sections.len(), 200);
    v.viewport_h = 800.0;
    settle(&mut v);
    let (loaded, total) = v.loaded();
    assert_eq!(total, 200);
    // An unchanged file is a header and a message row: 28 + 20 + 10 apart, so
    // 3 screens of 800 hold about 41 of them, not 200.
    assert!((1..60).contains(&loaded), "{loaded} files loaded at the start");
    assert!(v.sections[0].loaded.is_some() && v.sections[199].loaded.is_none());
    // Scrolling to the end reads the last ones.
    v.go_to_bottom();
    settle(&mut v);
    assert!(v.sections[199].loaded.is_some());
    assert!(v.loaded().0 < 120);
}

#[test]
fn jumping_to_a_far_file_keeps_it_at_the_top_while_its_neighbours_are_read() {
    let r = repo("far", 200, &[150]);
    let mut v = view(&r, 200);
    v.viewport_h = 600.0;
    settle(&mut v);
    v.go_to_file(150);
    settle(&mut v);
    assert!(v.pin.is_none());
    assert_eq!(v.scroll, v.tops()[150].min(v.max_scroll()));
    assert!(v.sections[150].loaded.is_some() && v.sections[0].loaded.is_some());
    assert!(v.sections[100].loaded.is_none());
    // The changed file is bigger than an unchanged one, and the header is still at the top.
    assert_eq!(v.sections[150].counts(), Some((1, 1)));
    assert!(v.scroll <= v.tops()[150]);
    // The wheel lets go of it.
    v.go_to_file(60);
    assert!(v.pin.is_some());
    v.scroll_by(5.0);
    assert!(v.pin.is_none());
}

#[test]
fn the_viewport_decides_what_is_near() {
    let r = repo("near", 40, &[]);
    let mut v = view(&r, 40);
    v.viewport_h = 400.0;
    let near = v.near();
    assert_eq!(near[0], 0);
    assert!(near.len() < 25 && near.len() > 3, "{near:?}");
    v.scroll = v.tops()[30];
    let near = v.near();
    assert!(near.contains(&30) && !near.contains(&0), "{near:?}");
}

#[test]
fn a_new_view_starts_at_the_focused_file() {
    let r = repo("focus", 5, &[]);
    let mut v = DiffView::new(request(&r, 5, Some("f3.txt")), Options::default(), || {});
    assert_eq!(v.cursor, Cursor { section: 3, row: None });
    // (Before the files are read the view is short: the file stays at the top.)
    v.viewport_h = 150.0;
    settle(&mut v);
    assert_eq!(v.scroll, v.tops()[3].min(v.max_scroll()));
    assert!(v.pin.is_none());
    // Not moved by the files near it being read: its header is where the view is.
    assert!(v.scroll > 0.0, "{}", v.scroll);
}

#[test]
fn j_and_k_walk_headers_and_rows_across_files() {
    let r = repo("walk", 3, &[0, 1, 2]);
    let mut v = view(&r, 3);
    settle(&mut v);
    assert_eq!(v.at(), (0, None));
    v.key(Key::J, NONE);
    assert_eq!(v.at(), (0, Some(0)));
    // The rows of file 0: 3 context, removed, added, 3 context, and the
    // folds before and after: 1 + 3 + 2 + 3 + 1.
    let rows = v.sections[0].cursor_rows();
    assert_eq!(rows, 10);
    for _ in 0..rows {
        v.key(Key::ArrowDown, NONE);
    }
    assert_eq!(v.at(), (1, None), "the next file's header");
    v.key(Key::K, NONE);
    assert_eq!(v.at(), (0, Some(rows - 1)));
    // Not past either end.
    v.key(Key::Home, NONE);
    v.key(Key::K, NONE);
    assert_eq!(v.at(), (0, None));
    v.key(Key::End, NONE);
    let last = v.at();
    v.key(Key::J, NONE);
    assert_eq!(v.at(), last);
    assert_eq!(last.0, 2);
}

#[test]
fn next_and_previous_change_cross_files() {
    let r = repo("changes", 3, &[0, 2]);
    let mut v = view(&r, 3);
    settle(&mut v);
    // Where the removed line is: after the fold and 3 lines of context.
    v.key(Key::CloseBracket, NONE);
    v.key(Key::C, NONE);
    assert_eq!(v.at(), (0, Some(4)));
    // File 1 has no change (nothing to stop at but its header), then file 2's.
    v.key(Key::N, NONE);
    assert_eq!(v.at(), (1, None));
    v.key(Key::N, NONE);
    assert_eq!(v.at(), (2, Some(4)));
    // The last one: it stays.
    v.key(Key::N, NONE);
    assert_eq!(v.at(), (2, Some(4)));
    // Back: [c and N.
    v.key(Key::OpenBracket, NONE);
    v.key(Key::C, NONE);
    assert_eq!(v.at(), (1, None));
    v.key(Key::N, SHIFT);
    assert_eq!(v.at(), (0, Some(4)));
    v.key(Key::N, SHIFT);
    assert_eq!(v.at(), (0, Some(4)));
    // The view scrolled to show the change it went to.
    v.key(Key::End, NONE);
    v.key(Key::N, SHIFT);
    let (y, _) = v.y_of(v.cursor);
    assert!(y >= v.scroll && y <= v.scroll + v.viewport_h);
}

#[test]
fn a_change_in_a_file_not_read_yet_is_waited_for() {
    let r = repo("wait", 80, &[70]);
    let mut v = view(&r, 80);
    settle(&mut v);
    assert!(v.sections[70].loaded.is_none());
    // Files 1.. have no change: each is a stop (its header), so walk by
    // files until the next one is a section not read.
    for _ in 0..80 {
        let before = v.at();
        v.key(Key::N, NONE);
        if v.jump.is_some() || v.at() == before {
            break;
        }
    }
    assert!(v.jump.is_some(), "a jump waits for the file");
    settle(&mut v);
    assert!(v.jump.is_none());
    assert!(v.cursor.section > 0);
}

#[test]
fn files_are_jumped_with_f() {
    let r = repo("files", 4, &[0, 1, 2, 3]);
    let mut v = view(&r, 4);
    settle(&mut v);
    v.key(Key::J, NONE);
    v.key(Key::J, NONE);
    v.key(Key::CloseBracket, NONE);
    v.key(Key::F, NONE);
    assert_eq!(v.at(), (1, None));
    assert_eq!(v.scroll, v.tops()[1].min(v.max_scroll()));
    v.key(Key::CloseBracket, NONE);
    v.key(Key::F, NONE);
    assert_eq!(v.at(), (2, None));
    // [f on a header: the one before; on a row: this file's own.
    v.key(Key::OpenBracket, NONE);
    v.key(Key::F, NONE);
    assert_eq!(v.at(), (1, None));
    v.key(Key::J, NONE);
    v.key(Key::OpenBracket, NONE);
    v.key(Key::F, NONE);
    assert_eq!(v.at(), (1, None));
    // Other keys after a "]" are not the pair.
    v.key(Key::CloseBracket, NONE);
    v.key(Key::J, NONE);
    assert_eq!(v.at(), (1, None));
}

#[test]
fn za_and_enter_fold_a_section() {
    let r = repo("fold", 2, &[0, 1]);
    let mut v = view(&r, 2);
    settle(&mut v);
    let open = v.section_height(0);
    v.key(Key::Z, NONE);
    v.key(Key::A, NONE);
    assert!(v.sections[0].folded);
    assert_eq!(v.section_height(0), v.geom.header_h);
    assert!(v.section_height(0) < open);
    // Folded: the cursor can't be on its rows.
    v.key(Key::J, NONE);
    assert_eq!(v.at(), (1, None));
    v.key(Key::K, NONE);
    assert_eq!(v.at(), (0, None));
    // Enter on the header opens it again.
    assert_eq!(v.key(Key::Enter, NONE), None);
    assert!(!v.sections[0].folded);
    assert_eq!(v.section_height(0), open);
}

#[test]
fn enter_on_a_fold_opens_it_and_on_a_line_opens_the_file() {
    let r = repo("enter", 1, &[0]);
    let mut v = view(&r, 1);
    settle(&mut v);
    // Row 0 is the fold of the 11 lines before the context.
    v.key(Key::J, NONE);
    assert_eq!(v.sections[0].cursor_rows(), 10);
    assert_eq!(v.key(Key::Enter, NONE), None);
    assert_eq!(v.sections[0].cursor_rows(), 10 - 1 + 11, "the fold's 11 lines are rows");
    // The cursor is on the first of them: line 1.
    assert_eq!(v.key(Key::Enter, NONE), Some(Action::Open { path: r.dir.join("f0.txt"), line: 1 }));
    // On the added line: its line in the new file, 15.
    v.cursor.row = Some(v.sections[0].rows.as_ref().map(|(.., rows)| match rows {
        Rows::Unified(r) => r.iter().position(|x| matches!(x, Row::Line(l) if l.kind == Kind::Added)).unwrap(),
        Rows::Split(_) => unreachable!(),
    }).unwrap());
    assert_eq!(v.key(Key::Enter, NONE), Some(Action::Open { path: r.dir.join("f0.txt"), line: 15 }));
    // On a removed line: where it was in the new file.
    v.key(Key::K, NONE);
    assert_eq!(v.key(Key::Enter, NONE), Some(Action::Open { path: r.dir.join("f0.txt"), line: 15 }));
}

#[test]
fn a_deleted_file_has_nothing_to_open() {
    let r = repo("deleted", 1, &[]);
    std::fs::remove_file(r.dir.join("f0.txt")).unwrap();
    let mut v = view(&r, 1);
    settle(&mut v);
    assert_eq!(v.sections[0].status(), Status::Deleted);
    assert_eq!(v.sections[0].counts(), Some((0, 30)));
    v.key(Key::J, NONE);
    assert_eq!(v.key(Key::Enter, NONE), None);
}

#[test]
fn q_and_escape_close_and_s_toggles_the_layout() {
    let r = repo("close", 1, &[0]);
    let mut v = view(&r, 1);
    settle(&mut v);
    assert_eq!(v.key(Key::Q, NONE), Some(Action::Close));
    assert_eq!(v.key(Key::Escape, NONE), Some(Action::Close));
    assert_eq!(v.layout, Layout::Unified);
    v.key(Key::S, NONE);
    assert_eq!(v.layout, Layout::Split);
    v.key(Key::S, NONE);
    assert_eq!(v.layout, Layout::Unified);
}

#[test]
fn the_cursor_stays_on_its_line_when_the_layout_changes() {
    let r = repo("layout", 1, &[0]);
    let mut v = view(&r, 1);
    settle(&mut v);
    // On the added line (unified row 6: fold, 3 context, removed, added).
    v.cursor = Cursor { section: 0, row: Some(5) };
    v.key(Key::S, NONE);
    assert_eq!(v.layout, Layout::Split);
    // Split: the fold, 3 context, then the changed pair.
    assert_eq!(v.cursor, Cursor { section: 0, row: Some(4) });
    v.key(Key::S, NONE);
    // Back on a changed line (the pair holds both, the removed one comes first).
    let Rows::Unified(rows) = &v.sections[0].rows.as_ref().unwrap().2 else { panic!() };
    assert!(matches!(rows[v.cursor.row.unwrap()], Row::Line(l) if l.kind != Kind::Context));
}

#[test]
fn the_cursor_and_the_rows_are_made_again_for_another_layout() {
    let r = repo("rows", 1, &[0]);
    let mut v = view(&r, 1);
    settle(&mut v);
    assert!(matches!(v.sections[0].rows, Some((Layout::Unified, 3, Rows::Unified(_)))));
    v.layout = Layout::Split;
    v.prepare();
    let Some((Layout::Split, 3, Rows::Split(rows))) = &v.sections[0].rows else { panic!() };
    // Fold, 3 context, the pair, 3 context, fold.
    assert_eq!(rows.len(), 9);
    v.context = 1;
    v.prepare();
    assert_eq!(v.sections[0].rows.as_ref().unwrap().2.len(), 5);
}

#[test]
fn copying_gives_the_lines_without_the_signs() {
    let r = repo("copy", 1, &[0]);
    let mut v = view(&r, 1);
    settle(&mut v);
    // From the removed line to the added one.
    v.cursor = Cursor { section: 0, row: Some(4) };
    v.key(Key::J, SHIFT);
    assert_eq!(v.anchor, Some(Cursor { section: 0, row: Some(4) }));
    assert_eq!(v.selected_text().as_deref(), Some("file 0 line 15\nfile 0 CHANGED"));
    // Without a selection: the cursor's line. A fold has no text.
    v.key(Key::J, NONE);
    assert_eq!(v.anchor, None);
    assert_eq!(v.selected_text().as_deref(), Some("file 0 line 16"));
    v.cursor = Cursor { section: 0, row: Some(0) };
    assert_eq!(v.selected_text(), None);
    // The header alone has none either.
    v.cursor = Cursor { section: 0, row: None };
    assert_eq!(v.selected_text(), None);
    // Side by side: the new side's text (the old side's where there is none).
    v.key(Key::S, NONE);
    v.cursor = Cursor { section: 0, row: Some(4) };
    assert_eq!(v.selected_text().as_deref(), Some("file 0 CHANGED"));
}

#[test]
fn a_change_on_disk_is_diffed_again_after_a_moment() {
    let r = repo("rediff", 2, &[0]);
    let mut v = view(&r, 2);
    settle(&mut v);
    assert_eq!(v.sections[0].counts(), Some((1, 1)));
    // An agent writes two more lines.
    let text: String = (1..=30).map(|l| if (15..=17).contains(&l) { format!("agent {l}\n") } else { format!("file 0 line {l}\n") }).collect();
    r.write("f0.txt", &text);
    let t0 = Instant::now();
    v.on_disk_changed(t0);
    // Not at once: more writes may follow.
    let wait = v.tick(t0);
    assert!(wait.is_some_and(|w| w <= Duration::from_millis(300)));
    std::thread::sleep(Duration::from_millis(50));
    v.poll();
    assert_eq!(v.sections[0].counts(), Some((1, 1)), "still the old diff");
    // After it: read again, on the worker.
    assert_eq!(v.tick(t0 + Duration::from_millis(400)), None);
    settle(&mut v);
    assert_eq!(v.sections[0].counts(), Some((3, 3)));
    // Files that don't read the disk aren't touched.
    let committed = FileSpec { left: Side::Ref("HEAD".into()), right: Side::Ref("HEAD".into()), ..spec(1) };
    assert!(!committed.reads_disk() && spec(0).reads_disk());
}

#[test]
fn the_cursor_keeps_to_rows_there_are_after_a_diff_again() {
    let r = repo("clamp", 1, &[0]);
    let mut v = view(&r, 1);
    settle(&mut v);
    v.cursor = Cursor { section: 0, row: Some(9) };
    // The file goes back to HEAD: nothing to see.
    r.git(&["checkout", "--", "f0.txt"]);
    v.on_disk_changed(Instant::now());
    v.tick(Instant::now() + Duration::from_secs(1));
    settle(&mut v);
    assert_eq!(v.cursor, Cursor { section: 0, row: None });
}

#[test]
fn totals_count_the_files_read() {
    let r = repo("totals", 3, &[0, 1]);
    let mut v = view(&r, 3);
    assert_eq!(v.totals(), (0, 0));
    settle(&mut v);
    assert_eq!(v.totals(), (2, 2));
    assert_eq!(v.loaded(), (3, 3));
}

#[test]
fn the_wheel_and_pages_scroll_within_bounds() {
    let r = repo("scroll", 6, &[0, 1, 2, 3, 4, 5]);
    let mut v = view(&r, 6);
    v.viewport_h = 300.0;
    settle(&mut v);
    v.scroll_by(-50.0);
    assert_eq!(v.scroll, 0.0);
    v.scroll_by(120.0);
    assert_eq!(v.scroll, 120.0);
    v.scroll_by(1e6);
    assert_eq!(v.scroll, v.max_scroll());
    assert!(v.max_scroll() > 0.0);
    v.go_to_top();
    // A page down moves the cursor by the rows that fit, less two.
    v.key(Key::PageDown, NONE);
    assert_eq!(v.page(), 13);
    assert_eq!(v.at(), (1, Some(1)));
    v.key(Key::PageUp, NONE);
    assert_eq!(v.at(), (0, None));
    // `G` and `gg`.
    v.key(Key::G, SHIFT);
    assert_eq!(v.cursor.section, 5);
    assert_eq!(v.scroll, v.max_scroll());
    v.key(Key::G, NONE);
    v.key(Key::G, NONE);
    assert_eq!((v.at(), v.scroll), ((0, None), 0.0));
}

#[test]
fn a_click_is_found_by_its_y() {
    let r = repo("locate", 2, &[0, 1]);
    let mut v = view(&r, 2);
    settle(&mut v);
    let tops = v.tops();
    assert_eq!(v.locate(tops[0] + 5.0), Some(Cursor { section: 0, row: None }));
    assert_eq!(v.locate(tops[0] + v.geom.header_h + 5.0), Some(Cursor { section: 0, row: Some(0) }));
    assert_eq!(v.locate(tops[0] + v.geom.header_h + v.geom.row_h * 3.5), Some(Cursor { section: 0, row: Some(3) }));
    assert_eq!(v.locate(tops[1] + 1.0), Some(Cursor { section: 1, row: None }));
    // The gap between sections is nobody's row.
    assert_eq!(v.locate(tops[0] + v.section_height(0) + 2.0), None);
}

#[test]
fn a_commits_files_are_listed_by_the_worker() {
    let r = repo("commit", 3, &[]);
    r.write("f1.txt", "changed\n");
    r.write("new.txt", "x\n");
    let oid = r.commit("second");
    let parent = r.git(&["rev-parse", "HEAD^"]);
    let commit = crate::git::Commit {
        oid,
        parents: vec![parent],
        author: String::new(),
        when: String::new(),
        date: String::new(),
        subject: String::new(),
        body: String::new(),
        refs: vec![],
    };
    let mut v = DiffView::new(
        Request { top: r.dir.clone(), compared: "abc → def".into(), files: Files::Commit(commit), focus: None, mode: Mode::ReadOnly },
        Options::default(),
        || {},
    );
    assert!(v.awaiting_files() && v.sections.is_empty());
    let start = Instant::now();
    while v.awaiting_files() {
        v.poll();
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(v.sections.len(), 2);
    settle(&mut v);
    let new = v.sections.iter().find(|s| s.spec.path == "new.txt").unwrap();
    assert_eq!(new.status(), Status::Added);
    let changed = v.sections.iter().find(|s| s.spec.path == "f1.txt").unwrap();
    assert_eq!(changed.counts(), Some((1, 30)));
}

#[test]
fn a_too_large_file_says_so_and_a_binary_one_too() {
    let r = repo("kinds", 1, &[]);
    std::fs::write(r.dir.join("blob.bin"), [0u8, 1, 2]).unwrap();
    r.write("big.txt", &"x\n".repeat(60));
    let files = vec![
        FileSpec { path: "blob.bin".into(), status: Status::Added, left: Side::Missing, ..spec(0) },
        FileSpec { path: "big.txt".into(), status: Status::Added, left: Side::Missing, ..spec(0) },
    ];
    let mut v = DiffView::new(
        Request { top: r.dir.clone(), compared: "x".into(), files: Files::List(files), focus: None, mode: Mode::ReadOnly },
        Options { max_lines: 50, ..Options::default() },
        || {},
    );
    settle(&mut v);
    assert!(matches!(v.sections[0].loaded.as_ref().unwrap().body, model::Body::Binary));
    assert!(matches!(v.sections[1].loaded.as_ref().unwrap().body, model::Body::TooLarge { lines: 60 }));
    // (The large one has a row for its button under the message.)
    assert_eq!(v.sections[1].body_rows(), 2);
    // Neither has rows to walk: each is a stop by its header.
    assert_eq!(v.sections[1].cursor_rows(), 0);
    v.key(Key::N, NONE);
    assert_eq!(v.at(), (1, None));
    // Enter on the header of the large one folds it (the button is the mouse's).
    v.key(Key::Enter, NONE);
    assert!(v.sections[1].folded);
}

#[test]
fn a_renamed_file_shows_both_names() {
    let r = Repo::new("renamed");
    r.write("old.txt", "a\nb\nc\n");
    r.commit("first");
    r.git(&["mv", "old.txt", "new.txt"]);
    let s = FileSpec { path: "new.txt".into(), old_path: Some("old.txt".into()), status: Status::Renamed, left: Side::Ref("HEAD".into()), right: Side::Disk };
    let mut v = DiffView::new(
        Request { top: r.dir.clone(), compared: "x".into(), files: Files::List(vec![s]), focus: None, mode: Mode::ReadOnly },
        Options::default(),
        || {},
    );
    settle(&mut v);
    assert_eq!(v.sections[0].spec.title(), "old.txt → new.txt");
    assert_eq!(v.sections[0].status(), Status::Renamed);
    // A pure rename: no lines changed, nothing to walk.
    assert_eq!(v.sections[0].cursor_rows(), 0);
    assert_eq!(v.sections[0].body_rows(), 1);
}

#[test]
fn colors_are_made_again_when_the_theme_changes() {
    let r = repo("theme", 1, &[0]);
    let mut v = view(&r, 1);
    settle(&mut v);
    assert!(v.sections[0].loaded.as_ref().unwrap().colors.is_none());
    let theme = std::sync::Arc::new(syntect::highlighting::ThemeSet::load_defaults().themes["base16-ocean.dark"].clone());
    v.set_coloring(Some(Coloring { theme: theme.clone(), fg: egui::Color32::WHITE }));
    assert!(v.sections[0].stale);
    settle(&mut v);
    // (".txt" has a grammar in the bundled set: plain text has no spans, but is known.)
    let same = Coloring { theme, fg: egui::Color32::WHITE };
    v.set_coloring(Some(same));
    assert!(!v.sections[0].stale, "the same theme again changes nothing");
}

#[test]
fn the_neovim_spec_of_a_file_names_its_sides() {
    use crate::scm_view::{neovim_spec, DiffSpec};
    let top = std::path::Path::new("/r");
    let staged = FileSpec { left: Side::Ref("HEAD".into()), right: Side::Index, ..spec(1) };
    let d = neovim_spec(top, &staged);
    assert_eq!((d.base.as_str(), d.right.as_deref(), d.deleted), ("HEAD", Some(""), false));
    assert_eq!((d.left_label.as_deref(), d.right_label.as_deref()), (Some("HEAD"), Some("index")));
    let work = FileSpec { left: Side::Index, right: Side::Disk, ..spec(1) };
    let d = neovim_spec(top, &work);
    assert_eq!((d.base.as_str(), d.right.clone()), ("", None));
    // And a Source Control spec makes the same file for the view.
    let spec = DiffSpec { top: top.into(), base: String::new(), path: "a.rs".into(), ..Default::default() };
    let f = spec.file_spec();
    assert_eq!((f.left, f.right, f.status), (Side::Index, Side::Disk, Status::Modified));
    let r = spec.request();
    assert_eq!((r.compared.as_str(), r.focus.as_deref()), ("index → working tree", Some("a.rs")));
}

// Pictures.

use crate::decorations::DecoColors;
use crate::scm_view::tests::system_font;
use crate::style::tests::themes;
use egui::{Color32, FontId, Pos2, Rect, Vec2};
use egui_kittest::Harness;

/// Two files changed on disk: a Rust file with edits in two places (and the
/// words within its changed lines), and a short text one with lines added
/// and removed.
fn pictures_repo(name: &str) -> Repo {
    let r = Repo::new(name);
    let before: String = [
        "use std::collections::HashMap;",
        "",
        "/// The total of the prices.",
        "fn total(prices: &[u32]) -> u32 {",
        "    let mut sum = 0;",
        "    for p in prices {",
        "        sum += p;",
        "    }",
        "    sum",
        "}",
        "",
        "fn label(count: usize) -> String {",
        "    format!(\"{} items\", count)",
        "}",
        "",
        "fn unrelated_one() {}",
        "fn unrelated_two() {}",
        "fn unrelated_three() {}",
        "fn unrelated_four() {}",
        "fn unrelated_five() {}",
        "fn unrelated_six() {}",
        "",
        "fn main() {",
        "    let prices = vec![1, 2, 3];",
        "    println!(\"{}\", total(&prices));",
        "}",
    ]
    .iter()
    .map(|l| format!("{l}\n"))
    .collect();
    let after = before
        .replace("for p in prices {\n        sum += p;", "for p in prices.iter() {\n        sum += *p;")
        .replace("format!(\"{} items\", count)", "format!(\"{count} items\")")
        .replace("let prices = vec![1, 2, 3];\n", "let prices = vec![1, 2, 3, 4];\n    let label = label(prices.len());\n    println!(\"{label}\");\n");
    r.write("src/lib.rs", &before);
    r.write("notes.txt", "one\ntwo\nthree\nfour\n");
    r.commit("first");
    r.write("src/lib.rs", &after);
    r.write("notes.txt", "one\nthree\nfour\nfive\nsix\n");
    r
}

/// The git colors as the app derives them for a dark and for a light theme.
fn deco_for(theme: &str, chrome: &Chrome) -> DecoColors {
    let (added, modified, deleted) = if theme == "dark" {
        (Color32::from_rgb(0x73, 0xc9, 0x91), Color32::from_rgb(0xe2, 0xc0, 0x8d), Color32::from_rgb(0xe0, 0x6c, 0x75))
    } else {
        (Color32::from_rgb(0x2e, 0x7d, 0x32), Color32::from_rgb(0x89, 0x55, 0x03), Color32::from_rgb(0xc6, 0x28, 0x28))
    };
    DecoColors { error: deleted, warning: modified, info: chrome.accent, added, modified, deleted, text: chrome.fg }
}

struct Pic {
    view: DiffView,
    chrome: Chrome,
    colors: DiffColors,
    /// The scroll bar over the view, as the app paints it (none unless a test sets a mode).
    bars: crate::scrollbar::Scrollbars,
}

use crate::style::Chrome;

fn pic_harness(name: &str, theme: &str, layout: Layout, size: Vec2, f: impl FnOnce(&mut DiffView)) -> (Harness<'static, Pic>, Repo) {
    pic_harness_mode(name, theme, layout, size, Mode::ReadOnly, f)
}

fn pic_harness_mode(name: &str, theme: &str, layout: Layout, size: Vec2, mode: Mode, f: impl FnOnce(&mut DiffView)) -> (Harness<'static, Pic>, Repo) {
    let r = pictures_repo(name);
    let (_, chrome, _) = themes().into_iter().find(|(n, ..)| *n == theme).unwrap();
    let files = vec![
        FileSpec { path: "src/lib.rs".into(), old_path: None, status: Status::Modified, left: Side::Ref("HEAD".into()), right: Side::Disk },
        FileSpec { path: "notes.txt".into(), old_path: None, status: Status::Modified, left: Side::Ref("HEAD".into()), right: Side::Disk },
    ];
    let mut view = DiffView::new(
        Request { top: r.dir.clone(), compared: "HEAD → working tree".into(), files: Files::List(files), focus: None, mode },
        Options { layout, ..Options::default() },
        || {},
    );
    // The syntax theme as the app makes it from a colorscheme: here a bundled one.
    let set = syntect::highlighting::ThemeSet::load_defaults();
    let syntax = if theme == "dark" { "base16-ocean.dark" } else { "InspiredGitHub" };
    view.set_coloring(Some(Coloring { theme: std::sync::Arc::new(set.themes[syntax].clone()), fg: chrome.fg }));
    view.viewport_h = size.y;
    settle(&mut view);
    f(&mut view);
    view.prepare();
    settle(&mut view);
    let deco = deco_for(theme, &chrome);
    let colors = DiffColors::new(&chrome, &deco, [None; 4]);
    let state = Pic { view, chrome, colors, bars: crate::scrollbar::Scrollbars::new(crate::scrollbar::Mode::Never) };
    let h = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(2.0)
        .with_accessibility_check(false)
        .wgpu()
        .with_context_setup(system_font)
        .build_ui_state(
            move |ui, s: &mut Pic| {
                ui.ctx().set_theme(if s.chrome.bg.r() < 128 { egui::Theme::Dark } else { egui::Theme::Light });
                let code = |_: bool, _: bool| (FontId::monospace(13.0), false);
                let look = Look { chrome: s.chrome, colors: s.colors, ui: FontId::proportional(13.0), code: &code, row_h: 19.0, char_w: 7.8, bar_h: 31.0 };
                // (The pictures of the bar are inside the window, which the others reach past by the margin.)
                let inset = if s.bars.mode == crate::scrollbar::Mode::Never { Vec2::ZERO } else { Vec2::splat(16.0) };
                let rect = Rect::from_min_size(ui.max_rect().min, size - inset);
                s.view.show(ui, rect, &look, true);
                let target = s.view.scrollbar_target(s.chrome.fg, s.chrome.bg);
                s.bars.paint(ui.ctx(), ui.painter(), target.into_iter().collect());
            },
            state,
        );
    (h, r)
}

#[test]
fn unified_and_split_pictures() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        for (layout, name) in [(Layout::Unified, "unified"), (Layout::Split, "split")] {
            let (mut h, _repo) = pic_harness(&format!("pic-{theme}-{name}"), theme, layout, Vec2::new(900.0, 560.0), |v| {
                // The cursor on a changed line.
                v.cursor = Cursor { section: 0, row: Some(5) };
            });
            h.run();
            h.snapshot(format!("diff_view_{name}_{theme}"));
            results.extend_harness(&mut h);
        }
    }
    results.unwrap();
}

#[test]
fn a_scrolled_view_keeps_the_header_of_the_section_it_is_in() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let (mut h, _repo) = pic_harness(&format!("sticky-{theme}"), theme, Layout::Unified, Vec2::new(700.0, 300.0), |v| {
            // Well into the first file, whose header stays at the top.
            v.scroll = 200.0;
            v.anchor = Some(Cursor { section: 0, row: Some(6) });
            v.cursor = Cursor { section: 0, row: Some(8) };
        });
        h.run();
        h.snapshot(format!("diff_view_sticky_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn a_folded_section_is_only_its_header() {
    let (mut h, _repo) = pic_harness("folded", "dark", Layout::Unified, Vec2::new(700.0, 240.0), |v| v.toggle_fold(0));
    h.run();
    h.snapshot("diff_view_folded_dark");
}

#[test]
fn a_narrow_view_drops_what_does_not_fit_in_its_bar() {
    let (mut h, _repo) = pic_harness("narrow", "dark", Layout::Unified, Vec2::new(400.0, 220.0), |_| {});
    h.run();
    h.snapshot("diff_view_narrow_dark");
}

/// A new file, a deleted one, a binary one, one too large for the view and a
/// rename, in the one picture.
#[test]
fn the_kinds_of_file_are_drawn() {
    let r = Repo::new("kinds-pic");
    r.write("gone.txt", "this\nwas here\n");
    r.write("old.txt", "one\ntwo\nthree\n");
    r.write("logo.bin", "x");
    r.commit("first");
    std::fs::remove_file(r.dir.join("gone.txt")).unwrap();
    r.git(&["mv", "old.txt", "new.txt"]);
    std::fs::write(r.dir.join("logo.bin"), [0u8, 1, 2]).unwrap();
    r.write("fresh.txt", "a brand new file\nwith two lines\n");
    r.write("huge.txt", &"x\n".repeat(60));
    let file = |path: &str, old: Option<&str>, status, left, right| FileSpec { path: path.into(), old_path: old.map(str::to_owned), status, left, right };
    let head = || Side::Ref("HEAD".into());
    let files = vec![
        file("fresh.txt", None, Status::Added, Side::Missing, Side::Disk),
        file("gone.txt", None, Status::Deleted, head(), Side::Disk),
        file("logo.bin", None, Status::Modified, head(), Side::Disk),
        file("huge.txt", None, Status::Added, Side::Missing, Side::Disk),
        file("new.txt", Some("old.txt"), Status::Renamed, head(), Side::Disk),
    ];
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let (_, chrome, _) = themes().into_iter().find(|(n, ..)| *n == theme).unwrap();
        let mut view = DiffView::new(
            Request { top: r.dir.clone(), compared: "HEAD → working tree".into(), files: Files::List(files.clone()), focus: None, mode: Mode::ReadOnly },
            Options { max_lines: 50, ..Options::default() },
            || {},
        );
        view.viewport_h = 700.0;
        settle(&mut view);
        let deco = deco_for(theme, &chrome);
        let state = Pic { view, chrome, colors: DiffColors::new(&chrome, &deco, [None; 4]), bars: crate::scrollbar::Scrollbars::new(crate::scrollbar::Mode::Never) };
        let mut h = Harness::builder()
            .with_size(Vec2::new(640.0, 560.0))
            .with_pixels_per_point(2.0)
            .with_accessibility_check(false)
            .wgpu()
            .with_context_setup(system_font)
            .build_ui_state(
                |ui, s: &mut Pic| {
                    ui.ctx().set_theme(if s.chrome.bg.r() < 128 { egui::Theme::Dark } else { egui::Theme::Light });
                    let code = |_: bool, _: bool| (FontId::monospace(13.0), false);
                    let look = Look { chrome: s.chrome, colors: s.colors, ui: FontId::proportional(13.0), code: &code, row_h: 19.0, char_w: 7.8, bar_h: 31.0 };
                    let rect = Rect::from_min_size(ui.max_rect().min, Vec2::new(640.0, 560.0));
                    s.view.show(ui, rect, &look, true);
                },
                state,
            );
        h.run();
        h.snapshot(format!("diff_view_kinds_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

// Editing the changes.

use super::edit::FileOp;
use crate::git;

/// `a.txt` of 30 lines, committed, with its lines 5, 15 and 25 changed on disk.
fn three_changes(name: &str) -> Repo {
    let r = Repo::new(name);
    let text = |changed: bool| -> String {
        (1..=30).map(|l| if changed && [5, 15, 25].contains(&l) { format!("changed {l}\n") } else { format!("line {l}\n") }).collect()
    };
    r.write("a.txt", &text(false));
    r.commit("first");
    r.write("a.txt", &text(true));
    r
}

fn mode_view(r: &Repo, mode: Mode, specs: Vec<FileSpec>) -> DiffView {
    let req = Request { top: r.dir.clone(), compared: "x".into(), files: Files::List(specs), focus: None, mode };
    let mut v = DiffView::new(req, Options::default(), || {});
    v.viewport_h = 800.0;
    settle(&mut v);
    v
}

fn file(path: &str, left: Side, right: Side) -> FileSpec {
    FileSpec { path: path.into(), old_path: None, status: Status::Modified, left, right }
}

/// The unstaged changes of `path` (index → working tree) as a Source Control view.
fn stage_view(r: &Repo) -> DiffView {
    mode_view(r, Mode::Stage, vec![file("a.txt", Side::Index, Side::Disk)])
}

/// The cursor on the `n`th change of section 0.
fn on_change(v: &mut DiffView, n: usize) {
    let starts = v.sections[0].rows.as_ref().unwrap().2.change_starts();
    v.cursor = Cursor { section: 0, row: Some(starts[n]) };
}

fn edit_of(a: Option<Action>) -> Edit {
    match a {
        Some(Action::Edit(e)) => e,
        other => panic!("not an edit: {other:?}"),
    }
}

fn run(r: &Repo, edit: Edit) {
    let op = edit.git_op().expect("a git edit");
    git::run(&r.dir, &op, &|_| Ok(())).unwrap();
}

#[test]
fn modes_come_from_what_is_compared() {
    let head = || Side::Ref("HEAD".into());
    assert_eq!(Mode::of(&Side::Index, &Side::Disk), Mode::Stage);
    assert_eq!(Mode::of(&Side::Missing, &Side::Disk), Mode::Stage);
    assert_eq!(Mode::of(&head(), &Side::Index), Mode::Unstage);
    assert_eq!(Mode::of(&Side::Missing, &Side::Index), Mode::Unstage);
    assert_eq!(Mode::of(&head(), &Side::Disk), Mode::ReadOnly);
    assert_eq!(Mode::of(&Side::Ref("abc".into()), &Side::Ref("def".into())), Mode::ReadOnly);
    assert!(Mode::ReadOnly.change_actions().is_empty() && Mode::ReadOnly.file_actions().is_empty());
}

#[test]
fn one_change_is_staged_unstaged_and_discarded_with_git_apply() {
    let r = three_changes("apply");
    let mut v = stage_view(&r);
    on_change(&mut v, 1);
    // Stage the middle change.
    let c = v.cursor;
    let edit = edit_of(v.do_action(Do::Stage, c));
    assert!(matches!(&edit, Edit::Patch { cached: true, reverse: false, .. }));
    run(&r, edit);
    let staged = r.git(&["diff", "--cached", "-U0"]);
    assert!(staged.contains("+changed 15") && !staged.contains("changed 5") && !staged.contains("changed 25"), "{staged}");
    let unstaged = r.git(&["diff", "-U0"]);
    assert!(unstaged.contains("+changed 5") && unstaged.contains("+changed 25") && !unstaged.contains("changed 15"), "{unstaged}");
    // Unstage it, from the view of what is staged.
    let mut u = mode_view(&r, Mode::Unstage, vec![file("a.txt", Side::Ref("HEAD".into()), Side::Index)]);
    on_change(&mut u, 0);
    let c = u.cursor;
    run(&r, edit_of(u.do_action(Do::Unstage, c)));
    assert_eq!(r.git(&["diff", "--cached"]), "");
    // Discard the last change (asks twice).
    let mut v = stage_view(&r);
    on_change(&mut v, 2);
    let c = v.cursor;
    assert!(v.do_action(Do::Discard, c).is_none());
    let edit = edit_of(v.do_action(Do::Discard, c));
    assert!(matches!(&edit, Edit::Patch { cached: false, reverse: true, .. }));
    run(&r, edit);
    let text = std::fs::read_to_string(r.dir.join("a.txt")).unwrap();
    assert!(text.contains("line 25\n") && text.contains("changed 5\n") && text.contains("changed 15\n"), "{text}");
    assert!(!text.contains("changed 25"));
}

#[test]
fn a_change_in_a_renamed_staged_file_is_unstaged_where_it_is() {
    let r = Repo::new("rename-apply");
    let text = |a: &str, b: &str| -> String { (1..=30).map(|l| if l == 3 { format!("{a}\n") } else if l == 27 { format!("{b}\n") } else { format!("line {l}\n") }).collect() };
    r.write("old.txt", &text("line 3", "line 27"));
    r.commit("first");
    r.git(&["mv", "old.txt", "new.txt"]);
    r.write("new.txt", &text("THREE", "TWENTY-SEVEN"));
    r.git(&["add", "-A"]);
    let spec = FileSpec {
        path: "new.txt".into(),
        old_path: Some("old.txt".into()),
        status: Status::Renamed,
        left: Side::Ref("HEAD".into()),
        right: Side::Index,
    };
    let mut v = mode_view(&r, Mode::Unstage, vec![spec]);
    assert_eq!(v.sections[0].diff().map(|d| (d.added, d.removed)), Some((2, 2)));
    on_change(&mut v, 1);
    let c = v.cursor;
    run(&r, edit_of(v.do_action(Do::Unstage, c)));
    // The last change is out of the index; the first stays, and so does the rename.
    let index = r.git(&["show", ":new.txt"]);
    assert!(index.contains("THREE") && index.contains("line 27") && !index.contains("TWENTY"), "{index}");
    assert!(r.git(&["diff", "--cached", "-M", "--name-status"]).starts_with("R"), "{}", r.git(&["diff", "--cached", "-M", "--name-status"]));
    // Both its names, for the whole file.
    let c = Cursor { section: 0, row: None };
    let Edit::Files { how, paths, .. } = edit_of(v.do_action(Do::UnstageFile, c)) else { panic!() };
    assert_eq!((how, paths), (FileOp::Unstage, vec!["new.txt".to_owned(), "old.txt".to_owned()]));
}

#[test]
fn a_patch_fits_files_without_a_last_line_end_with_crlf_and_with_spaces_in_the_name() {
    let r = Repo::new("odd-apply");
    // CRLF, no newline at the end, a name with a space.
    let lines = |a: &str, z: &str| -> String { (1..=20).map(|l| if l == 2 { a.to_owned() } else if l == 20 { z.to_owned() } else { format!("line {l}") }).collect::<Vec<_>>().join("\r\n") };
    r.write("my file.txt", &lines("line 2", "line 20"));
    r.commit("first");
    r.write("my file.txt", &lines("two", "twenty"));
    let spec = file("my file.txt", Side::Index, Side::Disk);
    let mut v = mode_view(&r, Mode::Stage, vec![spec.clone()]);
    on_change(&mut v, 1);
    let c = v.cursor;
    run(&r, edit_of(v.do_action(Do::Stage, c)));
    let index = r.git(&["show", ":my file.txt"]);
    assert!(index.contains("twenty") && index.contains("line 2\r") && !index.contains("two\r"), "{index:?}");
    // And the first change, discarded from the working tree.
    let mut v = mode_view(&r, Mode::Stage, vec![spec]);
    on_change(&mut v, 0);
    let c = v.cursor;
    v.do_action(Do::Discard, c);
    run(&r, edit_of(v.do_action(Do::Discard, c)));
    let disk = std::fs::read_to_string(r.dir.join("my file.txt")).unwrap();
    assert!(disk.contains("line 2\r\n") && disk.ends_with("twenty"), "{disk:?}");
}

#[test]
fn a_new_or_deleted_file_is_one_change_that_git_stages_and_discards_whole() {
    let r = Repo::new("whole");
    r.write("gone.txt", "a\nb\n");
    r.commit("first");
    std::fs::remove_file(r.dir.join("gone.txt")).unwrap();
    r.write("fresh.txt", "x\ny\n");
    let fresh = FileSpec { path: "fresh.txt".into(), old_path: None, status: Status::Added, left: Side::Missing, right: Side::Disk };
    let gone = FileSpec { path: "gone.txt".into(), old_path: None, status: Status::Deleted, left: Side::Index, right: Side::Disk };
    let mut v = mode_view(&r, Mode::Stage, vec![fresh, gone]);
    let c = Cursor { section: 0, row: Some(0) };
    assert_eq!(edit_of(v.do_action(Do::Stage, c)), Edit::Files { how: FileOp::Stage, paths: vec!["fresh.txt".into()], untracked: false });
    // (Discarding a new file is to the Trash, after asking.)
    assert!(v.do_action(Do::Discard, c).is_none());
    assert_eq!(edit_of(v.do_action(Do::Discard, c)), Edit::Files { how: FileOp::Discard, paths: vec!["fresh.txt".into()], untracked: true });
    let c = Cursor { section: 1, row: Some(0) };
    v.do_action(Do::Discard, c);
    assert_eq!(edit_of(v.do_action(Do::Discard, c)), Edit::Files { how: FileOp::Discard, paths: vec!["gone.txt".into()], untracked: false });
    run(&r, edit_of(v.do_action(Do::Stage, c)));
    assert!(r.git(&["diff", "--cached", "--name-status"]).starts_with("D"));
}

#[test]
fn discarding_asks_twice_and_any_other_key_asks_again() {
    let r = three_changes("confirm");
    let mut v = stage_view(&r);
    on_change(&mut v, 0);
    let x = |v: &mut DiffView| v.key(Key::X, SHIFT);
    assert_eq!(x(&mut v), None);
    assert!(v.is_armed(Do::Discard, 0, v.cursor.row));
    assert!(matches!(x(&mut v), Some(Action::Edit(Edit::Patch { cached: false, reverse: true, .. }))));
    assert!(!v.is_armed(Do::Discard, 0, v.cursor.row));
    // Another key between: asked again.
    assert_eq!(x(&mut v), None);
    v.key(Key::J, NONE);
    assert!(!v.is_armed(Do::Discard, 0, v.cursor.row));
    assert_eq!(x(&mut v), None);
    // Another change: asked again for that one.
    on_change(&mut v, 1);
    assert_eq!(x(&mut v), None);
    on_change(&mut v, 2);
    assert_eq!(x(&mut v), None, "pressed once at this change");
    // The whole file asks too.
    let h = Cursor { section: 0, row: None };
    v.armed = None;
    assert!(v.do_action(Do::DiscardFile, h).is_none() && v.is_armed(Do::DiscardFile, 0, None));
    assert!(v.do_action(Do::DiscardFile, h).is_some());
    // Staging doesn't ask.
    on_change(&mut v, 0);
    assert!(matches!(v.key(Key::Minus, NONE), Some(Action::Edit(Edit::Patch { cached: true, reverse: false, .. }))));
}

#[test]
fn keys_do_what_the_mode_offers_and_a_commit_is_read_only() {
    let r = three_changes("keys");
    let head = || file("a.txt", Side::Ref("HEAD".into()), Side::Disk);
    let mut ro = mode_view(&r, Mode::ReadOnly, vec![head()]);
    on_change(&mut ro, 0);
    for (key, m) in [(Key::R, NONE), (Key::Minus, NONE), (Key::X, SHIFT)] {
        assert_eq!(ro.key(key, m), None, "{key:?}");
    }
    assert!(!ro.changes_editable(0));
    // Staging has no `r`; the agents' changes no `-` or `X`.
    let mut st = stage_view(&r);
    on_change(&mut st, 0);
    assert_eq!(st.key(Key::R, NONE), None);
    let mut ag = mode_view(&r, Mode::Agents, vec![head()]);
    on_change(&mut ag, 0);
    assert_eq!((ag.key(Key::Minus, NONE), ag.key(Key::X, SHIFT)), (None, None));
    // On a line that isn't a change, or a header, nothing.
    ag.cursor = Cursor { section: 0, row: Some(0) };
    assert_eq!(ag.key(Key::R, NONE), None);
    ag.cursor = Cursor { section: 0, row: None };
    assert_eq!(ag.key(Key::R, NONE), None);
    // The staged list can only unstage.
    r.git(&["add", "a.txt"]);
    let mut un = mode_view(&r, Mode::Unstage, vec![file("a.txt", Side::Ref("HEAD".into()), Side::Index)]);
    on_change(&mut un, 0);
    assert_eq!(un.key(Key::X, SHIFT), None);
}

#[test]
fn reverting_a_change_names_its_lines_for_neovim() {
    let r = three_changes("revert");
    let head = || file("a.txt", Side::Ref("HEAD".into()), Side::Disk);
    let mut v = mode_view(&r, Mode::Agents, vec![head()]);
    on_change(&mut v, 1);
    let edit = edit_of(v.key(Key::R, NONE));
    // Line 15 (index 14): "changed 15" goes back to "line 15".
    assert_eq!(edit, Edit::Revert { path: "a.txt".into(), start: 14, expect: vec!["changed 15".into()], put: vec!["line 15".into()] });
    // The file: every line.
    let Edit::Revert { start, expect, put, .. } = edit_of(v.do_action(Do::RevertFile, Cursor { section: 0, row: None })) else { panic!() };
    assert_eq!((start, expect.len(), put.len(), expect[4].as_str(), put[4].as_str()), (0, 30, 30, "changed 5", "line 5"));
    // The buttons offered: not for a file with no changes.
    assert!(v.file_action_offered(Do::RevertFile, 0) && v.file_action_offered(Do::Accept, 0));
    assert_eq!(edit_of(v.do_action(Do::Accept, Cursor { section: 0, row: None })), Edit::Accept { path: "a.txt".into() });
    // Accepted: the buttons go at once (git takes a moment), until the file
    // is read again against the snapshot the accept made.
    assert!(!v.file_action_offered(Do::RevertFile, 0) && !v.file_action_offered(Do::Accept, 0));
    v.rebase("HEAD~0");
    settle(&mut v);
    assert!(v.file_action_offered(Do::Accept, 0), "read again: what it shows decides");
    let same = mode_view(&r, Mode::Agents, vec![file("a.txt", Side::Disk, Side::Disk)]);
    assert!(!same.file_action_offered(Do::Accept, 0) && !same.file_action_offered(Do::RevertFile, 0));
}

#[test]
fn a_created_file_reverts_to_nothing_and_a_deleted_one_to_its_lines() {
    let r = Repo::new("created");
    r.write("gone.txt", "a\nb\n");
    r.commit("first");
    std::fs::remove_file(r.dir.join("gone.txt")).unwrap();
    r.write("made.txt", "x\ny\n");
    let head = || Side::Ref("HEAD".into());
    let made = FileSpec { path: "made.txt".into(), old_path: None, status: Status::Added, left: Side::Missing, right: Side::Disk };
    let gone = FileSpec { path: "gone.txt".into(), old_path: None, status: Status::Deleted, left: head(), right: Side::Disk };
    let mut v = mode_view(&r, Mode::Agents, vec![made, gone]);
    assert_eq!(
        edit_of(v.do_action(Do::RevertFile, Cursor { section: 0, row: None })),
        Edit::Revert { path: "made.txt".into(), start: 0, expect: vec!["x".into(), "y".into()], put: vec![] }
    );
    assert_eq!(
        edit_of(v.do_action(Do::RevertFile, Cursor { section: 1, row: None })),
        Edit::Revert { path: "gone.txt".into(), start: 0, expect: vec![], put: vec!["a".into(), "b".into()] }
    );
}

/// Make the edit's change in the repository the way the app would, and tell the view.
fn staged(r: &Repo, v: &mut DiffView, edit: Edit) {
    run(r, edit);
    v.refresh(None);
    settle(v);
}

#[test]
fn after_a_change_is_gone_the_cursor_is_on_the_next_one() {
    let r = three_changes("place");
    let mut v = stage_view(&r);
    on_change(&mut v, 1);
    let c = v.cursor;
    let edit = edit_of(v.do_action(Do::Stage, c));
    staged(&r, &mut v, edit);
    // (Changes 5 and 25 are left: the one after 15 is 25.)
    let text_at = |v: &DiffView| -> String {
        let d = v.sections[0].diff().unwrap();
        let Some((.., Rows::Unified(rows))) = &v.sections[0].rows else { panic!() };
        let Some(Row::Line(l)) = v.cursor.row.map(|r| rows[r]) else { panic!("{:?}", v.cursor) };
        // (The first row of a change is its first removed line.)
        l.old.map_or_else(|| d.new[l.new.unwrap()].clone(), |o| d.old[o].clone())
    };
    assert_eq!(text_at(&v), "line 25");
    assert_eq!(v.sections[0].counts(), Some((2, 2)));
    // The last one staged: the cursor is on the one before (no more in the files after).
    let c = v.cursor;
    let edit = edit_of(v.do_action(Do::Stage, c));
    staged(&r, &mut v, edit);
    assert_eq!(text_at(&v), "line 5");
    // The first of two files: the next change is in the second.
    let r2 = three_changes("place2");
    r2.write("b.txt", "one\ntwo\n");
    r2.git(&["add", "b.txt"]);
    r2.git(&["commit", "-qm", "b"]);
    r2.write("b.txt", "one\nTWO\n");
    let mut v = mode_view(&r2, Mode::Stage, vec![file("a.txt", Side::Index, Side::Disk), file("b.txt", Side::Index, Side::Disk)]);
    for _ in 0..3 {
        let starts = v.sections[0].rows.as_ref().unwrap().2.change_starts();
        v.cursor = Cursor { section: 0, row: starts.last().copied() };
        let c = v.cursor;
        let edit = edit_of(v.do_action(Do::Stage, c));
        staged(&r2, &mut v, edit);
        if v.sections[0].counts() == Some((0, 0)) {
            break;
        }
    }
    assert_eq!(v.sections[0].counts(), Some((0, 0)), "shown as no changes, not removed");
    assert_eq!(v.sections.len(), 2);
    assert_eq!(v.cursor.section, 1, "{:?}", v.cursor);
}

#[test]
fn an_edit_that_failed_leaves_the_cursor_alone() {
    let r = three_changes("failed");
    let mut v = stage_view(&r);
    on_change(&mut v, 1);
    let at = v.cursor;
    let c = v.cursor;
    edit_of(v.do_action(Do::Stage, c));
    v.cancel_edit();
    v.refresh(None);
    settle(&mut v);
    assert_eq!(v.cursor, at);
}

#[test]
fn the_agents_snapshot_moving_on_is_read_against() {
    let r = three_changes("rebase");
    let tree = r.git(&["rev-parse", "HEAD^{tree}"]);
    let mut v = mode_view(&r, Mode::Agents, vec![file("a.txt", Side::Ref("HEAD".into()), Side::Disk)]);
    assert_eq!(v.sections[0].counts(), Some((3, 3)));
    v.rebase("HEAD");
    assert!(!v.sections[0].stale, "the same");
    v.rebase(&tree);
    assert_eq!(v.sections[0].spec.left, Side::Ref(tree.clone()));
    // (Read against a tree that has the file as it is now: no changes.)
    r.git(&["add", "a.txt"]);
    let now = r.git(&["write-tree"]);
    v.rebase(&now);
    settle(&mut v);
    assert_eq!(v.sections[0].counts(), Some((0, 0)));
    // Not for other modes.
    let mut s = stage_view(&r);
    s.rebase("x");
    assert_eq!(s.sections[0].spec.left, Side::Index);
}

#[test]
fn colon_asks_for_neovims_command_line() {
    let r = three_changes("colon");
    let mut v = stage_view(&r);
    assert_eq!(v.key(Key::Colon, NONE), Some(Action::Command));
    assert_eq!(v.key(Key::Semicolon, SHIFT), Some(Action::Command));
    assert_eq!(v.key(Key::Semicolon, NONE), None);
}

// Searching.

#[test]
#[allow(clippy::single_range_in_vec_init)]
fn a_find_is_case_blind_unless_it_has_capitals() {
    assert_eq!(find::occurrences("Foo foo FOO", "foo"), [0..3, 4..7, 8..11]);
    assert_eq!(find::occurrences("Foo foo FOO", "Foo"), [0..3]);
    assert!(find::occurrences("abc", "").is_empty() && find::occurrences("abc", "x").is_empty());
    assert_eq!(find::occurrences("aaa", "aa"), [0..2]);
    assert_eq!(find::occurrences("héllo héllo", "héllo").len(), 2);
}

/// Three files of ten lines; "needle" is in lines 2 and 8 of `f0.txt`, in `f2.txt`'s line 5, and
/// (when `far`) in the last of 100 files.
fn needle_repo(name: &str, files: usize, far: bool) -> Repo {
    let r = Repo::new(name);
    for i in 0..files {
        let text: String = (1..=10).map(|l| format!("file {i} line {l}\n")).collect();
        r.write(&format!("f{i}.txt"), &text);
    }
    r.commit("first");
    let edit = |i: usize, lines: &[usize]| {
        let text: String = (1..=10).map(|l| if lines.contains(&l) { format!("file {i} needle {l}\n") } else { format!("file {i} line {l}\n") }).collect();
        r.write(&format!("f{i}.txt"), &text);
    };
    edit(0, &[2, 8]);
    edit(2, &[5]);
    if far {
        edit(files - 1, &[3]);
    }
    r
}

fn find_view(r: &Repo, n: usize, text: &str) -> DiffView {
    let mut v = view(r, n);
    v.viewport_h = 600.0;
    settle(&mut v);
    v.open_find();
    v.find.as_mut().unwrap().text = text.into();
    v.find_changed();
    settle(&mut v);
    v
}

/// The text of the row the cursor is on.
fn cursor_text(v: &DiffView) -> String {
    let c = v.cursor;
    let texts = v.row_texts_for_test(c.section, c.row.unwrap());
    texts.into_iter().flatten().collect::<Vec<_>>().join("|")
}

impl DiffView {
    fn row_texts_for_test(&self, section: usize, row: usize) -> [Option<String>; 2] {
        let d = self.sections[section].diff().unwrap();
        let Some((.., Rows::Unified(rows))) = &self.sections[section].rows else { panic!() };
        let Row::Line(l) = rows[row] else { return [None, None] };
        let text = if l.kind == Kind::Removed { &d.old[l.old.unwrap()] } else { &d.new[l.new.unwrap()] };
        [Some(text.clone()), None]
    }
}

#[test]
#[allow(clippy::single_range_in_vec_init)]
fn a_find_goes_to_the_first_match_and_steps_across_files_and_around() {
    let r = needle_repo("find", 3, false);
    let mut v = find_view(&r, 3, "needle");
    // From the top: the first added line with it.
    assert_eq!(cursor_text(&v), "file 0 needle 2");
    assert_eq!(v.cursor.section, 0);
    v.find_step(true);
    assert_eq!(cursor_text(&v), "file 0 needle 8");
    // Over f1 (no match) to f2.
    v.find_step(true);
    assert_eq!((v.cursor.section, cursor_text(&v)), (2, "file 2 needle 5".to_owned()));
    // Past the last: round to the first.
    v.find_step(true);
    assert_eq!((v.cursor.section, cursor_text(&v)), (0, "file 0 needle 2".to_owned()));
    // And back.
    v.find_step(false);
    assert_eq!((v.cursor.section, cursor_text(&v)), (2, "file 2 needle 5".to_owned()));
    v.find_step(false);
    assert_eq!(cursor_text(&v), "file 0 needle 8");
    // The matches are those of the rows shown: three, in the lines added (the removed lines have none).
    assert_eq!(v.find_count(), 3);
    assert_eq!(v.marks(0, v.cursor.row.unwrap(), 0, "file 0 needle 8").0, [7..13]);
    assert_eq!(v.marks(0, v.cursor.row.unwrap(), 0, "file 0 needle 8").1, Some(0));
}

#[test]
fn a_find_with_no_match_says_so_and_esc_closes_the_field_first() {
    let r = needle_repo("find-none", 3, false);
    let mut v = find_view(&r, 3, "zzz");
    assert!(v.find.as_ref().unwrap().none);
    v.find.as_mut().unwrap().text = "NEEDLE".into();
    v.find_changed();
    assert!(v.find.as_ref().unwrap().none, "capitals match as they are");
    // Esc closes the field, not the view; a second one the view.
    assert_eq!(v.key(Key::Escape, NONE), None);
    assert!(v.find.is_none());
    assert_eq!(v.key(Key::Escape, NONE), Some(Action::Close));
}

#[test]
fn stepping_past_the_read_files_reads_the_ones_not_read() {
    let r = needle_repo("find-far", 100, true);
    let mut v = view(&r, 100);
    v.viewport_h = 600.0;
    settle(&mut v);
    assert!(v.sections[99].loaded.is_none());
    v.open_find();
    v.find.as_mut().unwrap().text = "needle".into();
    v.find_changed();
    settle(&mut v);
    assert_eq!(v.cursor.section, 0);
    v.find_step(true);
    v.find_step(true);
    assert_eq!(v.cursor.section, 2);
    // The next is in the last file: the files between are read on the way, and the cursor arrives.
    v.find_step(true);
    assert!(v.find.as_ref().unwrap().waiting(), "waits for the file");
    settle(&mut v);
    assert_eq!((v.cursor.section, cursor_text(&v)), (99, "file 99 needle 3".to_owned()));
    assert!(v.sections[99].loaded.is_some() && v.sections[50].loaded.is_some());
}

#[test]
fn both_layouts_find_in_either_side() {
    let r = needle_repo("find-split", 3, false);
    let mut v = view(&r, 3);
    v.layout = Layout::Split;
    v.viewport_h = 600.0;
    settle(&mut v);
    v.open_find();
    v.find.as_mut().unwrap().text = "line 2".into();
    v.find_changed();
    settle(&mut v);
    // f0's change replaces "file 0 line 2" (left) by "file 0 needle 2" (right): the match is on the left.
    let f = v.find.as_ref().unwrap();
    assert_eq!(f.current.map(|p| (p.0, p.2)), Some((0, 0)));
    let before = v.cursor;
    v.find_step(true);
    assert_ne!(v.cursor, before);
}

#[test]
fn a_change_shows_its_buttons_and_the_header_its_own() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        // Source Control: stage and discard, the cursor on the first change of the first file.
        let (mut h, _repo) = pic_harness_mode(&format!("btn-stage-{theme}"), theme, Layout::Unified, Vec2::new(900.0, 420.0), Mode::Stage, |v| {
            v.cursor = Cursor { section: 0, row: Some(5) };
        });
        h.run();
        h.snapshot(format!("diff_view_buttons_stage_{theme}"));
        results.extend_harness(&mut h);
    }
    // The agents' changes in the other layout, and a discard asked once.
    let (mut h, _repo) = pic_harness_mode("btn-agents", "dark", Layout::Split, Vec2::new(900.0, 420.0), Mode::Agents, |v| {
        v.cursor = Cursor { section: 0, row: Some(5) };
    });
    h.run();
    h.snapshot("diff_view_buttons_agents_dark");
    results.extend_harness(&mut h);
    let (mut h, _repo) = pic_harness_mode("btn-armed", "dark", Layout::Unified, Vec2::new(900.0, 420.0), Mode::Stage, |v| {
        v.cursor = Cursor { section: 0, row: Some(5) };
        let c = v.cursor;
        assert!(v.do_action(Do::Discard, c).is_none());
    });
    h.run();
    h.snapshot("diff_view_buttons_armed_dark");
    results.extend_harness(&mut h);
    results.unwrap();
}

#[test]
fn the_find_field_and_its_matches_are_drawn() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let (mut h, _repo) = pic_harness(&format!("find-{theme}"), theme, Layout::Unified, Vec2::new(900.0, 420.0), |v| {
            v.open_find();
            v.find.as_mut().unwrap().text = "sum".into();
            v.find_changed();
        });
        h.run();
        h.snapshot(format!("diff_view_find_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

/// A repo with `a.rs` of `lines` lines, one changed on disk, as a view's only file.
fn long_file_view(name: &str, lines: usize) -> (DiffView, Repo) {
    let r = Repo::new(name);
    let text = |n: &str| (1..=lines).map(|l| if l == 5 { format!("let {n} = 0;\n") } else { format!("let x{l} = 1;\n") }).collect::<String>();
    r.write("a.rs", &text("a"));
    r.commit("first");
    r.write("a.rs", &text("b"));
    let file = FileSpec { path: "a.rs".into(), ..spec(0) };
    let req = Request { files: Files::List(vec![file]), ..request(&r, 1, None) };
    let mut v = DiffView::new(req, Options::default(), || {});
    let theme = syntect::highlighting::ThemeSet::load_defaults().themes["base16-ocean.dark"].clone();
    v.set_coloring(Some(Coloring { theme: std::sync::Arc::new(theme), fg: Color32::WHITE }));
    (v, r)
}

#[test]
fn the_colors_of_a_long_file_come_after_its_rows_and_are_applied() {
    let (mut v, _r) = long_file_view("late-colors", source::COLORS_LATER_LINES + 100);
    settle(&mut v);
    assert!(v.sections[0].rows.is_some(), "the rows are there");
    let start = Instant::now();
    while v.sections[0].loaded.as_ref().is_some_and(|l| l.colors.is_none()) {
        v.poll();
        assert!(start.elapsed() < Duration::from_secs(20), "the colors didn't come");
        std::thread::sleep(Duration::from_millis(5));
    }
    let colors = v.sections[0].loaded.as_ref().unwrap().colors.as_ref().expect("rust is known");
    assert_eq!(colors.new.len(), source::COLORS_LATER_LINES + 100);
}

#[test]
fn only_the_lines_shown_are_colored_and_a_folds_when_it_opens() {
    let (mut v, _r) = long_file_view("partial-colors", 3000);
    settle(&mut v);
    let start = Instant::now();
    let colored = |v: &DiffView, line: usize| v.sections[0].loaded.as_ref().and_then(|l| l.colors.as_ref()).is_some_and(|c| c.new[line].is_some());
    while !colored(&v, 4) {
        v.poll();
        assert!(start.elapsed() < Duration::from_secs(20), "the colors didn't come");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The change (line 5) and the lines around it; not those the fold hides.
    assert!(colored(&v, 1) && colored(&v, 7));
    assert!(!colored(&v, 2000) && !colored(&v, 2999));
    // The fold opens: its lines are colored, those before kept.
    let Some((.., Rows::Unified(rows))) = &v.sections[0].rows else { panic!("unified rows") };
    let op = rows.iter().find_map(|r| if let Row::Fold { op, .. } = r { Some(*op) } else { None }).expect("a fold");
    v.expand(0, op);
    let start = Instant::now();
    while !colored(&v, 2000) {
        v.poll();
        assert!(start.elapsed() < Duration::from_secs(20), "the fold's colors didn't come");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(colored(&v, 4) && colored(&v, 2999));
}

#[test]
fn colors_made_for_an_older_read_of_a_file_are_ignored() {
    let (mut v, _r) = long_file_view("stale-colors", 20);
    v.set_coloring(None);
    settle(&mut v);
    let serial = v.sections[0].serial;
    let colors = |n| Some(source::Colors { old: vec![Some(vec![]); n], new: vec![Some(vec![]); n] });
    let have = |v: &DiffView| v.sections[0].loaded.as_ref().unwrap().colors.as_ref().map(|c| c.new.len());
    assert_eq!(have(&v), None);
    // The file was read again since: its older colors are not for this diff.
    v.take(Done::Colors { index: 0, serial: serial - 1, colors: colors(1) });
    assert_eq!(have(&v), None);
    v.take(Done::Colors { index: 0, serial: serial + 1, colors: colors(2) });
    assert_eq!(have(&v), None);
    v.take(Done::Colors { index: 0, serial, colors: colors(3) });
    assert_eq!(have(&v), Some(3));
    // Nor is an older diff taken for the file's.
    let old = Loaded { body: model::Body::Binary, colors: None, status: Status::Modified };
    v.take(Done::Loaded { index: 0, serial: serial - 1, loaded: old });
    assert!(matches!(v.sections[0].loaded.as_ref().unwrap().body, model::Body::Text(_)));
}

#[test]
fn a_short_file_is_colored_when_its_rows_come() {
    let (mut v, _r) = long_file_view("short-colors", 40);
    settle(&mut v);
    assert!(v.sections[0].loaded.as_ref().unwrap().colors.is_some(), "no flash of uncolored text");
}

#[test]
fn a_trackpad_scroll_that_comes_whole_in_one_frame_scrolls_the_view() {
    let (mut h, _repo) = pic_harness("wheel-frame", "dark", Layout::Unified, Vec2::new(700.0, 300.0), |_| {});
    let at = h.state().view.body.center();
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.run_steps(4);
    assert_eq!(h.state().view.scroll, 0.0);
    // A slow frame (a long file being drawn) gets the gesture's start, moves and end at
    // once; egui's smoothed delta is cleared by the end.
    let wheel = |phase, y| egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, y),
        phase,
        source: egui::MouseWheelSource::Trackpad,
        modifiers: Modifiers::NONE,
    };
    h.input_mut().events.extend([wheel(egui::TouchPhase::Start, 0.0), wheel(egui::TouchPhase::Move, -30.0), wheel(egui::TouchPhase::Move, -30.0), wheel(egui::TouchPhase::End, 0.0)]);
    h.run_steps(4);
    assert_eq!(h.state().view.scroll, 60.0);
    // A mouse wheel's notch is three rows.
    h.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: Vec2::new(0.0, -1.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(4);
    assert_eq!(h.state().view.scroll, 60.0 + 3.0 * h.state().view.geom.row_h);
}

#[test]
fn the_divider_between_the_sides_is_dragged_and_a_double_click_centers_it() {
    let (mut h, _repo) = pic_harness("divider", "dark", Layout::Split, Vec2::new(700.0, 300.0), |_| {});
    let body = h.state().view.body;
    let y = body.min.y + h.state().view.geom.header_h + h.state().view.geom.row_h * 2.5;
    let divider = body.min.x + h.state().view.split_left(body.width()) + 0.5;
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let to = Pos2::new(body.min.x + body.width() * 0.3, y);
    for ev in [egui::Event::PointerMoved(Pos2::new(divider, y)), button(Pos2::new(divider, y), true), egui::Event::PointerMoved(to), button(to, false)] {
        h.input_mut().events.push(ev);
        h.run_steps(2);
    }
    let split = h.state().view.split;
    assert!((split - 0.3).abs() < 0.01, "{split}");
    // The press was the divider's, not the start of a selection.
    assert!(!h.state().view.dragging);
    // Not past the least either side keeps.
    let far = Pos2::new(body.min.x + 2.0, y);
    let at = Pos2::new(body.min.x + h.state().view.split_left(body.width()) + 0.5, y);
    for ev in [egui::Event::PointerMoved(at), button(at, true), egui::Event::PointerMoved(far), button(far, false)] {
        h.input_mut().events.push(ev);
        h.run_steps(2);
    }
    assert_eq!(h.state().view.split, SPLIT_MIN);
    // A press and the pointer's moves after it in one frame: the press is on the divider.
    let at = Pos2::new(body.min.x + h.state().view.split_left(body.width()) + 0.5, y);
    let to = Pos2::new(body.min.x + body.width() * 0.4, y);
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.run_steps(2);
    h.input_mut().events.extend([button(at, true), egui::Event::PointerMoved(Pos2::new(at.x + 30.0, y)), egui::Event::PointerMoved(to)]);
    h.run_steps(2);
    h.input_mut().events.push(button(to, false));
    h.run_steps(2);
    let split = h.state().view.split;
    assert!((split - 0.4).abs() < 0.01, "{split}");
    // A double click puts it back in the middle. (The harness's clock moves a quarter
    // second a step: past a double click's, which is made longer here.)
    h.ctx.options_mut(|o| o.input_options.max_double_click_delay = 5.0);
    let at = Pos2::new(body.min.x + h.state().view.split_left(body.width()) + 0.5, y);
    for ev in [egui::Event::PointerMoved(at), button(at, true), button(at, false), button(at, true), button(at, false)] {
        h.input_mut().events.push(ev);
        h.run_steps(2);
    }
    assert_eq!(h.state().view.split, 0.5);
}

#[test]
fn the_scroll_bar_shows_where_the_view_is_in_all_its_files() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let (mut h, _repo) = pic_harness(&format!("bar-{theme}"), theme, Layout::Unified, Vec2::new(700.0, 300.0), |v| v.scroll = 120.0);
        h.state_mut().bars.mode = crate::scrollbar::Mode::Always;
        h.run();
        h.snapshot(format!("diff_view_scrollbar_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn there_is_no_scroll_bar_when_everything_is_in_view() {
    let (mut h, _repo) = pic_harness("bar-none", "dark", Layout::Unified, Vec2::new(700.0, 900.0), |_| {});
    assert!(h.state().view.content_height() < 900.0 - h.state().view.geom.header_h * 2.0);
    h.state_mut().bars.mode = crate::scrollbar::Mode::Always;
    h.run();
    let s = h.state_mut();
    let at = s.view.body.right_top() + Vec2::new(-5.0, 5.0);
    assert!(!s.bars.press(at).0);
}

#[test]
fn dragging_the_scroll_bar_scrolls_the_view_and_a_click_on_its_track_jumps() {
    let (mut h, _repo) = pic_harness("bar-drag", "dark", Layout::Unified, Vec2::new(700.0, 300.0), |_| {});
    h.state_mut().bars.mode = crate::scrollbar::Mode::Always;
    h.run();
    let s = h.state_mut();
    assert_eq!(s.view.scroll, 0.0);
    let (win, area) = {
        let t = s.view.scrollbar_target(s.chrome.fg, s.chrome.bg).expect("more than fits");
        (t.bar, t.area)
    };
    assert_eq!(win, crate::scrollbar::Bar::Diff);
    // The thumb is at the top: grab it, and pull it half way down the track.
    let x = area.max.x - 5.0;
    let (taken, first) = s.bars.press(Pos2::new(x, area.min.y + 4.0));
    assert!(taken);
    if let Some((_, y)) = first {
        s.view.scroll_to(y as f32);
    }
    assert_eq!(s.view.scroll, 0.0, "grabbed where it is");
    let (win, y) = s.bars.drag_to(Pos2::new(x, area.min.y + area.height() / 2.0)).expect("moved");
    assert_eq!(win, crate::scrollbar::Bar::Diff);
    s.view.scroll_to(y as f32);
    let max = s.view.max_scroll();
    assert!(max > 0.0 && s.view.scroll > 0.0 && s.view.scroll < max, "{} of {max}", s.view.scroll);
    // Dragged past the bottom: the end of the view.
    let (_, y) = s.bars.drag_to(Pos2::new(x, area.max.y + 200.0)).unwrap();
    s.view.scroll_to(y as f32);
    assert_eq!(s.view.scroll, max);
    s.bars.release();
    // A click on the track under the thumb takes it there; above it, back.
    h.run();
    let s = h.state_mut();
    let (_, top) = s.bars.press(Pos2::new(x, area.min.y + 2.0));
    s.view.scroll_to(top.expect("jumped").1 as f32);
    assert!(s.view.scroll < max / 4.0, "{}", s.view.scroll);
    s.bars.release();
}
