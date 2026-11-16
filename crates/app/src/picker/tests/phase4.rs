//! Typing at once (what Neovim hands over), Enter before the rows are there, the
//! palette's rows with their shortcuts, a resumed picker, a live selection.

use super::*;

fn keys(list: &[&str]) -> Vec<String> {
    list.iter().map(|k| (*k).to_owned()).collect()
}

fn type_keys(h: &mut Harness<'static, Env>, id: i64, list: &[&str]) {
    h.state_mut().typed.push((id, keys(list)));
    settle(h);
}

fn query(h: &Harness<'static, Env>) -> String {
    h.state().view.summary().unwrap().0.to_owned()
}

/// Only what is asked of the app: not that the box is up.
fn asked(h: &mut Harness<'static, Env>) -> Vec<Action> {
    accepted(h)
}

// ---- That the box is up ----

#[test]
fn the_box_says_it_is_up_once_and_only_once() {
    let mut h = harness("dark", 800.0, 500.0, files_open(5, FILES));
    let opened = |h: &mut Harness<'static, Env>| {
        std::mem::take(&mut h.state_mut().actions).into_iter().filter(|a| matches!(a, Action::Opened(_))).count()
    };
    assert_eq!(opened(&mut h), 1);
    type_text(&mut h, "a");
    h.run();
    assert_eq!(opened(&mut h), 0, "not again");
}

// ---- What was typed before ----

#[test]
fn what_neovim_hands_over_is_typed_in_the_field_in_order() {
    let mut h = harness("dark", 800.0, 500.0, files_open(5, FILES));
    type_keys(&mut h, 5, &["a", "p", "p"]);
    assert_eq!(query(&h), "app");
    assert_eq!(h.state().view.listed(), ["src/app.rs", "docs/app_notes.md", "src/app/tabs.rs"]);
    // Backspace, a space, a `<` (`<lt>`); other keys (Ctrl, arrows) are dropped.
    type_keys(&mut h, 5, &["<BS>", "<Space>", "<lt>", "<C-x>", "<Down>", "é"]);
    assert_eq!(query(&h), "ap <é");
    // Typed on at the cursor, which is at the end.
    type_keys(&mut h, 5, &["<BS>", "<BS>", "<BS>", "<BS>", "<BS>", "<BS>", "<BS>"]);
    assert_eq!(query(&h), "");
}

#[test]
fn another_pickers_keys_are_not_typed_here() {
    let mut h = harness("dark", 800.0, 500.0, files_open(5, FILES));
    type_keys(&mut h, 4, &["a", "b"]);
    assert_eq!(query(&h), "");
}

#[test]
fn enter_among_the_typed_keys_opens_what_was_typed_for() {
    let mut h = harness("dark", 800.0, 500.0, files_open(5, FILES));
    // "pick" is src/picker.rs, the second file; Enter takes the first match once ranked.
    type_keys(&mut h, 5, &["p", "i", "c", "k", "<CR>"]);
    assert_eq!(accepted(&mut h), [Action::Accept { id: 5, chosen: vec![choice(2)], how: "edit" }]);
}

#[test]
fn enter_alone_opens_the_first_row_and_keys_after_it_are_dropped() {
    let mut h = harness("dark", 800.0, 500.0, files_open(5, FILES));
    type_keys(&mut h, 5, &["<CR>", "x", "y"]);
    assert_eq!(accepted(&mut h), [Action::Accept { id: 5, chosen: vec![choice(1)], how: "edit" }]);
    assert_eq!(query(&h), "", "nothing after the Enter is typed");
}

#[test]
fn escape_among_the_typed_keys_closes_the_box() {
    let mut h = harness("dark", 800.0, 500.0, files_open(5, FILES));
    type_keys(&mut h, 5, &["a", "<Esc>"]);
    assert_eq!(accepted(&mut h), [Action::Cancel(5)]);
}

#[test]
fn a_key_in_the_frame_the_box_opens_lands_in_the_field() {
    let mut h = harness_with("dark", 800.0, 500.0, None);
    h.state_mut().open = Some(files_open(2, FILES));
    h.input_mut().events.push(Event::Text("sr".into()));
    // (Nothing to hand over: Neovim says so.)
    h.state_mut().typed.push((2, vec![]));
    settle(&mut h);
    assert_eq!(query(&h), "sr");
    let mut listed = h.state().view.listed();
    listed.sort();
    assert_eq!(listed, ["src/app.rs", "src/app/tabs.rs", "src/picker.rs"]);
}

