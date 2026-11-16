use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, Pos2, Rect, Vec2};
use egui_kittest::Harness;
use nvim_bridge::gui::{PickerChoice, PickerItem, PickerOpen};

use super::rank::*;
use super::*;
use crate::fonts::{self, FontSpec};
use crate::style::tests::themes;

mod phase2;
mod phase3;
mod phase4;

// ---- Ranking ----

fn paths(texts: &[&str]) -> Vec<RankItem> {
    texts.iter().map(|t| RankItem { text: (*t).to_owned(), bonus: 0, path: true }).collect()
}

fn order(items: &[RankItem], query: &str) -> Vec<String> {
    rank(items, query, &|| false).into_iter().map(|r| items[r.index].text.clone()).collect()
}

#[test]
fn letters_in_order_match_and_the_closer_ones_come_first() {
    let items = paths(&["a_long_prefix_c_o_n_f_i_g.txt", "src/config.rs", "README.md", "conf/other.rs"]);
    assert_eq!(order(&items, "config")[0], "src/config.rs");
    // Letters apart still match; letters out of order don't.
    assert!(order(&items, "cfg").contains(&"src/config.rs".to_owned()));
    assert!(order(&paths(&["abc.rs"]), "cba").is_empty());
}

#[test]
fn a_path_is_matched_by_its_name_first() {
    // The same letters, at the start of a folder or of the name.
    let items = paths(&["foo/lib/src.rs", "src/lib/foo.rs"]);
    assert_eq!(order(&items, "foo"), ["src/lib/foo.rs", "foo/lib/src.rs"]);
    // Plain text (a select's items) has no name to prefer.
    let plain: Vec<RankItem> = items.iter().map(|i| RankItem { path: false, ..i.clone() }).collect();
    let scores: Vec<i32> = rank(&plain, "foo", &|| false).iter().map(|r| r.score).collect();
    assert!(scores[0] - scores[1] < NAME_BONUS, "{scores:?}");
}

#[test]
fn words_and_the_fzf_atoms() {
    let items = paths(&["src/app.rs", "src/picker.rs", "tests/app_test.rs", "docs/app.md"]);
    // Words are matched apart, in any order.
    assert_eq!(order(&items, "rs app"), ["src/app.rs", "tests/app_test.rs"]);
    // `!` leaves out, `^` is the start, `$` the end.
    assert_eq!(order(&items, "app !test !md"), ["src/app.rs"]);
    assert_eq!(order(&items, "^src"), ["src/app.rs", "src/picker.rs"]);
    assert_eq!(order(&items, ".md$"), ["docs/app.md"]);
}

#[test]
fn case_matters_only_once_the_query_has_a_capital() {
    let items = paths(&["src/Readme.md", "src/readme.txt"]);
    assert_eq!(order(&items, "readme").len(), 2);
    assert_eq!(order(&items, "Readme"), ["src/Readme.md"]);
}

#[test]
fn ties_go_to_the_shorter_path_then_to_the_source_order() {
    let items = paths(&["dir/app.rs", "app.rs", "x/app.rs"]);
    assert_eq!(order(&items, "app"), ["app.rs", "x/app.rs", "dir/app.rs"]);
    // The same length: the one that came first.
    assert_eq!(order(&paths(&["bb/app.rs", "aa/app.rs"]), "app"), ["bb/app.rs", "aa/app.rs"]);
}

#[test]
fn a_buffers_bonus_wins_a_tie_and_not_a_clearly_better_match() {
    let mut items = paths(&["src/application.rs", "src/app.rs"]);
    assert_eq!(order(&items, "app")[0], "src/app.rs", "equal matches: the shorter");
    items[0].bonus = BUFFER_BONUS;
    assert_eq!(order(&items, "app")[0], "src/application.rs", "the buffer wins between similar matches");
    // The buffer matches by letters spread apart, the other by the name's start.
    let mut items = paths(&["docs/a_x_p_x_p.md", "src/app.rs"]);
    items[0].bonus = BUFFER_BONUS;
    assert_eq!(order(&items, "app")[0], "src/app.rs", "a clearly better match is not beaten");
}

