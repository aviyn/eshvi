//! Source Control in the side bar: which view shows, and the commands that
//! change it.

use std::path::Path;

use super::*;
use crate::git::{self, Op, OpDone};
use crate::side_bar::SideView;

/// The side bar's state that `:EshviExplorer` and `:EshviGit` change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SideState {
    pub visible: bool,
    pub view: SideView,
    /// The side bar has the keyboard.
    pub focused: bool,
}

impl SideState {
    fn hidden(self) -> Self {
        Self { visible: false, focused: false, ..self }
    }

    /// `:EshviExplorer action`. Showing, hiding and toggling keep the view
    /// that is current; focusing and revealing go to the explorer.
    pub fn explorer_command(self, action: &str) -> Self {
        match action {
            "show" => Self { visible: true, ..self },
            "hide" => self.hidden(),
            "focus" | "reveal" => Self { visible: true, view: SideView::Explorer, focused: true },
            _ if self.visible => self.hidden(),
            _ => Self { visible: true, ..self },
        }
    }

    /// `:EshviGit action`. Toggling shows Source Control, or hides the side
    /// bar if that is what it shows.
    pub fn git_command(self, action: &str) -> Self {
        match action {
            "show" => Self { visible: true, view: SideView::Git, ..self },
            "hide" => self.hidden(),
            // ("graph" also unfolds the Graph: `VimApp::git_command`.)
            "focus" | "graph" => Self { visible: true, view: SideView::Git, focused: true },
            _ if self.visible && self.view == SideView::Git => self.hidden(),
            _ => Self { visible: true, view: SideView::Git, ..self },
        }
    }
}

impl SideState {
    /// `:EshviTree action view` and `Eshvi.tree.show/hide`, for the side bar view
    /// `target`. `show` switches to it (taking the keyboard with `focus`), `focus`
    /// takes the keyboard, and again gives it back, `hide` goes back to the
    /// Explorer if it is showing, `toggle` shows it or hides it.
    pub fn tree_command(self, target: SideView, action: &str, focus: bool) -> Self {
        let shown = self.visible && self.view == target;
        match action {
            "hide" if shown => Self { view: SideView::Explorer, focused: false, ..self },
            "hide" => self,
            "focus" if shown && self.focused => Self { focused: false, ..self },
            "focus" => Self { visible: true, view: target, focused: true },
            "toggle" if shown => self.tree_command(target, "hide", false),
            _ => Self { visible: true, view: target, focused: focus || (shown && self.focused) },
        }
    }
}

impl VimApp {
    /// `:EshviTree`, `Eshvi.tree.show` and `hide`.
    pub(super) fn tree_show(&mut self, view: &str, action: &str, focus: bool) {
        // A menu item with `editor`: the keyboard goes to the editor, as for `open`.
        if action == "editor" {
            self.set_focus(Focus::Editor);
            return;
        }
        let Some(id) = self.side_views.by_view(view).map(|i| i.id) else { return };
        if !self.explorer_enabled {
            return;
        }
        let state = self.side_state().tree_command(SideView::Tree(id), action, focus);
        self.set_side_state(state);
    }

    fn side_state(&self) -> SideState {
        SideState { visible: self.explorer.visible, view: self.side, focused: self.explorer.focused }
    }

    fn set_side_state(&mut self, s: SideState) {
        self.explorer.visible = s.visible;
        self.side = s.view;
        match (s.focused, self.focus) {
            (true, _) => self.set_focus(Focus::Side),
            (false, Focus::Side) => self.set_focus(Focus::Editor),
            _ => {}
        }
    }

    /// `:EshviExplorer <action>` and the keyboard shortcuts.
    pub(super) fn explorer_command(&mut self, action: &str) {
        if !self.explorer_enabled {
            return;
        }
        self.set_side_state(self.side_state().explorer_command(action));
        if action == "reveal" {
            if let Some(p) = self.active_file.clone() {
                self.explorer.reveal(&p);
            }
        }
    }

    /// `:EshviGit <action>` and the keyboard shortcuts. (The panel is part
    /// of the explorer's side bar: off with it.) Focusing puts the keyboard
    /// in the message box.
    pub(super) fn git_command(&mut self, action: &str) {
        if !self.explorer_enabled {
            return;
        }
        let state = self.side_state().git_command(action);
        self.set_side_state(state);
        // (The Graph takes no keys of its own: the keyboard goes where
        // "focus" puts it.)
        if action == "graph" {
            self.scm.show_graph();
        }
        if matches!(action, "focus" | "graph") {
            self.scm.focus_message();
        }
    }

