use super::*;
use crate::git::{graph, Branch};
use crate::popup::PopupTheme;
use crate::side_bar::{self, SideView};
use crate::style::tests::themes;
use egui_kittest::Harness;

fn entry(path: &str, status: GitStatus) -> Entry {
    Entry { path: path.into(), orig: None, status }
}

/// A repository with a conflict, three staged files (one renamed) and
/// four changed ones, ahead of and behind its upstream.
fn fixture() -> Repo {
    Repo {
        top: PathBuf::from("/work/eshvi"),
        branch: Some("main".into()),
        head: Some("aaaaaaaaaaaa".into()),
        upstream: Some("origin/main".into()),
        ahead: 1,
        behind: 2,
        merge: vec![entry("src/conflict.rs", GitStatus::Conflict)],
        staged: vec![
            entry("src/app.rs", GitStatus::Modified),
            entry("src/new_panel.rs", GitStatus::Added),
            Entry { path: "docs/guide.md".into(), orig: Some("docs/readme.md".into()), status: GitStatus::Renamed },
        ],
        changes: vec![
            entry("Cargo.lock", GitStatus::Modified),
            entry("src/old_helper.rs", GitStatus::Deleted),
            entry("src/scm_view.rs", GitStatus::Modified),
            entry("notes/todo.txt", GitStatus::Untracked),
        ],
        operation: Operation::None,
        merge_message: None,
    }
}

/// The height of the Graph's second commit in a 600-high panel.
const COMMIT_ROW_Y: f32 = 465.0;
/// The top edge of the Graph's header there, and a row's height.
const GRAPH_TOP: f32 = 406.0;
const ROW_H: f32 = 23.0;

fn commit(oid: &str, parents: &[&str], subject: &str, refs: &[&str]) -> Commit {
    Commit {
        oid: oid.into(),
        parents: parents.iter().map(|p| (*p).to_owned()).collect(),
        author: "Avi".into(),
        when: "2 hours ago".into(),
        date: "Nov 2, 2026, 8:30 AM".into(),
        subject: subject.into(),
        body: String::new(),
        refs: refs.iter().map(|r| (*r).to_owned()).collect(),
    }
}

/// A short history with a merge in it.
fn fixture_details() -> Details {
    let commits = vec![
        commit("m", &["a", "b"], "Merge branch 'feature'", &["HEAD -> main", "origin/main"]),
        commit("a", &["c"], "Parse empty input without panicking", &[]),
        commit("b", &["c"], "Add the feature flag", &["feature"]),
        commit("c", &["d"], "Draw the side bar's header", &[]),
        commit("d", &[], "Initial commit", &["tag: v0.1"]),
    ];
    let branch = |name: &str, remote: bool, current: bool| Branch { name: name.into(), remote, current };
    Details {
        rows: graph(&commits),
        commits,
        branches: vec![branch("main", false, true), branch("feature", false, false), branch("origin/main", true, false)],
        remotes: vec!["origin".into()],
    }
}

fn view_with(repo: Option<Repo>) -> ScmView {
    let mut view = ScmView::new(13.0, true);
    view.set_root(Some(Path::new("/work/eshvi")));
    view.set_repo(repo);
    view.set_details(fixture_details());
    view
}

/// The system UI font in front of egui's, which has no "⋯" or "⌘": the
/// sidebar's font, as far as these pictures go.
pub(crate) fn system_font(ctx: &egui::Context) {
    use egui::epaint::text::FontPriority;
    for (file, priority) in [("SFNS.ttf", FontPriority::Highest), ("Apple Symbols.ttf", FontPriority::Lowest)] {
        let Ok(bytes) = std::fs::read(format!("/System/Library/Fonts/{file}")) else { continue };
        let family = egui::FontFamily::Proportional;
        let insert = egui::FontInsert::new(file, egui::FontData::from_owned(bytes), vec![egui::InsertFontFamily { family, priority }]);
        ctx.add_font(insert);
    }
}

struct State {
    view: ScmView,
    chrome: Chrome,
    theme: PopupTheme,
    side: SideView,
    focused: bool,
    actions: Vec<Action>,
}

/// The panel in a side bar `width` wide, as the app draws it (with the
/// default font in place of the sidebar's).
fn harness(name: &str, width: f32, height: f32, view: ScmView, side: SideView) -> Harness<'static, State> {
    let (_, chrome, theme) = themes().into_iter().find(|(n, ..)| *n == name).unwrap();
    let state = State { view, chrome, theme, side, focused: false, actions: Vec::new() };
    Harness::builder().with_size(Vec2::new(width, height)).with_pixels_per_point(2.0).with_accessibility_check(false).wgpu().with_context_setup(system_font).build_ui_state(
        |ui, s: &mut State| {
            // (The app follows the editor's background, too.)
            ui.ctx().set_theme(if s.chrome.bg.r() < 128 { egui::Theme::Dark } else { egui::Theme::Light });
            style::apply_to_egui(ui.ctx(), &s.theme);
            let colors = ExplorerColors::from_chrome(&s.chrome);
            // VS Code's git colors, for a dark and for a light theme.
            let (added, modified, deleted) = if s.chrome.bg.r() < 128 {
                (Color32::from_rgb(0x73, 0xc9, 0x91), Color32::from_rgb(0xe2, 0xc0, 0x8d), Color32::from_rgb(0xc7, 0x4e, 0x39))
            } else {
                (Color32::from_rgb(0x58, 0x7c, 0x0c), Color32::from_rgb(0x89, 0x55, 0x03), Color32::from_rgb(0xad, 0x07, 0x07))
            };
            let deco = DecoColors {
                error: Color32::from_rgb(0xf1, 0x4c, 0x4c),
                warning: Color32::from_rgb(0xcc, 0xa7, 0x00),
                info: Color32::from_rgb(0x37, 0x94, 0xff),
                added,
                modified,
                deleted,
                text: colors.fg,
            };
            // The side bar's panel, as the app makes it: a frame in the panel's color.
            egui::CentralPanel::default().frame(egui::Frame::new().fill(colors.bg)).show(ui, |ui| {
                let font = FontId::proportional(s.view.font_size);
                let row_h = side_bar::row_height(s.view.font_size);
                let count = s.view.change_count();
                let reserved = ScmView::header_reserved(row_h);
                let (header, _) = side_bar::header(ui, s.side, count, reserved, row_h, &font, &colors, &[], &mut crate::icons::IconTextures::default());
                if s.side == SideView::Explorer {
                    return;
                }
                let look = Look {
                    colors: &colors,
                    chrome: s.chrome,
                    theme: &s.theme,
                    deco: &deco,
                    small: style::scaled(&font, style::SMALL),
                    bold: FontId::proportional((s.view.font_size * style::BADGE).round()),
                    font,
                    row_h,
                    ppp: ui.ctx().pixels_per_point(),
                };
                let actions = s.view.draw(ui, header, &look, s.focused);
                s.actions.extend(actions);
            });
        },
        state,
    )
}

