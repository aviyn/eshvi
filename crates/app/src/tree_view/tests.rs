use super::*;
use crate::popup::PopupTheme;
use crate::scm_view::tests::system_font;
use crate::style::tests::themes;
use crate::style::Chrome;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

fn node(id: &str, label: &str, kind: &str, line: usize) -> TreeNode {
    TreeNode { id: id.into(), label: label.into(), kind: kind.into(), line: Some(line), ..Default::default() }
}

fn with(mut n: TreeNode, children: Vec<TreeNode>) -> TreeNode {
    n.children = children;
    n
}

/// A module with a function and a method that holds a variable, then a
/// variable and a class.
fn fixture() -> Vec<TreeNode> {
    let mut m = with(
        node("M", "M", "Module", 1),
        vec![
            node("M/foo", "foo", "Function", 2),
            with(node("M/bar", "bar", "Method", 3), vec![node("M/bar/inner", "inner", "Variable", 4)]),
        ],
    );
    m.hint = "mod".into();
    vec![m, node("tail", "tail", "Variable", 6), node("Alpha", "Alpha", "Class", 8)]
}

fn set(nodes: Vec<TreeNode>, gen: i64) -> TreeSet {
    TreeSet { view: "outline".into(), gen, nodes, ..Default::default() }
}

fn tree(nodes: Vec<TreeNode>) -> TreeView {
    let mut t = TreeView::new();
    t.set(set(nodes, 1));
    t
}

fn ids(t: &TreeView) -> Vec<String> {
    t.visible_ids()
}

fn k(t: &mut TreeView, key: Key) -> Vec<Action> {
    t.key(key, Modifiers::NONE)
}

#[test]
fn rows_are_the_open_nodes_with_their_depth_and_the_first_level_is_open_at_first() {
    let t = tree(fixture());
    assert_eq!(ids(&t), ["M", "M/foo", "M/bar", "tail", "Alpha"], "M is open, bar is not");
    let rows = t.rows();
    let depths: Vec<_> = rows.iter().map(|r| (r.depth, r.branch, r.open)).collect();
    assert_eq!(depths, [(0, true, true), (1, false, false), (1, true, false), (0, false, false), (0, false, false)]);
    // `expand` says how many levels: none, or all.
    let mut t = TreeView::new();
    t.set(TreeSet { expand: Some(0), ..set(fixture(), 1) });
    assert_eq!(ids(&t), ["M", "tail", "Alpha"]);
    t.set(TreeSet { expand: Some(5), ..set(fixture(), 2) });
    assert_eq!(ids(&t).len(), 6);
}

#[test]
fn what_the_user_opened_and_selected_stays_across_a_new_set_of_nodes() {
    let mut t = tree(fixture());
    // Open bar, close M's sibling-less state: M stays, and select inner.
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    assert_eq!(t.selected(), Some("M/bar"));
    k(&mut t, Key::ArrowRight);
    k(&mut t, Key::ArrowDown);
    assert_eq!((t.selected(), ids(&t).len()), (Some("M/bar/inner"), 6));
    // Typing refreshed the tree: a new gen, lines moved, a function added in M.
    let mut nodes = fixture();
    nodes[0].children.insert(0, node("M/new", "new", "Function", 2));
    nodes[0].children[2].line = Some(9);
    t.set(set(nodes, 2));
    assert_eq!(t.selected(), Some("M/bar/inner"), "the selection stays");
    assert_eq!(ids(&t), ["M", "M/new", "M/foo", "M/bar", "M/bar/inner", "tail", "Alpha"], "bar is still open");
    // One closed on purpose stays closed when its tree is sent again.
    k(&mut t, Key::ArrowLeft); // to bar
    k(&mut t, Key::ArrowLeft); // closes bar
    t.set(set(fixture(), 3));
    assert_eq!(ids(&t), ["M", "M/foo", "M/bar", "tail", "Alpha"]);
    // And a selection whose node is gone isn't drawn, and the first Down goes to the first row.
    t.select("gone", &[]);
    assert_eq!(k(&mut t, Key::ArrowDown), vec![]);
    assert_eq!(t.selected(), Some("M"));
}

#[test]
fn lazy_children_are_asked_for_once_and_shown_as_loading_until_they_come() {
    let lazy = || TreeNode { id: "lazy".into(), label: "lazy".into(), expandable: true, ..Default::default() };
    let closed = |nodes, gen| TreeSet { expand: Some(0), ..set(nodes, gen) };
    let mut t = TreeView::new();
    t.set(closed(vec![lazy(), node("x", "x", "", 1)], 1));
    assert!(t.request_missing().is_empty(), "closed: not asked");
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowRight);
    // Open and empty: one request, under the gen, and a Loading… line meanwhile.
    assert_eq!(t.request_missing(), vec![Action::Expand { id: "lazy".into(), gen: 1 }]);
    assert!(t.request_missing().is_empty(), "asked once");
    let rows = t.rows();
    assert_eq!(rows.iter().map(|r| r.node.map(|n| n.id.as_str())).collect::<Vec<_>>(), [Some("lazy"), None, Some("x")]);
    assert_eq!(rows[1].depth, 1);
    drop(rows);
    // An answer to an older gen is dropped (a new set came since: its gen is 2).
    t.set(closed(vec![lazy()], 2));
    t.set(TreeSet { parent: Some("lazy".into()), nodes: vec![node("old", "old", "", 1)], ..set(vec![], 1) });
    assert_eq!(ids(&t), ["lazy"], "still loading");
    // The new roots are open still, so it is asked for again under gen 2; the answer fills it.
    assert_eq!(t.request_missing(), vec![Action::Expand { id: "lazy".into(), gen: 2 }]);
    t.set(TreeSet { parent: Some("lazy".into()), nodes: vec![node("lazy/c", "c", "", 2)], ..set(vec![], 2) });
    assert_eq!(ids(&t), ["lazy", "lazy/c"]);
    assert!(t.request_missing().is_empty());
    // An empty answer is an answer: no more Loading…, no chevron.
    let mut t = TreeView::new();
    t.set(closed(vec![TreeNode { id: "e".into(), label: "e".into(), expandable: true, ..Default::default() }], 1));
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowRight);
    t.request_missing();
    t.set(TreeSet { parent: Some("e".into()), ..set(vec![], 1) });
    let rows = t.rows();
    assert_eq!((rows.len(), rows[0].branch), (1, false));
    assert!(t.request_missing().is_empty());
}

#[test]
fn selecting_with_a_path_opens_the_ancestors_and_scrolls_to_the_row() {
    // 60 leaves under one node in a small view.
    let leaves = || (0..60).map(|i| node(&format!("p/{i}"), &format!("leaf {i}"), "Variable", i + 2)).collect();
    let mut t = TreeView::new();
    t.set(TreeSet { expand: Some(0), ..set(vec![with(node("p", "p", "Module", 1), leaves())], 1) });
    assert_eq!(ids(&t), ["p"]);
    t.select("p/55", &["p".to_owned(), "p/55".to_owned()]);
    assert_eq!(t.selected(), Some("p/55"));
    assert_eq!(ids(&t).len(), 61, "p is open");
    let mut h = harness("dark", 260.0, 200.0, t);
    h.run();
    h.run();
    let offset = h.state().tree.offset;
    let row_h = side_bar_row_h();
    // Row 56 (from 0) is in view: the view is 200 high.
    assert!(offset <= 56.0 * row_h && 57.0 * row_h <= offset + 200.0, "offset {offset}");
    assert!(offset > 0.0);
    // Selected from Neovim once more, further up: it comes into view again.
    h.state_mut().tree.select("p/3", &["p".to_owned(), "p/3".to_owned()]);
    h.run();
    h.run();
    let offset = h.state().tree.offset;
    assert!(offset <= 4.0 * row_h && 5.0 * row_h <= offset + 200.0, "offset {offset}");
}

fn side_bar_row_h() -> f32 {
    crate::side_bar::row_height(13.0)
}