    /// A file at a commit, asked for by Neovim (a file's or a line's history):
    /// opened as the Graph opens a file under a commit.
    pub(super) fn git_file_diff(&mut self, ctx: &egui::Context, spec: scm_view::DiffSpec) {
        if self.config.git.diff == crate::config::DiffView::Native {
            return self.open_diff(ctx, spec.request());
        }
        self.bridge.send(spec.lua(self.config.git.diff.name()));
        // (The diff takes the keyboard.)
        self.set_focus(Focus::Editor);
    }

    /// An egui text field that keeps keyboard focus (the others lose it).
    pub(super) fn keeps_focus(&self, id: egui::Id) -> bool {
        id == explorer_view::edit_field_id()
            || id == problems_filter_id()
            || id == crate::tree_view::filter_id()
            || id == crate::picker::field_id()
            || [scm_view::message_id(), scm_view::prompt_id(), scm_view::prompt_url_id()].contains(&id)
            || self.term_debug_keeps_focus(id)
            || id == crate::diff_view::find_id()
            || self.bottom.views.values().any(|v| v.find_id() == id)
            || self.agents_keep_focus(id)
    }

    /// Something in the side bar has the keyboard for itself: a name or a
    /// message or tree filter being typed, or a question being asked.
    pub(super) fn side_wants_keyboard(&self, ctx: &egui::Context) -> bool {
        self.explorer.wants_keyboard() || self.scm.wants_keyboard(ctx) || ctx.memory(|m| m.has_focus(crate::tree_view::filter_id()))
    }

    /// What the panel asked for.
    pub(super) fn scm_action(&mut self, ctx: &egui::Context, action: scm_view::Action) {
        use scm_view::Action;
        match action {
            Action::Op(op) => self.run_git_op(op),
            Action::Diff(spec) => self.git_file_diff(ctx, spec),
            Action::DiffAll(req) => self.open_diff(ctx, req),
            Action::CommitDiff(commit) => {
                if let Some(top) = self.scm.top() {
                    if self.config.git.diff == crate::config::DiffView::Native {
                        self.open_diff(ctx, scm_view::commit_request(&top, &commit));
                    } else {
                        self.bridge.send(scm_view::commit_lua(&top, &commit));
                        self.set_focus(Focus::Editor);
                    }
                }
            }
            Action::Open(path) => {
                self.bridge.send(UiCommand::OpenFile { path, split: false });
                self.set_focus(Focus::Editor);
            }
            Action::Focus => self.set_focus(Focus::Side),
            Action::FocusEditor => self.set_focus(Focus::Editor),
            Action::Refresh => self.git_force = true,
            Action::ShowGitOutput => self.panel_command(ctx, "git_output", &["show".into()]),
            Action::CommitFiles(commit) => {
                if let Some(top) = self.scm.top() {
                    self.git_ops.files(&top, commit);
                }
            }
            Action::CommitStats(commit) => {
                if let Some(top) = self.scm.top() {
                    self.git_ops.stats(&top, commit);
                }
            }
            Action::CopyText(text) => ctx.copy_text(text),
            Action::Reveal(path) => actions::reveal_in_file_manager(&path),
            Action::SendToAgent(path) => {
                self.set_focus(Focus::Editor);
                let path = path.to_string_lossy();
                self.bridge.send(UiCommand::Lua { code: "return Eshvi.ide.send(...)".into(), args: vec![RpcValue::from(path.as_ref())] });
            }
        }
    }

    /// Start `op` in the repository (`Init`: in the explorer's folder).
    fn run_git_op(&mut self, op: Op) {
        let dir = match op {
            Op::Init => self.explorer.root().map(Path::to_path_buf),
            _ => self.scm.top(),
        };
        if let Some(dir) = dir {
            self.git_ops.run(&dir, op);
        }
    }

