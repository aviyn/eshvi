//! LSP locations, symbols, diagnostics and the live mode: the rows they make,
//! how they are filtered, and how a live picker asks for its rows.

use super::phase2::{code_answer, wait_for_request, CODE};
use super::*;
use crate::picker::preview::Subject;

fn locations_open() -> PickerOpen {
    let at = |path: &str, lnum: usize, line: &str, from: usize, to: usize| {
        let (lead, trimmed) = (line.len() - line.trim_start().len(), line.trim());
        PickerItem {
            text: path.into(),
            path: Some(format!("/w/{path}")),
            lnum: Some(lnum),
            col: Some(from + 1),
            search: Some(format!("{path} {trimmed}")),
            line: Some(nvim_bridge::gui::PickerLine {
                text: trimmed.into(),
                ranges: vec![(from - lead, to - lead)],
                line_ranges: vec![(from, to)],
            }),
            ..Default::default()
        }
    };
    PickerOpen {
        id: 1,
        title: "References".into(),
        source: "items".into(),
        cwd: "/w".into(),
        multi: true,
        preview: true,
        items: vec![
            at("src/lib.rs", 4, "fn needle(count: usize) -> String {", 3, 9),
            at("src/lib.rs", 5, "    let found = format!(\"{count} needles\"); // שלום", 8, 13),
            at("src/other/deep_module.rs", 12, "    let found = needle(count);", 16, 22),
            at("docs/guide.md", 3, "Call needle() to begin.", 5, 11),
        ],
        ..Default::default()
    }
}

fn symbols_open() -> PickerOpen {
    let symbol = |name: &str, kind: &str, depth: usize, desc: &str, lnum: usize| PickerItem {
        text: name.into(),
        kind: Some(kind.into()),
        depth,
        desc: desc.into(),
        lnum: Some(lnum),
        col: Some(1),
        buf: Some(1),
        path: Some("/w/src/lib.rs".into()),
        ..Default::default()
    };
    PickerOpen {
        id: 1,
        title: "Symbols".into(),
        source: "items".into(),
        cwd: "/w".into(),
        multi: true,
        preview: true,
        selected: 4,
        items: vec![
            symbol("fmt", "Module", 0, "", 1),
            symbol("Thing", "Struct", 0, "", 3),
            symbol("new", "Constructor", 1, "", 4),
            symbol("needle", "Function", 1, "", 4),
            symbol("count", "Field", 2, "", 5),
            symbol("found", "Variable", 2, "", 5),
            symbol("MAX", "Constant", 0, "lib", 7),
            symbol("Shape", "Interface", 0, "", 7),
            symbol("Kind", "Enum", 0, "", 7),
            symbol("Circle", "EnumMember", 1, "", 7),
        ],
        ..Default::default()
    }
}

fn diagnostics_open() -> PickerOpen {
    let d = |severity: u8, message: &str, path: &str, lnum: usize, from: usize, to: usize| PickerItem {
        text: message.into(),
        desc: format!("{path}:{lnum}"),
        search: Some(format!("{message} {path}")),
        severity: Some(severity),
        path: Some(format!("/w/{path}")),
        lnum: Some(lnum),
        col: Some(from + 1),
        marks: vec![(from, to)],
        ..Default::default()
    };
    PickerOpen {
        id: 1,
        title: "Diagnostics".into(),
        source: "items".into(),
        cwd: "/w".into(),
        multi: true,
        preview: true,
        items: vec![
            d(1, "cannot find value `found` in this scope", "src/lib.rs", 5, 4, 7),
            d(1, "mismatched types: expected `String`, found `usize`", "src/other/deep_module.rs", 12, 0, 4),
            d(2, "unused variable: `count`", "src/lib.rs", 4, 3, 8),
            d(3, "this function takes 1 argument", "docs/guide.md", 3, 0, 4),
            d(4, "consider using `format!` here", "src/lib.rs", 5, 4, 7),
        ],
        ..Default::default()
    }
}

fn live_open() -> PickerOpen {
    PickerOpen {
        id: 3,
        title: "Workspace symbols".into(),
        source: "items".into(),
        cwd: "/w".into(),
        multi: true,
        preview: true,
        live: true,
        hint: "Type to search symbols".into(),
        ..Default::default()
    }
}