#[test]
fn keys_typed_after_the_box_is_up_wait_for_what_neovim_took_and_come_after_it() {
    // "c" was typed first and is Neovim's to hand over; "ou" came after the box was up.
    // Without the wait the field had "ou" and then "c".
    let mut h = harness_with("dark", 800.0, 500.0, None);
    h.state_mut().handshake = true;
    h.state_mut().open = Some(files_open(2, FILES));
    h.run();
    h.input_mut().events.push(Event::Text("o".into()));
    h.input_mut().events.push(Event::Text("u".into()));
    h.run();
    assert_eq!(query(&h), "", "held until Neovim has handed over what it took");
    h.state_mut().typed.push((2, keys(&["c"])));
    settle(&mut h);
    assert_eq!(query(&h), "cou");
    // And a key after that goes straight in.
    type_text(&mut h, "n");
    assert_eq!(query(&h), "coun");
}

#[test]
fn held_keys_go_to_the_field_anyway_if_neovim_never_answers() {
    let mut h = harness_with("dark", 800.0, 500.0, None);
    h.state_mut().handshake = true;
    h.state_mut().open = Some(files_open(2, FILES));
    h.run();
    h.input_mut().events.push(Event::Text("ab".into()));
    h.run();
    assert_eq!(query(&h), "");
    std::thread::sleep(Duration::from_millis(600));
    h.run();
    h.run();
    assert_eq!(query(&h), "ab");
}

// ---- Enter before the rows are there ----

fn live_rows(texts: &[&str]) -> Vec<PickerItem> {
    texts.iter().map(|t| PickerItem { text: (*t).into(), path: Some(format!("/w/{t}")), lnum: Some(1), ..Default::default() }).collect()
}

#[test]
fn enter_before_a_live_answer_takes_its_first_row_when_it_comes() {
    let mut h = harness(
        "dark",
        800.0,
        400.0,
        PickerOpen { id: 3, title: "Workspace symbols".into(), source: "items".into(), cwd: "/w".into(), live: true, ..Default::default() },
    );
    type_keys(&mut h, 3, &["f", "o", "o", "<CR>"]);
    // The question went out once the query had been still; the Enter waits for the answer.
    let gen = h
        .state()
        .actions
        .iter()
        .find_map(|a| match a {
            Action::Query { gen, text, .. } => {
                assert_eq!(text, "foo");
                Some(*gen)
            }
            _ => None,
        })
        .expect("the query is asked");
    assert!(
        h.state().actions.iter().all(|a| !matches!(a, Action::Accept { .. } | Action::Cancel(_))),
        "Enter must not open nothing: {:?}",
        h.state().actions
    );
    h.run();
    assert!(asked(&mut h).iter().all(|a| matches!(a, Action::Query { .. })));
    // The answer comes: the first row is opened, not the second.
    assert!(h.state_mut().view.set_items(3, gen, live_rows(&["src/first.rs", "src/second.rs"])));
    h.run();
    let got = accepted(&mut h);
    assert_eq!(got.len(), 1, "{got:?}");
    let Action::Accept { id, chosen, how } = &got[0] else { panic!("{got:?}") };
    assert_eq!((*id, *how), (3, "edit"));
    assert_eq!(chosen[0].path.as_deref(), Some("/w/src/first.rs"));
}