#[test]
fn the_keys_move_open_close_choose_and_leave() {
    let mut t = tree(fixture());
    // Down and j from nothing: the first row; Up at the top stays.
    assert!(k(&mut t, Key::ArrowDown).is_empty());
    assert_eq!(t.selected(), Some("M"));
    k(&mut t, Key::ArrowUp);
    assert_eq!(t.selected(), Some("M"));
    k(&mut t, Key::J);
    k(&mut t, Key::J);
    assert_eq!(t.selected(), Some("M/bar"));
    k(&mut t, Key::K);
    assert_eq!(t.selected(), Some("M/foo"));
    // End, Home.
    k(&mut t, Key::End);
    assert_eq!(t.selected(), Some("Alpha"));
    k(&mut t, Key::ArrowDown);
    assert_eq!(t.selected(), Some("Alpha"), "the last stays");
    k(&mut t, Key::Home);
    assert_eq!(t.selected(), Some("M"));
    // Right on an open node: its first child. Right on a closed one opens it; again, the first child.
    k(&mut t, Key::ArrowRight);
    assert_eq!(t.selected(), Some("M/foo"));
    k(&mut t, Key::ArrowRight);
    assert_eq!(t.selected(), Some("M/foo"), "a leaf: nothing");
    k(&mut t, Key::J);
    k(&mut t, Key::L);
    assert_eq!(ids(&t).len(), 6, "bar opened");
    assert_eq!(t.selected(), Some("M/bar"));
    k(&mut t, Key::L);
    assert_eq!(t.selected(), Some("M/bar/inner"));
    // Left on a leaf: its parent; on an open node: closes it; on a closed one: its parent.
    k(&mut t, Key::H);
    assert_eq!(t.selected(), Some("M/bar"));
    k(&mut t, Key::ArrowLeft);
    assert_eq!(ids(&t).len(), 5);
    assert_eq!(t.selected(), Some("M/bar"));
    k(&mut t, Key::ArrowLeft);
    assert_eq!(t.selected(), Some("M"));
    k(&mut t, Key::ArrowLeft);
    assert_eq!(ids(&t), ["M", "tail", "Alpha"], "M closed");
    // Enter opens, Space peeks; neither moves the selection.
    assert_eq!(k(&mut t, Key::Enter), vec![Action::Accept { id: "M".into(), how: How::Open }]);
    assert_eq!(k(&mut t, Key::Space), vec![Action::Accept { id: "M".into(), how: How::Peek }]);
    // Esc goes back to Neovim.
    assert_eq!(k(&mut t, Key::Escape), vec![Action::FocusEditor]);
    // Keys with Cmd or Ctrl are not the view's.
    assert!(t.key(Key::ArrowDown, Modifiers::COMMAND).is_empty());
    assert_eq!(t.selected(), Some("M"));
}

#[test]
fn page_up_and_page_down_move_a_pages_worth_of_rows() {
    let leaves: Vec<TreeNode> = (0..100).map(|i| node(&format!("{i}"), &format!("n{i}"), "", i + 1)).collect();
    let mut h = harness("dark", 260.0, 230.0, tree(leaves));
    h.run();
    let page = h.state().tree.page;
    assert!((9..=10).contains(&page), "230 / 23 rows fit: {page}");
    let t = &mut h.state_mut().tree;
    k(t, Key::Home);
    k(t, Key::PageDown);
    assert_eq!(t.selected(), Some(page.to_string().as_str()));
    k(t, Key::PageDown);
    assert_eq!(t.selected(), Some((2 * page).to_string().as_str()));
    k(t, Key::PageUp);
    assert_eq!(t.selected(), Some(page.to_string().as_str()));
    k(t, Key::PageUp);
    k(t, Key::PageUp);
    assert_eq!(t.selected(), Some("0"), "not past the top");
    k(t, Key::End);
    k(t, Key::PageDown);
    assert_eq!(t.selected(), Some("99"));
}

#[test]
fn children_are_sorted_by_position_name_or_kind_in_each_parent() {
    let mut t = tree(fixture());
    assert_eq!(ids(&t), ["M", "M/foo", "M/bar", "tail", "Alpha"]);
    t.sort = Sort::Name;
    assert_eq!(ids(&t), ["Alpha", "M", "M/bar", "M/foo", "tail"], "case-insensitive, within M too");
    t.sort = Sort::Kind;
    // Module, Class, Variable at the top; Method before Function inside.
    assert_eq!(ids(&t), ["M", "M/bar", "M/foo", "Alpha", "tail"]);
    t.sort = Sort::Position;
    assert_eq!(ids(&t), ["M", "M/foo", "M/bar", "tail", "Alpha"]);
}

#[test]
fn collapse_all_folds_everything_including_the_top_level_and_new_nodes_open_as_usual() {
    let mut t = tree(fixture());
    k(&mut t, Key::J);
    k(&mut t, Key::J);
    k(&mut t, Key::J);
    k(&mut t, Key::L);
    assert_eq!(ids(&t).len(), 6);
    t.collapse_all();
    assert_eq!(ids(&t), ["M", "tail", "Alpha"]);
    // A node that comes after is open by the first-level rule.
    let mut nodes = fixture();
    nodes.push(with(node("N", "N", "Module", 12), vec![node("N/a", "a", "", 13)]));
    t.set(set(nodes, 2));
    assert_eq!(ids(&t), ["M", "tail", "Alpha", "N", "N/a"]);
}

#[test]
fn the_selection_follows_a_line_to_the_last_node_starting_at_or_before_it() {
    let mut t = tree(fixture());
    assert!(t.select_line(5));
    assert_eq!(t.selected(), Some("M/bar/inner"), "line 4 is the last at or before 5 (inside bar: opened to show it)");
    assert!(ids(&t).contains(&"M/bar/inner".to_string()));
    assert!(!t.select_line(5), "unchanged");
    assert!(t.select_line(7));
    assert_eq!(t.selected(), Some("tail"));
    assert!(t.select_line(100));
    assert_eq!(t.selected(), Some("Alpha"));
    // Above the first node: nothing.
    let mut t = tree(vec![node("a", "a", "", 10)]);
    assert!(!t.select_line(3));
}

/// The previews' scroll tells the Outline which heading is at the top.
#[test]
fn a_previews_scroll_picks_the_heading_at_its_top() {
    use nvim_bridge::gui::PreviewWin;
    let mut md = String::new();
    let mut headings = Vec::new();
    for i in 0..12 {
        headings.push(node(&format!("h{i}"), &format!("Heading {i}"), "H2", md.lines().count() + 1));
        md.push_str(&format!("## Heading {i}\n\n"));
        for p in 0..4 {
            md.push_str(&format!("Paragraph {p} of section {i}, with enough words in it to take a line or two of the preview.\n\n"));
        }
    }
    let mut t = tree(headings.clone());
    t.expand = 0;
    let mut previews = crate::preview::Previews::default();
    previews.set_wins(vec![PreviewWin { win: 1, buf: 7, line: 1, side: false, path: "/x/n.md".into(), search: None }]);
    previews.set_text(7, &md);
    let fonts = crate::fonts::install(&egui::Context::default(), &crate::fonts::FontSpec::default_with_size(13.0), None, None, None, None);
    let chrome = crate::style::Chrome::new(Color32::BLACK, Color32::WHITE, Color32::BLUE);
    let mut icons = crate::icons::IconTextures::default();
    let mut h = Harness::builder().with_size(Vec2::new(420.0, 300.0)).with_accessibility_check(false).build_ui_state(
        move |ui, p: &mut crate::preview::Previews| {
            p.show(ui, 1, None, &fonts, Color32::WHITE, true, &chrome, &mut icons);
        },
        previews,
    );
    h.run();
    assert_eq!(h.state_mut().take_scrolled(), None, "following the cursor isn't the user's scroll");
    let (_, _, content) = h.state().viewport(1).expect("drawn");
    assert!(content > 1500);
    let mut seen = Vec::new();
    for top in [0, content / 3, 2 * content / 3, content] {
        h.state_mut().scroll_to(1, top);
        h.run_steps(8);
        let (buf, line) = h.state_mut().take_scrolled().expect("a scroll by the user is reported");
        assert_eq!(buf, 7);
        seen.push(line);
        // The tree picks the last heading at or before it, whichever it is.
        t.select_line(line);
        let want = headings.iter().filter(|h| h.line.unwrap() <= line).last().unwrap();
        assert_eq!(t.selected(), Some(want.id.as_str()));
    }
    assert_eq!(seen[0], 1, "the top of the page");
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "further down, further on: {seen:?}");
}

