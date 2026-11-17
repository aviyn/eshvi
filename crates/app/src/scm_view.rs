//! Source Control in the side bar (VS Code style): the repository and its
//! branch, a message box and the commit button, the changed files in
//! sections, and the commit graph. It takes the explorer's place, drawn
//! like it (rows, icons, selection, menus, dialogs). It only asks: what
//! it wants done comes back as `Action`s, the app does them (`app/scm.rs`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, FontId, Id, Key, Margin, Modifiers, Pos2, Rect, RectAlign, Sense, Stroke, Vec2};

use nvim_bridge::{RpcValue, UiCommand};

use crate::decorations::{DecoColors, GitStatus};
use crate::diff_view::{compared, side_name, FileSpec, Files, Mode, Request, Side, Status};
use crate::explorer_view::{paint_chevron, ExplorerColors, ICON_PER_PT, INDENT_PER_PT};
use crate::fonts::GridFonts;
use crate::git::{After, Commit, CommitStats, Details, Entry, Op, Operation, Repo};
use crate::icons::{IconTextures, IconTheme};
use crate::popup::{mix, PopupTheme};
use crate::style::{self, Chrome, Glyph, Icon};

/// Space before the first and after the last thing in a row (the
/// explorer's rows start their content 10 points in).
const MARGIN: f32 = 10.0;
/// The graph shows at most this many lanes.
const MAX_LANES: usize = 6;
/// The message box grows to this many lines, then scrolls.
const MESSAGE_ROWS: f32 = 10.0;
/// How much of the room below the buttons the graph takes, until it's resized.
const GRAPH_SHARE: f32 = 0.35;

/// What the panel asks the app to do.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Change the repository (in its folder; `Init` in the explorer's).
    Op(Op),
    /// Show a file's changes.
    Diff(DiffSpec),
    /// Show all of a commit's changes.
    CommitDiff(Commit),
    /// Show all the changes of a list (Staged, or Changes) in the GUI's diff view.
    DiffAll(crate::diff_view::Request),
    /// Open a file in Neovim.
    Open(PathBuf),
    /// The side bar has the keyboard.
    Focus,
    /// Give keyboard focus back to Neovim.
    FocusEditor,
    /// Read the repository again now.
    Refresh,
    /// Show the bottom panel's Git output.
    ShowGitOutput,
    /// The files of a commit in the graph (answered by `set_files`).
    CommitFiles(Commit),
    /// How much a commit in the graph changed (answered by `set_stats`).
    CommitStats(Commit),
    CopyText(String),
    /// Show in Finder (or the platform's file manager).
    Reveal(PathBuf),
    SendToAgent(PathBuf),
}

/// A file's diff as `Eshvi.changes_diff` takes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiffSpec {
    pub top: PathBuf,
    /// The left side's ref ("" is the index).
    pub base: String,
    pub path: String,
    pub deleted: bool,
    /// The left side's path, if the file was renamed.
    pub left_path: Option<String>,
    /// A ref for a read-only right side ("" the index) instead of the file.
    pub right: Option<String>,
    pub left_label: Option<String>,
    pub right_label: Option<String>,
}

impl DiffSpec {
    /// The call that opens it; `how` is `[agent] diff`. Paths go as
    /// arguments: a file name can hold anything, `]==]` and quotes included.
    pub fn lua(&self, how: &str) -> UiCommand {
        let mut opts = Vec::new();
        for (key, value) in [
            ("left_path", &self.left_path),
            ("right", &self.right),
            ("left_label", &self.left_label),
            ("right_label", &self.right_label),
        ] {
            if let Some(v) = value {
                opts.push((RpcValue::from(key), RpcValue::from(v.as_str())));
            }
        }
        let mut args = vec![
            RpcValue::from(self.top.to_string_lossy().as_ref()),
            RpcValue::from(self.base.as_str()),
            RpcValue::from(self.path.as_str()),
            RpcValue::from(self.deleted),
            RpcValue::from(how),
        ];
        if !opts.is_empty() {
            args.push(RpcValue::Map(opts));
        }
        UiCommand::Lua { code: "return Eshvi.changes_diff(...)".into(), args }
    }
}

impl DiffSpec {
    /// The same file's diff for the GUI's view: the left side is a ref (the
    /// index when `base` is empty), the right a ref, the index or the file.
    pub fn file_spec(&self) -> FileSpec {
        let left = if self.base.is_empty() { Side::Index } else { Side::Ref(self.base.clone()) };
        let right = match self.right.as_deref() {
            None => Side::Disk,
            Some("") => Side::Index,
            Some(r) => Side::Ref(r.to_owned()),
        };
        let status = match (self.deleted, &self.left_path) {
            (true, _) => Status::Deleted,
            (_, Some(old)) if *old != self.path => Status::Renamed,
            _ => Status::Modified,
        };
        FileSpec { path: self.path.clone(), old_path: self.left_path.clone(), status, left, right }
    }

    /// The GUI's view with this one file.
    pub fn request(&self) -> Request {
        let file = self.file_spec();
        Request { top: self.top.clone(), compared: compared(&file.left, &file.right), focus: Some(self.path.clone()), mode: Mode::of(&file.left, &file.right), files: Files::List(vec![file]) }
    }
}

/// A file of the view as Neovim's diff takes it (a file too large to show in
/// the view: `Eshvi.changes_diff` with "tab").
pub fn neovim_spec(top: &Path, f: &FileSpec) -> DiffSpec {
    let base = match &f.left {
        Side::Ref(r) => r.clone(),
        _ => String::new(),
    };
    let right = match &f.right {
        Side::Disk | Side::Missing => None,
        Side::Index => Some(String::new()),
        Side::Ref(r) => Some(r.clone()),
    };
    DiffSpec {
        top: top.to_path_buf(),
        base,
        path: f.path.clone(),
        deleted: f.status == Status::Deleted,
        left_path: f.old_path.clone(),
        left_label: Some(side_name(&f.left)),
        right_label: Some(side_name(&f.right)),
        right,
    }
}

fn file_status(status: GitStatus) -> Status {
    match status {
        GitStatus::Untracked | GitStatus::Added => Status::Added,
        GitStatus::Deleted => Status::Deleted,
        GitStatus::Renamed => Status::Renamed,
        GitStatus::Modified | GitStatus::Conflict => Status::Modified,
    }
}

/// All of a list's changes for the GUI's view: Staged against `HEAD`, Changes
/// (the working tree) against the index, a new file against nothing. None
/// for the conflicts (they are only opened) and an empty list.
pub fn view_all_request(repo: &Repo, section: Section) -> Option<Request> {
    let (left, right) = match section {
        Section::Merge => return None,
        Section::Staged => (Side::Ref("HEAD".into()), Side::Index),
        Section::Changes => (Side::Index, Side::Disk),
    };
    let files: Vec<FileSpec> = section
        .entries(repo)
        .iter()
        .map(|e| {
            let status = file_status(e.status);
            FileSpec {
                path: e.path.clone(),
                old_path: e.orig.clone(),
                status,
                left: if status == Status::Added { Side::Missing } else { left.clone() },
                right: match (status, &right) {
                    (Status::Deleted, Side::Index) => Side::Missing,
                    _ => right.clone(),
                },
            }
        })
        .collect();
    (!files.is_empty()).then(|| Request { top: repo.top.clone(), compared: compared(&left, &right), files: Files::List(files), focus: None, mode: Mode::of(&left, &right) })
}

/// A commit's changes for the GUI's view: its files, against its first parent.
pub fn commit_request(top: &Path, commit: &Commit) -> Request {
    let short = |oid: &str| oid.chars().take(7).collect::<String>();
    let parent = commit.parents.first().map_or_else(|| "nothing".to_owned(), |p| short(p));
    Request { top: top.to_path_buf(), compared: format!("{parent} → {}", short(&commit.oid)), files: Files::Commit(commit.clone()), focus: None, mode: Mode::ReadOnly }
}

/// The lists of changed files, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    Merge,
    Staged,
    Changes,
}

impl Section {
    const ALL: [Section; 3] = [Section::Merge, Section::Staged, Section::Changes];

    fn title(self) -> &'static str {
        match self {
            Section::Merge => "Merge Changes",
            Section::Staged => "Staged Changes",
            Section::Changes => "Changes",
        }
    }

    fn entries(self, repo: &Repo) -> &[Entry] {
        match self {
            Section::Merge => &repo.merge,
            Section::Staged => &repo.staged,
            Section::Changes => &repo.changes,
        }
    }
}

/// A file row: its section and path (a file can be in two).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RowKey {
    pub section: Section,
    pub path: String,
}

fn find<'a>(repo: &'a Repo, key: &RowKey) -> Option<&'a Entry> {
    key.section.entries(repo).iter().find(|e| e.path == key.path)
}

/// The diff of a changed file; none for a new file or a conflict, which
/// are only opened.
pub fn diff_for(top: &Path, section: Section, e: &Entry) -> Option<DiffSpec> {
    let deleted = e.status == GitStatus::Deleted;
    let spec = DiffSpec { top: top.to_path_buf(), path: e.path.clone(), deleted, ..Default::default() };
    match section {
        Section::Merge => None,
        Section::Changes if e.status == GitStatus::Untracked => None,
        Section::Changes => Some(DiffSpec {
            left_label: Some("Index".into()),
            right_label: Some("Working Tree".into()),
            ..spec
        }),
        Section::Staged => Some(DiffSpec {
            base: "HEAD".into(),
            left_path: e.orig.clone(),
            right: Some(String::new()),
            left_label: Some("HEAD".into()),
            right_label: Some("Index".into()),
            ..spec
        }),
    }
}

/// A file of a commit, against the commit's first parent (a first commit:
/// an empty left, which `oid^` gives, as no such revision).
pub fn commit_diff(top: &Path, commit: &Commit, e: &Entry) -> DiffSpec {
    file_at_commit(top, &commit.oid, commit.parents.first().map(String::as_str), &e.path, e.orig.as_deref(), e.status == GitStatus::Deleted)
}

/// One file of commit `oid`, against its first parent (`None`: a root
/// commit, which has nothing to compare with). What a file under a commit in
/// the Graph opens, and what a picker's row of a file's history does.
pub fn file_at_commit(top: &Path, oid: &str, parent: Option<&str>, path: &str, left_path: Option<&str>, deleted: bool) -> DiffSpec {
    let short = |oid: &str| oid.chars().take(7).collect::<String>();
    DiffSpec {
        top: top.to_path_buf(),
        base: parent.map_or_else(|| format!("{oid}^"), str::to_owned),
        path: path.to_owned(),
        deleted,
        left_path: left_path.map(str::to_owned),
        right: Some(oid.to_owned()),
        left_label: Some(parent.map_or_else(|| "Empty".to_owned(), short)),
        right_label: Some(short(oid)),
    }
}

/// The call that opens all of a commit's changes (in a buffer, whatever
/// `[git] diff` says: they have no two sides to show).
pub fn commit_lua(top: &Path, commit: &Commit) -> UiCommand {
    let args = vec![RpcValue::from(top.to_string_lossy().as_ref()), RpcValue::from(commit.oid.as_str())];
    UiCommand::Lua { code: "return Eshvi.commit_diff(...)".into(), args }
}