#[test]
fn enter_before_the_walk_has_files_takes_the_first_when_they_come() {
    let dir = temp("early-enter");
    for name in ["alpha.rs", "beta.rs", "gamma.rs"] {
        std::fs::write(dir.join(name), "x").unwrap();
    }
    let open = PickerOpen {
        id: 8,
        title: "Find files".into(),
        source: "files".into(),
        cwd: dir.to_string_lossy().into_owned(),
        files: true,
        multi: true,
        ..Default::default()
    };
    let mut h = harness_with("dark", 800.0, 400.0, Some(open));
    // The same frame as the open: the walk has had no time to be read.
    h.state_mut().typed.push((8, keys(&["<CR>"])));
    settle(&mut h);
    let got = accepted(&mut h);
    assert_eq!(got.len(), 1, "{got:?}");
    let Action::Accept { chosen, how, .. } = &got[0] else { panic!("{got:?}") };
    assert_eq!(*how, "edit");
    assert_eq!(chosen[0].path.as_deref(), Some(dir.join("alpha.rs").to_str().unwrap()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn enter_with_nothing_to_open_opens_nothing_and_the_box_stays() {
    let dir = temp("nothing-early");
    let open = PickerOpen {
        id: 8,
        source: "files".into(),
        cwd: dir.to_string_lossy().into_owned(),
        files: true,
        ..Default::default()
    };
    let mut h = harness_with("dark", 800.0, 400.0, Some(open));
    h.state_mut().typed.push((8, keys(&["<CR>"])));
    settle(&mut h);
    assert_eq!(accepted(&mut h), []);
    assert!(h.state().view.is_open());
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The palette's rows ----

fn palette_open() -> PickerOpen {
    let menu = |text: &str, key: &str, gate: &str| PickerItem { text: text.into(), hint: key.into(), gate: Some(gate.into()), ..Default::default() };
    let mut items = vec![
        menu("File: Save", "⌘S", "save"),
        menu("File: Save As…", "⇧⌘S", "save_as"),
        menu("Edit: Copy", "⌘C", "copy"),
        menu("View: Side Bar", "⌘B", "explorer"),
        menu("Go: Go to File…", "⌘P", "goto_file"),
        menu("Window: Close Split", "", "close_split"),
    ];
    for (name, desc) in [(":Format", "Format the file"), (":GrepFor", "Search with a pattern")] {
        items.push(PickerItem { text: name.into(), desc: desc.into(), ..Default::default() });
    }
    PickerOpen { id: 1, title: "Command palette".into(), source: "items".into(), cwd: "/w".into(), items, ..Default::default() }
}

#[test]
fn rows_left_out_by_the_gate_keep_their_places_in_neovims_list() {
    let from_nvim = palette_open().items;
    let rows = Items::gated(PathBuf::from("/w"), None, from_nvim, &|item| item.gate.as_deref() != Some("copy"));
    let texts: Vec<&str> = rows.rows.iter().map(|r| r.text.as_str()).collect();
    assert!(!texts.contains(&"Edit: Copy"));
    assert_eq!(texts.len(), 7);
    // "View: Side Bar" was the fourth in Neovim's list.
    let side = rows.rows.iter().find(|r| r.text == "View: Side Bar").unwrap();
    assert_eq!((side.nvim, side.hint.as_str()), (Some(4), "⌘B"));
}

#[test]
fn the_palette_is_filtered_and_a_row_is_named_by_its_place() {
    let mut h = harness("dark", 800.0, 400.0, palette_open());
    type_text(&mut h, "save");
    assert_eq!(h.state().view.listed(), ["File: Save", "File: Save As…"]);
    // A word of the description is not matched, the name is.
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(accepted(&mut h), [Action::Accept { id: 1, chosen: vec![choice(1)], how: "edit" }]);
}

// ---- A resumed picker ----

#[test]
fn a_resumed_picker_has_its_query_and_its_row() {
    let mut open = files_open(4, FILES);
    open.query = "app".into();
    open.want = Some(nvim_bridge::gui::PickerWant { text: "docs/app_notes.md".into(), path: Some("/w/docs/app_notes.md".into()), lnum: None });
    let mut h = harness("dark", 800.0, 400.0, open);
    settle(&mut h);
    assert_eq!(query(&h), "app");
    assert_eq!(h.state().view.listed(), ["src/app.rs", "docs/app_notes.md", "src/app/tabs.rs"]);
    assert_eq!(h.state().view.summary().unwrap().3, 1, "on the row it was on");
    // And it says so again when it closes.
    let state = h.state().view.state();
    assert_eq!(state.query, "app");
    assert_eq!(state.row.unwrap().text, "docs/app_notes.md");
}

#[test]
fn a_row_that_is_gone_leaves_the_selection_at_the_top() {
    let mut open = files_open(4, FILES);
    open.want = Some(nvim_bridge::gui::PickerWant { text: "deleted.rs".into(), path: None, lnum: None });
    let mut h = harness("dark", 800.0, 400.0, open);
    settle(&mut h);
    assert_eq!(h.state().view.summary().unwrap().3, 0);
}

// ---- A selection that is applied live ----

fn schemes_open() -> PickerOpen {
    let mut open = items_open(6, &[("alpha", ""), ("beta", ""), ("gamma", ""), ("delta", "")]);
    open.on_select = true;
    open.selected = 2;
    open
}

fn selected_actions(h: &mut Harness<'static, Env>) -> Vec<usize> {
    h.state_mut()
        .actions
        .drain(..)
        .filter_map(|a| match a {
            Action::Selected { idx, .. } => Some(idx),
            _ => None,
        })
        .collect()
}

#[test]
fn the_selection_is_told_to_neovim_once_it_has_stayed_a_moment() {
    let mut h = harness("dark", 800.0, 400.0, schemes_open());
    h.run();
    // The row it opens on is what Neovim has: nothing to say.
    std::thread::sleep(Duration::from_millis(150));
    h.run();
    assert!(selected_actions(&mut h).is_empty());
    // Down, Down: not at once, and then only the row it stays on.
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert!(selected_actions(&mut h).is_empty(), "not before it has stayed");
    std::thread::sleep(Duration::from_millis(150));
    h.run();
    assert_eq!(selected_actions(&mut h), [4], "the fourth row, once");
    h.run();
    assert!(selected_actions(&mut h).is_empty(), "and not again");
    // Back where it was: that is a change too.
    press(&mut h, Modifiers::NONE, Key::ArrowUp);
    press(&mut h, Modifiers::NONE, Key::ArrowUp);
    std::thread::sleep(Duration::from_millis(150));
    h.run();
    assert_eq!(selected_actions(&mut h), [2]);
}

#[test]
fn a_selection_that_moves_on_before_the_moment_is_never_told() {
    let mut h = harness("dark", 800.0, 400.0, schemes_open());
    h.run();
    // Past the fourth and on to the first, within the moment: only the last place is said.
    for _ in 0..2 {
        press(&mut h, Modifiers::NONE, Key::ArrowDown);
    }
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    std::thread::sleep(Duration::from_millis(150));
    h.run();
    assert_eq!(selected_actions(&mut h), [1]);
}

#[test]
fn a_picker_that_does_not_apply_its_selection_says_nothing() {
    let mut h = harness("dark", 800.0, 400.0, items_open(6, &[("a", ""), ("b", "")]));
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    std::thread::sleep(Duration::from_millis(150));
    h.run();
    assert!(selected_actions(&mut h).is_empty());
}

// ---- Pictures ----

#[test]
fn the_command_palette_with_its_shortcuts() {
    snapshots("palette", 720.0, 360.0, palette_open, |_| {});
}

#[test]
fn the_palette_narrowed_to_a_word() {
    snapshots("palette_save", 720.0, 360.0, palette_open, |h| type_text(h, "sav"));
}

#[test]
fn keymaps_with_their_mode() {
    let keymaps = || PickerOpen {
        id: 1,
        title: "Keymaps".into(),
        source: "items".into(),
        cwd: "/w".into(),
        items: [("n", "<Space>ff", "Find Files"), ("n", "gd", "Goto Definition"), ("i", "jk", "<Esc>"), ("x", "<Space>sw", "Visual selection or word"), ("t", "<C-x>", "<Lua function>")]
            .iter()
            .map(|(mode, lhs, desc)| PickerItem { text: (*lhs).into(), prefix: (*mode).into(), desc: (*desc).into(), ..Default::default() })
            .collect(),
        ..Default::default()
    };
    snapshots("keymaps", 720.0, 280.0, keymaps, |_| {});
}

/// A file's history: the short hash is the dim prefix, wider than a mode, and
/// the subject starts after it (it once overlapped).
#[test]
fn a_files_history_with_its_hashes() {
    let history = || PickerOpen {
        id: 1,
        title: "History of hist.lua".into(),
        source: "items".into(),
        cwd: "/w".into(),
        items: [
            ("5c4d99b", "Add a counter", "Ann, 1 day ago"),
            ("09d03c6", "Fix the farewell", "Bea, 2 days ago"),
            ("3964bb1", "Say shalom as well", "Ann, 4 days ago"),
            ("a8727e9", "Rename the module to hist.lua", "Bea, 11 days ago"),
        ]
        .iter()
        .map(|(hash, subject, desc)| PickerItem { text: (*subject).into(), prefix: (*hash).into(), desc: (*desc).into(), ..Default::default() })
        .collect(),
        ..Default::default()
    };
    snapshots("history", 720.0, 240.0, history, |_| {});
}

#[test]
fn registers_in_the_grid_font() {
    let registers = || PickerOpen {
        id: 1,
        title: "Registers".into(),
        source: "items".into(),
        cwd: "/w".into(),
        items: [("\"\"", "let x = 1;↵let y = 2;"), ("\"0", "yanked words"), ("\"a", "fn main() { go(); }"), ("\":", "write")]
            .iter()
            .map(|(name, desc)| PickerItem { text: (*name).into(), desc: (*desc).into(), mono: true, ..Default::default() })
            .collect(),
        ..Default::default()
    };
    snapshots("registers", 720.0, 240.0, registers, |_| {});
}