// ---- Drawing and clicks ----

struct State {
    tree: TreeView,
    chrome: Chrome,
    theme: PopupTheme,
    focused: bool,
    actions: Vec<Action>,
}

fn harness(name: &str, width: f32, height: f32, tree: TreeView) -> Harness<'static, State> {
    let (_, chrome, theme) = themes().into_iter().find(|(n, ..)| *n == name).unwrap();
    let state = State { tree, chrome, theme, focused: false, actions: Vec::new() };
    Harness::builder().with_size(Vec2::new(width, height)).with_pixels_per_point(2.0).with_accessibility_check(false).wgpu().with_context_setup(system_font).build_ui_state(
        |ui, s: &mut State| {
            ui.ctx().set_theme(if s.chrome.bg.r() < 128 { egui::Theme::Dark } else { egui::Theme::Light });
            crate::style::apply_to_egui(ui.ctx(), &s.theme);
            let colors = ExplorerColors::from_chrome(&s.chrome);
            egui::CentralPanel::default().frame(egui::Frame::new().fill(colors.bg)).show(ui, |ui| {
                let look = Look { colors: &colors, font: FontId::proportional(13.0), font_size: 13.0, row_h: crate::side_bar::row_height(13.0) };
                let height = ui.available_height();
                let actions = s.tree.show(ui, &look, s.focused, height);
                s.actions.extend(actions);
            });
        },
        state,
    )
}

fn row_pos(row: usize) -> Pos2 {
    Pos2::new(120.0, side_bar_row_h() * (row as f32 + 0.5))
}

fn click(h: &mut Harness<'static, State>, pos: Pos2) {
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run();
}

#[test]
fn a_click_selects_and_chooses_and_the_chevron_only_opens() {
    let mut h = harness("dark", 260.0, 300.0, tree(fixture()));
    h.run();
    // The row of `foo` (the third): chosen, selected, and the pane asked to take the keyboard.
    click(&mut h, row_pos(1));
    let acts = std::mem::take(&mut h.state_mut().actions);
    assert_eq!(acts, vec![Action::Focus, Action::Accept { id: "M/foo".into(), how: How::Open }]);
    assert_eq!(h.state().tree.selected(), Some("M/foo"));
    // The chevron of `bar` (at the depth-1 indent): bar opens, nothing is chosen.
    let indent = (13.0 * INDENT_PER_PT).round();
    click(&mut h, Pos2::new(MARGIN + indent + 5.0, side_bar_row_h() * 2.5));
    let acts = std::mem::take(&mut h.state_mut().actions);
    assert_eq!(acts, vec![Action::Focus]);
    assert_eq!(ids(&h.state().tree).len(), 6);
    assert_eq!(h.state().tree.selected(), Some("M/bar"));
}

#[test]
fn a_double_click_chooses_once() {
    let mut h = harness("dark", 260.0, 300.0, tree(fixture()));
    h.run();
    let pos = row_pos(3);
    h.hover_at(pos);
    h.step();
    // (The test's frames are long: these would be too far apart.)
    h.ctx.options_mut(|o| o.input_options.max_double_click_delay = 10.0);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
            h.step();
        }
    }
    h.run();
    let chosen = h.state().actions.iter().filter(|a| matches!(a, Action::Accept { .. })).count();
    assert_eq!(chosen, 1, "{:?}", h.state().actions);
}

#[test]
fn a_right_click_asks_for_the_menu_and_shows_it_when_it_comes_and_runs_one_item() {
    let mut t = TreeView::new();
    t.set(TreeSet { menu: true, ..set(fixture(), 1) });
    let mut h = harness("dark", 260.0, 300.0, t);
    h.run();
    let pos = row_pos(3);
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run();
    assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Focus, Action::Menu { id: "tail".into() }]);
    // Neovim answers; the menu is there, and a click on an item runs it.
    h.state_mut().tree.set_menu("tail", vec!["Copy Path".into(), "Reveal".into()]);
    h.run();
    h.run();
    h.get_by_label("Reveal").click();
    h.run();
    h.run();
    assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Run { id: "tail".into(), index: 2 }]);
    // Closed after: the next frames draw no menu.
    h.run();
    assert!(h.query_by_label("Reveal").is_none());
    // A view without a menu hook asks for nothing.
    let mut h = harness("dark", 260.0, 300.0, tree(fixture()));
    h.run();
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run();
    assert!(h.state().actions.is_empty());
}

#[test]
fn a_message_stands_in_for_the_rows_and_an_icon_is_drawn_in_its_color() {
    let mut t = TreeView::new();
    t.set(TreeSet { message: Some("No symbols found".into()), ..set(vec![], 1) });
    let h = harness("dark", 260.0, 120.0, t);
    let mut results = egui_kittest::SnapshotResults::new();
    let mut h = h;
    h.run();
    h.snapshot("tree_message_dark");
    results.extend_harness(&mut h);
    // A node with an icon and the foreground of its group: the glyph is that color.
    let mut icon = node("a", "with icon", "", 1);
    icon.icon = "X".into();
    icon.fg = Some(0xff0000);
    let mut plain = node("b", "no icon, no kind", "", 2);
    plain.hint = "a hint".into();
    let mut h = harness("dark", 260.0, 80.0, tree(vec![icon, plain]));
    h.run();
    let image = h.render().unwrap();
    let ppp = 2.0;
    // The first row's icon slot is at the left, before the label.
    let slot = (MARGIN + 14.0) * ppp;
    let reddish = |x: u32, y: u32| {
        let p = image.get_pixel(x, y).0;
        p[0] > 180 && p[1] < 90 && p[2] < 90
    };
    let found = (slot as u32..(slot + 60.0) as u32).any(|x| (0..(side_bar_row_h() * ppp) as u32).any(|y| reddish(x, y)));
    assert!(found, "no red glyph in the icon slot");
    // The second has none, and its label is where an icon would start.
    h.snapshot("tree_icon_dark");
    results.extend_harness(&mut h);
    results.unwrap();
}