    /// Results of the operations: failures to the messages, then the status
    /// is read again, and buffers whose files git changed are too.
    pub(super) fn poll_git_ops(&mut self, ctx: &egui::Context) {
        for done in self.git_ops.poll() {
            match done {
                OpDone::Ran(op, result, ran) => {
                    if self.panel_enabled {
                        self.add_git_output(ctx, &ran);
                    }
                    // The diff view reads its files again, or forgets what it was doing.
                    if let Some(d) = &mut self.diff {
                        if result.is_ok() {
                            d.refresh(None);
                        } else {
                            d.cancel_edit();
                        }
                    }
                    match result {
                        Ok(()) => {
                            if matches!(op, Op::Commit { .. }) {
                                self.scm.committed();
                            }
                            // (A commit that is to push goes on, apart from it:
                            // if that fails the commit stands.)
                            if let Some(more) = git::next(&op) {
                                self.run_git_op(more);
                            }
                        }
                        Err(why) => self.editor.push_message("emsg", &failure_message(&op, &why, self.panel_enabled)),
                    }
                    self.git_force = true;
                    if op.touches_files() {
                        self.bridge.send(UiCommand::Command("silent! checktime".into()));
                    }
                }
                OpDone::Undone(message) => self.scm.restore_message(message),
                OpDone::Files(oid, files) => self.scm.set_files(oid, files),
                OpDone::Stats(oid, stats) => self.scm.set_stats(oid, stats),
            }
        }
        self.scm.set_running(self.git_ops.running.clone());
        if !self.git_ops.running.is_empty() {
            ctx.request_repaint();
        }
    }
}

/// How often the status is read while the window has focus.
pub(super) const GIT_PERIOD: std::time::Duration = std::time::Duration::from_secs(3);

/// How long until the next periodic read falls due, `since` the last
/// request. (A flat period from the frame at hand finds it a hair early, or,
/// after a read forced in between, not due yet, and puts it off a whole
/// period more.)
pub(super) fn wait_for_git(since: Option<std::time::Duration>) -> std::time::Duration {
    GIT_PERIOD.saturating_sub(since.unwrap_or_default()) + std::time::Duration::from_millis(20)
}

/// Most lines of git's own words in a message.
const WHY_LINES: usize = 6;

/// What to tell the user when `op` fails with `why` (what git said), short
/// as VS Code's: a conflict and a rejected push in a sentence, anything else
/// as "<What> failed:" and git's words, without its `hint:` lines, cut at
/// `WHY_LINES` (a hook's output is as welcome as git's). With the bottom panel
/// (`git_output`) it ends by saying where all of it is.
fn failure_message(op: &Op, why: &str, git_output: bool) -> String {
    let message = failure_sentence(op, why);
    if git_output {
        format!("{message}\n{SEE_OUTPUT}")
    } else {
        message
    }
}

/// The last line of a failure's message.
const SEE_OUTPUT: &str = "See the Git output (⇧⌘U).";

fn failure_sentence(op: &Op, why: &str) -> String {
    let conflicts = why.lines().filter(|l| l.contains("CONFLICT")).count();
    if conflicts > 0 {
        let files = if conflicts == 1 { "1 file".to_owned() } else { format!("{conflicts} files") };
        return format!("Merge conflicts in {files}. Resolve them, stage them and commit.");
    }
    if ["[rejected]", "fetch first", "non-fast-forward"].iter().any(|m| why.contains(m)) {
        return "Push rejected: the remote has commits you don't have. Pull or Sync first.".to_owned();
    }
    let mut lines: Vec<&str> = why.lines().filter(|l| !l.trim_start().starts_with("hint:")).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let more = lines.len() > WHY_LINES;
    lines.truncate(WHY_LINES);
    let mut message = format!("{} failed:", op_name(op));
    for line in lines {
        message.push('\n');
        message.push_str(line);
    }
    if more {
        message.push_str("\n…");
    }
    message
}