/// What a click on a row does: its diff, or the file itself.
pub fn open_changes(repo: &Repo, key: &RowKey) -> Option<Action> {
    let e = find(repo, key)?;
    Some(match diff_for(&repo.top, key.section, e) {
        Some(spec) => Action::Diff(spec),
        None => Action::Open(repo.top.join(&e.path)),
    })
}

/// What the big button under the message box does now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MainButton {
    Commit,
    /// Push a branch that has no upstream yet.
    Publish,
    Sync { ahead: u32, behind: u32 },
    /// Nothing to do.
    Idle,
}

/// Button `n` from the right of a title bar: its rect, its response, and the
/// color to draw it in. One that can't be used now is faded and still takes the
/// click, so it doesn't reach the bar behind it: the caller ignores it.
pub(crate) fn title_button(ui: &egui::Ui, colors: &ExplorerColors, row_h: f32, title: Rect, n: f32, name: &str, tip: &str, enabled: bool) -> (Rect, egui::Response, Color32) {
    let rect = Rect::from_center_size(Pos2::new(title.max.x - 18.0 - n * row_h, title.center().y), Vec2::splat(row_h));
    let resp = ui.interact(rect, Id::unique(("eshvi-scm-header", name)), Sense::CLICK).on_hover_text(tip);
    if !enabled {
        return (rect, resp, colors.dim.gamma_multiply(style::DISABLED));
    }
    if resp.hovered() {
        ui.painter().rect_filled(rect.shrink(2.0), style::RADIUS, colors.hover);
    }
    let color = if resp.hovered() { colors.fg } else { colors.dim };
    (rect, resp, color)
}

pub fn main_button(repo: &Repo) -> MainButton {
    if !repo.merge.is_empty() || !repo.staged.is_empty() || !repo.changes.is_empty() {
        MainButton::Commit
    } else if repo.branch.is_some() && repo.upstream.is_none() && repo.head.is_some() {
        MainButton::Publish
    } else if repo.ahead > 0 || repo.behind > 0 {
        MainButton::Sync { ahead: repo.ahead, behind: repo.behind }
    } else {
        MainButton::Idle
    }
}

/// What a commit asks for, as VS Code's smart commit.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// No message: ask for one, do nothing.
    NeedMessage,
    /// Nothing is staged: ask whether to stage everything.
    Confirm(Op),
    Run(Op),
}

/// Commit with `message`; `all` stages everything first (Commit All).
pub fn decide_commit(repo: &Repo, message: &str, amend: bool, all: bool, then: After) -> Decision {
    let message = message.trim_end().to_owned();
    if message.trim().is_empty() && !amend {
        return Decision::NeedMessage;
    }
    let commit = |stage_all| Op::Commit { message: message.clone(), amend, stage_all, then };
    if !amend && !all && repo.staged.is_empty() && !repo.changes.is_empty() {
        Decision::Confirm(commit(true))
    } else {
        Decision::Run(commit(all))
    }
}

/// The paths to stage: the changes and conflicts among `keys`.
fn stage_op(keys: &[RowKey]) -> Option<Op> {
    let paths: Vec<String> = keys.iter().filter(|k| k.section != Section::Staged).map(|k| k.path.clone()).collect();
    (!paths.is_empty()).then_some(Op::Stage(paths))
}

/// The paths to unstage: the staged among `keys`, a rename's old path too.
fn unstage_op(repo: &Repo, keys: &[RowKey]) -> Option<Op> {
    let mut paths = Vec::new();
    for key in keys.iter().filter(|k| k.section == Section::Staged) {
        paths.push(key.path.clone());
        paths.extend(find(repo, key).and_then(|e| e.orig.clone()));
    }
    (!paths.is_empty()).then_some(Op::Unstage(paths))
}

/// What discarding the changes among `keys` would do.
fn discard_of(repo: &Repo, keys: &[RowKey]) -> Option<Confirm> {
    let (mut tracked, mut untracked) = (Vec::new(), Vec::new());
    for key in keys.iter().filter(|k| k.section == Section::Changes) {
        match find(repo, key) {
            Some(e) if e.status == GitStatus::Untracked => untracked.push(e.path.clone()),
            Some(e) => tracked.push(e.path.clone()),
            None => {}
        }
    }
    (!tracked.is_empty() || !untracked.is_empty()).then_some(Confirm::Discard { tracked, untracked })
}

/// The remote to publish to: "origin" if there is one.
fn publish_remote(remotes: &[String]) -> Option<String> {
    remotes.iter().find(|r| *r == "origin").or(remotes.first()).cloned()
}

/// A question to answer before something is done.
#[derive(Clone, Debug, PartialEq)]
enum Confirm {
    Discard { tracked: Vec<String>, untracked: Vec<String> },
    /// Commit everything, though nothing is staged.
    StageAllCommit(Op),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptKind {
    Branch,
    /// `publish`: push the branch to it once added.
    Remote { publish: bool },
}

/// A one-line question (two for a remote: its name and URL).
#[derive(Clone, Debug)]
struct Prompt {
    kind: PromptKind,
    text: String,
    url: String,
    /// Focus the first field on the next frame.
    focus: bool,
}

/// The buttons on a row or a section's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Btn {
    OpenFile,
    Discard,
    Stage,
    /// A conflict's: stage it, marking it resolved.
    Resolve,
    Unstage,
    DiscardAll,
    StageAll,
    UnstageAll,
    /// All the list's changes in one diff view.
    ViewAll,
}

impl Btn {
    fn tip(self) -> &'static str {
        match self {
            Btn::OpenFile => "Open File",
            Btn::Discard => "Discard Changes",
            Btn::Stage => "Stage Changes",
            Btn::Resolve => "Stage (mark resolved)",
            Btn::Unstage => "Unstage Changes",
            Btn::DiscardAll => "Discard All Changes",
            Btn::StageAll => "Stage All Changes",
            Btn::UnstageAll => "Unstage All Changes",
            Btn::ViewAll => "View All Changes",
        }
    }

    /// The buttons of a section's file rows, left to right.
    fn row(section: Section) -> &'static [Btn] {
        match section {
            Section::Changes => &[Btn::OpenFile, Btn::Discard, Btn::Stage],
            Section::Staged => &[Btn::OpenFile, Btn::Unstage],
            Section::Merge => &[Btn::OpenFile, Btn::Resolve],
        }
    }

    fn header(section: Section) -> &'static [Btn] {
        match section {
            Section::Changes => &[Btn::ViewAll, Btn::DiscardAll, Btn::StageAll],
            Section::Staged => &[Btn::ViewAll, Btn::UnstageAll],
            Section::Merge => &[Btn::StageAll],
        }
    }
}

/// The message box (it keeps keyboard focus).
pub fn message_id() -> Id {
    Id::unique("eshvi-scm-message")
}

/// The prompt's field (the remote's name, or the branch's).
pub fn prompt_id() -> Id {
    Id::unique("eshvi-scm-prompt")
}

/// The prompt's second field (the remote's URL).
pub fn prompt_url_id() -> Id {
    Id::unique("eshvi-scm-prompt-url")
}

/// A dialog's width.
const DIALOG_WIDTH: f32 = 320.0;
/// A commit's tooltip: how wide, in rows, and how much of a long message.
const TOOLTIP_ROWS: f32 = 16.0;
const TOOLTIP_BODY_LINES: usize = 20;
const COPY_HASH: &str = "Copy Commit Hash";
/// How long, in seconds, the button shows a check after copying.
const COPIED_FOR: f64 = 1.5;

/// How the panel is drawn: the explorer's colors and sizes.
struct Look<'a> {
    colors: &'a ExplorerColors,
    chrome: Chrome,
    /// The popups', for the dialogs.
    theme: &'a PopupTheme,
    deco: &'a DecoColors,
    font: FontId,
    small: FontId,
    /// Uppercase titles.
    bold: FontId,
    row_h: f32,
    ppp: f32,
}

impl Look<'_> {
    /// `text` on one line, cut with "…" at `width`.
    fn fit(&self, ui: &egui::Ui, text: &str, font: &FontId, color: Color32, width: f32, strike: bool) -> Arc<egui::Galley> {
        let mut format = TextFormat::simple(font.clone(), color);
        if strike {
            format.strikethrough = Stroke::new(style::HAIRLINE, color);
        }
        let mut job = LayoutJob::single_section(text.to_owned(), format);
        job.wrap = TextWrapping::truncate_at_width(width.max(0.0));
        ui.painter().layout_job(job)
    }

    /// The fill a row's buttons sit on.
    fn surface(&self, selected: bool, focused: bool, hovered: bool) -> Color32 {
        crate::explorer_view::row_surface(self.colors, selected, focused, hovered)
    }

    /// A row's fill: the explorer's, for selected and for hovered rows.
    fn paint_row_bg(&self, ui: &egui::Ui, rect: Rect, selected: bool, focused: bool, hovered: bool) {
        crate::explorer_view::paint_row_bg(ui.painter(), self.colors, rect, selected, focused, hovered);
    }
}

/// The panel's state.
pub struct ScmView {
    pub font_size: f32,
    /// Offer "Send to Agent" (the agent panel exists).
    pub agent: bool,
    textures: IconTextures,
    /// The explorer's folder, if one is open.
    folder: Option<PathBuf>,
    /// The first status has arrived (`repo` is `None` for no repository).
    loaded: bool,
    repo: Option<Arc<Repo>>,
    details: Arc<Details>,
    /// Operations running (`GitOpsWorker::running`).
    running: Vec<&'static str>,
    message: String,
    /// The merge message went into the box already.
    merge_filled: bool,
    /// What it put there, to take out again if the merge is called off.
    merge_text: Option<String>,
    /// Commit asked for with no message: say so under the box.
    need_message: bool,
    /// Focus the message box on the next frame.
    message_focus: bool,
    message_rect: Option<Rect>,
    selected: Option<RowKey>,
    /// More selected rows (Cmd/Shift-click), the anchor among them.
    multi: Vec<RowKey>,
    scroll_to_selected: bool,
    collapsed: HashSet<Section>,
    graph_open: bool,
    /// The height the graph was dragged to.
    graph_height: Option<f32>,
    /// The commit or commit's file last clicked in the graph.
    graph_selected: Option<(String, Option<String>)>,
    /// Commits whose files show under them, and the files that came.
    expanded: HashSet<String>,
    files: HashMap<String, Vec<Entry>>,
    /// How much each commit hovered changed: `None` until it's answered.
    stats: HashMap<String, Option<CommitStats>>,
    /// The hash last copied from a commit's tooltip, and when.
    copied: Option<(String, f64)>,
    /// Where a commit's tooltip is: over the editor, which mustn't get its clicks.
    tooltip: Option<Rect>,
    confirm: Option<Confirm>,
    prompt: Option<Prompt>,
}