#[test]
fn tree_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (theme, width) in [("dark", 280.0), ("light", 280.0), ("dark", 150.0)] {
        let mut nodes = fixture();
        nodes[0].children[0].hint = "(a, b) -> number".into();
        let mut t = tree(nodes);
        t.select("M/foo", &["M".to_owned()]);
        let mut h = harness(theme, width, 160.0, t);
        h.state_mut().focused = true;
        h.run();
        h.hover_at(row_pos(3));
        h.run();
        h.snapshot(format!("tree_{theme}_{}", width as u32));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

// ---- Side bar views, update, keys ----

use crate::side_bar::{SideInfo, SideView, SideViews};
use nvim_bridge::gui::{TreeButton, TreeSide};

fn update(parent: Option<&str>, nodes: Vec<TreeNode>, gen: i64) -> TreeSet {
    TreeSet { view: "outline".into(), gen, parent: parent.map(str::to_owned), nodes, merge: true, ..Default::default() }
}

#[test]
fn an_update_merges_by_id_keeping_what_is_open_and_the_children_it_does_not_replace() {
    let mut t = tree(fixture());
    // bar is open, inner selected.
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowRight);
    k(&mut t, Key::ArrowDown);
    assert_eq!((t.selected(), ids(&t).len()), (Some("M/bar/inner"), 6));
    // The roots again, without the children (a refresh that does not know them), `tail` gone, `Beta` new.
    t.set(update(None, vec![node("M", "M", "Module", 1), node("Alpha", "Alpha", "Class", 8), node("Beta", "Beta", "Class", 9)], 1));
    assert_eq!(ids(&t), ["M", "M/foo", "M/bar", "M/bar/inner", "Alpha", "Beta"], "M keeps its children and bar stays open; tail is gone");
    assert_eq!(t.selected(), Some("M/bar/inner"));
    // M's children pushed on their own: foo gone, bar kept with its own, new `baz`.
    t.set(update(Some("M"), vec![node("M/bar", "bar", "Method", 3), node("M/baz", "baz", "Function", 5)], 1));
    assert_eq!(ids(&t), ["M", "M/bar", "M/bar/inner", "M/baz", "Alpha", "Beta"]);
    // An update that brings children for a node replaces its own.
    t.set(update(Some("M/bar"), vec![node("M/bar/other", "other", "Variable", 4)], 1));
    assert_eq!(ids(&t), ["M", "M/bar", "M/bar/other", "M/baz", "Alpha", "Beta"]);
    // It doesn't start a new generation: lazy requests asked under the old one stay valid, and an older one is dropped.
    t.set(update(Some("M"), vec![node("old", "old", "", 1)], 0));
    assert_eq!(ids(&t)[1], "M/bar", "an older gen's update is dropped");
    // Lazy children arrive through an update too ("Loading…" ends), for a node asked for.
    let lazy = TreeNode { id: "lz".into(), label: "lz".into(), expandable: true, ..Default::default() };
    t.set(TreeSet { expand: Some(1), ..set(vec![lazy.clone()], 5) });
    assert_eq!(t.request_missing(), vec![Action::Expand { id: "lz".into(), gen: 5 }]);
    t.set(update(Some("lz"), vec![node("lz/a", "a", "", 1)], 5));
    assert_eq!(ids(&t), ["lz", "lz/a"]);
    assert!(t.request_missing().is_empty());
    // An update to the roots clears a message.
    let mut t = TreeView::new();
    t.set(TreeSet { message: Some("No design".into()), ..set(vec![], 1) });
    t.set(update(None, vec![node("r", "r", "", 1)], 1));
    assert_eq!(ids(&t), ["r"]);
}

#[test]
fn the_keys_a_source_asked_for_reach_it_and_never_shadow_the_trees_own() {
    let mut t = tree(fixture());
    t.set_keys(vec!["/".into(), "y".into(), "J".into(), "j".into(), "?".into()]);
    // Nothing selected: the key comes with no node.
    assert_eq!(k(&mut t, Key::Slash), vec![Action::Key { id: None, key: "/".into() }]);
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    assert_eq!(k(&mut t, Key::Y), vec![Action::Key { id: Some("M/foo".into()), key: "y".into() }]);
    assert_eq!(t.selected(), Some("M/foo"), "a source's key moves nothing");
    // Shift makes it upper case; "?" is Shift-/.
    assert_eq!(t.key(Key::Slash, Modifiers::SHIFT), vec![Action::Key { id: Some("M/foo".into()), key: "?".into() }]);
    // j is the tree's own, in the list or not: it moves. So does J (Shift-j).
    assert_eq!(k(&mut t, Key::J), vec![]);
    assert_eq!(t.selected(), Some("M/bar"));
    assert_eq!(t.key(Key::J, Modifiers::SHIFT), vec![]);
    assert_eq!(t.selected(), Some("tail"));
    // Keys not asked for do nothing; Esc leaves, Enter chooses.
    assert_eq!(k(&mut t, Key::Z), vec![]);
    assert_eq!(k(&mut t, Key::Escape), vec![Action::FocusEditor]);
    assert_eq!(k(&mut t, Key::Enter), vec![Action::Accept { id: "tail".into(), how: How::Open }]);
    // A view with no rows (a message) still gives the key: the search is for that.
    let mut empty = TreeView::new();
    empty.set(TreeSet { message: Some("No design".into()), ..set(vec![], 1) });
    empty.set_keys(vec!["/".into()]);
    assert_eq!(k(&mut empty, Key::Slash), vec![Action::Key { id: None, key: "/".into() }]);
    // A modifier other than Shift: not the source's.
    assert_eq!(t.key(Key::Y, Modifiers::COMMAND), vec![]);
}

fn side_info(buttons: &[(&str, &str)]) -> SideInfo {
    SideInfo {
        id: 1,
        view: "slang".into(),
        title: "HIERARCHY".into(),
        icon: "type-hierarchy".into(),
        tooltip: "Hierarchy".into(),
        buttons: buttons.iter().map(|(icon, tooltip)| TreeButton { icon: (*icon).into(), tooltip: (*tooltip).into() }).collect(),
        filters: Vec::new(),
    }
}

#[test]
fn side_views_get_a_button_in_the_order_they_registered_and_lose_it_with_the_side_bar_falling_back() {
    let side = |title: &str| TreeSide { title: title.into(), icon: "refresh".into(), tooltip: String::new(), ..Default::default() };
    let mut views = SideViews::default();
    let a = views.register("a", side("A"));
    let b = views.register("b", side("B"));
    assert_ne!(a, b);
    assert_eq!(crate::side_bar::views(views.as_slice()).collect::<Vec<_>>(), [SideView::Explorer, SideView::Git, SideView::Tree(a), SideView::Tree(b)]);
    // Registering again changes the look and keeps the place and the number.
    assert_eq!(views.register("a", side("A2")), a);
    assert_eq!(views.by_view("a").unwrap().title, "A2");
    assert_eq!(views.as_slice().len(), 2);
    // One removed while showing: the Explorer. Another's, or a built-in view, stays.
    assert_eq!(views.unregister("a", SideView::Tree(a)), SideView::Explorer);
    assert_eq!(views.unregister("zzz", SideView::Git), SideView::Git);
    assert_eq!(views.unregister("b", SideView::Git), SideView::Git);
    assert!(views.as_slice().is_empty());
    // A new one gets a new number: no stale `SideView::Tree` can name it.
    assert!(views.register("a", side("A")) > b);
}

struct SideState {
    tree: TreeView,
    views: Vec<SideInfo>,
    side: SideView,
    chrome: Chrome,
    theme: PopupTheme,
    icons: crate::icons::IconTextures,
    actions: Vec<Action>,
    picked: Vec<SideView>,
    focused: bool,
}

/// The side bar as the app draws it for a tree view: the header (with `views` as extra
/// buttons), the source's buttons, Collapse All, and the rows.
fn side_harness(name: &str, width: f32, height: f32, tree: TreeView, views: Vec<SideInfo>, side: SideView) -> Harness<'static, SideState> {
    let (_, chrome, theme) = themes().into_iter().find(|(n, ..)| *n == name).unwrap();
    let state = SideState { tree, views, side, chrome, theme, icons: Default::default(), actions: Vec::new(), picked: Vec::new(), focused: false };
    Harness::builder().with_size(Vec2::new(width, height)).with_pixels_per_point(2.0).with_accessibility_check(false).wgpu().with_context_setup(system_font).build_ui_state(
        |ui, s: &mut SideState| {
            ui.ctx().set_theme(if s.chrome.bg.r() < 128 { egui::Theme::Dark } else { egui::Theme::Light });
            crate::style::apply_to_egui(ui.ctx(), &s.theme);
            let colors = ExplorerColors::from_chrome(&s.chrome);
            egui::CentralPanel::default().frame(egui::Frame::new().fill(colors.bg)).show(ui, |ui| {
                let font = FontId::proportional(13.0);
                let row_h = crate::side_bar::row_height(13.0);
                let info = match s.side {
                    SideView::Tree(id) => s.views.iter().find(|i| i.id == id).cloned(),
                    _ => None,
                };
                let reserved = 18.0 + row_h / 2.0 + 4.0 + row_h * info.as_ref().map_or(0, |i| i.header_buttons()) as f32;
                let (header, picked) = crate::side_bar::header(ui, s.side, 0, reserved, row_h, &font, &colors, &s.views, &mut s.icons);
                s.picked.extend(picked);
                if let Some(info) = info {
                    let look = Look { colors: &colors, font, font_size: 13.0, row_h };
                    let acts = s.tree.show_side(ui, header, &info, &look, &mut s.icons, s.focused);
                    s.actions.extend(acts);
                }
            });
        },
        state,
    )
}