#[test]
fn the_best_matches_say_where_they_matched() {
    let items = paths(&["src/app.rs", "src/other.rs"]);
    let ranked = rank(&items, "sa", &|| false);
    assert_eq!(ranked.len(), 1);
    // "s" of src and "a" of app.
    assert_eq!(ranked[0].indices, [0, 4]);
    let mut matcher = matcher(true);
    assert_eq!(highlight(&pattern("sa"), &mut matcher, "src/app.rs"), [0, 4]);
    // Characters, not bytes.
    assert_eq!(highlight(&pattern("é"), &mut matcher, "dir/éa.rs"), [4]);
}

#[test]
fn a_cancelled_ranking_stops_and_gives_nothing() {
    let items = paths(&vec!["src/app.rs"; 5000]);
    let asked = std::cell::Cell::new(0);
    let cancel = || {
        asked.set(asked.get() + 1);
        asked.get() > 3
    };
    assert!(rank(&items, "app", &cancel).is_empty());
    // (Asked every few hundred rows, not at the end only.)
    assert_eq!(asked.get(), 4);
    assert_eq!(rank(&items, "app", &|| false).len(), 5000);
}

#[test]
fn the_worker_answers_the_newest_query_and_drops_the_others() {
    let worker = RankWorker::new(|| {});
    worker.add(paths(&["src/app.rs", "src/picker.rs", "README.md"]));
    for (version, text) in [(1, "pick"), (2, "app")] {
        worker.query(version, text);
    }
    let mut last = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while last.as_ref().is_none_or(|d: &Done| d.version != 2) && Instant::now() < deadline {
        while let Ok(done) = worker.results.try_recv() {
            last = Some(done);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let done = last.expect("an answer");
    assert_eq!(done.version, 2);
    let matches = done.matches.expect("a query");
    assert_eq!(matches.iter().map(|m| m.index).collect::<Vec<_>>(), [0]);
    // An empty query lists everything, in order, without ranking.
    worker.query(3, "  ");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut empty = None;
    while empty.is_none() && Instant::now() < deadline {
        empty = worker.results.try_iter().find(|d| d.version == 3);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(empty.expect("an answer").matches.is_none());
}

// ---- Rows ----

fn nvim_item(text: &str, path: &str, group: &str) -> PickerItem {
    PickerItem { text: text.into(), path: Some(path.into()), group: Some(group.into()), ..Default::default() }
}

#[test]
fn smart_rows_are_buffers_then_recent_then_the_walk_each_file_once() {
    let mut items = Items::new(
        PathBuf::from("/w"),
        // (The file being edited isn't offered.)
        Some("/w/here.rs".into()),
        vec![
            nvim_item("b.rs", "/w/b.rs", "buffer"),
            nvim_item("a.rs", "/w/a.rs", "buffer"),
            nvim_item("old.rs", "/w/old.rs", "recent"),
            // Twice in Neovim's list: once.
            nvim_item("a.rs", "/w/a.rs", "recent"),
        ],
    );
    let texts = |items: &Items| items.rows.iter().map(|r| r.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(&items), ["b.rs", "a.rs", "old.rs"]);
    // The walk finds all of them and more: the new ones follow.
    let added = items.add_walked(vec!["a.rs".into(), "b.rs".into(), "c.rs".into(), "here.rs".into(), "old.rs".into(), "sub/d.rs".into()]);
    assert_eq!(added, 3..5);
    assert_eq!(texts(&items), ["b.rs", "a.rs", "old.rs", "c.rs", "sub/d.rs"]);
    // Neovim's rows are named by their place in its list; the walk's by their path.
    assert_eq!(items.rows[1].nvim, Some(2));
    assert_eq!(items.rows[1].group, Group::Buffer);
    assert_eq!((items.rows[3].nvim, items.rows[3].path.as_deref()), (None, Some("/w/c.rs")));
    // A later batch is checked against the earlier ones as well.
    assert_eq!(items.add_walked(vec!["c.rs".into()]), 5..5);
}

#[test]
fn a_word_is_taken_off_the_end_at_the_cursor() {
    assert_eq!(delete_word_back("foo bar", 7), ("foo ".into(), 4));
    assert_eq!(delete_word_back("foo bar  ", 9), ("foo ".into(), 4), "spaces, then the word");
    // A path goes a part at a time.
    assert_eq!(delete_word_back("src/app", 7), ("src/".into(), 4));
    assert_eq!(delete_word_back("src/", 4), ("src".into(), 3));
    // In the middle: the rest stays.
    assert_eq!(delete_word_back("foo bar baz", 7), ("foo  baz".into(), 4));
    assert_eq!(delete_word_back("", 0), (String::new(), 0));
    assert_eq!(delete_word_back("abc", 0), ("abc".into(), 0));
    // Characters, not bytes.
    assert_eq!(delete_word_back("שלום עולם", 9), ("שלום ".into(), 5));
}

#[test]
fn a_folder_is_cut_to_the_room_it_has() {
    let ten = |s: &str| s.chars().count() as f32 * 10.0;
    assert_eq!(truncate_to_width("src/app", 70.0, &ten), ("src/app".into(), 7), "fits");
    assert_eq!(truncate_to_width("src/application", 100.0, &ten), ("src/appli…".into(), 9));
    // Room for the "…" alone, or not even that.
    assert_eq!(truncate_to_width("src", 10.0, &ten), ("…".into(), 0));
    assert_eq!(truncate_to_width("src", 5.0, &ten), (String::new(), 0));
    // Whole characters of any width.
    assert_eq!(truncate_to_width("éééé", 30.0, &ten), ("éé…".into(), 2));
    assert_eq!(split_path("src/ui/app.rs"), ("src/ui", "app.rs"));
    assert_eq!(split_path("app.rs"), ("", "app.rs"));
}

#[test]
fn the_selection_wraps_and_a_page_stops_at_the_ends() {
    assert_eq!(step(0, 5, -1), 4);
    assert_eq!(step(4, 5, 1), 0);
    assert_eq!(step(2, 5, 1), 3);
    assert_eq!(step(0, 0, 1), 0);
    assert_eq!(step_clamped(2, 20, 12), 14);
    assert_eq!(step_clamped(15, 20, 12), 19);
    assert_eq!(step_clamped(3, 20, -12), 0);
}

// ---- The walk ----

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eshvi-picker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn collect(walk: &Walk) -> Vec<String> {
    let mut files = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match walk.found.recv_timeout(Duration::from_millis(50)) {
            Ok(Found::Files(batch)) => files.extend(batch),
            Ok(Found::Done) => break,
            Err(_) => {}
        }
    }
    files.sort();
    files
}

#[test]
fn the_walk_skips_ignored_and_hidden_files_and_does_not_follow_links() {
    let dir = temp("walk");
    let write = |path: &str| {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
    };
    // (No .git here: the .gitignore counts all the same.)
    write(".gitignore");
    std::fs::write(dir.join(".gitignore"), "ignored/\n*.log\n").unwrap();
    write("top.txt");
    write("visible/b.txt");
    write("ignored/x.txt");
    write("noise.log");
    write(".hidden");
    write(".hidden_dir/inside.txt");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.join("top.txt"), dir.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(dir.join("visible"), dir.join("linkdir")).unwrap();
    }
    let walk = Walk::files(dir.clone(), || {});
    let files = collect(&walk);
    #[cfg(unix)]
    assert_eq!(files, ["link.txt", "top.txt", "visible/b.txt"], "a link to a file is a file; a link to a folder is not entered");
    #[cfg(not(unix))]
    assert_eq!(files, ["top.txt", "visible/b.txt"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn git_files_are_the_tracked_ones_and_outside_a_repository_the_walk() {
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git").args(args).current_dir(dir).output();
        out.map(|o| o.status.success()).unwrap_or(false)
    };
    let dir = temp("git");
    // (Not a repository: everything the walk finds.)
    std::fs::write(dir.join("tracked.txt"), "x").unwrap();
    std::fs::write(dir.join("untracked.txt"), "x").unwrap();
    assert_eq!(collect(&Walk::git(dir.clone(), || {})), ["tracked.txt", "untracked.txt"]);
    if git(&dir, &["init", "-q"]) && git(&dir, &["add", "tracked.txt"]) {
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/deep.txt"), "x").unwrap();
        assert!(git(&dir, &["add", "sub/deep.txt"]));
        assert_eq!(collect(&Walk::git(dir.clone(), || {})), ["sub/deep.txt", "tracked.txt"]);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dropped_walk_stops() {
    let dir = temp("drop");
    let walk = Walk::files(dir.clone(), || {});
    let flag = walk.cancel_flag();
    assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));
    // Dropping the Walk is what the picker's closing does.
    drop(walk);
    assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The box ----

struct Env {
    view: PickerView,
    icons: IconTextures,
    actions: Vec<Action>,
    fonts: Option<GridFonts>,
    open: Option<PickerOpen>,
    /// Keys Neovim handed over (`gui_picker_typed`), given to the picker before the next frame.
    typed: Vec<(i64, Vec<String>)>,
    /// Keys are held for what Neovim hands over, until a test sends it (else the
    /// harness hands over nothing at once, as a Neovim with nothing typed does).
    handshake: bool,
    chrome: Chrome,
    theme: PopupTheme,
    code: CodeTheme,
    size: Vec2,
    placement: PreviewPlacement,
}

fn files_open(id: i64, texts: &[&str]) -> PickerOpen {
    PickerOpen {
        id,
        title: "Find files".into(),
        source: "items".into(),
        cwd: "/w".into(),
        files: true,
        multi: true,
        exclude: None,
        items: texts
            .iter()
            .map(|t| PickerItem { text: (*t).into(), path: Some(format!("/w/{t}")), ..Default::default() })
            .collect(),
        ..Default::default()
    }
}

fn items_open(id: i64, rows: &[(&str, &str)]) -> PickerOpen {
    PickerOpen {
        id,
        title: "Select one".into(),
        source: "items".into(),
        cwd: "/w".into(),
        files: false,
        multi: false,
        exclude: None,
        items: rows.iter().map(|(t, d)| PickerItem { text: (*t).into(), desc: (*d).into(), ..Default::default() }).collect(),
        ..Default::default()
    }
}

fn harness(theme: &str, width: f32, height: f32, open: PickerOpen) -> Harness<'static, Env> {
    harness_with(theme, width, height, Some(open))
}

/// A harness whose picker may open later: set `state_mut().open`.
fn harness_with(theme: &str, width: f32, height: f32, open: Option<PickerOpen>) -> Harness<'static, Env> {
    let (_, chrome, popup) = themes().into_iter().find(|(n, ..)| *n == theme).unwrap();
    let env = Env {
        view: PickerView::default(),
        icons: IconTextures::default(),
        actions: Vec::new(),
        fonts: None,
        open,
        typed: Vec::new(),
        handshake: false,
        code: CodeTheme::new(chrome.bg, chrome.fg, popup.accent),
        chrome,
        theme: popup,
        size: Vec2::new(width, height),
        placement: PreviewPlacement::default(),
    };
    // (More frames than egui_kittest's 4 to settle: the preview's wait before it asks Neovim
    // runs out during them on a slow CI runner, a repaint the first frames can't avoid.)
    let mut harness = Harness::builder().with_size(env.size).with_max_steps(16).with_pixels_per_point(2.0).with_accessibility_check(false).allow_missing_glyphs().wgpu().build_ui_state(
        |ui, env: &mut Env| {
            let ctx = ui.ctx().clone();
            ctx.set_theme(if env.chrome.bg.r() < 128 { egui::Theme::Dark } else { egui::Theme::Light });
            crate::style::apply_to_egui(&ctx, &env.theme);
            // Real fonts: they arrive the frame after they're installed.
            let fonts = env.fonts.get_or_insert_with(|| fonts::install(&ctx, &FontSpec::default_with_size(14.0), None, None, None, None)).clone();
            if !fonts.is_live(&ctx) {
                return;
            }
            let area = Rect::from_min_size(Pos2::ZERO, env.size);
            ui.painter().rect_filled(area, 0.0, env.chrome.bg);
            if let Some(open) = env.open.take() {
                let id = open.id;
                env.view.open(&ctx, open);
                if !env.handshake {
                    env.view.typed(&ctx, id, &[]);
                }
            }
            for (id, keys) in std::mem::take(&mut env.typed) {
                env.view.typed(&ctx, id, &keys);
            }
            let look = Look {
                theme: &env.theme,
                chrome: &env.chrome,
                fonts: &fonts,
                metrics: CellMetrics { width: 8.0, height: 20.0 },
                area,
                code: &env.code,
                problems: [Color32::from_rgb(0xf1, 0x4c, 0x4c), Color32::from_rgb(0xcc, 0xa7, 0x00), Color32::from_rgb(0x37, 0x94, 0xff)],
                preview: env.placement,
            };
            let actions = env.view.show(&ctx, &look, &mut env.icons);
            env.actions.extend(actions);
        },
        env,
    );
    harness.run();
    harness
}

/// Frames until the lists are of the latest query and the walk is done.
fn settle(h: &mut Harness<'static, Env>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    h.run();
    while Instant::now() < deadline && !(h.state().view.settled() && !h.state().view.walking()) {
        std::thread::sleep(Duration::from_millis(5));
        h.step();
    }
    h.run();
}

fn type_text(h: &mut Harness<'static, Env>, text: &str) {
    h.input_mut().events.push(Event::Text(text.into()));
    settle(h);
}

fn press(h: &mut Harness<'static, Env>, modifiers: Modifiers, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    settle(h);
}

const FILES: &[&str] = &["src/app.rs", "src/picker.rs", "Cargo.toml", "README.md", "docs/app_notes.md", "src/app/tabs.rs"];

fn accepted(h: &mut Harness<'static, Env>) -> Vec<Action> {
    // (That the box is up is its own test's.)
    std::mem::take(&mut h.state_mut().actions).into_iter().filter(|a| !matches!(a, Action::Opened(_))).collect()
}

fn choice(idx: usize) -> PickerChoice {
    PickerChoice { idx: Some(idx), ..Default::default() }
}

#[test]
fn typing_narrows_the_list_and_enter_opens_the_selected_row() {
    let mut h = harness("dark", 800.0, 500.0, files_open(7, FILES));
    assert_eq!(h.state().view.summary(), Some(("", 6, 6, 0)));
    assert_eq!(h.state().view.listed(), FILES);
    type_text(&mut h, "app");
    let (query, matches, total, selected) = h.state().view.summary().unwrap();
    assert_eq!((query, matches, total, selected), ("app", 3, 6, 0));
    assert_eq!(h.state().view.listed(), ["src/app.rs", "docs/app_notes.md", "src/app/tabs.rs"]);
    // Down, and Enter: the second match, named by its place in Neovim's list.
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert_eq!(h.state().view.summary().unwrap().3, 1);
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(accepted(&mut h), [Action::Accept { id: 7, chosen: vec![choice(5)], how: "edit" }]);
}

#[test]
fn the_selection_goes_back_to_the_top_when_the_query_changes() {
    let mut h = harness("dark", 800.0, 500.0, files_open(1, FILES));
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert_eq!(h.state().view.summary().unwrap().3, 2);
    type_text(&mut h, "s");
    assert_eq!(h.state().view.summary().unwrap().3, 0);
}

#[test]
fn escape_cancels_and_the_keys_never_reach_the_field() {
    let mut h = harness("dark", 800.0, 500.0, files_open(3, FILES));
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert_eq!(accepted(&mut h), [Action::Cancel(3)]);
    // Enter, Tab and the arrows are the picker's: the query stays empty.
    for key in [Key::Enter, Key::Tab, Key::ArrowUp] {
        press(&mut h, Modifiers::NONE, key);
    }
    assert_eq!(h.state().view.summary().unwrap().0, "");
}

#[test]
fn the_move_keys_wrap_round_and_a_page_does_not() {
    let rows: Vec<String> = (0..30).map(|i| format!("file{i:02}.txt")).collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let mut h = harness("dark", 800.0, 500.0, files_open(1, &rows));
    let selected = |h: &Harness<'static, Env>| h.state().view.summary().unwrap().3;
    for (modifiers, key, want) in [
        (Modifiers::NONE, Key::ArrowDown, 1),
        (Modifiers::CTRL, Key::N, 2),
        (Modifiers::CTRL, Key::J, 3),
        (Modifiers::NONE, Key::ArrowUp, 2),
        (Modifiers::CTRL, Key::P, 1),
        (Modifiers::CTRL, Key::K, 0),
        // Up from the first row wraps to the last.
        (Modifiers::NONE, Key::ArrowUp, 29),
        (Modifiers::NONE, Key::ArrowDown, 0),
        (Modifiers::NONE, Key::PageDown, 12),
        (Modifiers::NONE, Key::PageDown, 24),
        (Modifiers::NONE, Key::PageDown, 29),
        (Modifiers::NONE, Key::PageUp, 17),
    ] {
        press(&mut h, modifiers, key);
        assert_eq!(selected(&h), want, "{key:?}");
    }
    // None of them typed anything.
    assert_eq!(h.state().view.summary().unwrap().0, "");
}

#[test]
fn tab_marks_and_moves_on_and_enter_then_sends_them_to_the_quickfix_list() {
    let mut h = harness("dark", 800.0, 500.0, files_open(4, FILES));
    press(&mut h, Modifiers::NONE, Key::Tab);
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(h.state().view.marked(), ["src/app.rs", "src/picker.rs"]);
    assert_eq!(h.state().view.summary().unwrap().3, 2, "on to the next row");
    // Shift-Tab marks and goes up (Cargo.toml, then picker.rs is unmarked); a marked row's mark comes off.
    press(&mut h, Modifiers::SHIFT, Key::Tab);
    press(&mut h, Modifiers::SHIFT, Key::Tab);
    assert_eq!(h.state().view.marked(), ["src/app.rs", "Cargo.toml"]);
    assert_eq!(h.state().view.summary().unwrap().3, 0);
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(accepted(&mut h), [Action::Accept { id: 4, chosen: vec![choice(1), choice(3)], how: "quickfix" }]);
}

#[test]
fn tab_does_nothing_where_marking_is_off() {
    let mut h = harness("dark", 800.0, 500.0, items_open(2, &[("one", ""), ("two", "")]));
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert!(h.state().view.marked().is_empty());
    assert_eq!(h.state().view.summary().unwrap().3, 0, "and it does not move");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(accepted(&mut h), [Action::Accept { id: 2, chosen: vec![choice(1)], how: "edit" }]);
}

#[test]
fn control_s_v_and_t_open_in_a_split_a_vertical_split_and_a_tab() {
    for (key, how) in [(Key::S, "split"), (Key::V, "vsplit"), (Key::T, "tab")] {
        let mut h = harness("dark", 800.0, 500.0, files_open(5, FILES));
        press(&mut h, Modifiers::CTRL, key);
        assert_eq!(accepted(&mut h), [Action::Accept { id: 5, chosen: vec![choice(1)], how }], "{key:?}");
    }
}

#[test]
fn control_u_clears_the_query_and_control_w_takes_a_word_off() {
    let mut h = harness("dark", 800.0, 500.0, files_open(1, FILES));
    type_text(&mut h, "src/app");
    press(&mut h, Modifiers::CTRL, Key::W);
    assert_eq!(h.state().view.summary().unwrap().0, "src/");
    press(&mut h, Modifiers::CTRL, Key::W);
    assert_eq!(h.state().view.summary().unwrap().0, "src");
    // Typing goes on after the cut.
    type_text(&mut h, "/pi");
    assert_eq!(h.state().view.summary().unwrap().0, "src/pi");
    assert_eq!(h.state().view.listed()[0], "src/picker.rs");
    press(&mut h, Modifiers::CTRL, Key::U);
    assert_eq!(h.state().view.summary(), Some(("", 6, 6, 0)));
}

#[test]
fn hebrew_and_accents_are_typed_and_matched() {
    let mut h = harness("dark", 800.0, 500.0, files_open(1, &["docs/שלום.md", "docs/café.md", "docs/plain.md"]));
    type_text(&mut h, "של");
    assert_eq!(h.state().view.listed(), ["docs/שלום.md"]);
    press(&mut h, Modifiers::CTRL, Key::U);
    type_text(&mut h, "café");
    assert_eq!(h.state().view.listed(), ["docs/café.md"]);
}

#[test]
fn a_click_on_a_row_opens_it() {
    let mut h = harness("dark", 800.0, 500.0, files_open(6, FILES));
    let row = h.state().view.row_rect(3).expect("the fourth row is drawn");
    h.hover_at(row.center());
    h.run();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: row.center(), button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run();
    }
    assert_eq!(accepted(&mut h), [Action::Accept { id: 6, chosen: vec![choice(4)], how: "edit" }]);
}

#[test]
fn the_mouse_over_a_row_draws_no_second_highlight() {
    let mut h = harness("dark", 800.0, 500.0, files_open(6, FILES));
    let row = h.state().view.row_rect(3).expect("the fourth row is drawn");
    // (The left half of the row, with the mouse at its right end: the test's renderer draws a pointer.)
    let left_half = |image: &image::RgbaImage| -> Vec<image::Rgba<u8>> {
        let (x, y) = ((row.min.x * 2.0) as u32, (row.min.y * 2.0) as u32);
        let (w, h) = ((row.width()) as u32, (row.height() * 2.0) as u32);
        (y..y + h).flat_map(|py| (x..x + w).map(move |px| (px, py))).map(|(px, py)| *image.get_pixel(px, py)).collect()
    };
    h.hover_at(Pos2::new(5.0, 495.0));
    h.run();
    let away = left_half(&h.render().unwrap());
    h.hover_at(Pos2::new(row.max.x - 20.0, row.center().y));
    h.run();
    assert!(away == left_half(&h.render().unwrap()), "the row under the mouse looks the same as without it");
    assert_eq!(h.state().view.state.as_ref().unwrap().selected, 0, "and the selection stays where the keyboard left it");
}

#[test]
fn the_box_is_at_the_top_center_and_as_wide_as_it_can_be() {
    let h = harness("dark", 1000.0, 600.0, files_open(1, FILES));
    let rect = h.state().view.rect().unwrap();
    assert_eq!(rect.width(), WIDTH);
    assert!((rect.center().x - 500.0).abs() < 1.0, "{rect:?}");
    assert!(rect.min.y < 20.0, "{rect:?}");
    // A narrow window: inside it, with its margins.
    let h = harness("dark", 300.0, 400.0, files_open(1, FILES));
    let rect = h.state().view.rect().unwrap();
    assert!(rect.min.x >= 0.0 && rect.max.x <= 300.0, "{rect:?}");
}

#[test]
fn the_box_keeps_its_height_when_the_list_shrinks() {
    let rows: Vec<String> = (0..40).map(|i| format!("src/file{i:02}.rs")).collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let mut h = harness("dark", 800.0, 600.0, files_open(1, &rows));
    let tall = h.state().view.rect().unwrap().height();
    type_text(&mut h, "file03");
    assert_eq!(h.state().view.summary().unwrap().1, 1);
    assert_eq!(h.state().view.rect().unwrap().height(), tall, "one row");
    type_text(&mut h, "x");
    assert_eq!(h.state().view.summary().unwrap().1, 0);
    assert_eq!(h.state().view.rect().unwrap().height(), tall, "no results");
    press(&mut h, Modifiers::CTRL, Key::U);
    assert_eq!(h.state().view.rect().unwrap().height(), tall);
}

#[test]
fn the_list_shows_twelve_rows_and_scrolls_to_the_selection() {
    let rows: Vec<String> = (0..40).map(|i| format!("file{i:02}.txt")).collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let mut h = harness("dark", 800.0, 600.0, files_open(1, &rows));
    assert!(h.state().view.row_rect(11).is_some());
    assert!(h.state().view.row_rect(15).is_none(), "only the rows in view are made");
    for _ in 0..20 {
        press(&mut h, Modifiers::NONE, Key::ArrowDown);
    }
    h.run();
    let rect = h.state().view.rect().unwrap();
    let selected = h.state().view.row_rect(20).expect("the selected row is drawn");
    assert!(selected.min.y >= rect.min.y && selected.max.y <= rect.max.y, "{selected:?} in {rect:?}");
}

#[test]
fn a_walk_fills_the_list_as_files_arrive() {
    let dir = temp("box");
    for name in ["alpha.rs", "beta.rs", "sub/gamma.rs"] {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
    }
    let open = PickerOpen {
        id: 1,
        title: "Find files".into(),
        source: "smart".into(),
        cwd: dir.to_string_lossy().into_owned(),
        files: true,
        multi: true,
        exclude: Some(dir.join("alpha.rs").to_string_lossy().into_owned()),
        // A buffer first; the walk finds it again.
        items: vec![nvim_item("beta.rs", &dir.join("beta.rs").to_string_lossy(), "buffer")],
        ..Default::default()
    };
    let mut h = harness("dark", 800.0, 500.0, open);
    settle(&mut h);
    // (The one being edited, alpha.rs, is left out.)
    assert_eq!(h.state().view.listed(), ["beta.rs", "sub/gamma.rs"]);
    type_text(&mut h, "gam");
    assert_eq!(h.state().view.listed(), ["sub/gamma.rs"]);
    // A file the walk found is named by its path, not a place in Neovim's list.
    press(&mut h, Modifiers::NONE, Key::Enter);
    let path = dir.join("sub/gamma.rs").to_string_lossy().into_owned();
    assert_eq!(
        accepted(&mut h),
        [Action::Accept { id: 1, chosen: vec![PickerChoice { path: Some(path), ..Default::default() }], how: "edit" }]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_input_is_kept_from_neovim() {
    let key = Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
    for ev in [key, Event::Text("a".into()), Event::Paste("a".into()), Event::Copy, Event::PointerMoved(Pos2::ZERO)] {
        assert!(takes_input(&ev), "{ev:?}");
    }
    assert!(!takes_input(&Event::WindowFocused(true)));
}

// ---- Pictures ----

fn snapshots(name: &str, width: f32, height: f32, open: impl Fn() -> PickerOpen, act: impl Fn(&mut Harness<'static, Env>)) {
    let mut results = egui_kittest::SnapshotResults::new();
    for (theme, ..) in themes() {
        let mut h = harness(theme, width, height, open());
        settle(&mut h);
        act(&mut h);
        h.run();
        h.snapshot(format!("picker_{name}_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn file_list_with_matches_highlighted_and_one_selected() {
    snapshots(
        "files",
        720.0,
        360.0,
        || {
            let mut open = files_open(1, FILES);
            open.items[4].group = Some("buffer".into());
            open.items.push(PickerItem {
                text: "crates/app/src/picker/very/deeply/nested/folders/that/need/cutting/short/app_helper.rs".into(),
                path: Some("/w/x".into()),
                ..Default::default()
            });
            open
        },
        |h| {
            type_text(h, "app");
            press(h, Modifiers::NONE, Key::Tab);
        },
    );
}

#[test]
fn file_list_before_typing() {
    snapshots("files_empty", 720.0, 360.0, || files_open(1, FILES), |_| {});
}

#[test]
fn items_with_a_description() {
    snapshots(
        "items",
        720.0,
        300.0,
        || {
            items_open(
                1,
                &[
                    ("Rename symbol", "textDocument/rename"),
                    ("Extract function", "refactor.extract"),
                    ("Organize imports", "source.organizeImports"),
                    ("Add missing impl", "quickfix"),
                ],
            )
        },
        |h| {
            type_text(h, "re");
        },
    );
}

#[test]
fn no_results() {
    snapshots("none", 720.0, 300.0, || files_open(1, FILES), |h| type_text(h, "zzzq"));
}

#[test]
fn a_narrow_window() {
    snapshots(
        "narrow",
        280.0,
        300.0,
        || {
            let mut open = files_open(1, FILES);
            open.items.push(PickerItem {
                text: "crates/app/src/picker/very/deeply/nested/app_helper.rs".into(),
                path: Some("/w/x".into()),
                ..Default::default()
            });
            open
        },
        |h| type_text(h, "app"),
    );
}

// ---- Speed (run by hand: `cargo test --release -- --ignored --nocapture a_big_tree`) ----

#[test]
#[ignore]
fn a_big_tree_walks_and_ranks_fast_enough_to_type_into() {
    let dir = temp("big");
    let mut made = 0;
    for d in 0..200 {
        for s in 0..5 {
            let folder = dir.join(format!("pkg{d:03}/sub{s}/deep"));
            std::fs::create_dir_all(&folder).unwrap();
            for f in 0..100 {
                std::fs::File::create(folder.join(format!("file_{d:03}_{s}_{f:03}.rs"))).unwrap();
                made += 1;
            }
        }
    }
    let started = Instant::now();
    let walk = Walk::files(dir.clone(), || {});
    let mut first = None;
    let mut files = Vec::new();
    loop {
        match walk.found.recv_timeout(Duration::from_secs(60)) {
            Ok(Found::Files(batch)) => {
                first.get_or_insert(started.elapsed());
                files.extend(batch);
            }
            Ok(Found::Done) | Err(_) => break,
        }
    }
    eprintln!("walk: {} files of {made}, first batch after {:?}, all after {:?}", files.len(), first.unwrap(), started.elapsed());
    let items: Vec<RankItem> = files.iter().map(|t| RankItem { text: t.clone(), bonus: 0, path: true }).collect();
    for query in ["f", "file_15", "file_153_2", "pkg15 rs", "zzz", "deep 0042"] {
        let started = Instant::now();
        let ranked = rank(&items, query, &|| false);
        eprintln!("rank {query:?}: {} matches in {:?}", ranked.len(), started.elapsed());
    }
    let _ = std::fs::remove_dir_all(dir);
}