/// The width of one lane of the graph.
/// "3 files changed, 10 insertions(+), 2 deletions(-)", as git words it:
/// what's unchanged is left out, and nothing at all for an empty commit.
fn stats_words(stats: CommitStats) -> Vec<(String, Option<GitStatus>)> {
    let many = |n: usize, one: &str, more: &str| format!("{n} {}", if n == 1 { one } else { more });
    let mut words = Vec::new();
    if stats.files > 0 {
        words.push((many(stats.files, "file changed", "files changed"), None));
    }
    if stats.insertions > 0 {
        words.push((many(stats.insertions, "insertion(+)", "insertions(+)"), Some(GitStatus::Added)));
    }
    if stats.deletions > 0 {
        words.push((many(stats.deletions, "deletion(-)", "deletions(-)"), Some(GitStatus::Deleted)));
    }
    words
}

/// What hovering a commit shows, as in VS Code: who and when, the whole
/// message, how much changed, the hash with a button to copy it, and the
/// refs. Returns whether the button was clicked.
fn commit_tooltip(ui: &mut egui::Ui, look: &Look, commit: &Commit, stats: Option<CommitStats>, copied: bool) -> bool {
    let colors = look.colors;
    let line = |ui: &mut egui::Ui, text: String, font: &FontId, color: Color32| {
        ui.label(egui::RichText::new(text).font(font.clone()).color(color));
    };
    ui.set_max_width(look.row_h * TOOLTIP_ROWS);
    // (Lines close together, a little more between the parts.)
    ui.spacing_mut().item_spacing.y = 0.0;
    let gap = style::GAP / 2.0;
    line(ui, format!("{}, {} ({})", commit.author, commit.when, commit.date), &look.small, colors.dim);
    ui.add_space(gap);
    line(ui, commit.subject.clone(), &look.font, colors.fg);
    if !commit.body.is_empty() {
        ui.add_space(gap);
        let mut body: Vec<&str> = commit.body.lines().take(TOOLTIP_BODY_LINES + 1).collect();
        if body.len() > TOOLTIP_BODY_LINES {
            body[TOOLTIP_BODY_LINES] = "…";
        }
        line(ui, body.join("\n"), &look.small, colors.fg);
    }
    let words = stats_words(stats.unwrap_or_default());
    if !words.is_empty() {
        ui.add_space(gap);
        let mut job = LayoutJob::default();
        for (i, (text, status)) in words.iter().enumerate() {
            if i > 0 {
                job.append(", ", 0.0, TextFormat::simple(look.small.clone(), colors.dim));
            }
            let color = status.map_or(colors.dim, |s| look.deco.git(s));
            job.append(text, 0.0, TextFormat::simple(look.small.clone(), color));
        }
        ui.label(job);
    }
    ui.add_space(gap);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let short: String = commit.oid.chars().take(7).collect();
        line(ui, short, &look.small, colors.dim);
        let size = Vec2::splat(style::button_size(look.row_h));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::CLICK);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, COPY_HASH));
        let hovered = resp.hovered();
        style::paint_button_fill(ui.painter(), rect, &look.chrome, look.theme.bg, hovered, false);
        let icon = if copied { Icon::Check } else { Icon::Files };
        style::paint_icon(ui.painter(), icon, rect.center(), style::icon_size(look.row_h), look.chrome.text(hovered));
        let clicked = resp.on_hover_text(if copied { "Copied" } else { COPY_HASH }).clicked();
        if !commit.refs.is_empty() {
            ui.add_space(style::GAP);
            line(ui, commit.refs.join("  "), &look.small, colors.dim);
        }
        clicked
    })
    .inner
}

fn lane_width(look: &Look) -> f32 {
    (look.row_h / 2.0).round()
}

/// Where lane `lane` runs, in a row at `rect` (the last lane takes the rest).
fn lane_x(rect: Rect, lane: usize, lane_w: f32) -> f32 {
    rect.min.x + MARGIN + lane.min(MAX_LANES - 1) as f32 * lane_w + lane_w / 2.0
}

/// The height of a pane at the foot of a view (the Graph, the Outline), of the
/// room `rest` below the buttons: what it was dragged to or its share, leaving
/// the lists at least a few rows.
pub(crate) fn pane_body(rest: f32, row_h: f32, dragged: Option<f32>) -> f32 {
    dragged.unwrap_or(rest * GRAPH_SHARE).max(row_h * 3.0).min((rest - row_h * 5.0).max(0.0))
}

/// A menu's list of `rows` items: as tall as it is, scrolling only when
/// that is more than half the screen. (A scroll area alone is sized by the
/// popup's first frame and stays two rows high.)
fn scrolling_list(ui: &mut egui::Ui, rows: usize, add: impl FnOnce(&mut egui::Ui)) {
    let most = ui.ctx().content_rect().height() * 0.5;
    let row = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
    if rows as f32 * row * 1.2 <= most {
        add(ui);
    } else {
        egui::ScrollArea::vertical().max_height(most).min_scrolled_height(most).show(ui, add);
    }
}

fn lane_color(look: &Look, lane: usize) -> Color32 {
    [look.colors.accent, look.deco.added, look.deco.modified, look.deco.deleted][lane % 4]
}

impl ScmView {
    pub fn new(font_size: f32, agent: bool) -> Self {
        Self {
            font_size,
            agent,
            textures: IconTextures::default(),
            folder: None,
            loaded: false,
            repo: None,
            details: Arc::default(),
            running: Vec::new(),
            message: String::new(),
            merge_filled: false,
            merge_text: None,
            need_message: false,
            message_focus: false,
            message_rect: None,
            selected: None,
            multi: Vec::new(),
            scroll_to_selected: false,
            collapsed: HashSet::new(),
            graph_open: true,
            graph_height: None,
            graph_selected: None,
            expanded: HashSet::new(),
            files: HashMap::new(),
            stats: HashMap::new(),
            copied: None,
            tooltip: None,
            confirm: None,
            prompt: None,
        }
    }

    /// The folder the explorer shows (where `Init` goes).
    pub fn set_root(&mut self, folder: Option<&Path>) {
        if self.folder.as_deref() != folder {
            self.folder = folder.map(Path::to_path_buf);
        }
    }

    /// The repository as `git status` just saw it (`None`: not one).
    pub fn set_repo(&mut self, repo: Option<Repo>) {
        self.loaded = true;
        if self.repo.as_deref() == repo.as_ref() {
            return;
        }
        let moved = self.repo.as_ref().map(|r| &r.top) != repo.as_ref().map(|r| &r.top);
        // Another repository: the message was for the last one.
        if moved && self.repo.is_some() {
            self.message.clear();
            self.need_message = false;
            self.merge_filled = false;
            self.merge_text = None;
        }
        match &repo {
            Some(r) => {
                // A merge's message goes into the box once, if it's empty;
                // when the merge is gone (aborted) it goes out again, if
                // the box still holds just that.
                match &r.merge_message {
                    Some(m) if !self.merge_filled => {
                        self.merge_filled = true;
                        if self.message.trim().is_empty() {
                            self.message = m.clone();
                            self.merge_text = Some(m.clone());
                        }
                    }
                    Some(_) => {}
                    None => {
                        self.merge_filled = false;
                        if self.merge_text.take().is_some_and(|m| m == self.message) {
                            self.message.clear();
                        }
                    }
                }
                if moved {
                    self.selected = None;
                    self.multi.clear();
                }
                // A selected row that left its section (staged, unstaged,
                // discarded) passes the selection to the one now in its
                // place, so `s` `s` `s` walks down the list.
                let gone = |k: &RowKey| find(r, k).is_none();
                if let Some(key) = self.selected.clone().filter(gone) {
                    let at = self.repo.as_deref().and_then(|old| key.section.entries(old).iter().position(|e| e.path == key.path));
                    self.selected = at.and_then(|at| self.successor(r, key.section, at));
                    self.scroll_to_selected = self.selected.is_some();
                }
                self.multi.retain(|k| !gone(k));
            }
            None => {
                self.selected = None;
                self.multi.clear();
            }
        }
        self.repo = repo.map(Arc::new);
    }

    /// The row to select when the one at `at` in `section` is gone: the row
    /// now at that index (or the last one), else the first row of the next
    /// open section that has any, else the last of an earlier one.
    fn successor(&self, repo: &Repo, section: Section, at: usize) -> Option<RowKey> {
        let key = |s: Section, i: usize| s.entries(repo).get(i).map(|e| RowKey { section: s, path: e.path.clone() });
        let own = section.entries(repo);
        if !own.is_empty() {
            return key(section, at.min(own.len() - 1));
        }
        let open = |s: &Section| !self.collapsed.contains(s) && !s.entries(repo).is_empty();
        let after = Section::ALL.into_iter().skip_while(|s| *s != section).skip(1).find(open);
        let before = Section::ALL.into_iter().take_while(|s| *s != section).filter(open).last();
        after.and_then(|s| key(s, 0)).or_else(|| before.and_then(|s| key(s, s.entries(repo).len() - 1)))
    }

    /// The branches and the graph; the files shown under commits are
    /// asked for again.
    pub fn set_details(&mut self, details: Details) {
        self.details = Arc::new(details);
        self.files.clear();
    }

    /// The files of commit `oid`, as asked for.
    pub fn set_files(&mut self, oid: String, files: Vec<Entry>) {
        self.files.insert(oid, files);
    }

    /// How much commit `oid` changed, as asked for. (A commit never changes:
    /// these stay when the graph is read again.)
    pub fn set_stats(&mut self, oid: String, stats: CommitStats) {
        self.stats.insert(oid, Some(stats));
    }

    pub fn over_tooltip(&self, pos: Pos2) -> bool {
        self.tooltip.is_some_and(|r| r.contains(pos))
    }

    pub fn set_running(&mut self, running: Vec<&'static str>) {
        self.running = running;
    }

    /// A commit went through: the message is done with.
    pub fn committed(&mut self) {
        self.message.clear();
        self.need_message = false;
        self.merge_text = None;
    }

    /// The message of a commit just undone goes back into the box, unless
    /// something is being written there.
    pub fn restore_message(&mut self, message: String) {
        if self.message.trim().is_empty() {
            self.message = message;
        }
    }

    /// The repository as last read.
    pub fn repo(&self) -> Option<&Repo> {
        self.repo.as_deref()
    }

    pub fn change_count(&self) -> usize {
        self.repo.as_ref().map_or(0, |r| r.change_count())
    }

    /// The repository's folder.
    pub fn top(&self) -> Option<PathBuf> {
        self.repo.as_ref().map(|r| r.top.clone())
    }

    /// Put the keyboard in the message box.
    pub fn focus_message(&mut self) {
        self.message_focus = true;
    }

    /// Unfold the Graph (`:EshviGit graph`).
    pub fn show_graph(&mut self) {
        self.graph_open = true;
    }

    /// The panel isn't shown: its questions go with it.
    pub fn close_dialogs(&mut self) {
        self.confirm = None;
        self.prompt = None;
        self.message_focus = false;
    }