/// x of the middle of header button `n` (from 0, the Explorer's) in the side bar.
fn header_button_x(n: usize) -> f32 {
    let row_h = side_bar_row_h();
    crate::style::GAP + row_h / 2.0 + n as f32 * (row_h + crate::style::GAP / 3.0)
}

fn press(h: &mut Harness<'static, SideState>, pos: Pos2, button: egui::PointerButton) {
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
    }
    h.run();
}

fn hierarchy() -> Vec<TreeNode> {
    let mut top = with(
        node("top", "top", "", 1),
        vec![
            {
                let mut p = node("top.WIDTH", "WIDTH", "", 2);
                p.icon = "P".into();
                p.fg = Some(0xc75ae8);
                p.hint = "int".into();
                p.tooltip = "WIDTH = 8".into();
                p
            },
            {
                let mut u = node("top.u", "u", "", 3);
                u.icon = "I".into();
                u.fg = Some(0xefbd5d);
                u.hint = "leaf_mod".into();
                u.expandable = true;
                u
            },
        ],
    );
    top.icon = "I".into();
    top.fg = Some(0xefbd5d);
    top.hint = "top_mod".into();
    vec![top]
}

#[test]
fn the_header_gets_a_third_button_that_picks_the_view_and_loses_it_on_unregister() {
    let mut h = side_harness("dark", 300.0, 200.0, tree(hierarchy()), vec![side_info(&[("refresh", "Refresh")])], SideView::Explorer);
    h.run();
    let y = side_bar_row_h() * 0.6;
    press(&mut h, Pos2::new(header_button_x(2), y), egui::PointerButton::Primary);
    assert_eq!(h.state_mut().picked.pop(), Some(SideView::Tree(1)), "the third button is the tree's");
    press(&mut h, Pos2::new(header_button_x(1), y), egui::PointerButton::Primary);
    assert_eq!(h.state_mut().picked.pop(), Some(SideView::Git));
    // Unregistered: nothing is at the third place.
    h.state_mut().views.clear();
    h.run();
    press(&mut h, Pos2::new(header_button_x(2), y), egui::PointerButton::Primary);
    assert_eq!(h.state_mut().picked.pop(), None);
}

#[test]
fn a_header_button_and_collapse_all_are_at_the_right_and_a_click_on_each_does_its_own_thing() {
    let mut h = side_harness("dark", 300.0, 200.0, tree(hierarchy()), vec![side_info(&[("refresh", "Refresh")])], SideView::Tree(1));
    h.run();
    let (row_h, y) = (side_bar_row_h(), side_bar_row_h() * 0.6);
    assert_eq!(ids(&h.state().tree), ["top", "top.WIDTH", "top.u"]);
    // Collapse All is the rightmost, the source's button just before it.
    press(&mut h, Pos2::new(300.0 - 18.0 - row_h, y), egui::PointerButton::Primary);
    assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Button { index: 1, selected: None, expanded: vec!["top".into()] }]);
    assert_eq!(ids(&h.state().tree).len(), 3, "the source's button folds nothing");
    press(&mut h, Pos2::new(300.0 - 18.0, y), egui::PointerButton::Primary);
    assert_eq!(ids(&h.state().tree), ["top"], "Collapse All folds everything");
    assert!(h.state().actions.is_empty());
}

#[test]
fn the_side_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    // The header with three views, at widths where the title is whole, cut, and gone.
    for theme in ["dark", "light"] {
        for width in [320.0, 200.0, 150.0] {
            let mut h = side_harness(theme, width, 40.0, tree(vec![]), vec![side_info(&[("refresh", "Refresh")])], SideView::Tree(1));
            h.run();
            h.snapshot(format!("tree_side_header_{theme}_{}", width as u32));
            results.extend_harness(&mut h);
        }
    }
    // A lazy node open and loading, a param selected, the whole view.
    for theme in ["dark", "light"] {
        let mut t = TreeView::new();
        t.set(TreeSet { expand: Some(2), ..set(hierarchy(), 1) });
        t.select("top.WIDTH", &["top".to_owned()]);
        let mut h = side_harness(theme, 300.0, 200.0, t, vec![side_info(&[("refresh", "Refresh")])], SideView::Tree(1));
        h.state_mut().focused = true;
        h.run();
        h.run();
        h.snapshot(format!("tree_side_loading_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn a_nodes_tooltip_shows_on_hover_in_the_apps_tooltip_style() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let mut h = side_harness(theme, 300.0, 200.0, tree(hierarchy()), vec![side_info(&[])], SideView::Tree(1));
        h.run();
        let row_h = side_bar_row_h();
        // The header is 1.2 rows; the param is the second row.
        h.hover_at(Pos2::new(120.0, row_h * 1.2 + row_h * 1.5));
        for _ in 0..40 {
            h.step();
        }
        assert!(h.query_by_label("WIDTH = 8").is_some(), "no tooltip");
        h.snapshot(format!("tree_side_tooltip_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn a_right_click_menu_is_drawn_for_a_side_view_like_the_explorers() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let mut t = TreeView::new();
        t.set(TreeSet { menu: true, ..set(hierarchy(), 1) });
        let mut h = side_harness(theme, 300.0, 200.0, t, vec![side_info(&[])], SideView::Tree(1));
        h.run();
        let row_h = side_bar_row_h();
        press(&mut h, Pos2::new(120.0, row_h * 1.2 + row_h * 1.5), egui::PointerButton::Secondary);
        assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Focus, Action::Menu { id: "top.WIDTH".into() }]);
        h.state_mut().tree.set_menu("top.WIDTH", vec!["Copy Path".into(), "Copy Value".into(), "Copy File Path".into()]);
        h.run();
        h.run();
        h.get_by_label("Copy Value");
        h.snapshot(format!("tree_side_menu_{theme}"));
        results.extend_harness(&mut h);
        h.get_by_label("Copy Value").click();
        h.run();
        h.run();
        assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Run { id: "top.WIDTH".into(), index: 2 }]);
    }
    results.unwrap();
}

/// Found in the VM: the Outline and a side bar view drew their rows in one scroll area
/// (the same id in the same panel), so the hierarchy opened where the Outline was scrolled to.
#[test]
fn a_side_view_scrolls_on_its_own_not_where_the_outline_was() {
    struct Both {
        outline: TreeView,
        side: TreeView,
        show_side: bool,
        chrome: Chrome,
        icons: crate::icons::IconTextures,
    }
    let leaves = || (0..60).map(|i| node(&format!("p/{i}"), &format!("leaf {i}"), "Variable", i + 2)).collect();
    let mut outline = TreeView::new();
    outline.set(TreeSet { expand: Some(0), ..set(vec![with(node("p", "p", "Module", 1), leaves())], 1) });
    outline.select("p/55", &["p".to_owned(), "p/55".to_owned()]);
    let mut side = TreeView::new();
    side.set(TreeSet { view: "slang".into(), ..set(vec![with(node("q", "q", "Module", 1), leaves())], 1) });
    let (_, chrome, theme) = themes().into_iter().find(|(n, ..)| *n == "dark").unwrap();
    let state = Both { outline, side, show_side: false, chrome, icons: Default::default() };
    let mut h = Harness::builder().with_size(Vec2::new(260.0, 200.0)).with_pixels_per_point(2.0).with_accessibility_check(false).build_ui_state(
        move |ui, s: &mut Both| {
            crate::style::apply_to_egui(ui.ctx(), &theme);
            let colors = ExplorerColors::from_chrome(&s.chrome);
            egui::CentralPanel::default().show(ui, |ui| {
                let look = Look { colors: &colors, font: FontId::proportional(13.0), font_size: 13.0, row_h: crate::side_bar::row_height(13.0) };
                let height = ui.available_height();
                if s.show_side {
                    let header = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover()).0;
                    s.side.show_side(ui, header, &side_info(&[]), &look, &mut s.icons, false);
                } else {
                    s.outline.show(ui, &look, false, height);
                }
            });
        },
        state,
    );
    h.run();
    h.run();
    assert!(h.state().outline.offset > 100.0, "the outline is scrolled to its selection: {}", h.state().outline.offset);
    h.state_mut().show_side = true;
    h.run();
    h.run();
    assert_eq!(h.state().side.offset, 0.0, "the side view starts at its top");
}