fn listed(h: &Harness<'static, Env>) -> Vec<String> {
    h.state().view.listed()
}

// ---- Filtering ----

#[test]
fn symbols_are_filtered_by_their_name_alone() {
    let mut h = harness("dark", 1100.0, 480.0, symbols_open());
    settle(&mut h);
    assert_eq!(h.state().view.summary().map(|s| (s.1, s.3)), Some((10, 3)), "the row Neovim chose is selected on open");
    // The kind's word and the indent are no part of what is matched.
    for query in ["Function", "Method", "Struct", "Interface", "Variable"] {
        type_text(&mut h, query);
        let hits = listed(&h);
        let kinds = ["Function", "Struct", "Interface", "Variable"];
        assert!(hits.iter().all(|t| !kinds.contains(&t.as_str()) || t.as_str() == query), "{query}: {hits:?}");
        press(&mut h, Modifiers::CTRL, Key::U);
    }
    type_text(&mut h, "needle");
    assert_eq!(listed(&h), ["needle"]);
    press(&mut h, Modifiers::CTRL, Key::U);
    type_text(&mut h, "fo");
    assert_eq!(listed(&h), ["found"]);
    // Source order with no query.
    press(&mut h, Modifiers::CTRL, Key::U);
    assert_eq!(listed(&h)[..3], ["fmt", "Thing", "new"]);
    // Enter names the row Neovim knows it by.
    let Some(Action::Accept { chosen, .. }) = accepted_after_enter(&mut h) else { panic!("no accept") };
    assert_eq!(chosen[0].idx, Some(1));
}

fn accepted_after_enter(h: &mut Harness<'static, Env>) -> Option<Action> {
    accepted(h);
    press(h, Modifiers::NONE, Key::Enter);
    accepted(h).into_iter().find(|a| matches!(a, Action::Accept { .. }))
}

#[test]
fn locations_are_filtered_by_path_and_line_text() {
    let mut h = harness("dark", 1100.0, 480.0, locations_open());
    settle(&mut h);
    type_text(&mut h, "guide");
    assert_eq!(listed(&h), ["docs/guide.md"]);
    press(&mut h, Modifiers::CTRL, Key::U);
    type_text(&mut h, "deep found");
    assert_eq!(listed(&h), ["src/other/deep_module.rs"], "words match in the path and in the line");
    press(&mut h, Modifiers::CTRL, Key::U);
    type_text(&mut h, "format");
    assert_eq!(listed(&h), ["src/lib.rs"]);
    // The preview marks the stretch the location is.
    assert_eq!(h.state().view.preview_marks(), vec![8..13]);
    let Some(Action::Accept { chosen, .. }) = accepted_after_enter(&mut h) else { panic!() };
    assert_eq!((chosen[0].idx, chosen[0].lnum, chosen[0].col, chosen[0].text.as_deref()), (Some(2), Some(5), Some(9), Some("let found = format!(\"{count} needles\"); // שלום")));
}

#[test]
fn diagnostics_are_filtered_by_message_and_path() {
    let mut h = harness("dark", 1100.0, 480.0, diagnostics_open());
    settle(&mut h);
    type_text(&mut h, "'unused");
    assert_eq!(listed(&h), ["unused variable: `count`"]);
    press(&mut h, Modifiers::CTRL, Key::U);
    type_text(&mut h, "'guide");
    assert_eq!(listed(&h), ["this function takes 1 argument"], "the path is filtered too");
    press(&mut h, Modifiers::CTRL, Key::U);
    // The order is Neovim's, by severity, until a query ranks them.
    assert_eq!(listed(&h)[0], "cannot find value `found` in this scope");
    assert_eq!(h.state().view.preview_marks(), vec![4..7]);
}

// ---- The live mode ----

fn queries(h: &mut Harness<'static, Env>) -> Vec<(i64, String)> {
    accepted(h)
        .into_iter()
        .filter_map(|a| match a {
            Action::Query { gen, text, .. } => Some((gen, text)),
            _ => None,
        })
        .collect()
}