    /// A text field or a question has the keyboard.
    pub fn wants_keyboard(&self, ctx: &egui::Context) -> bool {
        self.confirm.is_some()
            || self.prompt.is_some()
            || ctx.memory(|m| [message_id(), prompt_id(), prompt_url_id()].into_iter().any(|id| m.has_focus(id)))
    }

    // --- selection ---

    /// The rows of the open sections, in order.
    fn rows(&self) -> Vec<RowKey> {
        let Some(repo) = &self.repo else { return Vec::new() };
        let open = Section::ALL.into_iter().filter(|s| !self.collapsed.contains(s));
        open.flat_map(|s| s.entries(repo).iter().map(move |e| RowKey { section: s, path: e.path.clone() })).collect()
    }

    /// The selected rows: the extra selection, else the selected row.
    fn selection(&self) -> Vec<RowKey> {
        if self.multi.is_empty() {
            self.selected.iter().cloned().collect()
        } else {
            self.multi.clone()
        }
    }

    fn is_selected(&self, key: &RowKey) -> bool {
        if self.multi.is_empty() {
            self.selected.as_ref() == Some(key)
        } else {
            self.multi.contains(key)
        }
    }

    /// What a row's menu acts on: the selection when the row is in it, else
    /// just the row (which becomes the selection).
    fn targets_for(&mut self, key: &RowKey) -> Vec<RowKey> {
        if !self.is_selected(key) {
            self.selected = Some(key.clone());
            self.multi.clear();
        }
        self.selection()
    }

    fn select(&mut self, key: RowKey) {
        self.selected = Some(key);
        self.multi.clear();
        self.scroll_to_selected = true;
    }

    // --- doing things ---

    /// Commit with the message in the box.
    fn commit(&mut self, repo: &Repo, amend: bool, all: bool, then: After) -> Vec<Action> {
        match decide_commit(repo, &self.message, amend, all, then) {
            Decision::NeedMessage => {
                self.need_message = true;
                self.message_focus = true;
                vec![Action::Focus]
            }
            Decision::Confirm(op) => {
                self.confirm = Some(Confirm::StageAllCommit(op));
                Vec::new()
            }
            Decision::Run(op) => vec![Action::Op(op)],
        }
    }

    /// The main button's action.
    fn main_action(&mut self, repo: &Repo) -> Vec<Action> {
        match main_button(repo) {
            MainButton::Commit => self.commit(repo, false, false, After::Nothing),
            MainButton::Publish => self.publish(),
            MainButton::Sync { .. } => vec![Action::Op(Op::Sync)],
            MainButton::Idle => Vec::new(),
        }
    }

    /// Push the branch to a remote, asking for one first if there is none.
    fn publish(&mut self) -> Vec<Action> {
        match publish_remote(&self.details.remotes) {
            Some(remote) => vec![Action::Op(Op::Publish(remote))],
            None => {
                self.ask_remote(true);
                Vec::new()
            }
        }
    }

    fn ask_remote(&mut self, publish: bool) {
        // The first remote is "origin", unless it's named otherwise.
        let text = if self.details.remotes.is_empty() { "origin".to_owned() } else { String::new() };
        self.prompt = Some(Prompt { kind: PromptKind::Remote { publish }, text, url: String::new(), focus: true });
    }

    fn ask_branch(&mut self) {
        self.prompt = Some(Prompt { kind: PromptKind::Branch, text: String::new(), url: String::new(), focus: true });
    }

    /// What a finished prompt does.
    fn accept_prompt(&mut self) -> Vec<Action> {
        let Some(prompt) = self.prompt.take() else { return Vec::new() };
        let (text, url) = (prompt.text.trim().replace(' ', "-"), prompt.url.trim().to_owned());
        match prompt.kind {
            PromptKind::Branch if text.is_empty() => Vec::new(),
            PromptKind::Branch => vec![Action::Op(Op::CreateBranch(text))],
            PromptKind::Remote { .. } if text.is_empty() || url.is_empty() => {
                self.prompt = Some(Prompt { focus: true, ..prompt });
                Vec::new()
            }
            PromptKind::Remote { publish } => {
                let mut ops = vec![Action::Op(Op::AddRemote { name: text.clone(), url })];
                if publish {
                    ops.push(Action::Op(Op::Publish(text)));
                }
                ops
            }
        }
    }

    /// Keys while the list has the keyboard.
    pub fn key(&mut self, key: Key, mods: Modifiers) -> Option<Action> {
        if key == Key::Escape {
            return Some(Action::FocusEditor);
        }
        if mods.command || mods.ctrl || mods.alt {
            return None;
        }
        let repo = self.repo.clone()?;
        let rows = self.rows();
        let idx = self.selected.as_ref().and_then(|s| rows.iter().position(|r| r == s));
        let go = |this: &mut Self, i: usize| {
            if let Some(row) = rows.get(i.min(rows.len().saturating_sub(1))) {
                this.select(row.clone());
            }
        };
        match key {
            Key::ArrowUp | Key::K => go(self, idx.map_or(0, |i| i.saturating_sub(1))),
            Key::ArrowDown | Key::J => go(self, idx.map_or(0, |i| i + 1)),
            Key::Home => go(self, 0),
            Key::End => go(self, rows.len().saturating_sub(1)),
            Key::Enter | Key::O => return open_changes(&repo, self.selected.as_ref()?),
            Key::F => {
                let e = find(&repo, self.selected.as_ref()?)?;
                return Some(Action::Open(repo.top.join(&e.path)));
            }
            Key::S | Key::Space => return stage_op(&self.selection()).map(Action::Op),
            Key::U => return unstage_op(&repo, &self.selection()).map(Action::Op),
            Key::D => self.confirm = discard_of(&repo, &self.selection()),
            Key::C => {
                self.message_focus = true;
                return Some(Action::Focus);
            }
            Key::R => return Some(Action::Refresh),
            _ => {}
        }
        None
    }

    /// Tab from the message box: to the first row.
    fn focus_list(&mut self) -> Vec<Action> {
        if let Some(first) = self.rows().into_iter().next() {
            self.select(first);
        }
        vec![Action::Focus]
    }
}

// --- drawing ---

impl ScmView {
    /// How much of the header's right end the three buttons take.
    pub fn header_reserved(row_h: f32) -> f32 {
        18.0 + row_h * 2.5 + 4.0
    }