#[test]
fn the_ids_open_as_drawn_are_given_to_a_button_and_an_update_with_none_drops_the_children() {
    let mut t = tree(fixture());
    // M is open (the first level), bar isn't.
    assert_eq!(t.expanded_ids(), ["M"]);
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowRight);
    assert_eq!(t.expanded_ids(), ["M", "M/bar"]);
    // Closed under an open one: not in it. A closed parent hides what is open below it.
    k(&mut t, Key::ArrowUp);
    k(&mut t, Key::ArrowUp);
    k(&mut t, Key::ArrowLeft);
    assert_eq!(t.expanded_ids(), Vec::<String>::new(), "M is closed and bar is out of sight");
    // A node that was loaded and an update with none: the children go, and it asks again when opened.
    let lazy = TreeNode { id: "lz".into(), label: "lz".into(), expandable: true, ..Default::default() };
    let mut t = TreeView::new();
    t.set(set(vec![lazy], 5));
    assert_eq!(t.request_missing(), vec![Action::Expand { id: "lz".into(), gen: 5 }]);
    t.set(update(Some("lz"), vec![node("lz/a", "a", "", 1)], 5));
    assert_eq!(ids(&t), ["lz", "lz/a"]);
    t.set(update(Some("lz"), vec![], 5));
    assert_eq!(ids(&t), ["lz", ], "its children are gone");
    // (It is open still, so it is asked for again at once; closed, it would wait for being opened.)
    assert_eq!(t.request_missing(), vec![Action::Expand { id: "lz".into(), gen: 5 }]);
    assert!(t.rows()[0].branch, "it still has a chevron");
}

#[test]
fn a_hint_that_only_repeats_the_label_is_not_drawn() {
    let mut n = node("a", "cv32e40p_top", "", 1);
    n.hint = "cv32e40p_top".into();
    assert!(!shows_hint(&n));
    n.hint = "cv32e40p_core".into();
    assert!(shows_hint(&n));
    n.hint.clear();
    assert!(!shows_hint(&n));
}

// ---- The Filter menu ----

fn grouped(id: &str, group: &str) -> TreeNode {
    TreeNode { group: group.into(), ..node(id, id, "", 1) }
}

/// A module with a port, a signal, an instance that has only a port, and a loose node of no group.
fn design() -> Vec<TreeNode> {
    let mut top = grouped("top", "inst");
    top.children = vec![
        grouped("top.clk", "port"),
        grouped("top.r", "signal"),
        with(grouped("top.u", "inst"), vec![grouped("top.u.d", "port")]),
        TreeNode { id: "top.x".into(), label: "x".into(), ..Default::default() },
    ];
    vec![top]
}

#[test]
fn a_hidden_group_loses_its_rows_at_once_and_comes_back_with_what_was_open() {
    let mut t = tree(design());
    t.open_node("top.u", true);
    assert_eq!(ids(&t), ["top", "top.clk", "top.r", "top.u", "top.u.d", "top.x"]);
    t.set_shown("signal", false);
    assert_eq!(ids(&t), ["top", "top.clk", "top.u", "top.u.d", "top.x"]);
    // No group, no filter; a second one hides as well; j and k only see what is drawn.
    t.set_shown("port", false);
    assert_eq!(ids(&t), ["top", "top.u", "top.x"]);
    k(&mut t, Key::J);
    k(&mut t, Key::J);
    assert_eq!(t.selected(), Some("top.u"));
    t.set_shown("port", true);
    t.set_shown("signal", true);
    assert_eq!(ids(&t), ["top", "top.clk", "top.r", "top.u", "top.u.d", "top.x"], "top.u is still open");
}

#[test]
fn a_node_with_all_its_children_hidden_has_nothing_to_open() {
    let mut t = tree(design());
    t.set_shown("port", false);
    let rows = t.rows();
    let u = rows.iter().find(|r| r.node.is_some_and(|n| n.id == "top.u")).unwrap();
    assert!(!u.branch && !u.open);
    drop(rows);
    t.set_shown("port", true);
    assert!(t.rows().iter().find(|r| r.node.is_some_and(|n| n.id == "top.u")).unwrap().branch);
}

#[test]
fn a_node_not_loaded_yet_can_still_be_opened_and_a_loading_row_is_not_filtered() {
    let mut u = grouped("u", "inst");
    u.expandable = true;
    let mut t = tree(vec![u]);
    t.set_shown("port", false);
    assert!(t.rows()[0].branch, "its children may be anything");
    t.open_node("u", true);
    assert_eq!(t.rows().len(), 2, "the node and Loading…");
}

#[test]
fn a_reveal_shows_a_hidden_node_until_the_selection_moves_on() {
    let mut t = tree(design());
    t.set_shown("port", false);
    t.set_shown("inst", false);
    assert_eq!(ids(&t), [] as [&str; 0], "the module is hidden and everything with it");
    // Found by a search: it and what it is under are drawn.
    t.select("top.u.d", &["top".into(), "top.u".into(), "top.u.d".into()]);
    assert_eq!(ids(&t), ["top", "top.r", "top.u", "top.u.d", "top.x"], "what isn't hidden shows under them");
    k(&mut t, Key::K);
    assert_eq!(t.selected(), Some("top.u"));
    assert_eq!(ids(&t), ["top", "top.r", "top.u", "top.x"], "the port, selected no more, is hidden again");
}

#[test]
fn hiding_the_selected_row_moves_the_selection_to_the_node_above_it_that_is_still_shown() {
    let mut t = tree(design());
    t.select("top.u.d", &["top".into(), "top.u".into(), "top.u.d".into()]);
    t.set_shown("port", false);
    assert_eq!(t.selected(), Some("top.u"));
    assert_eq!(ids(&t), ["top", "top.r", "top.u", "top.x"]);
    // Several levels: to the last before the first one hidden.
    let mut t = tree(design());
    t.select("top.u.d", &["top".into(), "top.u".into(), "top.u.d".into()]);
    t.set_hidden(["port".to_owned()]);
    t.set_shown("inst", false);
    assert_eq!(t.selected(), None, "nothing above the module");
    assert!(ids(&t).is_empty());
}

fn filter_info() -> SideInfo {
    let mut info = side_info(&[("refresh", "Refresh")]);
    info.filters = vec![
        nvim_bridge::gui::TreeFilter { id: "port".into(), label: "Ports".into(), hidden: false },
        nvim_bridge::gui::TreeFilter { id: "signal".into(), label: "Signals".into(), hidden: false },
    ];
    info
}