#[test]
fn panel_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (theme, width) in [("dark", 320.0), ("light", 320.0), ("dark", 200.0)] {
        let mut view = view_with(Some(fixture()));
        view.selected = Some(RowKey { section: Section::Staged, path: "src/new_panel.rs".into() });
        view.expanded.insert("a".into());
        view.files.insert("a".into(), vec![entry("src/parser.rs", GitStatus::Modified), entry("tests/empty.rs", GitStatus::Added)]);
        let mut h = harness(theme, width, 600.0, view, SideView::Git);
        h.state_mut().focused = true;
        h.run();
        // The pointer over a file in Changes.
        h.hover_at(Pos2::new(width / 2.0, 330.0));
        h.run();
        h.snapshot(format!("scm_panel_{theme}_{}", width as u32));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

/// Hovering a commit asks once for how much it changed, and its tooltip has
/// the whole message and the answer.
#[test]
fn a_commits_tooltip_has_its_message_and_stats() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let mut view = view_with(Some(fixture()));
        let mut details = fixture_details();
        details.commits[1].body = "An empty file parsed as one blank line, then panicked.\n\nReturn no lines at once.".into();
        details.commits[1].refs = vec!["origin/fix".into()];
        view.set_details(details);
        let mut h = harness(theme, 320.0, 600.0, view, SideView::Git);
        h.run();
        // The second row of the Graph, found from the panel's foot.
        h.hover_at(Pos2::new(160.0, COMMIT_ROW_Y));
        h.run();
        let asks = |acts: Vec<Action>| acts.into_iter().filter(|a| matches!(a, Action::CommitStats(_))).count();
        assert_eq!(asks(std::mem::take(&mut h.state_mut().actions)), 1);
        h.run();
        assert_eq!(asks(std::mem::take(&mut h.state_mut().actions)), 0);
        let oid = "a".to_owned();
        h.state_mut().view.set_stats(oid, CommitStats { files: 3, insertions: 42, deletions: 1 });
        for _ in 0..40 {
            h.step();
        }
        h.snapshot(format!("scm_commit_tooltip_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

/// The button in a commit's tooltip copies the whole hash.
#[test]
fn a_commits_tooltip_copies_its_hash() {
    use egui_kittest::kittest::Queryable;
    let mut view = view_with(Some(fixture()));
    let mut details = fixture_details();
    details.commits[1].oid = "0123456789abcdef0123456789abcdef01234567".into();
    details.rows = graph(&details.commits);
    view.set_details(details);
    let mut h = harness("dark", 320.0, 600.0, view, SideView::Git);
    h.run();
    h.hover_at(Pos2::new(160.0, COMMIT_ROW_Y));
    for _ in 0..40 {
        h.step();
    }
    h.state_mut().actions.clear();
    h.get_by_label(COPY_HASH).click();
    for _ in 0..4 {
        h.step();
    }
    let acts = std::mem::take(&mut h.state_mut().actions);
    assert!(acts.contains(&Action::CopyText("0123456789abcdef0123456789abcdef01234567".into())), "{acts:?}");
    assert!(h.state().view.tooltip.is_some());
}

/// A commit's right-click menu opens all its changes.
#[test]
fn a_commits_menu_opens_its_changes() {
    use egui_kittest::kittest::Queryable;
    let mut view = view_with(Some(fixture()));
    let details = fixture_details();
    let second = details.commits[1].clone();
    view.set_details(details);
    let mut h = harness("dark", 320.0, 600.0, view, SideView::Git);
    h.run();
    let pos = Pos2::new(160.0, COMMIT_ROW_Y);
    h.hover_at(pos);
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().actions.clear();
    h.get_by_label("Open Changes").click();
    for _ in 0..4 {
        h.step();
    }
    let acts = std::mem::take(&mut h.state_mut().actions);
    assert!(acts.contains(&Action::CommitDiff(second.clone())), "{acts:?}");
    let (code, args) = lua_call(commit_lua(Path::new("/r"), &second));
    assert_eq!(code, "return Eshvi.commit_diff(...)");
    assert_eq!(args, vec![RpcValue::from("/r"), RpcValue::from(second.oid.as_str())]);
}

/// A click on a commit makes it the selection, in the lists' place, and opens
/// it; another closes it and it stays selected. A file under it takes over.
#[test]
fn a_commit_clicked_is_selected_and_open() {
    let mut view = view_with(Some(fixture()));
    view.selected = Some(RowKey { section: Section::Staged, path: "src/new_panel.rs".into() });
    let details = fixture_details();
    let oid = details.commits[1].oid.clone();
    view.set_details(details);
    let mut h = harness("dark", 320.0, 600.0, view, SideView::Git);
    h.run();
    click(&mut h, Pos2::new(160.0, COMMIT_ROW_Y));
    assert_eq!(h.state().view.graph_selected, Some((oid.clone(), None)));
    assert!(h.state().view.expanded.contains(&oid));
    assert_eq!(h.state().view.selected, None);
    h.state_mut().view.files.insert(oid.clone(), vec![entry("src/parser.rs", GitStatus::Modified)]);
    h.run();
    click(&mut h, Pos2::new(160.0, COMMIT_ROW_Y + ROW_H));
    assert_eq!(h.state().view.graph_selected, Some((oid.clone(), Some("src/parser.rs".to_owned()))));
    click(&mut h, Pos2::new(160.0, COMMIT_ROW_Y));
    assert_eq!(h.state().view.graph_selected, Some((oid.clone(), None)));
    assert!(!h.state().view.expanded.contains(&oid));
}

/// The Graph's title bar: Fetch, Pull, Push; Publish with no upstream.
#[test]
fn the_graphs_title_bar_fetches_pulls_and_pushes() {
    // Right to left along the title, at its middle (below the band that resizes).
    let at = |n: f32| Pos2::new(320.0 - 18.0 - n * ROW_H, GRAPH_TOP + ROW_H / 2.0 + 2.0);
    let mut view = view_with(Some(fixture()));
    let mut details = fixture_details();
    details.remotes = vec!["origin".into()];
    view.set_details(details);
    let mut h = harness("dark", 320.0, 600.0, view, SideView::Git);
    h.run();
    for (n, op) in [(0.0, Op::Push), (1.0, Op::Pull), (2.0, Op::Fetch)] {
        click(&mut h, at(n));
        assert_eq!(h.state_mut().actions.drain(..).collect::<Vec<_>>(), vec![Action::Op(op)]);
    }
    assert!(h.state().view.graph_open, "the buttons don't fold the Graph");
}

/// No upstream: Push becomes Publish, Pull does nothing; while git runs, none do.
#[test]
fn the_graphs_title_bar_publishes_and_waits() {
    let at = |n: f32| Pos2::new(320.0 - 18.0 - n * ROW_H, GRAPH_TOP + ROW_H / 2.0 + 2.0);
    let mut repo = fixture();
    repo.upstream = None;
    let mut view = view_with(Some(repo));
    let mut details = fixture_details();
    details.remotes = vec!["origin".into()];
    view.set_details(details);
    let mut h = harness("dark", 320.0, 600.0, view, SideView::Git);
    h.run();
    click(&mut h, at(1.0));
    assert!(h.state().actions.is_empty());
    assert!(h.state().view.graph_open, "a dead button doesn't fold the Graph either");
    click(&mut h, at(0.0));
    assert_eq!(h.state_mut().actions.drain(..).collect::<Vec<_>>(), vec![Action::Op(Op::Publish("origin".into()))]);
    h.state_mut().view.running.push("Pushing");
    h.run();
    click(&mut h, at(0.0));
    click(&mut h, at(2.0));
    assert!(h.state().actions.is_empty());
    assert!(h.state().view.graph_open);
}

/// Dragging the band above the Graph resizes it, as far as the lists leave
/// room for; a double click goes back to its share.
#[test]
fn the_graph_resizes() {
    let mut view = view_with(Some(fixture()));
    view.set_details(fixture_details());
    let mut h = harness("dark", 320.0, 600.0, view, SideView::Git);
    h.run();
    assert_eq!(h.state().view.graph_height, None);
    let drag = |h: &mut Harness<'static, State>, from: f32, to: f32| {
        let (a, b) = (Pos2::new(160.0, from), Pos2::new(160.0, to));
        h.hover_at(a);
        h.step();
        h.event(egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        h.hover_at(b);
        h.step();
        h.event(egui::Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.step();
        h.step();
    };
    // Up by 40: the rows below it are as they were, 40 higher.
    drag(&mut h, GRAPH_TOP, GRAPH_TOP - 40.0);
    let bigger = h.state().view.graph_height.expect("the band wasn't dragged");
    // As far as it goes: the lists keep some rows.
    drag(&mut h, GRAPH_TOP - 40.0, 0.0);
    assert!(h.state().view.graph_height.unwrap() > bigger);
    // Dragging anywhere else doesn't.
    let most = h.state().view.graph_height;
    drag(&mut h, 590.0, 500.0);
    assert_eq!(h.state().view.graph_height, most);
    // The double click.
    let top = GRAPH_TOP - (most.unwrap() - (bigger - 40.0));
    let pos = Pos2::new(160.0, top);
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
    h.step();
    assert_eq!(h.state().view.graph_height, None);
    assert_eq!(pane_body(500.0, 20.0, Some(10_000.0)), 400.0);
    assert_eq!(pane_body(500.0, 20.0, Some(0.0)), 60.0);
    assert_eq!(pane_body(500.0, 20.0, None), 175.0);
}

#[test]
fn stats_read_as_git_words_them() {
    let words = |files, insertions, deletions| {
        stats_words(CommitStats { files, insertions, deletions }).into_iter().map(|(w, _)| w).collect::<Vec<_>>().join(", ")
    };
    assert_eq!(words(3, 42, 2), "3 files changed, 42 insertions(+), 2 deletions(-)");
    assert_eq!(words(1, 1, 0), "1 file changed, 1 insertion(+)");
    assert_eq!(words(1, 0, 1), "1 file changed, 1 deletion(-)");
    assert_eq!(words(1, 0, 0), "1 file changed");
    assert_eq!(words(0, 0, 0), "");
}

/// The branch menu lists them all when there is room for them: not a
/// scroll area of two rows.
#[test]
fn the_branch_menu_is_as_tall_as_its_branches() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let mut view = view_with(Some(fixture()));
        let mut details = fixture_details();
        let branch = |name: &str, remote: bool, current: bool| Branch { name: name.into(), remote, current };
        details.branches = vec![
            branch("main", false, true),
            branch("feature", false, false),
            branch("fix/scroll", false, false),
            branch("origin/main", true, false),
            branch("origin/remote-only", true, false),
            branch("origin/release", true, false),
        ];
        view.set_details(details);
        let mut h = harness(theme, 320.0, 600.0, view, SideView::Git);
        h.run();
        // The repository row, below the header.
        click(&mut h, Pos2::new(120.0, 40.0));
        h.run();
        h.snapshot(format!("scm_branch_menu_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn other_states_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        // Not a repository, and no folder.
        let mut view = ScmView::new(13.0, true);
        view.set_root(Some(Path::new("/work/plain")));
        view.set_repo(None);
        let mut h = harness(theme, 260.0, 130.0, view, SideView::Git);
        h.run();
        h.snapshot(format!("scm_not_a_repo_{theme}"));
        results.extend_harness(&mut h);

        // Clean and ahead: Sync; no upstream: Publish; nothing to do.
        let clean = Repo { merge: vec![], staged: vec![], changes: vec![], ..fixture() };
        let publish = Repo { upstream: None, ahead: 0, behind: 0, ..clean.clone() };
        let idle = Repo { ahead: 0, behind: 0, ..clean.clone() };
        for (name, repo) in [("sync", clean), ("publish", publish), ("idle", idle)] {
            let mut view = view_with(Some(repo));
            view.set_details(Details::default());
            let mut h = harness(theme, 320.0, 250.0, view, SideView::Git);
            h.run();
            h.snapshot(format!("scm_{name}_{theme}"));
            results.extend_harness(&mut h);
        }

        // Typing, a long message, and the hint under the box.
        let mut view = view_with(Some(fixture()));
        view.message = "Draw the panel\n\nIt takes the explorer's place, with the same rows.\nThird line\nFourth".into();
        // A row is selected while the box has the keyboard: no ring on the row.
        view.selected = Some(RowKey { section: Section::Staged, path: "src/new_panel.rs".into() });
        view.focus_message();
        let mut h = harness(theme, 320.0, 480.0, view, SideView::Git);
        h.state_mut().focused = true;
        h.run();
        h.run();
        h.snapshot(format!("scm_message_{theme}"));
        results.extend_harness(&mut h);
        let mut view = view_with(Some(fixture()));
        view.need_message = true;
        let mut h = harness(theme, 320.0, 330.0, view, SideView::Git);
        h.run();
        h.snapshot(format!("scm_need_message_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn header_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        for (view, count) in [(SideView::Explorer, 0), (SideView::Git, 8), (SideView::Git, 120)] {
            let mut v = view_with(Some(fixture()));
            if count == 0 {
                v.set_repo(Some(Repo { merge: vec![], staged: vec![], changes: vec![], ..fixture() }));
            } else if count > 99 {
                let many = (0..count).map(|i| entry(&format!("f{i}.rs"), GitStatus::Modified)).collect();
                v.set_repo(Some(Repo { changes: many, ..fixture() }));
            }
            let mut h = harness(theme, 300.0, 40.0, v, view);
            h.run();
            h.snapshot(format!("scm_header_{theme}_{view:?}_{count}").to_lowercase());
            results.extend_harness(&mut h);
        }
    }
    results.unwrap();
}

fn key(section: Section, path: &str) -> RowKey {
    RowKey { section, path: path.into() }
}

#[test]
fn rows_and_selection_move_over_the_open_sections_only() {
    let mut v = view_with(Some(fixture()));
    let paths = |v: &ScmView| v.rows().into_iter().map(|k| (k.section, k.path)).collect::<Vec<_>>();
    assert_eq!(paths(&v).len(), 8);
    assert_eq!(paths(&v)[0], (Section::Merge, "src/conflict.rs".to_owned()));
    // Down from nothing selects the first, then goes on; the end stops.
    v.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(v.selected, Some(key(Section::Merge, "src/conflict.rs")));
    v.key(Key::J, Modifiers::NONE);
    assert_eq!(v.selected, Some(key(Section::Staged, "src/app.rs")));
    v.key(Key::End, Modifiers::NONE);
    assert_eq!(v.selected, Some(key(Section::Changes, "notes/todo.txt")));
    v.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(v.selected, Some(key(Section::Changes, "notes/todo.txt")));
    v.key(Key::Home, Modifiers::NONE);
    assert_eq!(v.selected, Some(key(Section::Merge, "src/conflict.rs")));
    v.key(Key::K, Modifiers::NONE);
    assert_eq!(v.selected, Some(key(Section::Merge, "src/conflict.rs")));
    // A folded section's files are skipped.
    v.collapsed.insert(Section::Staged);
    assert!(paths(&v).iter().all(|(s, _)| *s != Section::Staged));
    v.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(v.selected, Some(key(Section::Changes, "Cargo.lock")));
}

#[test]
fn a_rows_menu_acts_on_the_selection_it_is_in() {
    let mut v = view_with(Some(fixture()));
    let (a, b, c) = (key(Section::Changes, "Cargo.lock"), key(Section::Changes, "src/scm_view.rs"), key(Section::Staged, "src/app.rs"));
    v.selected = Some(a.clone());
    v.multi = vec![a.clone(), b.clone()];
    assert!(v.is_selected(&b) && !v.is_selected(&c));
    assert_eq!(v.targets_for(&b), vec![a.clone(), b.clone()]);
    // A row outside the selection: just it, and it becomes the selection.
    assert_eq!(v.targets_for(&c), vec![c.clone()]);
    assert!(v.multi.is_empty() && v.selected == Some(c));
    // Staging takes the changes and leaves the staged files alone; unstaging
    // the reverse, with a rename's old path.
    let repo = fixture();
    let all = vec![a.clone(), key(Section::Staged, "docs/guide.md"), key(Section::Merge, "src/conflict.rs")];
    assert_eq!(stage_op(&all), Some(Op::Stage(vec!["Cargo.lock".into(), "src/conflict.rs".into()])));
    assert_eq!(unstage_op(&repo, &all), Some(Op::Unstage(vec!["docs/guide.md".into(), "docs/readme.md".into()])));
    assert_eq!(stage_op(&[key(Section::Staged, "src/app.rs")]), None);
    // Discarding splits tracked files from new ones, and only takes changes.
    let all = vec![a, key(Section::Changes, "notes/todo.txt"), key(Section::Staged, "src/app.rs")];
    assert_eq!(
        discard_of(&repo, &all),
        Some(Confirm::Discard { tracked: vec!["Cargo.lock".into()], untracked: vec!["notes/todo.txt".into()] })
    );
    assert_eq!(discard_of(&repo, &[key(Section::Staged, "src/app.rs")]), None);
}

#[test]
fn each_kind_of_row_opens_the_right_diff() {
    let repo = fixture();
    let top = PathBuf::from("/work/eshvi");
    let diff = |section, path: &str| match open_changes(&repo, &key(section, path)) {
        Some(Action::Diff(spec)) => spec,
        other => panic!("{path}: {other:?}"),
    };
    // Changes: the index against the file.
    let spec = diff(Section::Changes, "src/scm_view.rs");
    assert_eq!(
        spec,
        DiffSpec {
            top: top.clone(),
            path: "src/scm_view.rs".into(),
            left_label: Some("Index".into()),
            right_label: Some("Working Tree".into()),
            ..Default::default()
        }
    );
    // Deleted in the working tree: the index's version alone.
    let spec = diff(Section::Changes, "src/old_helper.rs");
    assert!(spec.deleted && spec.base.is_empty() && spec.right.is_none());
    // Staged: HEAD against the index, a rename from its old path.
    let spec = diff(Section::Staged, "docs/guide.md");
    assert_eq!((spec.base.as_str(), spec.right.as_deref()), ("HEAD", Some("")));
    assert_eq!(spec.left_path.as_deref(), Some("docs/readme.md"));
    assert_eq!((spec.left_label.as_deref(), spec.right_label.as_deref()), (Some("HEAD"), Some("Index")));
    assert!(!spec.deleted);
    let gone = Repo { staged: vec![entry("x.rs", GitStatus::Deleted)], ..fixture() };
    assert!(matches!(open_changes(&gone, &key(Section::Staged, "x.rs")), Some(Action::Diff(s)) if s.deleted));
    // A new file and a conflict are opened, not compared.
    assert_eq!(open_changes(&repo, &key(Section::Changes, "notes/todo.txt")), Some(Action::Open(top.join("notes/todo.txt"))));
    assert_eq!(open_changes(&repo, &key(Section::Merge, "src/conflict.rs")), Some(Action::Open(top.join("src/conflict.rs"))));
    assert_eq!(open_changes(&repo, &key(Section::Changes, "nope.rs")), None);
    // A file of a commit: against its first parent, or against nothing.
    let merge = commit("1234567890", &["abcdef0123", "other"], "m", &[]);
    let spec = commit_diff(&top, &merge, &entry("a.rs", GitStatus::Deleted));
    assert_eq!((spec.base.as_str(), spec.right.as_deref()), ("abcdef0123", Some("1234567890")));
    assert_eq!((spec.left_label.as_deref(), spec.right_label.as_deref()), (Some("abcdef0"), Some("1234567")));
    assert!(spec.deleted);
    let first = commit("1234567890", &[], "first", &[]);
    let spec = commit_diff(&top, &first, &entry("a.rs", GitStatus::Added));
    assert_eq!(spec.base, "1234567890^");
    assert_eq!(spec.left_label.as_deref(), Some("Empty"));
    // A picker's row of a file's history asks for the same spec (renames too).
    let renamed = Entry { orig: Some("old.rs".into()), ..entry("a.rs", GitStatus::Renamed) };
    assert_eq!(
        commit_diff(&top, &merge, &renamed),
        file_at_commit(&top, "1234567890", Some("abcdef0123"), "a.rs", Some("old.rs"), false)
    );
    assert_eq!(commit_diff(&top, &first, &renamed), file_at_commit(&top, "1234567890", None, "a.rs", Some("old.rs"), false));
}

#[test]
fn the_graph_is_unfolded_for_git_graph() {
    let mut view = ScmView::new(13.0, true);
    view.graph_open = false;
    view.show_graph();
    assert!(view.graph_open);
}

fn lua_call(cmd: UiCommand) -> (String, Vec<RpcValue>) {
    let UiCommand::Lua { code, args } = cmd else { panic!("not a Lua call: {cmd:?}") };
    (code, args)
}

#[test]
fn the_lua_call_is_the_same_without_options() {
    let plain = DiffSpec { top: "/r".into(), base: "HEAD".into(), path: "a.txt".into(), deleted: true, ..Default::default() };
    let s = RpcValue::from;
    let (code, args) = lua_call(plain.lua("float"));
    assert_eq!(code, "return Eshvi.changes_diff(...)");
    assert_eq!(args, vec![s("/r"), s("HEAD"), s("a.txt"), RpcValue::from(true), s("float")]);
    let spec = DiffSpec {
        left_path: Some("old.txt".into()),
        right: Some(String::new()),
        left_label: Some("HEAD".into()),
        right_label: Some("Index".into()),
        ..plain
    };
    let (_, args) = lua_call(spec.lua("tab"));
    let opts = RpcValue::Map(vec![
        (s("left_path"), s("old.txt")),
        (s("right"), s("")),
        (s("left_label"), s("HEAD")),
        (s("right_label"), s("Index")),
    ]);
    assert_eq!(args, vec![s("/r"), s("HEAD"), s("a.txt"), RpcValue::from(true), s("tab"), opts]);
}

/// A file name can hold `]==]`, quotes and a newline: they travel as
/// arguments, never as part of the code.
#[test]
fn a_hostile_file_name_stays_an_argument() {
    let name = "x]==]);os.execute('boom');--[==[\n\"q\"";
    let spec = DiffSpec { top: "/r]==]".into(), base: "HEAD".into(), path: name.into(), left_path: Some(name.into()), ..Default::default() };
    let (code, args) = lua_call(spec.lua("tab"));
    assert!(!code.contains(name) && !code.contains("]==]"), "{code}");
    assert_eq!(args[2], RpcValue::from(name));
    let RpcValue::Map(opts) = &args[5] else { panic!("no options") };
    assert_eq!(opts[0].1, RpcValue::from(name));
}

#[test]
fn the_main_button_has_four_states() {
    let clean = Repo { merge: vec![], staged: vec![], changes: vec![], ..fixture() };
    assert_eq!(main_button(&fixture()), MainButton::Commit);
    assert_eq!(main_button(&Repo { merge: vec![], staged: vec![], ..fixture() }), MainButton::Commit);
    assert_eq!(main_button(&Repo { merge: vec![], changes: vec![], ..fixture() }), MainButton::Commit);
    // Nothing to commit: publish a branch without an upstream, else sync.
    assert_eq!(main_button(&Repo { upstream: None, ..clean.clone() }), MainButton::Publish);
    assert_eq!(main_button(&clean), MainButton::Sync { ahead: 1, behind: 2 });
    assert_eq!(main_button(&Repo { ahead: 0, behind: 3, ..clean.clone() }), MainButton::Sync { ahead: 0, behind: 3 });
    assert_eq!(main_button(&Repo { ahead: 0, behind: 0, ..clean.clone() }), MainButton::Idle);
    // Nowhere to publish yet: no commit, or no branch.
    assert_eq!(main_button(&Repo { upstream: None, head: None, ahead: 0, behind: 0, ..clean.clone() }), MainButton::Idle);
    assert_eq!(main_button(&Repo { upstream: None, branch: None, ahead: 0, behind: 0, ..clean }), MainButton::Idle);
}

#[test]
fn smart_commit_has_three_outcomes() {
    let repo = fixture();
    let commit = |stage_all, amend, then| Op::Commit { message: "Fix it".into(), amend, stage_all, then };
    // No message: ask for one (an amend keeps its own).
    assert_eq!(decide_commit(&repo, "  \n", false, false, After::Nothing), Decision::NeedMessage);
    assert_eq!(
        decide_commit(&repo, "", true, false, After::Nothing),
        Decision::Run(Op::Commit { message: String::new(), amend: true, stage_all: false, then: After::Nothing })
    );
    // Something is staged: commit it, as it is.
    assert_eq!(decide_commit(&repo, "Fix it\n", false, false, After::Nothing), Decision::Run(commit(false, false, After::Nothing)));
    assert_eq!(decide_commit(&repo, "Fix it", false, false, After::Sync), Decision::Run(commit(false, false, After::Sync)));
    // Nothing is staged but there are changes: ask before staging them.
    let unstaged = Repo { staged: vec![], ..fixture() };
    assert_eq!(decide_commit(&unstaged, "Fix it", false, false, After::Push), Decision::Confirm(commit(true, false, After::Push)));
    // ... unless that is what was asked for, or it is an amend.
    assert_eq!(decide_commit(&unstaged, "Fix it", false, true, After::Nothing), Decision::Run(commit(true, false, After::Nothing)));
    assert_eq!(decide_commit(&unstaged, "Fix it", true, false, After::Nothing), Decision::Run(commit(false, true, After::Nothing)));
}

#[test]
fn the_commit_button_asks_and_keeps_the_text() {
    let repo = fixture();
    let mut v = view_with(Some(fixture()));
    // No message: the box takes focus and says what is missing.
    assert_eq!(v.commit(&repo, false, false, After::Nothing), vec![Action::Focus]);
    assert!(v.need_message && v.message_focus);
    // With one: the op, and the text stays until it has worked.
    v.message = "Fix it".into();
    let acts = v.commit(&repo, false, false, After::Nothing);
    assert!(matches!(&acts[..], [Action::Op(Op::Commit { .. })]));
    assert_eq!(v.message, "Fix it");
    v.committed();
    assert!(v.message.is_empty());
    // Commit & Push: the commit is done with the box once it has worked, and
    // the push that follows (`git::next`) doesn't bring the text back or need it.
    v.message = "Fix it".into();
    let acts = v.commit(&repo, false, false, After::Push);
    let [Action::Op(op @ Op::Commit { then: After::Push, .. })] = &acts[..] else { panic!("{acts:?}") };
    assert_eq!(crate::git::next(op), Some(Op::Push));
    assert_eq!(v.message, "Fix it", "kept while the commit runs");
    v.committed();
    assert!(v.message.is_empty() && !v.need_message);
    // Nothing staged: a question, with the op to run on a yes.
    let mut v = view_with(Some(Repo { staged: vec![], ..fixture() }));
    v.message = "Fix it".into();
    assert!(v.commit(&Repo { staged: vec![], ..fixture() }, false, false, After::Nothing).is_empty());
    assert!(matches!(v.confirm, Some(Confirm::StageAllCommit(Op::Commit { stage_all: true, .. }))));
}

#[test]
fn keys_do_what_the_explorers_do() {
    let repo = fixture();
    let top = repo.top.clone();
    let mut v = view_with(Some(fixture()));
    assert_eq!(v.key(Key::Escape, Modifiers::NONE), Some(Action::FocusEditor));
    // Nothing is selected: nothing to open or stage.
    assert_eq!(v.key(Key::Enter, Modifiers::NONE), None);
    v.select(key(Section::Changes, "Cargo.lock"));
    assert!(matches!(v.key(Key::Enter, Modifiers::NONE), Some(Action::Diff(s)) if s.path == "Cargo.lock"));
    assert!(matches!(v.key(Key::O, Modifiers::NONE), Some(Action::Diff(_))));
    assert_eq!(v.key(Key::F, Modifiers::NONE), Some(Action::Open(top.join("Cargo.lock"))));
    assert_eq!(v.key(Key::S, Modifiers::NONE), Some(Action::Op(Op::Stage(vec!["Cargo.lock".into()]))));
    assert_eq!(v.key(Key::Space, Modifiers::NONE), Some(Action::Op(Op::Stage(vec!["Cargo.lock".into()]))));
    assert_eq!(v.key(Key::U, Modifiers::NONE), None);
    // Discarding asks first.
    assert_eq!(v.key(Key::D, Modifiers::NONE), None);
    assert_eq!(v.confirm, Some(Confirm::Discard { tracked: vec!["Cargo.lock".into()], untracked: vec![] }));
    v.confirm = None;
    v.select(key(Section::Staged, "docs/guide.md"));
    assert_eq!(v.key(Key::U, Modifiers::NONE), Some(Action::Op(Op::Unstage(vec!["docs/guide.md".into(), "docs/readme.md".into()]))));
    assert_eq!(v.key(Key::S, Modifiers::NONE), None);
    assert_eq!(v.key(Key::D, Modifiers::NONE), None);
    assert_eq!(v.confirm, None);
    // To the message box, and refresh.
    assert_eq!(v.key(Key::C, Modifiers::NONE), Some(Action::Focus));
    assert!(v.message_focus);
    assert_eq!(v.key(Key::R, Modifiers::NONE), Some(Action::Refresh));
    // Keys with a modifier are not the list's.
    assert_eq!(v.key(Key::S, Modifiers::COMMAND), None);
    // Without a repository there is nothing to move over.
    let mut none = view_with(None);
    assert_eq!(none.key(Key::ArrowDown, Modifiers::NONE), None);
    assert_eq!(none.key(Key::Escape, Modifiers::NONE), Some(Action::FocusEditor));
}

#[test]
fn a_merge_message_goes_into_the_box_once() {
    let merging = |message: Option<&str>| Repo {
        operation: Operation::Merge,
        merge_message: message.map(str::to_owned),
        ..fixture()
    };
    let mut v = view_with(Some(fixture()));
    v.set_repo(Some(merging(Some("Merge branch 'x'"))));
    assert_eq!(v.message, "Merge branch 'x'");
    // Cleared by the user: not put back while the merge goes on.
    v.message.clear();
    v.set_repo(Some(Repo { changes: vec![], ..merging(Some("Merge branch 'x'")) }));
    assert!(v.message.is_empty());
    // The merge is over; the next one fills it again.
    v.set_repo(Some(fixture()));
    v.set_repo(Some(merging(Some("Merge branch 'y'"))));
    assert_eq!(v.message, "Merge branch 'y'");
    // A message that was typed stays.
    let mut v = view_with(Some(fixture()));
    v.message = "mine".into();
    v.set_repo(Some(merging(Some("Merge branch 'x'"))));
    assert_eq!(v.message, "mine");
    v.set_repo(Some(fixture()));
    assert_eq!(v.message, "mine");
}

#[test]
fn an_aborted_merge_takes_its_message_out_of_the_box() {
    let merging = Repo { operation: Operation::Merge, merge_message: Some("Merge branch 'x'".into()), ..fixture() };
    // Still just what was put there: gone with the merge.
    let mut v = view_with(Some(fixture()));
    v.set_repo(Some(merging.clone()));
    v.set_repo(Some(fixture()));
    assert!(v.message.is_empty());
    // Added to by the user: theirs now.
    v.set_repo(Some(merging.clone()));
    v.message.push_str(" and more");
    v.set_repo(Some(fixture()));
    assert_eq!(v.message, "Merge branch 'x' and more");
    // The merge's commit clears it itself, and a later abort has nothing to remove.
    let mut v = view_with(Some(fixture()));
    v.set_repo(Some(merging));
    v.committed();
    v.message = "next".into();
    v.set_repo(Some(fixture()));
    assert_eq!(v.message, "next");
}

#[test]
fn another_repository_starts_with_an_empty_box() {
    let mut v = view_with(Some(fixture()));
    v.message = "half a message".into();
    v.need_message = true;
    v.set_repo(Some(Repo { branch: Some("dev".into()), ..fixture() }));
    assert_eq!(v.message, "half a message");
    v.set_repo(Some(Repo { top: "/other".into(), ..fixture() }));
    assert!(v.message.is_empty() && !v.need_message);
    // Typed before the first status came: kept.
    let mut v = ScmView::new(13.0, true);
    v.message = "early".into();
    v.set_repo(Some(fixture()));
    assert_eq!(v.message, "early");
}

#[test]
fn rows_that_are_gone_are_not_selected() {
    let mut v = view_with(Some(fixture()));
    v.select(key(Section::Changes, "Cargo.lock"));
    v.multi = vec![key(Section::Changes, "Cargo.lock"), key(Section::Changes, "src/scm_view.rs")];
    v.set_repo(Some(Repo { changes: vec![entry("src/scm_view.rs", GitStatus::Modified)], ..fixture() }));
    assert_eq!(v.multi, vec![key(Section::Changes, "src/scm_view.rs")]);
    // Another repository, or none: nothing carries over.
    v.select(key(Section::Changes, "src/scm_view.rs"));
    v.set_repo(Some(Repo { top: "/other".into(), ..fixture() }));
    assert_eq!(v.selected, None);
}

/// The repository after `staged` and `changes` are what they are.
fn with_files(staged: &[&str], changes: &[&str]) -> Repo {
    let entries = |paths: &[&str]| paths.iter().map(|p| entry(p, GitStatus::Modified)).collect();
    Repo { merge: vec![], staged: entries(staged), changes: entries(changes), ..fixture() }
}

#[test]
fn the_selection_goes_on_to_the_row_that_takes_the_place() {
    let mut v = view_with(Some(with_files(&[], &["a", "b", "c"])));
    v.select(key(Section::Changes, "a"));
    // `s` on a: it is staged, b is now first in Changes.
    v.set_repo(Some(with_files(&["a"], &["b", "c"])));
    assert_eq!(v.selected, Some(key(Section::Changes, "b")));
    v.set_repo(Some(with_files(&["a", "b"], &["c"])));
    assert_eq!(v.selected, Some(key(Section::Changes, "c")));
    // The last one gone: Changes is empty, so there is nowhere further on, and
    // the one before in the list is the last of the section above.
    v.set_repo(Some(with_files(&["a", "b", "c"], &[])));
    assert_eq!(v.selected, Some(key(Section::Staged, "c")));

    // In the middle, and at the end of a section: the same index, or the last.
    let mut v = view_with(Some(with_files(&[], &["a", "b", "c"])));
    v.select(key(Section::Changes, "b"));
    v.set_repo(Some(with_files(&["b"], &["a", "c"])));
    assert_eq!(v.selected, Some(key(Section::Changes, "c")));
    v.set_repo(Some(with_files(&["b", "c"], &["a"])));
    assert_eq!(v.selected, Some(key(Section::Changes, "a")));

    // Unstaging the only staged file: the first row of the next section.
    let mut v = view_with(Some(with_files(&["a"], &["b"])));
    v.select(key(Section::Staged, "a"));
    v.set_repo(Some(with_files(&[], &["a", "b"])));
    assert_eq!(v.selected, Some(key(Section::Changes, "a")));

    // Unstaging the last of several staged ones: the new last.
    let mut v = view_with(Some(with_files(&["a", "b"], &["c"])));
    v.select(key(Section::Staged, "b"));
    v.set_repo(Some(with_files(&["a"], &["b", "c"])));
    assert_eq!(v.selected, Some(key(Section::Staged, "a")));

    // A folded section is not landed in; a row that stays stays selected;
    // nothing selected, nothing selected.
    let mut v = view_with(Some(with_files(&["a"], &["b"])));
    v.collapsed.insert(Section::Changes);
    v.select(key(Section::Staged, "a"));
    v.set_repo(Some(with_files(&[], &["a", "b"])));
    assert_eq!(v.selected, None);
    let mut v = view_with(Some(with_files(&["a"], &["b"])));
    v.select(key(Section::Staged, "a"));
    v.set_repo(Some(with_files(&["a"], &[])));
    assert_eq!(v.selected, Some(key(Section::Staged, "a")));
    let mut v = view_with(Some(with_files(&[], &["a", "b"])));
    v.set_repo(Some(with_files(&["a"], &["b"])));
    assert_eq!(v.selected, None);
}

#[test]
fn publishing_asks_for_a_remote_when_there_is_none() {
    let mut v = view_with(Some(fixture()));
    assert_eq!(v.publish(), vec![Action::Op(Op::Publish("origin".into()))]);
    let mut details = fixture_details();
    details.remotes = vec!["upstream".into(), "fork".into()];
    v.set_details(details.clone());
    assert_eq!(v.publish(), vec![Action::Op(Op::Publish("upstream".into()))]);
    details.remotes = vec![];
    v.set_details(details);
    assert!(v.publish().is_empty());
    let prompt = v.prompt.clone().unwrap();
    assert_eq!((prompt.kind, prompt.text.as_str()), (PromptKind::Remote { publish: true }, "origin"));
    // Both fields are needed; then the remote is added and the branch pushed.
    assert!(v.accept_prompt().is_empty() && v.prompt.is_some());
    v.prompt.as_mut().unwrap().url = " git@host:me/repo.git ".into();
    assert_eq!(
        v.accept_prompt(),
        vec![
            Action::Op(Op::AddRemote { name: "origin".into(), url: "git@host:me/repo.git".into() }),
            Action::Op(Op::Publish("origin".into())),
        ]
    );
    assert!(v.prompt.is_none());
    // A branch name: spaces become dashes, empty is nothing.
    v.ask_branch();
    v.prompt.as_mut().unwrap().text = " my new branch ".into();
    assert_eq!(v.accept_prompt(), vec![Action::Op(Op::CreateBranch("my-new-branch".into()))]);
    v.ask_branch();
    assert!(v.accept_prompt().is_empty() && v.prompt.is_none());
}

#[test]
fn only_dialogs_and_fields_want_the_keyboard() {
    let ctx = egui::Context::default();
    let mut v = view_with(Some(fixture()));
    assert!(!v.wants_keyboard(&ctx));
    v.confirm = Some(Confirm::Discard { tracked: vec![], untracked: vec![] });
    assert!(v.wants_keyboard(&ctx));
    // Out of sight, the questions go.
    v.close_dialogs();
    assert!(!v.wants_keyboard(&ctx));
    v.ask_branch();
    assert!(v.wants_keyboard(&ctx));
    v.close_dialogs();
    assert!(!v.wants_keyboard(&ctx));
}

/// The pointer's position over the row after `rows` rows of the list
/// (the sections start under the message box and the button).
fn list_row(h: &Harness<'static, State>, rows: f32) -> Pos2 {
    let v = &h.state().view;
    let row_h = side_bar::row_height(v.font_size);
    let top = v.message_rect.unwrap().max.y + row_h / 2.0 + row_h + 4.0;
    Pos2::new(60.0, top + row_h * (rows + 0.5))
}

fn click_row(h: &mut Harness<'static, State>, rows: f32) {
    let pos = list_row(h, rows);
    click(h, pos);
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
fn clicking_rows_buttons_and_sections() {
    let mut h = harness("dark", 320.0, 600.0, view_with(Some(fixture())), SideView::Git);
    h.run();
    // Layout: Merge header (0), its file, Staged header (2), 3 files,
    // Changes header (6), Cargo.lock (7), old_helper.rs, scm_view.rs (9),
    // todo.txt (10).
    // A click opens the diff and takes the keyboard.
    click_row(&mut h, 9.0);
    let acts = std::mem::take(&mut h.state_mut().actions);
    assert_eq!(acts.len(), 2, "{acts:?}");
    assert_eq!(acts[0], Action::Focus);
    assert!(matches!(&acts[1], Action::Diff(s) if s.path == "src/scm_view.rs"), "{acts:?}");
    assert_eq!(h.state().view.selected, Some(key(Section::Changes, "src/scm_view.rs")));
    // A new file is opened.
    click_row(&mut h, 10.0);
    let acts = std::mem::take(&mut h.state_mut().actions);
    assert_eq!(acts.last(), Some(&Action::Open(PathBuf::from("/work/eshvi/notes/todo.txt"))));
    // Cmd-click adds a row to the selection, and nothing opens.
    let pos = list_row(&h, 7.0);
    h.hover_at(pos);
    h.event(egui::Event::ModifiersChanged(Modifiers::COMMAND));
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::COMMAND });
    }
    h.run();
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run();
    assert_eq!(h.state().view.selection().len(), 2);
    assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Focus]);
    // A button at the right of a hovered row stages it: found by trying
    // along the row from its right end (the clicks before it select the row).
    let y = list_row(&h, 7.0).y;
    let mut staged = None;
    for x in (200..300).rev().step_by(3) {
        h.state_mut().actions.clear();
        click(&mut h, Pos2::new(x as f32, y));
        if let Some(Action::Op(op)) = h.state().actions.iter().find(|a| matches!(a, Action::Op(_))) {
            staged = Some(op.clone());
            break;
        }
    }
    assert_eq!(staged, Some(Op::Stage(vec!["Cargo.lock".into()])));
    // (A button's click isn't the row's: nothing was opened.)
    assert!(!h.state().actions.iter().any(|a| matches!(a, Action::Diff(_) | Action::Open(_))), "{:?}", h.state().actions);
    // A section's header folds it, and unfolds it again.
    click_row(&mut h, 6.0);
    assert!(h.state().view.collapsed.contains(&Section::Changes));
    click_row(&mut h, 6.0);
    assert!(!h.state().view.collapsed.contains(&Section::Changes));
}

#[test]
fn the_message_box_commits_with_cmd_enter_and_leaves_with_tab_and_esc() {
    let mut h = harness("dark", 320.0, 600.0, view_with(Some(fixture())), SideView::Git);
    h.state_mut().view.focus_message();
    h.run();
    h.run();
    assert!(h.ctx.memory(|m| m.has_focus(message_id())));
    assert!(h.state().view.wants_keyboard(&h.ctx));
    // (The focus asked for is asked for once.)
    assert!(!h.state().view.message_focus);
    h.state_mut().view.message = "Fix it".into();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run();
    let acts = std::mem::take(&mut h.state_mut().actions);
    assert!(acts.iter().any(|a| matches!(a, Action::Op(Op::Commit { message, .. }) if message == "Fix it")), "{acts:?}");
    // A second line, not a tab, from Enter; Tab goes to the first row.
    h.key_press(Key::Tab);
    h.run();
    assert!(!h.ctx.memory(|m| m.has_focus(message_id())));
    assert_eq!(h.state().view.selected, Some(key(Section::Merge, "src/conflict.rs")));
    assert!(h.state().view.message == "Fix it", "Tab typed into the box: {:?}", h.state().view.message);
    assert!(std::mem::take(&mut h.state_mut().actions).contains(&Action::Focus));
    // Esc gives the keyboard back to the editor.
    h.state_mut().view.focus_message();
    h.run();
    h.run();
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.ctx.memory(|m| m.has_focus(message_id())));
    assert!(std::mem::take(&mut h.state_mut().actions).contains(&Action::FocusEditor));
}