/// Frames until a query is sent (or `ms` have gone by).
fn until_query(h: &mut Harness<'static, Env>, ms: u64) -> Vec<(i64, String)> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    let mut sent = Vec::new();
    while Instant::now() < deadline && sent.is_empty() {
        std::thread::sleep(Duration::from_millis(10));
        h.step();
        sent.extend(queries(h));
    }
    sent
}

fn symbol_rows(names: &[&str]) -> Vec<PickerItem> {
    names
        .iter()
        .map(|n| PickerItem {
            text: (*n).into(),
            kind: Some("Function".into()),
            desc: "src/lib.rs".into(),
            path: Some("/w/src/lib.rs".into()),
            lnum: Some(3),
            col: Some(1),
            ..Default::default()
        })
        .collect()
}

/// When the live query last changed, while it waits to be sent.
fn live_pending<'a>(h: &'a mut Harness<'static, Env>) -> &'a mut Option<Instant> {
    &mut h.state_mut().view.state.as_mut().unwrap().live.as_mut().unwrap().pending
}

#[test]
fn a_live_query_is_sent_once_it_has_been_still_and_nothing_is_filtered_here() {
    let mut h = harness("dark", 1100.0, 480.0, live_open());
    settle(&mut h);
    assert!(queries(&mut h).is_empty(), "nothing is asked for an empty query");
    // Rows from an earlier query stay whatever is typed: the app filters nothing.
    h.input_mut().events.push(Event::Text("f".into()));
    h.step();
    assert!(queries(&mut h).is_empty(), "not before the query has been still");
    // Typing again starts the wait over. (A mark no wait could reach, not sleeps: a slow machine
    // overruns those.)
    let far = Instant::now() + Duration::from_secs(3600);
    *live_pending(&mut h) = Some(far);
    h.input_mut().events.push(Event::Text("o".into()));
    h.step();
    assert!(live_pending(&mut h).is_some_and(|since| since < far), "typing again starts the wait over");
    assert!(queries(&mut h).is_empty(), "the wait for the longer query has only begun");
    let sent = until_query(&mut h, 2000);
    assert_eq!(sent, [(1, "fo".to_string())], "the query as it was when it was still");
    assert!(h.state().view.live_waiting());
    // The answer: its rows, whatever they are called.
    assert!(h.state_mut().view.set_items(3, 1, symbol_rows(&["alpha", "beta"])));
    h.run();
    assert_eq!(listed(&h), ["alpha", "beta"], "no local filtering: neither holds `fo`");
    assert!(!h.state().view.live_waiting());
    // Another query: a new number.
    h.input_mut().events.push(Event::Text("x".into()));
    let sent = until_query(&mut h, 2000);
    assert_eq!(sent, [(2, "fox".to_string())]);
}

#[test]
fn an_older_answer_is_dropped_and_the_old_rows_stay_until_the_new_ones_come() {
    let mut h = harness("dark", 1100.0, 480.0, live_open());
    settle(&mut h);
    type_text(&mut h, "a");
    assert_eq!(until_query(&mut h, 2000), [(1, "a".to_string())]);
    assert!(h.state_mut().view.set_items(3, 1, symbol_rows(&["apple", "avocado"])));
    // Two queries in flight.
    h.input_mut().events.push(Event::Text("b".into()));
    assert_eq!(until_query(&mut h, 2000), [(2, "ab".to_string())]);
    h.input_mut().events.push(Event::Text("c".into()));
    assert_eq!(until_query(&mut h, 2000), [(3, "abc".to_string())]);
    // The rows of the first stay while nothing newer has come.
    h.run();
    assert_eq!(listed(&h), ["apple", "avocado"]);
    // The answer to the second arrives after the third was sent: dropped.
    assert!(!h.state_mut().view.set_items(3, 2, symbol_rows(&["stale"])));
    assert_eq!(listed(&h), ["apple", "avocado"]);
    // Another picker's answer is nobody's.
    assert!(!h.state_mut().view.set_items(9, 3, symbol_rows(&["other"])));
    assert!(h.state_mut().view.set_items(3, 3, symbol_rows(&["abacus"])));
    h.run();
    assert_eq!(listed(&h), ["abacus"]);
    // An answer twice is taken once.
    assert!(!h.state_mut().view.set_items(3, 3, symbol_rows(&["again"])));
}