#[test]
fn the_filter_button_opens_a_menu_of_toggles_that_stays_open_and_tells_neovim() {
    let mut h = side_harness("dark", 300.0, 240.0, tree(design()), vec![filter_info()], SideView::Tree(1));
    h.run();
    let (row_h, y) = (side_bar_row_h(), side_bar_row_h() * 0.6);
    press(&mut h, Pos2::new(300.0 - 18.0 - 2.0 * row_h, y), egui::PointerButton::Primary);
    assert!(h.state().actions.is_empty(), "the button is the app's own");
    h.get_by_label("Ports").click();
    h.run();
    assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Filter { group: "port".into(), shown: false }]);
    assert_eq!(ids(&h.state().tree), ["top", "top.r", "top.u", "top.x"]);
    // Still open: the next toggle is one click away.
    h.get_by_label("Signals").click();
    h.run();
    assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Filter { group: "signal".into(), shown: false }]);
    assert_eq!(ids(&h.state().tree), ["top", "top.u", "top.x"]);
    // A click outside closes it, and the refresh button is still where it was.
    press(&mut h, Pos2::new(150.0, 200.0), egui::PointerButton::Primary);
    h.run();
    assert!(h.query_by_label("Signals").is_none());
    h.state_mut().actions.clear();
    press(&mut h, Pos2::new(300.0 - 18.0 - row_h, y), egui::PointerButton::Primary);
    assert!(matches!(h.state().actions.as_slice(), [Action::Button { index: 1, .. }]), "{:?}", h.state().actions);
}

// ---- Nodes that start open ----

fn started(id: &str, children: Vec<TreeNode>) -> TreeNode {
    let mut n = with(node(id, id, "", 1), children);
    n.open = true;
    n
}

/// A chain `a` > `b` > `c` > leaf, every link coming open, under a root that is open anyway.
fn chain() -> Vec<TreeNode> {
    vec![started("a", vec![started("b", vec![started("c", vec![node("leaf", "leaf", "", 1)])])])]
}

#[test]
fn a_node_that_comes_open_is_open_the_first_time_and_the_selection_stays() {
    let mut t = TreeView::new();
    t.set(TreeSet { expand: Some(0), ..set(chain(), 1) });
    assert_eq!(ids(&t), ["a", "b", "c", "leaf"], "opened without a key, though no level is open by default");
    assert_eq!(t.selected(), None, "the selection did not move");
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    assert_eq!(t.selected(), Some("b"));
    // Children that come later (a lazy answer, an update) open their open nodes too.
    let lazy = TreeNode { id: "lz".into(), label: "lz".into(), expandable: true, ..Default::default() };
    t.set(TreeSet { expand: Some(0), ..set(vec![lazy], 2) });
    assert_eq!(t.request_missing(), vec![]);
    t.set(TreeSet { parent: Some("lz".into()), ..set(vec![started("x", vec![node("y", "y", "", 1)])], 2) });
    t.open_node("lz", true);
    assert_eq!(ids(&t), ["lz", "x", "y"]);
    t.set(update(Some("x"), vec![started("p", vec![node("q", "q", "", 1)])], 2));
    assert_eq!(ids(&t), ["lz", "x", "p", "q"], "the new children of x come open too");
}

#[test]
fn a_node_the_user_closed_is_not_opened_again_by_a_later_update_but_a_new_set_starts_over() {
    let mut t = TreeView::new();
    t.set(TreeSet { expand: Some(0), ..set(chain(), 1) });
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowLeft);
    assert_eq!((t.selected(), ids(&t)), (Some("b"), ["a", "b"].map(String::from).to_vec()), "b closed by the user");
    // The same nodes pushed again, open as before: b stays closed.
    t.set(update(Some("a"), vec![started("b", vec![started("c", vec![node("leaf", "leaf", "", 1)])])], 1));
    assert_eq!(ids(&t), ["a", "b"]);
    t.set(update(None, chain(), 1));
    assert_eq!(ids(&t), ["a", "b"]);
    // A node new to the tree opens, though.
    t.set(update(Some("a"), vec![started("b", vec![]), started("n", vec![node("m", "m", "", 1)])], 1));
    assert_eq!(ids(&t), ["a", "b", "n", "m"]);
    // A new set of roots is a new tree: what it brings open is open.
    t.set(TreeSet { expand: Some(0), ..set(chain(), 2) });
    assert_eq!(ids(&t), ["a", "b", "c", "leaf"]);
}

#[test]
fn an_open_node_is_drawn_open_and_closes_by_its_chevron_and_stays_closed_after_an_update() {
    let mut t = TreeView::new();
    t.set(TreeSet { expand: Some(0), ..set(chain(), 1) });
    let mut h = harness("dark", 260.0, 300.0, t);
    h.run();
    assert_eq!(ids(&h.state().tree).len(), 4);
    // The chevron of `a` (the first row, depth 0).
    click(&mut h, Pos2::new(MARGIN + 5.0, side_bar_row_h() * 0.5));
    assert_eq!(ids(&h.state().tree), ["a"], "closed by a click");
    h.state_mut().tree.set(update(None, chain(), 1));
    h.run();
    assert_eq!(ids(&h.state().tree), ["a"], "the update did not open it again");
}

// ---- Filter by Name ----

/// The field open with `text` in it.
fn filtering(t: &mut TreeView, text: &str) {
    t.open_filter();
    t.name_filter.as_mut().unwrap().text = text.into();
}

#[test]
fn a_name_filter_keeps_the_labels_that_have_the_text_in_any_case_and_the_nodes_above_them() {
    let mut t = tree(fixture());
    // Only the label is looked at: M's hint is "mod", and the kinds are words too.
    filtering(&mut t, "MOD");
    assert_eq!(ids(&t), [] as [&str; 0]);
    filtering(&mut t, "VARIABLE");
    assert_eq!(ids(&t), [] as [&str; 0]);
    // "a": bar, tail and Alpha; M is shown for bar; bar's child doesn't match so it goes.
    filtering(&mut t, "A");
    assert_eq!(ids(&t), ["M", "M/bar", "tail", "Alpha"]);
    // A match under closed nodes is found (it is loaded) and its ancestors are drawn open.
    filtering(&mut t, "inN");
    assert_eq!(ids(&t), ["M", "M/bar", "M/bar/inner"]);
    let rows = t.rows();
    assert_eq!(rows.iter().map(|r| (r.depth, r.branch, r.open)).collect::<Vec<_>>(), [(0, true, true), (1, true, true), (2, false, false)]);
    drop(rows);
    // Nothing typed yet: every row.
    filtering(&mut t, "");
    assert_eq!(ids(&t), ["M", "M/foo", "M/bar", "tail", "Alpha"]);
    assert_eq!(match_ranges("Alpha alp", "alp"), [0..3, 6..9]);
    assert_eq!(match_ranges("abc", "x"), Vec::<std::ops::Range<usize>>::new());
}

#[test]
fn what_was_open_is_what_is_open_again_when_the_filter_is_cleared() {
    let mut t = tree(fixture());
    t.open_node("M", false);
    assert_eq!(ids(&t), ["M", "tail", "Alpha"]);
    filtering(&mut t, "inner");
    assert_eq!(ids(&t), ["M", "M/bar", "M/bar/inner"], "drawn open for what is under them");
    assert_eq!(t.expanded_ids(), Vec::<String>::new(), "the real state (a refresh button passes it) is untouched");
    t.close_filter();
    assert_eq!(ids(&t), ["M", "tail", "Alpha"], "M is closed as it was");
    t.open_node("M", true);
    t.open_node("M/bar", true);
    filtering(&mut t, "foo");
    assert_eq!(ids(&t), ["M", "M/foo"], "bar has nothing that matches: it goes with its child");
    t.close_filter();
    assert_eq!(ids(&t), ["M", "M/foo", "M/bar", "M/bar/inner", "tail", "Alpha"]);
}

#[test]
fn a_node_a_kind_toggle_hides_stays_hidden_though_it_matches() {
    let mut t = tree(design());
    t.open_node("top.u", true);
    filtering(&mut t, "t");
    // (The labels are the ids, but top.x's.)
    assert_eq!(ids(&t), ["top", "top.clk", "top.r", "top.u", "top.u.d"]);
    t.set_shown("signal", false);
    assert_eq!(ids(&t), ["top", "top.clk", "top.u", "top.u.d"], "the signal matches and stays hidden");
    t.set_shown("port", false);
    assert_eq!(ids(&t), ["top", "top.u"]);
    // ... and one the toggles hide with the name filter on and a match under it: gone with its children.
    t.set_shown("inst", false);
    assert_eq!(ids(&t), [] as [&str; 0]);
}