/// Tab and Enter typed for Neovim must not focus and "click" the panel's
/// buttons (the explorer's test says why): the message box takes focus
/// from Tab, but only for a frame.
#[test]
fn tab_and_enter_do_not_click_the_panels_widgets() {
    let mut h = harness("dark", 320.0, 600.0, view_with(Some(fixture())), SideView::Git);
    h.run();
    h.key_press(Key::Tab);
    h.run();
    h.run();
    assert!(!h.ctx.memory(|m| m.has_focus(message_id())), "Tab focused the box");
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().actions.is_empty(), "{:?}", h.state().actions);
    assert!(h.state().view.message.is_empty());
}

#[test]
fn questions_are_answered_with_enter_and_cancel() {
    let discard = Confirm::Discard { tracked: vec!["a.rs".into(), "b.rs".into()], untracked: vec!["new.txt".into()] };
    let op = Op::Discard { tracked: vec!["a.rs".into(), "b.rs".into()], untracked: vec!["new.txt".into()] };
    let mut h = harness("dark", 320.0, 400.0, view_with(Some(fixture())), SideView::Git);
    h.state_mut().view.confirm = Some(discard.clone());
    h.run();
    assert!(h.state().view.wants_keyboard(&h.ctx));
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Op(op)]);
    assert!(h.state().view.confirm.is_none());
    // Esc asks for nothing.
    h.state_mut().view.confirm = Some(discard);
    h.run();
    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().actions.is_empty() && h.state().view.confirm.is_none());
    // A name is typed in the prompt's own field, which has the keyboard.
    h.state_mut().view.ask_branch();
    h.run();
    h.run();
    assert!(h.ctx.memory(|m| m.has_focus(prompt_id())));
    h.event(egui::Event::Text("topic".into()));
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(std::mem::take(&mut h.state_mut().actions), vec![Action::Op(Op::CreateBranch("topic".into()))]);
    assert!(h.state().view.prompt.is_none());
}

#[test]
fn dialog_snapshots() {
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["dark", "light"] {
        let mut v = view_with(Some(fixture()));
        v.confirm = Some(Confirm::Discard { tracked: vec!["src/app.rs".into()], untracked: vec!["notes/todo.txt".into(), "x.txt".into()] });
        let mut h = harness(theme, 360.0, 200.0, v, SideView::Git);
        h.run();
        h.snapshot(format!("scm_confirm_{theme}"));
        results.extend_harness(&mut h);
        let mut v = view_with(Some(fixture()));
        v.set_details(Details::default());
        v.ask_remote(true);
        let mut h = harness(theme, 360.0, 220.0, v, SideView::Git);
        h.run();
        h.run();
        h.snapshot(format!("scm_prompt_{theme}"));
        results.extend_harness(&mut h);
    }
    results.unwrap();
}