    /// Draw the panel below `header` (the side bar's: the buttons go at its
    /// right end). `focused`: the side bar has the keyboard.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        header: Rect,
        colors: &ExplorerColors,
        theme: &PopupTheme,
        fonts: &GridFonts,
        deco: &DecoColors,
        focused: bool,
    ) -> Vec<Action> {
        let mut font = fonts.sidebar_font_id(false);
        font.size = self.font_size;
        let mut bold = fonts.sidebar_font_id(true);
        bold.size = (self.font_size * style::BADGE).round();
        let look = Look {
            colors,
            chrome: colors.chrome(),
            theme,
            deco,
            small: style::scaled(&font, style::SMALL),
            font,
            bold,
            row_h: crate::side_bar::row_height(self.font_size),
            ppp: ui.ctx().pixels_per_point(),
        };
        self.draw(ui, header, &look, focused)
    }

    fn draw(&mut self, ui: &mut egui::Ui, header: Rect, look: &Look, focused: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        self.tooltip = None;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let repo = self.repo.clone();
        match (&self.folder, self.loaded, &repo) {
            (None, ..) => {
                self.message_focus = false;
                self.note(ui, look, "Open a folder to use source control.");
            }
            // Not asked yet.
            (Some(_), false, _) => {}
            (Some(_), true, None) => {
                self.message_focus = false;
                self.note(ui, look, "The folder currently open doesn't have a git repository.");
                let rect = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h + 4.0), Sense::hover()).0;
                let button = rect.shrink2(Vec2::new(MARGIN, 0.0));
                if self.primary_button(ui, look, button, "Initialize Repository", None, !self.running.is_empty(), false).0.clicked() {
                    actions.push(Action::Op(Op::Init));
                }
            }
            (Some(_), true, Some(repo)) => {
                self.header_buttons(ui, header, look, repo, &mut actions);
                self.repo_row(ui, look, repo, &mut actions);
                ui.add_space(look.row_h / 6.0);
                self.message_box(ui, look, repo, &mut actions);
                ui.add_space(look.row_h / 4.0);
                self.main_button_row(ui, look, repo, &mut actions);
                ui.add_space(look.row_h / 4.0);
                // (The list is not the one with the keyboard while the box is.)
                let in_list = focused && !ui.memory(|m| m.has_focus(message_id()));
                self.panes(ui, look, repo, in_list, &mut actions);
            }
        }
        self.paint_progress(ui, header, look);
        self.dialogs(ui, look, &mut actions);
        actions
    }

    /// A dim paragraph, with the side margin.
    fn note(&self, ui: &mut egui::Ui, look: &Look, text: &str) {
        ui.add_space(look.row_h / 2.0);
        let galley = ui.painter().layout(text.to_owned(), look.small.clone(), look.colors.dim, ui.available_width() - 2.0 * MARGIN);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), galley.size().y + look.row_h / 2.0), Sense::hover());
        ui.painter().galley(rect.min + Vec2::new(MARGIN, 0.0), galley, look.colors.dim);
    }

    /// The thin accent line sliding along the header's foot while git works.
    fn paint_progress(&self, ui: &egui::Ui, header: Rect, look: &Look) {
        if self.running.is_empty() {
            return;
        }
        let track = Rect::from_min_size(Pos2::new(header.min.x, header.max.y - 2.0), Vec2::new(header.width(), 2.0));
        let w = track.width();
        // A third of the track sliding across, over and over.
        let t = (ui.input(|i| i.time) * 0.8).fract() as f32;
        let x = track.min.x + (w * 4.0 / 3.0) * t - w / 3.0;
        let fill = Rect::from_x_y_ranges(x.max(track.min.x)..=(x + w / 3.0).min(track.max.x), track.y_range());
        if fill.width() > 0.0 {
            ui.painter().rect_filled(fill, 0.0, look.colors.accent);
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }

    /// The header's buttons: Commit, Refresh and the "⋯" menu, right to left.
    fn header_buttons(&mut self, ui: &mut egui::Ui, header: Rect, look: &Look, repo: &Repo, actions: &mut Vec<Action>) {
        let button = |ui: &egui::Ui, n: f32, name: &str, tip: &str| title_button(ui, look.colors, look.row_h, header, n, name, tip, true);
        let (rect, resp, color) = button(ui, 0.0, "more", "More Actions…");
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "⋯", look.font.clone(), color);
        egui::Popup::menu(&resp).show(|ui| self.more_menu(ui, repo, actions));
        let (rect, resp, color) = button(ui, 1.0, "refresh", "Refresh");
        style::paint_icon(ui.painter(), Icon::Refresh, rect.center(), style::icon_size(look.row_h), color);
        if resp.clicked() {
            actions.push(Action::Refresh);
        }
        let (rect, resp, color) = button(ui, 2.0, "commit", "Commit");
        style::paint_icon(ui.painter(), Icon::Check, rect.center(), style::icon_size(look.row_h), color);
        if resp.clicked() {
            actions.extend(self.commit(repo, false, false, After::Nothing));
        }
    }

    /// The Graph's title bar, as in VS Code: Fetch, Pull, and Push, or Publish
    /// for a branch with no upstream. Shown whether it's open or not.
    fn graph_buttons(&mut self, ui: &egui::Ui, title: Rect, look: &Look, repo: &Repo, actions: &mut Vec<Action>) {
        let idle = self.running.is_empty();
        let publish = repo.upstream.is_none() && repo.branch.is_some() && repo.head.is_some();
        let upstream = repo.upstream.is_some();
        let (tip, can_push) = if publish { ("Publish Branch", true) } else { ("Push", upstream) };
        let (can_push, can_pull) = (idle && can_push, idle && upstream);
        let can_fetch = idle && !self.details.remotes.is_empty();
        let (rect, resp, color) = title_button(ui, look.colors, look.row_h, title, 0.0, "push", tip, can_push);
        self.textures.paint_codicon(ui.painter(), "repo-push", rect.center(), style::icon_size(look.row_h), color);
        if resp.clicked() && can_push {
            if publish {
                actions.extend(self.publish());
            } else {
                actions.push(Action::Op(Op::Push));
            }
        }
        let (rect, resp, color) = title_button(ui, look.colors, look.row_h, title, 1.0, "pull", "Pull", can_pull);
        self.textures.paint_codicon(ui.painter(), "repo-pull", rect.center(), style::icon_size(look.row_h), color);
        if resp.clicked() && can_pull {
            actions.push(Action::Op(Op::Pull));
        }
        let (rect, resp, color) = title_button(ui, look.colors, look.row_h, title, 2.0, "fetch", "Fetch", can_fetch);
        self.textures.paint_codicon(ui.painter(), "git-fetch", rect.center(), style::icon_size(look.row_h), color);
        if resp.clicked() && can_fetch {
            actions.push(Action::Op(Op::Fetch));
        }
    }

    /// The "⋯" menu.
    fn more_menu(&mut self, ui: &mut egui::Ui, repo: &Repo, actions: &mut Vec<Action>) {
        let item = |ui: &mut egui::Ui, enabled: bool, label: &str| ui.add_enabled(enabled, egui::Button::new(label)).clicked();
        let (upstream, head) = (repo.upstream.is_some(), repo.head.is_some());
        let (staged, changes) = (!repo.staged.is_empty(), !repo.changes.is_empty());
        let op = |actions: &mut Vec<Action>, op: Op| actions.push(Action::Op(op));
        let abort = match repo.operation {
            Operation::None => None,
            Operation::Merge => Some("Abort Merge"),
            Operation::Rebase => Some("Abort Rebase"),
            Operation::CherryPick => Some("Abort Cherry-Pick"),
        };
        if let Some(label) = abort {
            if item(ui, true, label) {
                op(actions, Op::Abort(repo.operation));
            }
            ui.separator();
        }
        if item(ui, upstream, "Pull") {
            op(actions, Op::Pull);
        }
        if item(ui, upstream, "Push") {
            op(actions, Op::Push);
        }
        if item(ui, !self.details.remotes.is_empty(), "Fetch") {
            op(actions, Op::Fetch);
        }
        if item(ui, upstream, "Sync") {
            op(actions, Op::Sync);
        }
        ui.separator();
        ui.menu_button("Commit", |ui| {
            if item(ui, true, "Commit") {
                actions.extend(self.commit(repo, false, false, After::Nothing));
            }
            if item(ui, staged, "Commit Staged") {
                actions.extend(self.commit(repo, false, false, After::Nothing));
            }
            if item(ui, staged || changes, "Commit All") {
                actions.extend(self.commit(repo, false, true, After::Nothing));
            }
            if item(ui, head, "Commit (Amend)") {
                actions.extend(self.commit(repo, true, false, After::Nothing));
            }
            if item(ui, head, "Undo Last Commit") {
                op(actions, Op::UndoCommit);
            }
        });
        ui.menu_button("Changes", |ui| {
            if item(ui, changes, "Stage All Changes") {
                op(actions, Op::StageAll);
            }
            if item(ui, staged, "Unstage All Changes") {
                op(actions, Op::UnstageAll);
            }
            if item(ui, changes, "Discard All Changes") {
                let all: Vec<RowKey> = repo.changes.iter().map(|e| RowKey { section: Section::Changes, path: e.path.clone() }).collect();
                self.confirm = discard_of(repo, &all);
            }
        });
        ui.menu_button("Branch", |ui| {
            if item(ui, true, "Create Branch…") {
                self.ask_branch();
            }
            ui.menu_button("Merge Branch", |ui| {
                let others: Vec<_> = self.details.branches.iter().filter(|b| b.remote || repo.branch.as_deref() != Some(&b.name)).map(|b| b.name.clone()).collect();
                if others.is_empty() {
                    item(ui, false, "No other branches");
                }
                scrolling_list(ui, others.len(), |ui| {
                    for name in others {
                        if item(ui, true, &name) {
                            op(actions, Op::Merge(name));
                        }
                    }
                });
            });
            if item(ui, repo.branch.is_some() && !upstream && head, "Publish Branch") {
                actions.extend(self.publish());
            }
        });
        ui.menu_button("Remote", |ui| {
            if item(ui, true, "Add Remote…") {
                self.ask_remote(false);
            }
        });
        ui.menu_button("Stash", |ui| {
            if item(ui, staged || changes, "Stash") {
                op(actions, Op::Stash { untracked: false });
            }
            if item(ui, staged || changes, "Stash (Include Untracked)") {
                op(actions, Op::Stash { untracked: true });
            }
            if item(ui, true, "Pop Stash") {
                op(actions, Op::StashPop);
            }
        });
        ui.separator();
        if item(ui, true, "Show Git Output") {
            actions.push(Action::ShowGitOutput);
        }
    }

    /// The repository's name and branch; a click opens the branch menu.
    fn repo_row(&mut self, ui: &mut egui::Ui, look: &Look, repo: &Repo, actions: &mut Vec<Action>) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::CLICK);
        let was_open = egui::Popup::menu(&resp).is_open();
        if resp.hovered() || was_open {
            ui.painter().rect_filled(rect, 0.0, look.colors.hover);
        }
        let cy = rect.center().y;
        let room = rect.width() - 2.0 * MARGIN;
        let name = repo.top.file_name().map(|n| n.to_string_lossy().to_uppercase()).unwrap_or_default();
        let name = look.fit(ui, &name, &look.bold, look.colors.fg, room * 0.4, false);
        let mut x = rect.min.x + MARGIN;
        ui.painter().galley(Pos2::new(x, cy - name.size().y / 2.0), name.clone(), look.colors.fg);
        x += name.size().x + MARGIN;
        let icon = style::icon_size(look.row_h);
        let branch = match (&repo.branch, &repo.head) {
            (Some(b), _) => b.clone(),
            (None, Some(oid)) => oid.chars().take(7).collect(),
            (None, None) => String::new(),
        };
        let behind = (repo.behind > 0).then(|| format!("↓{}", repo.behind));
        let ahead = (repo.ahead > 0).then(|| format!("↑{}", repo.ahead));
        let counts = [behind, ahead].into_iter().flatten().collect::<Vec<_>>().join(" ");
        let counts_w = if counts.is_empty() { 0.0 } else { ui.painter().layout_no_wrap(counts.clone(), look.small.clone(), look.colors.dim).size().x + 6.0 };
        let right = rect.max.x - MARGIN;
        if !branch.is_empty() && right - x > icon * 3.0 {
            style::paint_icon(ui.painter(), Icon::Branch, Pos2::new(x + icon / 2.0, cy), icon, look.colors.fg);
            x += icon + 4.0;
            let g = look.fit(ui, &branch, &look.font, look.colors.fg, right - x - counts_w, false);
            ui.painter().galley(Pos2::new(x, cy - g.size().y / 2.0), g.clone(), look.colors.fg);
            x += g.size().x + 6.0;
        }
        if counts_w > 0.0 && x + counts_w <= right + 6.0 {
            ui.painter().text(Pos2::new(x, cy), Align2::LEFT_CENTER, &counts, look.small.clone(), look.colors.dim);
        }
        egui::Popup::menu(&resp).show(|ui| self.branch_menu(ui, repo, actions));
        if resp.clicked() && !was_open {
            // The branches may have moved since.
            actions.push(Action::Refresh);
        }
    }

    /// Create a branch, switch to one, or start tracking a remote one.
    fn branch_menu(&mut self, ui: &mut egui::Ui, repo: &Repo, actions: &mut Vec<Action>) {
        if ui.button("Create Branch…").clicked() {
            self.ask_branch();
        }
        ui.separator();
        let details = self.details.clone();
        let local = |name: &str| details.branches.iter().any(|b| !b.remote && b.name == name);
        let remote: Vec<_> =
            details.branches.iter().filter(|b| b.remote && !local(b.name.split_once('/').map_or("", |(_, n)| n))).collect();
        let rows = details.branches.len() + 1;
        scrolling_list(ui, rows, |ui| {
            // (The mark follows the status, which is never a refresh behind.)
            for b in details.branches.iter().filter(|b| !b.remote) {
                let current = repo.branch.as_deref() == Some(&b.name);
                let button = egui::Button::new(&b.name).shortcut_text(if current { "✓" } else { "" });
                if ui.add(button).clicked() && !current {
                    actions.push(Action::Op(Op::Checkout(b.name.clone())));
                }
            }
            if details.branches.is_empty() {
                if let Some(b) = &repo.branch {
                    ui.add_enabled(false, egui::Button::new(b).shortcut_text("✓"));
                }
            }
            if !remote.is_empty() {
                ui.separator();
                for b in remote {
                    if ui.button(&b.name).clicked() {
                        actions.push(Action::Op(Op::Track(b.name.clone())));
                    }
                }
            }
        });
    }

    /// The commit message, in a field like the Problems filter's.
    fn message_box(&mut self, ui: &mut egui::Ui, look: &Look, repo: &Repo, actions: &mut Vec<Action>) {
        let id = message_id();
        let had_focus = ui.ctx().memory(|m| m.has_focus(id));
        // ⌘⏎ commits and Tab goes to the list, before the field sees them.
        let (commit, tab) = if had_focus {
            ui.input_mut(|i| (i.consume_key(Modifiers::COMMAND, Key::Enter), i.consume_key(Modifiers::NONE, Key::Tab)))
        } else {
            (false, false)
        };
        let colors = look.colors;
        let text_h = ui.ctx().fonts_mut(|f| f.row_height(&look.font));
        let pad = ((look.row_h - text_h) / 2.0).floor() as i8;
        let avail = ui.available_rect_before_wrap();
        let outer = Rect::from_min_max(avail.min + Vec2::new(MARGIN, 0.0), Pos2::new(avail.max.x - MARGIN, avail.max.y));
        let hint = match &repo.branch {
            Some(b) => format!("Message (⌘⏎ to commit on \"{b}\")"),
            None => "Message (⌘⏎ to commit)".to_owned(),
        };
        let mut resp = None;
        let used = ui
            .scope_builder(egui::UiBuilder::new().max_rect(outer), |ui| {
                style::text_field(ui, &look.chrome, id, Margin::symmetric(style::GAP as i8, pad), |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("eshvi-scm-message-scroll")
                        .max_height(text_h * MESSAGE_ROWS)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let edit = egui::TextEdit::multiline(&mut self.message)
                                .id(id)
                                .font(look.font.clone())
                                .text_color(colors.fg)
                                .desired_rows(1)
                                .desired_width(f32::INFINITY)
                                .lock_focus(true)
                                .frame(egui::Frame::NONE);
                            let field = ui.add(edit);
                            // The hint is cut short, not wrapped (a narrow
                            // panel would double the box).
                            if self.message.is_empty() {
                                let g = look.fit(ui, &hint, &look.font, colors.dim, field.rect.width(), false);
                                ui.painter().galley(field.rect.min, g, colors.dim);
                            }
                            resp = Some(field);
                        });
                })
                .1
            })
            .inner;
        self.message_rect = Some(used);
        let Some(resp) = resp else { return };
        if std::mem::take(&mut self.message_focus) {
            resp.request_focus();
        }
        if resp.gained_focus() {
            // Focus by a click only: Tab is Neovim's (completion), but egui
            // gives it to the first field when none has focus.
            if ui.input(|i| i.key_pressed(Key::Tab)) {
                resp.surrender_focus();
            } else {
                actions.push(Action::Focus);
            }
        }
        // (Esc made egui drop the focus before this pass.)
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            actions.push(Action::FocusEditor);
        }
        if tab {
            resp.surrender_focus();
            actions.extend(self.focus_list());
        }
        if commit {
            actions.extend(self.commit(repo, false, false, After::Nothing));
        }
        if !self.message.trim().is_empty() {
            self.need_message = false;
        }
        if self.need_message {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::hover());
            ui.painter().text(
                Pos2::new(rect.min.x + MARGIN, rect.center().y),
                Align2::LEFT_CENTER,
                "Type a commit message",
                look.small.clone(),
                colors.dim,
            );
        }
    }

    /// A button in the accent, `more` with a "▾" part at its right end that
    /// opens a menu. Returns the button's and that part's responses.
    #[allow(clippy::too_many_arguments)]
    fn primary_button(
        &self,
        ui: &egui::Ui,
        look: &Look,
        rect: Rect,
        label: &str,
        icon: Option<Icon>,
        disabled: bool,
        more: bool,
    ) -> (egui::Response, Option<egui::Response>) {
        let more_w = if more { look.row_h } else { 0.0 };
        let main = Rect::from_min_max(rect.min, Pos2::new(rect.max.x - more_w, rect.max.y));
        let side = Rect::from_min_max(Pos2::new(main.max.x, rect.min.y), rect.max);
        let main_resp = ui.interact(main, Id::unique(("eshvi-scm-primary", label)), Sense::CLICK);
        let side_resp = more.then(|| ui.interact(side, Id::unique("eshvi-scm-primary-more"), Sense::CLICK));
        let p = ui.painter();
        let base = if disabled { look.chrome.shade(look.colors.bg, style::HOVER) } else { style::primary_fill(look.colors.accent, false) };
        let (text, text_color) = if disabled { (look.colors.dim, look.colors.dim) } else { (Color32::WHITE, Color32::WHITE) };
        p.rect_filled(rect, style::RADIUS, base);
        if !disabled {
            let lit = [(main_resp.hovered(), main), (side_resp.as_ref().is_some_and(|r| r.hovered()), side)];
            for (_, part) in lit.into_iter().filter(|(hovered, _)| *hovered) {
                p.with_clip_rect(part).rect_filled(rect, style::RADIUS, style::primary_fill(look.colors.accent, true));
            }
        }
        // The icon and the label, together in the middle.
        let size = style::icon_size(look.row_h);
        let gap = if icon.is_some() { size + style::GAP } else { 0.0 };
        let galley = look.fit(ui, label, &look.font, text_color, main.width() - gap - 2.0 * style::GAP, false);
        let left = main.center().x - (gap + galley.size().x) / 2.0;
        if let Some(icon) = icon {
            style::paint_icon(p, icon, Pos2::new(left + size / 2.0, main.center().y), size, text);
        }
        p.galley(Pos2::new(left + gap, main.center().y - galley.size().y / 2.0), galley, text_color);
        if more {
            let line = mix(base, Color32::WHITE, style::ON);
            p.vline(side.min.x, side.shrink2(Vec2::new(0.0, 5.0)).y_range(), Stroke::new(style::HAIRLINE, line));
            style::paint_icon(p, Icon::ChevronDown, side.center(), size, text);
        }
        (main_resp, side_resp)
    }

    /// The big button under the message box: Commit, Publish Branch, Sync
    /// Changes, or nothing to do.
    fn main_button_row(&mut self, ui: &mut egui::Ui, look: &Look, repo: &Repo, actions: &mut Vec<Action>) {
        let state = main_button(repo);
        let busy = self.running.first().copied();
        let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h + 4.0), Sense::hover());
        let rect = outer.shrink2(Vec2::new(MARGIN, 0.0));
        let (label, icon) = match (&state, busy) {
            (_, Some(running)) => (format!("{running}…"), None),
            (MainButton::Commit | MainButton::Idle, _) => ("Commit".to_owned(), Some(Icon::Check)),
            (MainButton::Publish, _) => ("Publish Branch".to_owned(), Some(Icon::Sync)),
            (MainButton::Sync { ahead, behind }, _) => {
                let counts = [(*behind > 0).then(|| format!("↓{behind}")), (*ahead > 0).then(|| format!("↑{ahead}"))];
                let counts = counts.into_iter().flatten().collect::<Vec<_>>().join(" ");
                (format!("Sync Changes {counts}"), Some(Icon::Sync))
            }
        };
        let disabled = busy.is_some() || state == MainButton::Idle;
        let more = state == MainButton::Commit && busy.is_none();
        let (main, side) = self.primary_button(ui, look, rect, &label, icon, disabled, more);
        if main.clicked() && !disabled {
            actions.extend(self.main_action(repo));
        }
        if let Some(side) = side {
            let popup = egui::Popup::menu(&side).align(egui::RectAlign::BOTTOM_END);
            popup.show(|ui| {
                for (label, amend, then) in [
                    ("Commit", false, After::Nothing),
                    ("Commit (Amend)", true, After::Nothing),
                    ("Commit & Push", false, After::Push),
                    ("Commit & Sync", false, After::Sync),
                ] {
                    if ui.button(label).clicked() {
                        actions.extend(self.commit(repo, amend, false, then));
                    }
                }
            });
        }
    }

    /// The sections of files and, below them, the graph.
    fn panes(&mut self, ui: &mut egui::Ui, look: &Look, repo: &Repo, focused: bool, actions: &mut Vec<Action>) {
        let rest = ui.available_height();
        let body = if self.graph_open { pane_body(rest, look.row_h, self.graph_height) } else { 0.0 };
        let lists = (rest - look.row_h - body).max(look.row_h * 2.0);
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        egui::ScrollArea::vertical().id_salt("eshvi-scm-list").max_height(lists).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for section in Section::ALL {
                let entries = section.entries(repo);
                if entries.is_empty() && section != Section::Changes {
                    continue;
                }
                self.section_header(ui, look, repo, section, actions);
                if self.collapsed.contains(&section) {
                    continue;
                }
                for e in entries {
                    self.file_row(ui, look, repo, section, e, focused, scroll_to, actions);
                }
            }
        });
        self.graph_pane(ui, look, repo, rest, body, focused, actions);
    }

    /// "Changes (3)": a click folds the section; hovered, its buttons.
    fn section_header(&mut self, ui: &mut egui::Ui, look: &Look, repo: &Repo, section: Section, actions: &mut Vec<Action>) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::CLICK);
        let hovered = ui.rect_contains_pointer(rect);
        let colors = look.colors;
        if hovered {
            ui.painter().rect_filled(rect, 0.0, colors.hover);
        }
        let open = !self.collapsed.contains(&section);
        let cy = rect.center().y;
        let x = rect.min.x + MARGIN;
        paint_chevron(ui.painter(), Pos2::new(x + 5.0, cy), open, colors.dim);
        // The count in a round badge.
        let entries = section.entries(repo).len();
        let count = entries.to_string();
        let badge_font = style::scaled(&look.font, style::BADGE);
        let g = ui.painter().layout_no_wrap(count, badge_font, colors.fg);
        let h = g.size().y + 2.0;
        let size = Vec2::new((g.size().x + style::GAP).max(h), h);
        let badge = Rect::from_min_size(Pos2::new(rect.max.x - MARGIN - size.x, cy - h / 2.0), size);
        let level = if hovered { style::ON } else { style::SUBTLE };
        ui.painter().rect_filled(badge, style::round(h), look.chrome.shade(colors.bg, level));
        ui.painter().galley(badge.center() - g.size() / 2.0, g, colors.fg);
        // Buttons in the room left of it (nothing to act on: none).
        let mut right = badge.min.x - style::GAP;
        let mut clicked = None;
        if hovered && entries > 0 {
            let side = style::button_size(look.row_h);
            for btn in Btn::header(section).iter().rev() {
                let r = Rect::from_center_size(Pos2::new(right - side / 2.0, cy), Vec2::splat(side));
                if self.button(ui, look, r, *btn, Id::unique(("eshvi-scm-section", section)), colors.hover) {
                    clicked = Some(*btn);
                }
                right = r.min.x - 2.0;
            }
        }
        let title = look.fit(ui, section.title(), &look.font, colors.fg, right - (x + 16.0) - style::GAP, false);
        ui.painter().galley(Pos2::new(x + 16.0, cy - title.size().y / 2.0), title, colors.fg);
        if let Some(btn) = clicked {
            self.header_button(btn, section, repo, actions);
        } else if resp.clicked() && !self.collapsed.remove(&section) {
            self.collapsed.insert(section);
        }
    }

    /// A header button's action.
    fn header_button(&mut self, btn: Btn, section: Section, repo: &Repo, actions: &mut Vec<Action>) {
        match (btn, section) {
            (Btn::StageAll, Section::Merge) => {
                let paths = repo.merge.iter().map(|e| e.path.clone()).collect();
                actions.push(Action::Op(Op::Stage(paths)));
            }
            (Btn::StageAll, _) => actions.push(Action::Op(Op::StageAll)),
            (Btn::UnstageAll, _) => actions.push(Action::Op(Op::UnstageAll)),
            (Btn::ViewAll, _) => actions.extend(view_all_request(repo, section).map(Action::DiffAll)),
            (Btn::DiscardAll, _) => {
                let all: Vec<RowKey> = repo.changes.iter().map(|e| RowKey { section, path: e.path.clone() }).collect();
                self.confirm = discard_of(repo, &all);
            }
            _ => {}
        }
    }

    /// A small button (an icon or a "+"/"−") in `rect`; true when clicked.
    fn button(&self, ui: &egui::Ui, look: &Look, rect: Rect, btn: Btn, id: Id, surface: Color32) -> bool {
        let resp = ui.interact(rect, id.with(btn), Sense::CLICK).on_hover_text(btn.tip());
        let (p, hovered) = (ui.painter(), resp.hovered());
        let glyph = match btn {
            Btn::Stage | Btn::Resolve | Btn::StageAll => Some(Glyph::Plus),
            Btn::Unstage | Btn::UnstageAll => Some(Glyph::Minus),
            _ => None,
        };
        match glyph {
            Some(glyph) => {
                style::glyph_button(p, glyph, rect.center(), look.row_h, &look.chrome, surface, hovered);
            }
            None => {
                style::paint_button_fill(p, rect, &look.chrome, surface, hovered, false);
                let icon = match btn {
                    Btn::OpenFile => Icon::OpenFile,
                    Btn::ViewAll => Icon::Files,
                    _ => Icon::Undo,
                };
                style::paint_icon(p, icon, rect.center(), style::icon_size(look.row_h), look.chrome.text(hovered));
            }
        }
        resp.clicked()
    }

    /// A file's icon, name, folder and status letter, and its buttons when
    /// `buttons` has any (the row is hovered or selected). Returns the
    /// button clicked.
    #[allow(clippy::too_many_arguments)]
    fn paint_file(
        &mut self,
        ui: &egui::Ui,
        look: &Look,
        rect: Rect,
        e: &Entry,
        id: Id,
        buttons: &[Btn],
        surface: Color32,
    ) -> Option<Btn> {
        let cy = rect.center().y;
        let color = look.deco.git(e.status);
        let mut right = rect.max.x - 12.0;
        let letter = ui.painter().text(Pos2::new(right, cy), Align2::RIGHT_CENTER, e.status.letter(), look.small.clone(), color);
        right = letter.min.x - style::GAP;
        let mut clicked = None;
        let side = style::button_size(look.row_h);
        for btn in buttons.iter().rev() {
            let r = Rect::from_center_size(Pos2::new(right - side / 2.0, cy), Vec2::splat(side));
            if self.button(ui, look, r, *btn, id, surface) {
                clicked = Some(*btn);
            }
            right = r.min.x - 2.0;
        }
        let indent = (self.font_size * INDENT_PER_PT).round();
        let icon_size = (self.font_size * ICON_PER_PT).round();
        let x = rect.min.x + MARGIN + indent;
        let (dir, name) = e.path.rsplit_once('/').map_or(("", e.path.as_str()), |(d, n)| (d, n));
        let icon = IconTheme::get().file_icon(name);
        let icon_rect = Rect::from_center_size(Pos2::new(x + 14.0 + icon_size / 2.0, cy), Vec2::splat(icon_size));
        let px = (icon_size * look.ppp).round() as u32;
        if let Some(tex) = self.textures.get(ui.ctx(), icon, px) {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(tex.id(), icon_rect, uv, Color32::WHITE);
        }
        let left = icon_rect.max.x + style::GAP;
        let end = right - style::GAP;
        // The name keeps the text color; the status shows only in the letter.
        let name = look.fit(ui, name, &look.font, look.colors.fg, end - left, e.status == GitStatus::Deleted);
        ui.painter().galley(Pos2::new(left, cy - name.size().y / 2.0), name.clone(), look.colors.fg);
        // The folder, dimmed, in what is left.
        let at = left + name.size().x + style::GAP;
        if !dir.is_empty() && end - at > look.small.size * 2.0 {
            let dir = look.fit(ui, dir, &look.small, look.colors.dim, end - at, false);
            ui.painter().galley(Pos2::new(at, cy - dir.size().y / 2.0), dir, look.colors.dim);
        }
        clicked
    }

    /// A changed file's row.
    #[allow(clippy::too_many_arguments)]
    fn file_row(
        &mut self,
        ui: &mut egui::Ui,
        look: &Look,
        repo: &Repo,
        section: Section,
        e: &Entry,
        focused: bool,
        scroll_to: bool,
        actions: &mut Vec<Action>,
    ) {
        let key = RowKey { section, path: e.path.clone() };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::CLICK);
        resp.context_menu(|ui| self.file_menu(ui, repo, &key, actions));
        if !ui.is_rect_visible(rect) {
            return;
        }
        let selected = self.is_selected(&key);
        let hovered = ui.rect_contains_pointer(rect);
        look.paint_row_bg(ui, rect, selected, focused, hovered);
        let buttons = if selected || hovered { Btn::row(section) } else { &[] };
        let surface = look.surface(selected, focused, hovered);
        let id = Id::unique(("eshvi-scm-file", section, &e.path));
        let clicked = self.paint_file(ui, look, rect, e, id, buttons, surface);
        if selected && scroll_to {
            resp.scroll_to_me(None);
        }
        match clicked {
            Some(Btn::OpenFile) => actions.push(Action::Open(repo.top.join(&e.path))),
            Some(Btn::Discard) => {
                let targets = self.targets_for(&key);
                self.confirm = discard_of(repo, &targets);
            }
            Some(Btn::Unstage) => {
                let targets = self.targets_for(&key);
                actions.extend(unstage_op(repo, &targets).map(Action::Op));
            }
            Some(_) => {
                let targets = self.targets_for(&key);
                actions.extend(stage_op(&targets).map(Action::Op));
            }
            None if resp.clicked() => {
                actions.push(Action::Focus);
                // Cmd-click adds or removes a row, Shift-click selects the
                // rows from the last one clicked.
                let mods = ui.input(|i| i.modifiers);
                if mods.shift {
                    let rows = self.rows();
                    let at = |k: &RowKey| rows.iter().position(|r| r == k);
                    let anchor = self.selected.as_ref().and_then(at).or(at(&key)).unwrap_or(0);
                    let (a, b) = (anchor.min(at(&key).unwrap_or(0)), anchor.max(at(&key).unwrap_or(0)));
                    self.multi = rows[a..=b].to_vec();
                } else if mods.command {
                    if self.multi.is_empty() {
                        self.multi.extend(self.selected.clone());
                    }
                    match self.multi.iter().position(|k| k == &key) {
                        Some(at) => {
                            self.multi.remove(at);
                        }
                        None => self.multi.push(key.clone()),
                    }
                    self.selected = Some(key);
                } else {
                    self.select(key.clone());
                    self.scroll_to_selected = false;
                    actions.extend(open_changes(repo, &key));
                }
            }
            None => {}
        }
    }

    /// A file row's right-click menu, for the selection it is in.
    fn file_menu(&mut self, ui: &mut egui::Ui, repo: &Repo, key: &RowKey, actions: &mut Vec<Action>) {
        let targets = self.targets_for(key);
        let many = targets.len() > 1;
        let abs = |k: &RowKey| repo.top.join(&k.path);
        if ui.button("Open Changes").clicked() {
            actions.extend(open_changes(repo, key));
        }
        if ui.button("Open File").clicked() {
            actions.push(Action::Open(abs(key)));
        }
        ui.separator();
        if key.section == Section::Staged {
            if ui.button("Unstage").clicked() {
                actions.extend(unstage_op(repo, &targets).map(Action::Op));
            }
        } else if ui.button("Stage").clicked() {
            actions.extend(stage_op(&targets).map(Action::Op));
        }
        if key.section == Section::Changes && ui.button("Discard Changes").clicked() {
            self.confirm = discard_of(repo, &targets);
        }
        ui.separator();
        let untracked: Vec<String> = targets
            .iter()
            .filter(|k| k.section == Section::Changes && find(repo, k).is_some_and(|e| e.status == GitStatus::Untracked))
            .map(|k| format!("/{}", k.path))
            .collect();
        if !untracked.is_empty() && ui.button("Add to .gitignore").clicked() {
            actions.push(Action::Op(Op::Ignore(untracked)));
        }
        if ui.button("Reveal in Finder").clicked() {
            actions.push(Action::Reveal(abs(key)));
        }
        if ui.button(if many { "Copy Paths" } else { "Copy Path" }).clicked() {
            let text: Vec<String> = targets.iter().map(|k| abs(k).to_string_lossy().into_owned()).collect();
            actions.push(Action::CopyText(text.join("\n")));
        }
        if ui.button(if many { "Copy Relative Paths" } else { "Copy Relative Path" }).clicked() {
            let text: Vec<String> = targets.iter().map(|k| k.path.clone()).collect();
            actions.push(Action::CopyText(text.join("\n")));
        }
        if self.agent {
            ui.separator();
            if ui.button("Send to Agent").clicked() {
                actions.extend(targets.iter().map(|k| Action::SendToAgent(abs(k))));
            }
        }
    }

    /// The commit graph, pinned at the panel's foot.
    fn graph_pane(&mut self, ui: &mut egui::Ui, look: &Look, repo: &Repo, rest: f32, body: f32, focused: bool, actions: &mut Vec<Action>) {
        let colors = look.colors;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::CLICK);
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect, 0.0, colors.hover);
        }
        p.hline(rect.x_range(), rect.min.y + 0.5, Stroke::new(style::HAIRLINE, look.chrome.line()));
        paint_chevron(p, Pos2::new(rect.min.x + MARGIN + 5.0, rect.center().y), self.graph_open, colors.dim);
        p.text(Pos2::new(rect.min.x + MARGIN + 16.0, rect.center().y), Align2::LEFT_CENTER, "Graph", look.font.clone(), colors.fg);
        if resp.clicked() {
            self.graph_open = !self.graph_open;
        }
        self.graph_buttons(ui, rect, look, repo, actions);
        if !self.graph_open {
            return;
        }
        // The band along its top edge resizes it (as the bottom panel's does);
        // a double click goes back to its share.
        let band = Rect::from_x_y_ranges(rect.x_range(), rect.min.y - 3.0..=rect.min.y + 4.0);
        let sash = ui.interact(band, ui.scope_id().with("eshvi-scm-graph-sash"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
        if sash.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            self.graph_height = Some(pane_body(rest, look.row_h, Some(body - sash.drag_delta().y)));
        }
        if sash.double_clicked() {
            self.graph_height = None;
        }
        // The one active selection: the graph's, or else the lists'.
        let focused = focused && self.selected.is_none();
        let details = self.details.clone();
        egui::ScrollArea::vertical().id_salt("eshvi-scm-graph").max_height(body).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            if details.commits.is_empty() {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::hover());
                let at = Pos2::new(rect.min.x + MARGIN, rect.center().y);
                ui.painter().text(at, Align2::LEFT_CENTER, "No commits yet", look.small.clone(), colors.dim);
                return;
            }
            let lanes = details
                .rows
                .iter()
                .flat_map(|r| r.top.iter().chain(&r.bottom).flat_map(|&(a, b)| [a, b]).chain([r.lane]))
                .max()
                .map_or(1, |m| m + 1)
                .min(MAX_LANES);
            for (commit, row) in details.commits.iter().zip(&details.rows) {
                self.commit_row(ui, look, commit, row, lanes, focused, actions);
                if self.expanded.contains(&commit.oid) {
                    for e in self.files.get(&commit.oid).cloned().unwrap_or_default() {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::CLICK);
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let hovered = ui.rect_contains_pointer(rect);
                        let selected = self.graph_selected.as_ref().is_some_and(|(oid, path)| *oid == commit.oid && path.as_ref() == Some(&e.path));
                        look.paint_row_bg(ui, rect, selected, focused, hovered);
                        // The lanes go on past the commit, and the row sits right of them.
                        let (lane_w, past) = (lane_width(look), row.bottom.iter().map(|&(_, b)| b));
                        for lane in past {
                            let x = lane_x(rect, lane, lane_w);
                            let line = [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)];
                            ui.painter().line_segment(line, Stroke::new(style::STROKE, lane_color(look, lane)));
                        }
                        let mut inner = rect;
                        inner.min.x += lanes as f32 * lane_w;
                        let id = Id::unique(("eshvi-scm-commit-file", &commit.oid, &e.path));
                        self.paint_file(ui, look, inner, &e, id, &[], look.surface(selected, focused, hovered));
                        if resp.clicked() {
                            self.select_in_graph(&commit.oid, Some(&e.path));
                            actions.push(Action::Diff(commit_diff(&repo.top, commit, &e)));
                        }
                    }
                }
            }
        });
    }

    /// Row `path` of commit `oid`, or the commit itself, becomes the selection.
    fn select_in_graph(&mut self, oid: &str, path: Option<&str>) {
        self.graph_selected = Some((oid.to_owned(), path.map(str::to_owned)));
        self.selected = None;
        self.multi.clear();
    }

    /// A commit: its lanes, subject and refs; a click shows its files.
    fn commit_row(
        &mut self,
        ui: &mut egui::Ui,
        look: &Look,
        commit: &Commit,
        row: &crate::git::GraphRow,
        lanes: usize,
        focused: bool,
        actions: &mut Vec<Action>,
    ) {
        let colors = look.colors;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), look.row_h), Sense::CLICK);
        if resp.clicked() {
            self.select_in_graph(&commit.oid, None);
            if self.expanded.remove(&commit.oid) {
                self.files.remove(&commit.oid);
            } else {
                self.expanded.insert(commit.oid.clone());
                if !self.files.contains_key(&commit.oid) {
                    actions.push(Action::CommitFiles(commit.clone()));
                }
            }
        }
        resp.context_menu(|ui| {
            if ui.button("Open Changes").clicked() {
                self.select_in_graph(&commit.oid, None);
                actions.push(Action::CommitDiff(commit.clone()));
            }
        });
        if resp.hovered() && !self.stats.contains_key(&commit.oid) {
            self.stats.insert(commit.oid.clone(), None);
            actions.push(Action::CommitStats(commit.clone()));
        }
        let stats = self.stats.get(&commit.oid).copied().flatten();
        let now = ui.input(|i| i.time);
        let copied = self.copied.as_ref().is_some_and(|(oid, at)| *oid == commit.oid && now - at < COPIED_FOR);
        // At the row's right, as in VS Code: it has a button to reach, and the
        // pointer can still go on to the rows below.
        let mut tooltip = egui::containers::Tooltip::for_enabled(&resp);
        tooltip.popup = tooltip.popup.align(RectAlign::RIGHT_START).align_alternatives(&RectAlign::MENU_ALIGNS);
        if let Some(shown) = tooltip.show(|ui| commit_tooltip(ui, look, commit, stats, copied)) {
            self.tooltip = Some(shown.response.rect);
            if shown.inner {
                self.copied = Some((commit.oid.clone(), now));
                actions.push(Action::CopyText(commit.oid.clone()));
            }
        }
        if copied {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(COPIED_FOR));
        }
        if !ui.is_rect_visible(rect) {
            return;
        }
        let selected = self.graph_selected.as_ref().is_some_and(|(oid, path)| *oid == commit.oid && path.is_none());
        look.paint_row_bg(ui, rect, selected, focused, resp.hovered());
        let p = ui.painter();
        let (cy, lane_w) = (rect.center().y, lane_width(look));
        let x_of = |lane: usize| lane_x(rect, lane, lane_w);
        for &(a, b) in &row.top {
            let line = [Pos2::new(x_of(a), rect.min.y), Pos2::new(x_of(b), cy)];
            p.line_segment(line, Stroke::new(style::STROKE, lane_color(look, a)));
        }
        for &(a, b) in &row.bottom {
            let line = [Pos2::new(x_of(a), cy), Pos2::new(x_of(b), rect.max.y)];
            p.line_segment(line, Stroke::new(style::STROKE, lane_color(look, b)));
        }
        let dot = Pos2::new(x_of(row.lane), cy);
        p.circle_filled(dot, lane_w / 3.0, lane_color(look, row.lane));
        let left = rect.min.x + MARGIN + lanes as f32 * lane_w + style::GAP;
        // Refs at the right (HEAD lit), as many as fit in a third of the row.
        let small = &look.small;
        let mut right = rect.max.x - MARGIN;
        let limit = rect.max.x - rect.width() * 0.4;
        for r in &commit.refs {
            let (name, head) = match r.strip_prefix("HEAD -> ") {
                Some(name) => (name, true),
                None => (r.as_str(), r == "HEAD"),
            };
            let g = p.layout_no_wrap(name.to_owned(), small.clone(), if head { colors.fg } else { look.chrome.text(false) });
            let chip = Rect::from_min_size(
                Pos2::new(right - g.size().x - style::GAP * 2.0, cy - (g.size().y + 2.0) / 2.0),
                Vec2::new(g.size().x + style::GAP * 2.0, g.size().y + 2.0),
            );
            if chip.min.x < limit.max(left + look.row_h) {
                break;
            }
            let level = if head { style::ON } else { style::SUBTLE };
            p.rect_filled(chip, style::round(chip.height()), look.chrome.shade(look.surface(selected, focused, false), level));
            p.galley(chip.center() - g.size() / 2.0, g, colors.fg);
            right = chip.min.x - 4.0;
        }
        let subject = commit.subject.lines().next().unwrap_or_default();
        let base = colors.fg.gamma_multiply(0.85);
        let g = look.fit(ui, subject, &look.font, base, right - left - style::GAP, false);
        ui.painter().galley(Pos2::new(left, cy - g.size().y / 2.0), g, base);
    }

    /// Questions: before discarding, before committing everything, and
    /// the name of a branch or a remote.
    fn dialogs(&mut self, ui: &mut egui::Ui, look: &Look, actions: &mut Vec<Action>) {
        if let Some(confirm) = self.confirm.clone() {
            let (title, notes, button, op) = match &confirm {
                Confirm::Discard { tracked, untracked } => {
                    let count = tracked.len() + untracked.len();
                    let what = match (&tracked[..], &untracked[..]) {
                        ([one], []) | ([], [one]) => format!("\"{}\"", one.rsplit('/').next().unwrap_or(one)),
                        _ => format!("{count} files"),
                    };
                    let mut notes = Vec::new();
                    if !tracked.is_empty() {
                        notes.push("This can't be undone.");
                    }
                    if !untracked.is_empty() {
                        notes.push("New files go to the Trash.");
                    }
                    let op = Op::Discard { tracked: tracked.clone(), untracked: untracked.clone() };
                    (format!("Discard changes in {what}?"), notes, "Discard Changes", op)
                }
                Confirm::StageAllCommit(op) => (
                    "There are no staged changes to commit.".to_owned(),
                    vec!["Stage all your changes and commit them directly?"],
                    "Stage All & Commit",
                    op.clone(),
                ),
            };
            let theme = look.theme;
            let modal = style::dialog(ui.ctx(), Id::unique("eshvi-scm-confirm"), theme, DIALOG_WIDTH, |ui| {
                style::dialog_title(ui, theme, &title);
                ui.add_space(4.0);
                for note in notes {
                    ui.label(egui::RichText::new(note).small());
                }
                ui.add_space(10.0);
                let mut choice = None;
                ui.horizontal(|ui| {
                    if style::dialog_button(ui, theme, button, true).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        choice = Some(true);
                    }
                    if style::dialog_button(ui, theme, "Cancel", false).clicked() {
                        choice = Some(false);
                    }
                });
                choice
            });
            match modal.inner {
                Some(true) => {
                    self.confirm = None;
                    self.multi.clear();
                    actions.push(Action::Op(op));
                }
                Some(false) => self.confirm = None,
                None if modal.should_close() => self.confirm = None,
                None => {}
            }
        }
        if self.prompt.is_some() {
            self.prompt_dialog(ui, look, actions);
        }
    }

    fn prompt_dialog(&mut self, ui: &mut egui::Ui, look: &Look, actions: &mut Vec<Action>) {
        let Some(prompt) = &mut self.prompt else { return };
        let kind = prompt.kind;
        let (title, hint, button) = match kind {
            PromptKind::Branch => ("Create Branch", "Branch name", "Create Branch"),
            PromptKind::Remote { .. } => ("Add Remote", "Remote name", "Add Remote"),
        };
        let mut accept = false;
        let mut cancel = false;
        let theme = look.theme;
        let text_h = ui.ctx().fonts_mut(|f| f.row_height(&look.font));
        let margin = Margin::symmetric(style::GAP as i8, ((look.row_h - text_h) / 2.0).floor() as i8);
        let modal = style::dialog(ui.ctx(), Id::unique("eshvi-scm-prompt-modal"), theme, DIALOG_WIDTH, |ui| {
            style::dialog_title(ui, theme, title);
            ui.add_space(6.0);
            let field = |ui: &mut egui::Ui, id: Id, text: &mut String, hint: &str| {
                style::text_field(ui, &look.chrome, id, margin, |ui| {
                    let edit = egui::TextEdit::singleline(text)
                        .id(id)
                        .font(look.font.clone())
                        .text_color(theme.fg)
                        .hint_text(egui::RichText::new(hint).color(theme.dim_fg))
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE);
                    ui.add(edit)
                })
                .0
            };
            let first = field(ui, prompt_id(), &mut prompt.text, hint);
            let last = if matches!(kind, PromptKind::Remote { .. }) {
                ui.add_space(4.0);
                let url = field(ui, prompt_url_id(), &mut prompt.url, "Remote URL");
                if std::mem::take(&mut prompt.focus) {
                    // The name is filled in when it's the first remote.
                    if prompt.text.is_empty() { first.request_focus() } else { url.request_focus() }
                }
                if first.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    url.request_focus();
                }
                url
            } else {
                if std::mem::take(&mut prompt.focus) {
                    first.request_focus();
                }
                first
            };
            if last.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                accept = true;
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if style::dialog_button(ui, theme, button, true).clicked() {
                    accept = true;
                }
                if style::dialog_button(ui, theme, "Cancel", false).clicked() {
                    cancel = true;
                }
            });
        });
        if accept {
            actions.extend(self.accept_prompt());
        } else if cancel || modal.should_close() {
            self.prompt = None;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