#[test]
fn the_selection_that_stops_matching_moves_to_the_first_row_shown() {
    let mut t = tree(fixture());
    k(&mut t, Key::J);
    k(&mut t, Key::J);
    k(&mut t, Key::J);
    assert_eq!(t.selected(), Some("M/bar"));
    filtering(&mut t, "tail");
    t.fit_selection();
    assert_eq!(t.selected(), Some("tail"));
    // Still a match: it stays. (And bar, matched too, is not taken by the first row.)
    filtering(&mut t, "a");
    t.fit_selection();
    assert_eq!(t.selected(), Some("tail"), "tail has an a");
    filtering(&mut t, "zzz");
    t.fit_selection();
    assert_eq!(t.selected(), None, "no row to give it to");
    // Nothing selected: the first row of the matches has it.
    filtering(&mut t, "alp");
    t.fit_selection();
    assert_eq!(t.selected(), Some("Alpha"));
    // Closed: no query, the selection stays where it is.
    t.close_filter();
    t.fit_selection();
    assert_eq!(t.selected(), Some("Alpha"));
}

#[test]
fn f_opens_the_field_esc_closes_it_before_it_gives_the_keyboard_back_and_the_arrows_use_the_rows_shown() {
    let mut t = tree(fixture());
    assert!(t.name_filter.is_none());
    assert_eq!(k(&mut t, Key::F), vec![]);
    assert!(t.name_filter.as_ref().is_some_and(|f| f.focus));
    // (F with Shift is a source's key.)
    let mut t2 = tree(fixture());
    t2.set_keys(vec!["F".into()]);
    assert_eq!(t2.key(Key::F, Modifiers::SHIFT), vec![Action::Key { id: None, key: "F".into() }]);
    assert!(t2.name_filter.is_none());
    // Up and Down walk what is shown; Enter chooses.
    filtering(&mut t, "a");
    k(&mut t, Key::ArrowDown);
    assert_eq!(t.selected(), Some("M"));
    k(&mut t, Key::ArrowDown);
    k(&mut t, Key::ArrowDown);
    assert_eq!(t.selected(), Some("tail"), "foo isn't shown, nor is inner");
    k(&mut t, Key::ArrowUp);
    assert_eq!(t.selected(), Some("M/bar"));
    assert_eq!(k(&mut t, Key::Enter), vec![Action::Accept { id: "M/bar".into(), how: How::Open }]);
    // Esc: the field goes, all the rows come back, the keyboard stays; the next one leaves.
    assert_eq!(k(&mut t, Key::Escape), vec![]);
    assert!(t.name_filter.is_none());
    assert_eq!(ids(&t), ["M", "M/foo", "M/bar", "tail", "Alpha"]);
    assert_eq!(k(&mut t, Key::Escape), vec![Action::FocusEditor]);
}

#[test]
fn only_a_node_not_asked_for_yet_makes_the_search_partial() {
    let mut t = tree(hierarchy());
    assert!(t.has_unloaded(), "u may have children");
    t.set(TreeSet { parent: Some("top.u".into()), nodes: vec![node("top.u.a", "a", "", 1)], ..set(vec![], 1) });
    assert!(!t.has_unloaded());
    // A node of a hidden group isn't looked into: its rows can't match anyway.
    let mut lazy = grouped("l", "port");
    lazy.expandable = true;
    let mut t = tree(vec![lazy]);
    assert!(t.has_unloaded());
    t.set_shown("port", false);
    assert!(!t.has_unloaded());
}

#[test]
fn typing_in_the_field_narrows_the_rows_and_its_keys_work_without_leaving_it() {
    let mut h = harness("dark", 260.0, 200.0, tree(fixture()));
    h.state_mut().focused = true;
    h.run();
    h.state_mut().tree.open_filter();
    h.run();
    h.run();
    assert!(h.ctx.memory(|m| m.has_focus(filter_id())), "the field has the keyboard");
    h.input_mut().events.push(egui::Event::Text("A".into()));
    h.run();
    assert_eq!(ids(&h.state().tree), ["M", "M/bar", "tail", "Alpha"]);
    assert_eq!(h.state().tree.selected(), Some("M"), "the first row has it: nothing was selected");
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(h.state().tree.selected(), Some("tail"));
    assert!(h.ctx.memory(|m| m.has_focus(filter_id())), "the arrows didn't take the keyboard from the field");
    // Enter chooses the row and lets go of the keyboard (the editor has it now).
    h.state_mut().actions.clear();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().actions, vec![Action::Accept { id: "tail".into(), how: How::Open }]);
    assert!(!h.ctx.memory(|m| m.has_focus(filter_id())));
    assert!(h.state().tree.name_filter.is_some(), "the rows stay narrowed");
    // Back in: Esc clears and closes.
    h.state_mut().tree.open_filter();
    h.run();
    h.run();
    assert!(h.ctx.memory(|m| m.has_focus(filter_id())));
    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().tree.name_filter.is_none());
    assert!(!h.ctx.memory(|m| m.has_focus(filter_id())));
    assert_eq!(ids(&h.state().tree).len(), 5);
    assert_eq!(h.state().tree.selected(), Some("tail"), "the selection stays");
}

#[test]
fn the_filter_menu_has_filter_by_name_above_the_toggles_and_it_opens_the_field() {
    let mut h = side_harness("dark", 300.0, 220.0, tree(hierarchy()), vec![filter_info()], SideView::Tree(1));
    h.run();
    let row_h = side_bar_row_h();
    // The Filter button is left of the source's one.
    press(&mut h, Pos2::new(300.0 - 18.0 - 2.0 * row_h, row_h * 0.6), egui::PointerButton::Primary);
    h.run();
    h.get_by_label("Show");
    h.get_by_label("Ports");
    h.get_by_label("Filter by Name…").click();
    h.run();
    h.run();
    assert!(h.state().tree.name_filter.is_some());
    assert!(h.ctx.memory(|m| m.has_focus(filter_id())));
    assert!(h.state().actions.contains(&Action::Focus), "the view takes the keyboard");
}

#[test]
fn name_filter_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        // Matches marked in the label, the hint where it is, and the line saying `u` was not searched.
        let mut t = TreeView::new();
        t.set(TreeSet { expand: Some(2), ..set(hierarchy(), 1) });
        t.select("top.WIDTH", &["top".to_owned()]);
        filtering(&mut t, "wid");
        t.name_filter.as_mut().unwrap().focus = false;
        let mut h = side_harness(theme, 300.0, 200.0, t, vec![filter_info()], SideView::Tree(1));
        h.state_mut().focused = true;
        h.run();
        h.run();
        assert_eq!(h.state().tree.selected(), Some("top.WIDTH"));
        h.snapshot(format!("tree_filter_matches_{theme}"));
        results.extend_harness(&mut h);
        // Nothing matches; the field has the keyboard (the accent edge).
        let mut t = TreeView::new();
        t.set(set(fixture(), 1));
        filtering(&mut t, "zz");
        let mut h = side_harness(theme, 300.0, 200.0, t, vec![filter_info()], SideView::Tree(1));
        h.state_mut().focused = true;
        h.run();
        h.run();
        h.snapshot(format!("tree_filter_none_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn a_folder_row_opens_and_closes_instead_of_being_accepted() {
    let mut group = with(node("G", "group", "", 0), vec![node("G/a", "a", "", 1)]);
    group.folder = true;
    let mut t = tree(vec![group, node("b", "b", "", 2)]);
    assert_eq!(ids(&t), ["G", "G/a", "b"]);
    k(&mut t, Key::ArrowDown);
    assert_eq!(t.selected(), Some("G"));
    assert_eq!(k(&mut t, Key::Enter), vec![], "Enter only closes it");
    assert_eq!(ids(&t), ["G", "b"]);
    assert_eq!(k(&mut t, Key::Space), vec![], "and Space opens it");
    assert_eq!(ids(&t), ["G", "G/a", "b"]);
    assert_eq!(t.selected(), Some("G"));
}