#[test]
fn emptying_a_live_query_clears_the_rows_and_drops_an_answer_on_its_way() {
    let mut h = harness("dark", 1100.0, 480.0, live_open());
    settle(&mut h);
    type_text(&mut h, "ab");
    assert_eq!(until_query(&mut h, 2000), [(1, "ab".to_string())]);
    assert!(h.state_mut().view.set_items(3, 1, symbol_rows(&["abc"])));
    h.input_mut().events.push(Event::Text("d".into()));
    assert_eq!(until_query(&mut h, 2000), [(2, "abd".to_string())]);
    press(&mut h, Modifiers::CTRL, Key::U);
    h.run();
    assert!(listed(&h).is_empty(), "an empty query shows the hint, not rows");
    std::thread::sleep(Duration::from_millis(300));
    h.step();
    assert!(queries(&mut h).is_empty(), "and asks for nothing");
    assert!(!h.state_mut().view.set_items(3, 2, symbol_rows(&["late"])), "the answer on its way is not wanted");
    assert!(listed(&h).is_empty());
}

#[test]
fn a_live_row_is_accepted_by_what_it_holds() {
    let mut h = harness("dark", 1100.0, 480.0, live_open());
    settle(&mut h);
    type_text(&mut h, "a");
    until_query(&mut h, 2000);
    assert!(h.state_mut().view.set_items(3, 1, symbol_rows(&["alpha"])));
    h.run();
    let Some(Action::Accept { chosen, how, .. }) = accepted_after_enter(&mut h) else { panic!() };
    assert_eq!(how, "edit");
    // Its place in the list for a plugin's `on_choice` (a row may hold no path), and what it holds.
    assert_eq!((chosen[0].idx, chosen[0].path.as_deref(), chosen[0].lnum), (Some(1), Some("/w/src/lib.rs"), Some(3)));
}

// ---- Pictures ----

fn colors(theme: &str) -> [Color32; 3] {
    if theme == "dark" {
        [Color32::from_rgb(0xc6, 0x78, 0xdd), Color32::from_rgb(0x98, 0xc3, 0x79), Color32::from_rgb(0x5c, 0x63, 0x70)]
    } else {
        [Color32::from_rgb(0xa6, 0x26, 0xa4), Color32::from_rgb(0x50, 0xa1, 0x4f), Color32::from_rgb(0xa0, 0xa1, 0xa7)]
    }
}

fn picture(name: &str, width: f32, height: f32, open: fn() -> PickerOpen, act: impl Fn(&mut Harness<'static, Env>)) {
    let wants_preview = !open().items.is_empty();
    let mut results = egui_kittest::SnapshotResults::new();
    for (theme, ..) in themes() {
        let mut h = harness(theme, width, height, open());
        settle(&mut h);
        act(&mut h);
        if width >= 700.0 && wants_preview {
            let Action::Preview { request, .. } = wait_for_request(&mut h) else { unreachable!() };
            assert!(matches!(request.key, Subject::Path(_) | Subject::Buf(_)));
            code_answer(&mut h, &request, colors(theme));
        }
        h.run();
        h.snapshot(format!("picker_{name}_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
    let _ = CODE;
}

#[test]
fn references_with_a_preview_and_the_range_marked() {
    picture("references", 1100.0, 440.0, locations_open, |h| {
        accepted(h);
        press(h, Modifiers::NONE, Key::ArrowDown);
    });
}

#[test]
fn symbols_with_badges_and_the_one_at_the_cursor_selected() {
    picture("symbols", 1100.0, 440.0, symbols_open, |_| {});
}

#[test]
fn diagnostics_with_the_problems_icons_and_colors() {
    picture("diagnostics", 1100.0, 440.0, diagnostics_open, |h| {
        accepted(h);
        press(h, Modifiers::NONE, Key::ArrowDown);
    });
}

#[test]
fn a_live_picker_before_typing_says_what_to_do() {
    picture("live_hint", 1100.0, 440.0, live_open, |_| {});
}

#[test]
fn a_narrow_window_keeps_the_rows_inside_it() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (theme, ..) in themes() {
        let mut h = harness(theme, 300.0, 330.0, symbols_open());
        settle(&mut h);
        h.snapshot(format!("picker_symbols_narrow_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}