/// What failed, for the message.
fn op_name(op: &Op) -> &'static str {
    match op {
        Op::Init => "Initialize",
        Op::Stage(_) | Op::StageAll => "Stage",
        Op::Unstage(_) | Op::UnstageAll => "Unstage",
        Op::Discard { .. } => "Discard",
        Op::Apply { cached: true, reverse: false, .. } => "Stage",
        Op::Apply { cached: true, reverse: true, .. } => "Unstage",
        Op::Apply { cached: false, .. } => "Discard",
        Op::Commit { .. } => "Commit",
        Op::UndoCommit => "Undo commit",
        Op::Pull => "Pull",
        Op::Push => "Push",
        Op::Fetch => "Fetch",
        Op::Sync => "Sync",
        Op::Publish(_) => "Publish",
        Op::Checkout(_) | Op::Track(_) => "Checkout",
        Op::CreateBranch(_) => "Create branch",
        Op::Merge(_) => "Merge",
        Op::Abort(_) => "Abort",
        Op::Stash { .. } => "Stash",
        Op::StashPop => "Pop stash",
        Op::AddRemote { .. } => "Add remote",
        Op::Ignore(_) => "Ignore",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::After;

    #[test]
    fn git_errors_are_short() {
        // A conflict is a sentence, with the number of files, and no Abort.
        let merge = "Auto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt\nAuto-merging b.txt\nCONFLICT (content): Merge conflict in b.txt\nAutomatic merge failed; fix conflicts and then commit the result.";
        assert_eq!(failure_sentence(&Op::Pull, merge), "Merge conflicts in 2 files. Resolve them, stage them and commit.");
        assert_eq!(
            failure_sentence(&Op::Merge("x".into()), "CONFLICT (add/add): Merge conflict in a.txt"),
            "Merge conflicts in 1 file. Resolve them, stage them and commit."
        );
        assert!(!failure_sentence(&Op::Pull, merge).contains("Abort"));
        // A rejected push, however git words it.
        let rejected = "To /tmp/remote.git\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs\nhint: Updates were rejected because the remote contains work\nhint: that you do not have locally.";
        let sentence = "Push rejected: the remote has commits you don't have. Pull or Sync first.";
        assert_eq!(failure_sentence(&Op::Push, rejected), sentence);
        assert_eq!(failure_sentence(&Op::Sync, "! [rejected] main -> main (non-fast-forward)"), sentence);
        assert_eq!(failure_sentence(&Op::Push, "Updates were rejected: fetch first"), sentence);
        // The rest: git's words without the hints.
        let other = "fatal: unable to access 'https://x/': Could not resolve host\nhint: check your network";
        assert_eq!(failure_sentence(&Op::Fetch, other), "Fetch failed:\nfatal: unable to access 'https://x/': Could not resolve host");
        // A hook's output is kept, up to six lines, then an ellipsis.
        let hook = (1..=9).map(|i| format!("lint error {i}")).collect::<Vec<_>>().join("\n");
        let message = failure_sentence(&Op::Push, &hook);
        assert_eq!(message.lines().count(), 1 + 6 + 1);
        assert!(message.starts_with("Push failed:\nlint error 1\n") && message.ends_with("lint error 6\n…"));
        let six = (1..=6).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n");
        assert!(!failure_sentence(&Op::Push, &six).ends_with('…'));
        // Nothing but hints: still says what failed.
        assert_eq!(failure_sentence(&Op::Pull, "hint: nope"), "Pull failed:");
    }

    #[test]
    fn a_failure_says_where_all_of_git_s_output_is() {
        assert_eq!(failure_message(&Op::Push, "fatal: no", true), "Push failed:\nfatal: no\nSee the Git output (⇧⌘U).");
        let rejected = failure_message(&Op::Push, "! [rejected] main -> main (fetch first)", true);
        assert!(rejected.starts_with("Push rejected:") && rejected.ends_with("\nSee the Git output (⇧⌘U)."));
        // Without the bottom panel there is no such place to point to.
        assert_eq!(failure_message(&Op::Push, "fatal: no", false), "Push failed:\nfatal: no");
    }

    #[test]
    fn a_failed_operation_says_what_failed_and_what_git_said() {
        let push = failure_sentence(&Op::Push, "fatal: Authentication failed");
        assert_eq!(push, "Push failed:\nfatal: Authentication failed");
        let commit = Op::Commit { message: "x".into(), amend: false, stage_all: false, then: After::Sync };
        assert!(failure_sentence(&commit, "no").starts_with("Commit failed:"));
        // (Its push or sync is an operation of its own, and says so.)
        assert_eq!(failure_sentence(&Op::Sync, "no"), "Sync failed:\nno");
        assert_eq!(failure_sentence(&Op::Checkout("a".into()), "no"), "Checkout failed:\nno");
        // The ones that touch files make Neovim's buffers read them again.
        assert!(Op::Pull.touches_files() && Op::Checkout("a".into()).touches_files());
        assert!(!Op::Push.touches_files() && !Op::StageAll.touches_files());
    }

    #[test]
    fn the_next_status_read_is_scheduled_for_when_it_falls_due() {
        use std::time::Duration;
        let ms = Duration::from_millis;
        assert_eq!(wait_for_git(Some(Duration::ZERO)), GIT_PERIOD + ms(20));
        // A read forced a second ago: two seconds left, not a whole period.
        assert_eq!(wait_for_git(Some(ms(1000))), ms(2020));
        // Already due: right away.
        assert_eq!(wait_for_git(Some(Duration::from_secs(9))), ms(20));
        assert_eq!(wait_for_git(None), GIT_PERIOD + ms(20));
    }

    const HIDDEN: SideState = SideState { visible: false, view: SideView::Explorer, focused: false };
    const EXPLORER: SideState = SideState { visible: true, view: SideView::Explorer, focused: false };
    const GIT: SideState = SideState { visible: true, view: SideView::Git, focused: false };

    const TREE: SideView = SideView::Tree(1);

    #[test]
    fn tree_command_shows_a_trees_view_focuses_it_and_again_gives_the_keyboard_back() {
        let shown = SideState { visible: true, view: TREE, focused: false };
        assert_eq!(HIDDEN.tree_command(TREE, "show", false), shown, "opens a hidden side bar");
        assert_eq!(EXPLORER.tree_command(TREE, "show", false), shown);
        assert_eq!(GIT.tree_command(TREE, "show", true), SideState { focused: true, ..shown });
        // Showing what shows already leaves the keyboard where it is.
        let held = SideState { focused: true, ..shown };
        assert_eq!(held.tree_command(TREE, "show", false), held);
        // The keyboard was in another view: showing this one doesn't hand it over.
        assert_eq!(SideState { focused: true, ..EXPLORER }.tree_command(TREE, "show", false), shown);
        // Focus takes it; again gives it back and stays.
        let on = HIDDEN.tree_command(TREE, "focus", false);
        assert_eq!(on, held);
        assert_eq!(on.tree_command(TREE, "focus", false), shown);
        // Hide goes back to the Explorer only if this is what shows; another view stays.
        assert_eq!(held.tree_command(TREE, "hide", false), EXPLORER);
        assert_eq!(GIT.tree_command(TREE, "hide", false), GIT);
        assert_eq!(HIDDEN.tree_command(TREE, "hide", false), HIDDEN);
        // Toggle.
        assert_eq!(EXPLORER.tree_command(TREE, "toggle", false), shown);
        assert_eq!(shown.tree_command(TREE, "toggle", false), EXPLORER);
        // Another tree view's is its own.
        assert_eq!(shown.tree_command(SideView::Tree(2), "hide", false), shown);
    }

    #[test]
    fn git_command_shows_source_control_in_the_explorers_place() {
        assert_eq!(HIDDEN.git_command("focus"), SideState { focused: true, ..GIT });
        assert_eq!(EXPLORER.git_command("focus"), SideState { focused: true, ..GIT });
        assert_eq!(EXPLORER.git_command("show"), GIT);
        assert_eq!(HIDDEN.git_command("show"), GIT);
        // Toggle: hidden or the other view shows it; showing it hides the bar.
        assert_eq!(HIDDEN.git_command("toggle"), GIT);
        assert_eq!(EXPLORER.git_command("toggle"), GIT);
        let focused = SideState { focused: true, ..GIT };
        assert_eq!(focused.git_command("toggle"), SideState { visible: false, focused: false, ..GIT });
        assert_eq!(focused.git_command("hide"), SideState { visible: false, focused: false, ..GIT });
        // No argument is a toggle too.
        assert_eq!(GIT.git_command(""), SideState { visible: false, ..GIT });
        // The Graph's own part is the view's (`show_graph`); the side bar is as for "focus".
        assert_eq!(HIDDEN.git_command("graph"), SideState { focused: true, ..GIT });
        assert_eq!(EXPLORER.git_command("graph"), SideState { focused: true, ..GIT });
    }

    #[test]
    fn explorer_command_goes_back_to_the_explorer_only_to_focus() {
        assert_eq!(GIT.explorer_command("focus"), SideState { focused: true, ..EXPLORER });
        assert_eq!(GIT.explorer_command("reveal"), SideState { focused: true, ..EXPLORER });
        // Showing, hiding and toggling keep the view.
        assert_eq!(SideState { visible: false, ..GIT }.explorer_command("show"), GIT);
        assert_eq!(GIT.explorer_command("toggle"), SideState { visible: false, ..GIT });
        assert_eq!(SideState { visible: false, ..GIT }.explorer_command("toggle"), GIT);
        let focused = SideState { focused: true, ..GIT };
        assert_eq!(focused.explorer_command("hide"), SideState { visible: false, focused: false, ..GIT });
        assert_eq!(HIDDEN.explorer_command("focus"), SideState { focused: true, ..EXPLORER });
    }
}
