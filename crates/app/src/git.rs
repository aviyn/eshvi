//! Git for the explorer's decorations and the Source Control panel: `git
//! status`, the branches and the commit graph on one background thread, and
//! everything that changes the repository on another (a push must not hold
//! up the status).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::decorations::GitStatus;

/// A changed file, relative to the repository.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    /// Where a renamed file came from.
    pub orig: Option<String>,
    pub status: GitStatus,
}

/// What's being finished, when it's not a plain commit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Operation {
    #[default]
    None,
    Merge,
    Rebase,
    CherryPick,
}

/// The repository as `git status` sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Repo {
    pub top: PathBuf,
    /// `None`: detached.
    pub branch: Option<String>,
    /// `None`: no commit yet.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Conflicts, then what `git commit` would take, then the rest.
    pub merge: Vec<Entry>,
    pub staged: Vec<Entry>,
    pub changes: Vec<Entry>,
    pub operation: Operation,
    /// `MERGE_MSG` without its comments, while finishing a merge.
    pub merge_message: Option<String>,
}

impl Repo {
    /// The badge's number, as VS Code counts: the groups added up, so a file
    /// both staged and changed counts twice.
    pub fn change_count(&self) -> usize {
        self.merge.len() + self.staged.len() + self.changes.len()
    }

    /// Status by absolute path, for the explorer and tabs: a file both
    /// staged and changed shows the more telling of the two.
    pub fn decorations(&self) -> HashMap<PathBuf, GitStatus> {
        let mut files = HashMap::new();
        for e in self.staged.iter().chain(&self.changes) {
            let status = match files.get(&self.top.join(&e.path)) {
                Some(GitStatus::Added) if e.status != GitStatus::Deleted => GitStatus::Added,
                Some(&GitStatus::Deleted) => GitStatus::Deleted,
                Some(&GitStatus::Renamed) if e.status != GitStatus::Deleted => GitStatus::Renamed,
                _ => e.status,
            };
            files.insert(self.top.join(&e.path), status);
        }
        for e in &self.merge {
            files.insert(self.top.join(&e.path), GitStatus::Conflict);
        }
        files
    }
}

fn entry_status(code: u8) -> Option<GitStatus> {
    match code {
        b'M' | b'T' => Some(GitStatus::Modified),
        b'A' | b'C' => Some(GitStatus::Added),
        b'D' => Some(GitStatus::Deleted),
        b'R' => Some(GitStatus::Renamed),
        _ => None,
    }
}

/// Parse `git status --porcelain=v2 --branch -z`.
pub fn parse_status(top: &Path, out: &[u8]) -> Repo {
    let mut repo = Repo { top: top.to_path_buf(), ..Default::default() };
    let mut fields = out.split(|&b| b == 0).filter(|f| !f.is_empty()).map(String::from_utf8_lossy);
    while let Some(line) = fields.next() {
        let Some((kind, rest)) = line.split_once(' ') else { continue };
        let tail = |skip: usize| rest.splitn(skip + 1, ' ').last().unwrap_or_default().to_owned();
        match kind {
            "#" => {
                let Some((key, value)) = rest.split_once(' ') else { continue };
                match key {
                    "branch.oid" if value != "(initial)" => repo.head = Some(value.to_owned()),
                    "branch.head" if value != "(detached)" => repo.branch = Some(value.to_owned()),
                    "branch.upstream" => repo.upstream = Some(value.to_owned()),
                    "branch.ab" => {
                        let mut counts = value.split(' ').map(|n| n.trim_start_matches(['+', '-']).parse().unwrap_or(0));
                        repo.ahead = counts.next().unwrap_or(0);
                        repo.behind = counts.next().unwrap_or(0);
                    }
                    _ => {}
                }
            }
            "?" => repo.changes.push(Entry { path: rest.to_owned(), orig: None, status: GitStatus::Untracked }),
            "u" => repo.merge.push(Entry { path: tail(9), orig: None, status: GitStatus::Conflict }),
            "1" | "2" => {
                let xy = rest.as_bytes();
                if xy.len() < 2 {
                    continue;
                }
                let (path, orig) = if kind == "2" { (tail(8), fields.next().map(|o| o.into_owned())) } else { (tail(7), None) };
                if let Some(status) = entry_status(xy[0]) {
                    repo.staged.push(Entry { path: path.clone(), orig, status });
                }
                if let Some(status) = entry_status(xy[1]) {
                    repo.changes.push(Entry { path, orig: None, status });
                }
            }
            _ => {}
        }
    }
    for list in [&mut repo.merge, &mut repo.staged, &mut repo.changes] {
        list.sort_by(|a, b| a.path.cmp(&b.path));
    }
    repo
}

/// The repository containing `dir`, as it is now. `None` if there is none.
pub fn status(dir: &Path) -> Option<Repo> {
    let top = Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--show-toplevel", "--absolute-git-dir"]).output().ok()?;
    if !top.status.success() {
        return None;
    }
    let top = String::from_utf8_lossy(&top.stdout).into_owned();
    let mut lines = top.lines();
    let (top, git_dir) = (PathBuf::from(lines.next()?), PathBuf::from(lines.next()?));
    let out = Command::new("git")
        .arg("-C")
        .arg(&top)
        .args(["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mut repo = parse_status(&top, &out.stdout);
    repo.operation = if git_dir.join("MERGE_HEAD").exists() {
        Operation::Merge
    } else if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        Operation::Rebase
    } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
        Operation::CherryPick
    } else {
        Operation::None
    };
    if repo.operation == Operation::Merge {
        repo.merge_message = std::fs::read_to_string(git_dir.join("MERGE_MSG")).ok().map(|m| {
            let lines: Vec<&str> = m.lines().filter(|l| !l.starts_with('#')).collect();
            lines.join("\n").trim().to_owned()
        });
    }
    Some(repo)
}

/// A commit in the graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    /// "2 hours ago".
    pub when: String,
    /// "Nov 2, 2026, 8:30 AM".
    pub date: String,
    pub subject: String,
    /// The message after its subject.
    pub body: String,
    /// "HEAD -> master", "origin/master", "tag: v1".
    pub refs: Vec<String>,
}

/// How much a commit changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CommitStats {
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// How a commit's row is drawn: the lane its dot is in, and the lines through
/// the row, each from a lane at one edge to a lane at the middle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphRow {
    pub lane: usize,
    pub top: Vec<(usize, usize)>,
    pub bottom: Vec<(usize, usize)>,
}

/// The lanes for `commits`, newest first.
pub fn graph(commits: &[Commit]) -> Vec<GraphRow> {
    // Which commit each lane is waiting for.
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let free = |lanes: &mut Vec<Option<&str>>| match lanes.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    };
    for c in commits {
        let lane = match lanes.iter().position(|l| *l == Some(c.oid.as_str())) {
            Some(i) => i,
            None => free(&mut lanes),
        };
        let mut top = Vec::new();
        // Lanes waiting for some other commit run through this row.
        let mut through = Vec::new();
        for (i, waiting) in lanes.iter().enumerate() {
            match waiting {
                Some(oid) if *oid == c.oid => top.push((i, lane)),
                Some(_) => {
                    top.push((i, i));
                    through.push(i);
                }
                None => {}
            }
        }
        for slot in lanes.iter_mut() {
            if *slot == Some(c.oid.as_str()) {
                *slot = None;
            }
        }
        let mut bottom = Vec::new();
        for (n, parent) in c.parents.iter().enumerate() {
            let at = lanes.iter().position(|l| *l == Some(parent.as_str()));
            let at = at.unwrap_or_else(|| {
                let i = if n == 0 { lane } else { free(&mut lanes) };
                lanes[i] = Some(parent);
                i
            });
            bottom.push((lane, at));
        }
        // A line joining a lane doesn't end the lane's own line.
        for i in through {
            if !bottom.contains(&(i, i)) {
                bottom.push((i, i));
            }
        }
        rows.push(GraphRow { lane, top, bottom });
    }
    rows
}

#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    /// "master", or "origin/master" for a remote one.
    pub name: String,
    pub remote: bool,
    pub current: bool,
}

/// What the panel shows besides the status: asked for only while it's open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub commits: Vec<Commit>,
    pub rows: Vec<GraphRow>,
    pub branches: Vec<Branch>,
    pub remotes: Vec<String>,
}

const LOG_LIMIT: &str = "-n100";

fn parse_log(out: &[u8]) -> Vec<Commit> {
    let mut commits = Vec::new();
    for record in out.split(|&b| b == 0) {
        let record = String::from_utf8_lossy(record);
        let mut f = record.trim_start_matches('\n').split('\x1f');
        let (Some(oid), Some(parents), Some(author), Some(when), Some(date), Some(subject), Some(refs), Some(body)) =
            (f.next(), f.next(), f.next(), f.next(), f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        commits.push(Commit {
            oid: oid.to_owned(),
            parents: parents.split_whitespace().map(str::to_owned).collect(),
            author: author.to_owned(),
            when: when.to_owned(),
            date: date.to_owned(),
            subject: subject.to_owned(),
            body: body.trim().to_owned(),
            refs: refs.split(", ").filter(|r| !r.is_empty()).map(str::to_owned).collect(),
        });
    }
    commits
}

fn parse_branches(out: &str) -> Vec<Branch> {
    let mut branches = Vec::new();
    for line in out.lines() {
        let Some((head, name)) = line.split_once(' ') else { continue };
        let name = name.trim_start();
        if let Some(local) = name.strip_prefix("refs/heads/") {
            branches.push(Branch { name: local.to_owned(), remote: false, current: head == "*" });
        } else if let Some(remote) = name.strip_prefix("refs/remotes/") {
            if !remote.ends_with("/HEAD") {
                branches.push(Branch { name: remote.to_owned(), remote: true, current: false });
            }
        }
    }
    branches
}

/// The commits of HEAD and its upstream, the branches and the remotes.
pub fn details(top: &Path) -> Details {
    let text = |args: &[&str]| {
        let out = Command::new("git").arg("-C").arg(top).args(args).output().ok()?;
        out.status.success().then_some(out.stdout)
    };
    let format = "--format=%H%x1f%P%x1f%an%x1f%ar%x1f%ad%x1f%s%x1f%D%x1f%b";
    let date = "--date=format:%b %-d, %Y, %-I:%M %p";
    let commits = text(&["log", LOG_LIMIT, "-z", format, date, "HEAD", "@{upstream}"])
        .or_else(|| text(&["log", LOG_LIMIT, "-z", format, date, "HEAD"]))
        .map(|out| parse_log(&out))
        .unwrap_or_default();
    let branches = text(&["for-each-ref", "--format=%(HEAD) %(refname)", "refs/heads", "refs/remotes"])
        .map(|out| parse_branches(&String::from_utf8_lossy(&out)))
        .unwrap_or_default();
    let remotes = text(&["remote"])
        .map(|out| String::from_utf8_lossy(&out).lines().map(str::to_owned).collect())
        .unwrap_or_default();
    Details { rows: graph(&commits), commits, branches, remotes }
}

/// A commit against its first parent (a first commit: all of it), told `how`.
fn commit_diff(top: &Path, commit: &Commit, how: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(top);
    match commit.parents.first() {
        Some(parent) => cmd.args(["diff", "-M"]).args(how).args([parent, &commit.oid]),
        None => cmd.args(["diff-tree", "--root", "--no-commit-id", "-r", "-M"]).args(how).arg(&commit.oid),
    };
    cmd
}

/// `git diff --numstat`: a line for each file, "-" for a binary file's counts.
fn parse_numstat(out: &str) -> CommitStats {
    let mut stats = CommitStats::default();
    for line in out.lines() {
        let mut counts = line.split('\t').map(|n| n.parse().unwrap_or(0));
        stats.files += 1;
        stats.insertions += counts.next().unwrap_or(0);
        stats.deletions += counts.next().unwrap_or(0);
    }
    stats
}

/// How much a commit changed.
pub fn commit_stats(top: &Path, commit: &Commit) -> CommitStats {
    let Ok(out) = commit_diff(top, commit, &["--numstat"]).output() else { return CommitStats::default() };
    parse_numstat(&String::from_utf8_lossy(&out.stdout))
}

/// A commit's files.
pub fn commit_files(top: &Path, commit: &Commit) -> Vec<Entry> {
    let Ok(out) = commit_diff(top, commit, &["--name-status", "-z"]).output() else { return Vec::new() };
    let mut entries = Vec::new();
    let mut fields = out.stdout.split(|&b| b == 0).filter(|f| !f.is_empty()).map(|f| String::from_utf8_lossy(f).into_owned());
    while let (Some(code), Some(first)) = (fields.next(), fields.next()) {
        let status = entry_status(code.as_bytes()[0]).unwrap_or(GitStatus::Modified);
        if matches!(code.as_bytes()[0], b'R' | b'C') {
            entries.push(Entry { path: fields.next().unwrap_or_default(), orig: Some(first), status });
        } else {
            entries.push(Entry { path: first, orig: None, status });
        }
    }
    entries
}

/// What the panel asks git to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Init,
    Stage(Vec<String>),
    StageAll,
    /// `orig` paths of renames included.
    Unstage(Vec<String>),
    UnstageAll,
    /// Changes in tracked files undone; untracked files to the Trash.
    Discard { tracked: Vec<String>, untracked: Vec<String> },
    /// One change of a file, as a patch (`git apply`): to the index with
    /// `cached`, to the working tree without; `reverse` takes it out again
    /// (unstaging a change, discarding one).
    Apply { patch: String, cached: bool, reverse: bool },
    Commit { message: String, amend: bool, stage_all: bool, then: After },
    UndoCommit,
    Pull,
    Push,
    Fetch,
    Sync,
    Publish(String),
    Checkout(String),
    /// A remote branch, as a local one following it.
    Track(String),
    CreateBranch(String),
    Merge(String),
    Abort(Operation),
    Stash { untracked: bool },
    StashPop,
    AddRemote { name: String, url: String },
    Ignore(Vec<String>),
}

/// What a commit goes on to do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum After {
    #[default]
    Nothing,
    Push,
    Sync,
}

impl Op {
    /// While it runs ("Pushing…").
    pub fn label(&self) -> &'static str {
        match self {
            Op::Init => "Initializing",
            Op::Stage(_) | Op::StageAll => "Staging",
            Op::Unstage(_) | Op::UnstageAll => "Unstaging",
            Op::Discard { .. } => "Discarding",
            Op::Apply { cached: true, reverse: false, .. } => "Staging",
            Op::Apply { cached: true, reverse: true, .. } => "Unstaging",
            Op::Apply { cached: false, .. } => "Discarding",
            Op::Commit { .. } | Op::UndoCommit => "Committing",
            Op::Pull => "Pulling",
            Op::Push | Op::Publish(_) => "Pushing",
            Op::Fetch => "Fetching",
            Op::Sync => "Syncing",
            Op::Checkout(_) | Op::Track(_) | Op::CreateBranch(_) => "Switching branch",
            Op::Merge(_) => "Merging",
            Op::Abort(_) => "Aborting",
            Op::Stash { .. } | Op::StashPop => "Stashing",
            Op::AddRemote { .. } => "Adding remote",
            Op::Ignore(_) => "Ignoring",
        }
    }

    /// Whether files may have changed under Neovim's buffers.
    pub fn touches_files(&self) -> bool {
        !matches!(
            self,
            Op::Init | Op::Stage(_) | Op::StageAll | Op::Unstage(_) | Op::UnstageAll | Op::Apply { cached: true, .. } | Op::Commit { .. } | Op::Push | Op::Fetch | Op::Publish(_) | Op::AddRemote { .. } | Op::CreateBranch(_)
        )
    }
}

/// What to run once `op` has worked: a commit goes on to push or sync. They
/// are separate operations so that a push that fails leaves the commit made,
/// and only the push is tried again.
pub fn next(op: &Op) -> Option<Op> {
    match op {
        Op::Commit { then: After::Push, .. } => Some(Op::Push),
        Op::Commit { then: After::Sync, .. } => Some(Op::Sync),
        _ => None,
    }
}

/// The `.gitignore` line for one path of the repository, and nothing else:
/// from the root (a name alone would match in every folder), and with what
/// a pattern reads as more than itself escaped (`*`, `?`, `[`, `\`, a space
/// at the end; a `#` or `!` in front is plain after the `/`).
fn ignore_pattern(path: &str) -> String {
    let mut out = String::from("/");
    for c in path.trim_start_matches('/').chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    if out.ends_with(' ') {
        out.insert(out.len() - 1, '\\');
    }
    out
}

/// One git command an operation ran, with all it said: what the Git output shows.
#[derive(Clone, Debug)]
pub struct Ran {
    pub args: Vec<String>,
    /// Where it ran (not shown: the Git output is of the one repository Source Control has).
    #[allow(dead_code)]
    pub dir: PathBuf,
    pub when: chrono::DateTime<chrono::Local>,
    pub took: Duration,
    /// `None`: git ended by a signal (or didn't start).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    /// What the Git output shows for it, as a terminal reads it (colors by
    /// escape sequences, so the theme's ANSI colors apply): the time, the
    /// command in bold (with how long it took when that was over a second),
    /// its output as it came, a red `exit N` when it failed, and a blank line.
    /// Lines end with "\r\n".
    pub fn transcript(&self) -> String {
        const DIM: &str = "\x1b[2m";
        const BOLD: &str = "\x1b[1m";
        const RED: &str = "\x1b[31m";
        const OFF: &str = "\x1b[0m";
        let mut out = format!("{DIM}[{}]{OFF} {BOLD}git", self.when.format("%H:%M:%S"));
        for arg in &self.args {
            out.push(' ');
            out.push_str(&quote_arg(arg));
        }
        out.push_str(OFF);
        if self.took >= Duration::from_secs(1) {
            out.push_str(&format!(" {DIM}({:.1}s){OFF}", self.took.as_secs_f32()));
        }
        out.push_str("\r\n");
        for text in [&self.stdout, &self.stderr] {
            // (Git ends its lines with "\n": a terminal wants "\r\n".)
            for line in text.lines() {
                out.push_str(line);
                out.push_str("\r\n");
            }
        }
        match self.code {
            Some(0) => {}
            Some(code) => out.push_str(&format!("{RED}exit {code}{OFF}\r\n")),
            None => out.push_str(&format!("{RED}ended by a signal{OFF}\r\n")),
        }
        out.push_str("\r\n");
        out
    }
}

/// An argument as a shell would need it written to be one word, on one line.
fn quote_arg(arg: &str) -> String {
    let plain = !arg.is_empty() && arg.chars().all(|c| c.is_alphanumeric() || "-_./=:@%+,~^".contains(c));
    if plain {
        return arg.to_owned();
    }
    // (A new line in a commit message would break the line: it is shown as `\n`.)
    let escaped = arg.replace('\'', r"'\''").replace("\r\n", "\\n").replace('\n', "\\n");
    format!("'{escaped}'")
}

thread_local! {
    /// While `Some`, what `run_git` does on this thread is put here.
    static RECORD: std::cell::RefCell<Option<Vec<Ran>>> = const { std::cell::RefCell::new(None) };
}

/// Do `f`, and with it what git commands it ran on this thread (and their output).
fn recording<T>(f: impl FnOnce() -> T) -> (T, Vec<Ran>) {
    RECORD.with(|r| *r.borrow_mut() = Some(Vec::new()));
    let result = f();
    (result, RECORD.with(|r| r.borrow_mut().take()).unwrap_or_default())
}

/// Do `f` without recording what it runs.
fn unrecorded<T>(f: impl FnOnce() -> T) -> T {
    let kept = RECORD.with(|r| r.borrow_mut().take());
    let result = f();
    RECORD.with(|r| *r.borrow_mut() = kept);
    result
}

fn record(ran: impl FnOnce() -> Ran) {
    RECORD.with(|r| {
        if let Some(list) = r.borrow_mut().as_mut() {
            list.push(ran());
        }
    });
}

/// `git` in `dir`, never waiting for an answer: no terminal to ask in, an
/// editor that accepts what it's given. `Err`: what git said.
fn run_git(dir: &Path, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    // (Only where there are paths: with none, `stash -u` leaves untracked files.)
    if args.contains(&"--") {
        cmd.arg("--literal-pathspecs");
    }
    cmd.arg("-C").arg(dir).args(args);
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_EDITOR", "true").env("GIT_MERGE_AUTOEDIT", "no");
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let (when, started) = (chrono::Local::now(), Instant::now());
    let mut child = cmd.spawn().map_err(|e| format!("git doesn't run: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    record(|| Ran {
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        dir: dir.to_path_buf(),
        when,
        took: started.elapsed(),
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).trim_end().to_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).trim_end().to_owned(),
    });
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if out.status.success() {
        return Ok(stdout);
    }
    // (A conflict is told on stdout, the rest on stderr.)
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    let why = [stdout, stderr].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
    Err(if why.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { why })
}

/// A pull that doesn't stop to ask how to reconcile: the user's own settings
/// if there are any, else a merge (as VS Code's Sync).
fn pull(dir: &Path) -> Result<(), String> {
    // (Not recorded: `config --get` ends with 1 for a setting that isn't set.)
    let set = |key: &str| unrecorded(|| run_git(dir, &["config", "--get", key], None).is_ok());
    let mut args = vec!["pull", "--no-edit"];
    if !set("pull.rebase") && !set("pull.ff") {
        args.push("--no-rebase");
    }
    run_git(dir, &args, None).map(|_| ())
}

/// The full message of the last commit.
fn last_message(dir: &Path) -> Option<String> {
    let out = run_git(dir, &["log", "-1", "--format=%B"], None).ok()?;
    let message = out.trim_end();
    (!message.is_empty()).then(|| message.to_owned())
}

fn push(dir: &Path) -> Result<(), String> {
    run_git(dir, &["push"], None).map(|_| ())
}

fn with_paths<'a>(args: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    let mut all = args.to_vec();
    all.push("--");
    all.extend(paths.iter().map(String::as_str));
    all
}

/// Do `op` in the repository (or, for `Init`, the folder) `dir`.
pub fn run(dir: &Path, op: &Op, trash: &dyn Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    let git = |args: &[&str]| run_git(dir, args, None).map(|_| ());
    match op {
        Op::Init => git(&["init"]),
        Op::Stage(paths) => git(&with_paths(&["add", "-A"], paths)),
        Op::StageAll => git(&["add", "-A"]),
        Op::Unstage(paths) => git(&with_paths(&["reset", "-q"], paths)),
        Op::UnstageAll => git(&["reset", "-q"]),
        Op::Discard { tracked, untracked } => {
            if !tracked.is_empty() {
                git(&with_paths(&["checkout"], tracked))?;
            }
            for path in untracked {
                trash(&dir.join(path))?;
            }
            Ok(())
        }
        Op::Apply { patch, cached, reverse } => {
            let mut args = vec!["apply", "--whitespace=nowarn"];
            if *cached {
                args.push("--cached");
            }
            if *reverse {
                args.push("-R");
            }
            run_git(dir, &args, Some(patch)).map(|_| ())
        }
        Op::Commit { message, amend, stage_all, .. } => {
            if *stage_all {
                git(&["add", "-A"])?;
            }
            let mut args = vec!["commit", "--quiet"];
            if *amend {
                args.push("--amend");
            }
            if *amend && message.trim().is_empty() {
                args.push("--no-edit");
                git(&args)
            } else {
                args.extend(["-F", "-"]);
                run_git(dir, &args, Some(message)).map(|_| ())
            }
        }
        Op::UndoCommit => git(&["reset", "--soft", "-q", "HEAD~1"]),
        Op::Pull => pull(dir),
        Op::Push => push(dir),
        Op::Fetch => git(&["fetch", "--all", "--prune"]),
        Op::Sync => pull(dir).and_then(|()| push(dir)),
        Op::Publish(remote) => git(&["push", "-u", remote, "HEAD"]),
        Op::Checkout(branch) => git(&["checkout", branch]),
        Op::Track(remote) => git(&["checkout", "--track", remote]),
        Op::CreateBranch(name) => git(&["checkout", "-b", name]),
        Op::Merge(branch) => git(&["merge", "--no-edit", branch]),
        Op::Abort(Operation::Rebase) => git(&["rebase", "--abort"]),
        Op::Abort(Operation::CherryPick) => git(&["cherry-pick", "--abort"]),
        Op::Abort(_) => git(&["merge", "--abort"]),
        Op::Stash { untracked: true } => git(&["stash", "push", "--include-untracked"]),
        Op::Stash { untracked: false } => git(&["stash", "push"]),
        Op::StashPop => git(&["stash", "pop"]),
        Op::AddRemote { name, url } => git(&["remote", "add", name, url]),
        Op::Ignore(paths) => {
            let file = dir.join(".gitignore");
            let mut text = std::fs::read_to_string(&file).unwrap_or_default();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            for path in paths {
                text.push_str(&ignore_pattern(path));
                text.push('\n');
            }
            std::fs::write(&file, text).map_err(|e| format!("Could not write .gitignore: {e}"))
        }
    }
}

/// Results of the status thread.
pub enum StatusResult {
    /// `None`: not in a repository. `details`: when asked for and changed.
    Status(Option<Repo>, Option<Details>),
}

struct StatusRequest {
    dir: PathBuf,
    /// The panel is open: keep the graph and branches current.
    details: bool,
    /// Read them even if HEAD and its upstream didn't move.
    reload: bool,
}

/// Runs `git status` off the UI thread when asked, at most every
/// `MIN_INTERVAL`, and delivers results through `results`.
pub struct GitStatusWorker {
    requests: mpsc::Sender<StatusRequest>,
    pub results: mpsc::Receiver<StatusResult>,
    last_request: Option<Instant>,
}

const MIN_INTERVAL: Duration = Duration::from_millis(500);

impl GitStatusWorker {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<StatusRequest>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("git-status".into())
            .spawn(move || {
                // What the details were last read for.
                let mut read_for: Option<(PathBuf, Option<String>, u32, u32)> = None;
                while let Ok(mut req) = req_rx.recv() {
                    // Coalesce queued requests.
                    while let Ok(next) = req_rx.try_recv() {
                        let (details, reload) = (req.details || next.details, req.reload || next.reload);
                        req = StatusRequest { details, reload, ..next };
                    }
                    let repo = status(&req.dir);
                    let details = repo.as_ref().filter(|_| req.details).and_then(|r| {
                        let key = (r.top.clone(), r.head.clone(), r.ahead, r.behind);
                        if !req.reload && read_for.as_ref() == Some(&key) {
                            return None;
                        }
                        read_for = Some(key);
                        Some(details(&r.top))
                    });
                    if res_tx.send(StatusResult::Status(repo, details)).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .expect("git status thread");
        Self { requests: req_tx, results: res_rx, last_request: None }
    }

    /// Ask for a refresh of the repository containing `dir`. Requests closer
    /// than `MIN_INTERVAL` are dropped unless `force`, which also reads the
    /// details again when they're wanted.
    pub fn refresh(&mut self, dir: &Path, force: bool, details: bool) {
        if !force && self.last_request.is_some_and(|t| t.elapsed() < MIN_INTERVAL) {
            return;
        }
        self.last_request = Some(Instant::now());
        let _ = self.requests.send(StatusRequest { dir: dir.to_path_buf(), details, reload: force });
    }

    /// Time since the last request, for periodic refreshes.
    pub fn since_last(&self) -> Option<Duration> {
        self.last_request.map(|t| t.elapsed())
    }
}

/// What the operations thread is asked.
enum OpRequest {
    Run(PathBuf, Op),
    Files(PathBuf, Commit),
    Stats(PathBuf, Commit),
}

/// What the operations thread answers.
pub enum OpDone {
    /// `op` finished: `Err` is what git said; then the git commands it ran.
    Ran(Op, Result<(), String>, Vec<Ran>),
    /// After `Op::UndoCommit` worked: the message of the commit it undid.
    Undone(String),
    Files(String, Vec<Entry>),
    Stats(String, CommitStats),
}

/// Runs what changes the repository, one thing at a time, in the order asked.
pub struct GitOpsWorker {
    requests: mpsc::Sender<OpRequest>,
    results: mpsc::Receiver<OpDone>,
    /// Operations asked for and not finished, oldest first.
    pub running: Vec<&'static str>,
}

impl GitOpsWorker {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<OpRequest>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("git-ops".into())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    let mut done = Vec::new();
                    match req {
                        OpRequest::Run(dir, op) => {
                            // (Read before the reset: it's what the box gets back.)
                            let undone = (op == Op::UndoCommit).then(|| last_message(&dir));
                            let (result, ran) = recording(|| run(&dir, &op, &crate::trash_can::move_to_trash));
                            let ok = result.is_ok();
                            done.push(OpDone::Ran(op, result, ran));
                            if let (true, Some(Some(message))) = (ok, undone) {
                                done.push(OpDone::Undone(message));
                            }
                        }
                        OpRequest::Files(top, commit) => done.push(OpDone::Files(commit.oid.clone(), commit_files(&top, &commit))),
                        OpRequest::Stats(top, commit) => done.push(OpDone::Stats(commit.oid.clone(), commit_stats(&top, &commit))),
                    }
                    if done.into_iter().any(|d| res_tx.send(d).is_err()) {
                        break;
                    }
                    wake();
                }
            })
            .expect("git operations thread");
        Self { requests: req_tx, results: res_rx, running: Vec::new() }
    }

    pub fn run(&mut self, dir: &Path, op: Op) {
        self.running.push(op.label());
        let _ = self.requests.send(OpRequest::Run(dir.to_path_buf(), op));
    }

    pub fn files(&self, top: &Path, commit: Commit) {
        let _ = self.requests.send(OpRequest::Files(top.to_path_buf(), commit));
    }

    pub fn stats(&self, top: &Path, commit: Commit) {
        let _ = self.requests.send(OpRequest::Stats(top.to_path_buf(), commit));
    }

    pub fn poll(&mut self) -> Vec<OpDone> {
        let done: Vec<OpDone> = self.results.try_iter().collect();
        for d in &done {
            if matches!(d, OpDone::Ran(..)) && !self.running.is_empty() {
                self.running.remove(0);
            }
        }
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_operation_records_its_git_commands_with_their_output() {
        let f = Fixture::new("record");
        std::fs::write(f.me.join("rec.txt"), "one\n").unwrap();
        let (result, ran) = recording(|| run(&f.me, &Op::StageAll, &no_trash));
        assert!(result.is_ok());
        assert_eq!(ran.len(), 1);
        assert_eq!(ran[0].args, ["add", "-A"]);
        assert_eq!((ran[0].dir.as_path(), ran[0].code), (f.me.as_path(), Some(0)));
        // A commit is quiet (and its message goes in on stdin, which isn't recorded).
        let (result, ran) = recording(|| run(&f.me, &commit_op("Record it"), &no_trash));
        assert!(result.is_ok());
        let commit = ran.iter().find(|r| r.args.first().map(String::as_str) == Some("commit")).expect("the commit");
        assert!(commit.args.contains(&"--quiet".to_owned()) && commit.code == Some(0));
        // A push to the remote tells what it did, on stderr.
        let (result, ran) = recording(|| run(&f.me, &Op::Push, &no_trash));
        assert!(result.is_ok());
        let push = ran.iter().find(|r| r.args.first().map(String::as_str) == Some("push")).expect("the push");
        assert!(push.stderr.contains("main -> main"), "{}", push.stderr);
        // (Nothing is kept once taken.)
        assert!(RECORD.with(|r| r.borrow().is_none()));
    }

    #[test]
    fn a_failing_operation_records_what_git_said_and_its_exit_code() {
        let f = Fixture::new("record-fail");
        let (result, ran) = recording(|| run(&f.me, &commit_op("nothing to commit"), &no_trash));
        assert!(result.is_err());
        let commit = ran.last().expect("the commit");
        assert_eq!(commit.args.first().map(String::as_str), Some("commit"));
        assert_eq!(commit.code, Some(1));
        assert!(commit.stdout.contains("nothing to commit") || commit.stderr.contains("nothing to commit"));
    }

    #[test]
    fn reading_the_repository_records_nothing() {
        let f = Fixture::new("record-read");
        let ((repo, details), ran) = recording(|| (status(&f.me), details(&f.me)));
        assert!(repo.is_some() && !details.commits.is_empty());
        assert!(ran.is_empty());
    }

    fn ran(args: &[&str], code: Option<i32>, took_ms: u64, stdout: &str, stderr: &str) -> Ran {
        Ran {
            args: args.iter().map(|a| a.to_string()).collect(),
            dir: PathBuf::from("/repo"),
            when: chrono::Local::now(),
            took: Duration::from_millis(took_ms),
            code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn a_command_is_written_with_its_output_and_how_it_ended() {
        let t = ran(&["push", "origin", "main"], Some(0), 200, "", "remote: Create a pull request\nTo origin\n   a..b  main -> main").transcript();
        // The time (dim), the command (bold), the output as it was, a blank line.
        assert!(t.starts_with("\x1b[2m[") && t.contains("]\x1b[0m \x1b[1mgit push origin main\x1b[0m\r\nremote: Create"), "{t:?}");
        assert!(t.ends_with("main -> main\r\n\r\n"), "{t:?}");
        assert!(!t.contains("exit") && !t.contains("(0."));
        // A failure ends in a red exit line; a slow one says how long it took.
        let t = ran(&["commit", "-m", "two words"], Some(1), 2300, "", "hook said no").transcript();
        assert!(t.contains("git commit -m 'two words'\x1b[0m \x1b[2m(2.3s)\x1b[0m\r\nhook said no\r\n\x1b[31mexit 1\x1b[0m\r\n\r\n"), "{t:?}");
        // Both streams are shown, stdout first; a signal is said as such.
        let t = ran(&["pull"], None, 5, "out", "err").transcript();
        assert!(t.contains("\r\nout\r\nerr\r\n\x1b[31mended by a signal"), "{t:?}");
    }

    #[test]
    fn arguments_with_spaces_or_new_lines_are_quoted_on_one_line() {
        assert_eq!(quote_arg("main"), "main");
        assert_eq!(quote_arg("--format=%B"), "--format=%B");
        assert_eq!(quote_arg("two words"), "'two words'");
        assert_eq!(quote_arg("it's"), r"'it'\''s'");
        assert_eq!(quote_arg("title\n\nbody"), "'title\\n\\nbody'");
        assert_eq!(quote_arg(""), "''");
    }

    #[test]
    fn a_pull_does_not_record_its_look_at_the_settings() {
        let f = Fixture::new("record-pull");
        let (result, ran) = recording(|| run(&f.me, &Op::Pull, &no_trash));
        assert!(result.is_ok(), "{result:?}");
        // (`config --get` ends with 1 when a setting isn't set: not a failure to show.)
        assert!(ran.iter().all(|r| r.args.first().map(String::as_str) != Some("config")), "{ran:?}");
        assert!(ran.iter().any(|r| r.args.first().map(String::as_str) == Some("pull")));
    }

    #[test]
    fn numstat_counts_files_and_lines() {
        assert_eq!(parse_numstat(""), CommitStats::default());
        let out = "10\t2\tsrc/a.rs\n0\t3\tb.txt\n-\t-\tlogo.png\n1\t1\t{old => new}.rs\n";
        assert_eq!(parse_numstat(out), CommitStats { files: 4, insertions: 11, deletions: 6 });
    }

    #[test]
    fn a_commit_in_the_graph_has_its_body_date_and_stats() {
        let f = Fixture::new("stats");
        std::fs::write(f.me.join("stats.txt"), "one\ntwo\n").unwrap();
        sh(&f.me, &["add", "stats.txt"]);
        sh(&f.me, &["commit", "-m", "Add stats", "-m", "First paragraph.\n\nSecond paragraph."]);
        std::fs::write(f.me.join("stats.txt"), "one\nthree\nfour\n").unwrap();
        sh(&f.me, &["commit", "-am", "Change stats"]);
        let d = details(&f.me);
        // (Commits of the same second come in any order.)
        let by_subject = |subject: &str| d.commits.iter().find(|c| c.subject == subject).unwrap();
        let (second, first) = (by_subject("Change stats"), by_subject("Add stats"));
        assert_eq!((second.subject.as_str(), second.body.as_str()), ("Change stats", ""));
        assert_eq!((first.subject.as_str(), first.body.as_str()), ("Add stats", "First paragraph.\n\nSecond paragraph."));
        assert!(first.date.contains(", 20"), "{}", first.date);
        assert_eq!(commit_stats(&f.me, second), CommitStats { files: 1, insertions: 2, deletions: 1 });
        assert_eq!(commit_stats(&f.me, first), CommitStats { files: 1, insertions: 2, deletions: 0 });
        // A first commit has no parent to compare with.
        let root = by_subject("first");
        assert!(root.parents.is_empty());
        assert!(commit_stats(&f.me, root).files > 0);
    }

    fn entry(path: &str, status: GitStatus) -> Entry {
        Entry { path: path.into(), orig: None, status }
    }

    #[test]
    fn parses_status_v2() {
        let top = Path::new("/repo");
        let out = b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 a a src/a b.rs\0\
1 MM N... 100644 100644 100644 a b both.rs\0\
1 A. N... 000000 100644 100644 0 b added.rs\0\
1 .D N... 100644 100644 000000 a a gone.rs\0\
2 R. N... 100644 100644 100644 a a R100 new name.rs\0old name.rs\0\
u UU N... 100644 100644 100644 100644 a b c fight.rs\0\
? new.txt\0! ignored.txt\0";
        let r = parse_status(top, out);
        assert_eq!((r.head.as_deref(), r.branch.as_deref(), r.upstream.as_deref()), (Some("abc"), Some("main"), Some("origin/main")));
        assert_eq!((r.ahead, r.behind), (2, 1));
        assert_eq!(r.merge, [entry("fight.rs", GitStatus::Conflict)]);
        assert_eq!(
            r.staged,
            [
                entry("added.rs", GitStatus::Added),
                entry("both.rs", GitStatus::Modified),
                Entry { path: "new name.rs".into(), orig: Some("old name.rs".into()), status: GitStatus::Renamed },
            ]
        );
        assert_eq!(
            r.changes,
            [
                entry("both.rs", GitStatus::Modified),
                entry("gone.rs", GitStatus::Deleted),
                entry("new.txt", GitStatus::Untracked),
                entry("src/a b.rs", GitStatus::Modified),
            ]
        );
        // (both.rs is in two groups and counts in both, as in VS Code.)
        assert_eq!(r.change_count(), 8);
        let m = r.decorations();
        assert_eq!(m[Path::new("/repo/src/a b.rs")], GitStatus::Modified);
        assert_eq!(m[Path::new("/repo/new.txt")], GitStatus::Untracked);
        assert_eq!(m[Path::new("/repo/added.rs")], GitStatus::Added);
        assert_eq!(m[Path::new("/repo/new name.rs")], GitStatus::Renamed);
        assert!(!m.contains_key(Path::new("/repo/old name.rs")));
        assert_eq!(m[Path::new("/repo/gone.rs")], GitStatus::Deleted);
        assert_eq!(m[Path::new("/repo/fight.rs")], GitStatus::Conflict);
        // No commit yet, detached.
        let r = parse_status(top, b"# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!((r.head, r.branch), (None, None));
    }

    #[test]
    fn a_file_added_then_changed_still_shows_as_added() {
        let r = parse_status(Path::new("/r"), b"1 AM N... 000000 100644 100644 0 b new.rs\0");
        assert_eq!(r.decorations()[Path::new("/r/new.rs")], GitStatus::Added);
        let r = parse_status(Path::new("/r"), b"1 MD N... 100644 100644 000000 a b gone.rs\0");
        assert_eq!(r.decorations()[Path::new("/r/gone.rs")], GitStatus::Deleted);
    }

    fn commit(oid: &str, parents: &[&str]) -> Commit {
        Commit { oid: oid.into(), parents: parents.iter().map(|p| (*p).to_owned()).collect(), ..Default::default() }
    }

    #[test]
    fn lanes_of_a_merge() {
        // m merges a (first parent) and b, which both come from base.
        let rows = graph(&[commit("m", &["a", "b"]), commit("a", &["base"]), commit("b", &["base"]), commit("base", &[])]);
        assert_eq!(rows[0], GraphRow { lane: 0, top: vec![], bottom: vec![(0, 0), (0, 1)] });
        assert_eq!(rows[1], GraphRow { lane: 0, top: vec![(0, 0), (1, 1)], bottom: vec![(0, 0), (1, 1)] });
        assert_eq!(rows[2], GraphRow { lane: 1, top: vec![(0, 0), (1, 1)], bottom: vec![(1, 0), (0, 0)] });
        assert_eq!(rows[3], GraphRow { lane: 0, top: vec![(0, 0)], bottom: vec![] });
    }

    /// A repository with a commit, a bare remote it tracks, and a second
    /// clone of it (someone else).
    struct Fixture {
        base: PathBuf,
        me: PathBuf,
        other: PathBuf,
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn no_trash(path: &Path) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }

    fn ok(dir: &Path, op: Op) {
        run(dir, &op, &no_trash).unwrap_or_else(|e| panic!("{op:?}: {e}"));
    }

    /// `op` and what it goes on to, as the app runs them.
    fn ok_and_next(dir: &Path, op: Op) {
        ok(dir, op.clone());
        if let Some(more) = next(&op) {
            ok(dir, more);
        }
    }

    fn commit_op(message: &str) -> Op {
        Op::Commit { message: message.into(), amend: false, stage_all: false, then: After::Nothing }
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("eshvi-git-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let base = base.canonicalize().unwrap();
            let (me, other, remote) = (base.join("me"), base.join("other"), base.join("remote.git"));
            sh(&base, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
            for dir in [&me, &other] {
                sh(&base, &["clone", "-q", remote.to_str().unwrap(), dir.to_str().unwrap()]);
                sh(dir, &["config", "user.name", "T"]);
                sh(dir, &["config", "user.email", "t@t"]);
                sh(dir, &["checkout", "-q", "-B", "main"]);
            }
            std::fs::write(me.join("a.txt"), "one\n").unwrap();
            std::fs::write(me.join("b.txt"), "b\n").unwrap();
            sh(&me, &["add", "."]);
            sh(&me, &["commit", "-qm", "first"]);
            sh(&me, &["push", "-q", "-u", "origin", "main"]);
            sh(&other, &["pull", "-q", "origin", "main"]);
            sh(&other, &["branch", "-q", "--set-upstream-to=origin/main", "main"]);
            Self { base, me, other }
        }

        fn repo(&self) -> Repo {
            status(&self.me).expect("a repository")
        }

        /// Someone else pushes a change to `file`.
        fn incoming(&self, file: &str, text: &str) {
            sh(&self.other, &["pull", "-q", "--no-rebase", "--no-edit"]);
            std::fs::write(self.other.join(file), text).unwrap();
            sh(&self.other, &["add", "."]);
            sh(&self.other, &["commit", "-qm", "theirs"]);
            sh(&self.other, &["push", "-q"]);
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn stage_unstage_and_discard() {
        let f = Fixture::new("stage");
        let paths = |list: &[Entry]| list.iter().map(|e| e.path.clone()).collect::<Vec<_>>();
        std::fs::write(f.me.join("a.txt"), "two\n").unwrap();
        std::fs::remove_file(f.me.join("b.txt")).unwrap();
        // Names git must take literally.
        for name in ["new [1].txt", "sp ace*.txt", "ünï.txt"] {
            std::fs::write(f.me.join(name), "n\n").unwrap();
        }
        let r = f.repo();
        assert_eq!(paths(&r.changes), ["a.txt", "b.txt", "new [1].txt", "sp ace*.txt", "ünï.txt"]);
        assert!(r.staged.is_empty());

        ok(&f.me, Op::Stage(vec!["a.txt".into(), "b.txt".into(), "new [1].txt".into()]));
        let r = f.repo();
        assert_eq!(paths(&r.staged), ["a.txt", "b.txt", "new [1].txt"]);
        assert_eq!(paths(&r.changes), ["sp ace*.txt", "ünï.txt"]);

        ok(&f.me, Op::Unstage(vec!["new [1].txt".into(), "b.txt".into()]));
        let r = f.repo();
        assert_eq!(paths(&r.staged), ["a.txt"]);

        // Discard: the deletion undone, the untracked file gone; what's
        // staged stays.
        ok(&f.me, Op::Discard { tracked: vec!["b.txt".into()], untracked: vec!["sp ace*.txt".into()] });
        assert_eq!(std::fs::read_to_string(f.me.join("b.txt")).unwrap(), "b\n");
        assert!(!f.me.join("sp ace*.txt").exists());
        assert_eq!(paths(&f.repo().staged), ["a.txt"]);
        // (A literal "*": not every other file.)
        assert!(f.me.join("ünï.txt").exists());

        ok(&f.me, Op::StageAll);
        assert_eq!(paths(&f.repo().staged), ["a.txt", "new [1].txt", "ünï.txt"]);
        ok(&f.me, Op::UnstageAll);
        assert!(f.repo().staged.is_empty());

        // A rename is seen as one, and unstages with where it came from.
        std::fs::rename(f.me.join("b.txt"), f.me.join("c.txt")).unwrap();
        ok(&f.me, Op::Stage(vec!["b.txt".into(), "c.txt".into()]));
        let r = f.repo();
        let rename = r.staged.iter().find(|e| e.path == "c.txt").unwrap();
        assert_eq!((rename.orig.as_deref(), rename.status), (Some("b.txt"), GitStatus::Renamed));
        ok(&f.me, Op::Unstage(vec!["c.txt".into(), "b.txt".into()]));
        assert!(!f.repo().staged.iter().any(|e| e.path == "c.txt"));
    }

    #[test]
    fn commit_amend_and_undo() {
        let f = Fixture::new("commit");
        let subject = || sh(&f.me, &["log", "-1", "--format=%s"]);
        std::fs::write(f.me.join("a.txt"), "two\n").unwrap();
        // Nothing staged: git says so, nothing happens.
        assert!(run(&f.me, &commit_op("no"), &no_trash).is_err());
        assert_eq!(subject(), "first");

        ok(&f.me, Op::Stage(vec!["a.txt".into()]));
        ok(&f.me, commit_op("second\n\nwith a body, \"quotes\" and $HOME"));
        assert_eq!(subject(), "second");
        assert_eq!(sh(&f.me, &["log", "-1", "--format=%b"]), "with a body, \"quotes\" and $HOME");
        let r = f.repo();
        assert_eq!(r.ahead, 1);
        assert!(r.staged.is_empty() && r.changes.is_empty());

        // Everything at once, new files too.
        std::fs::write(f.me.join("new.txt"), "n\n").unwrap();
        ok(&f.me, Op::Commit { message: "all".into(), amend: false, stage_all: true, then: After::Nothing });
        assert_eq!(sh(&f.me, &["show", "HEAD:new.txt"]), "n");

        // Amend: a new message; or, with none, the same one.
        ok(&f.me, Op::Commit { message: "all, again".into(), amend: true, stage_all: false, then: After::Nothing });
        assert_eq!(subject(), "all, again");
        std::fs::write(f.me.join("a.txt"), "three\n").unwrap();
        ok(&f.me, Op::Commit { message: String::new(), amend: true, stage_all: true, then: After::Nothing });
        assert_eq!(subject(), "all, again");
        assert_eq!(sh(&f.me, &["show", "HEAD:a.txt"]), "three");

        // Undo: the commit goes, what it changed stays staged.
        ok(&f.me, Op::UndoCommit);
        assert_eq!(subject(), "second");
        assert!(f.repo().staged.iter().any(|e| e.path == "new.txt"));

        // Commit & push.
        ok_and_next(&f.me, Op::Commit { message: "pushed".into(), amend: false, stage_all: true, then: After::Push });
        let r = f.repo();
        assert_eq!((r.ahead, r.behind), (0, 0));
        assert_eq!(sh(&f.base.join("remote.git"), &["log", "-1", "--format=%s", "main"]), "pushed");
    }

    #[test]
    fn push_pull_and_sync_with_a_local_remote() {
        let f = Fixture::new("sync");
        // Outgoing.
        std::fs::write(f.me.join("a.txt"), "mine\n").unwrap();
        ok(&f.me, Op::Commit { message: "mine".into(), amend: false, stage_all: true, then: After::Nothing });
        assert_eq!(f.repo().ahead, 1);
        ok(&f.me, Op::Push);
        assert_eq!(f.repo().ahead, 0);

        // Incoming: seen after a fetch, in the graph too; then pulled.
        f.incoming("b.txt", "theirs\n");
        assert_eq!(f.repo().behind, 0);
        ok(&f.me, Op::Fetch);
        assert_eq!(f.repo().behind, 1);
        let d = details(&f.me);
        // Commits made in the same second come in either order.
        let theirs = d.commits.iter().find(|c| c.subject == "theirs").expect("theirs in the graph");
        assert!(theirs.refs.iter().any(|r| r == "origin/main"), "{:?}", theirs.refs);
        ok(&f.me, Op::Pull);
        assert_eq!(f.repo().behind, 0);
        assert_eq!(std::fs::read_to_string(f.me.join("b.txt")).unwrap(), "theirs\n");

        // Both sides moved: Sync merges, whatever pull.rebase isn't, and
        // pushes.
        f.incoming("b.txt", "theirs 2\n");
        std::fs::write(f.me.join("a.txt"), "mine 2\n").unwrap();
        ok(&f.me, Op::Commit { message: "mine 2".into(), amend: false, stage_all: true, then: After::Nothing });
        ok(&f.me, Op::Fetch);
        let r = f.repo();
        assert_eq!((r.ahead, r.behind), (1, 1));
        ok(&f.me, Op::Sync);
        let r = f.repo();
        assert_eq!((r.ahead, r.behind), (0, 0));
        let d = details(&f.me);
        assert_eq!(d.commits[0].parents.len(), 2, "a merge");
        assert_eq!(d.rows[0].bottom.len(), 2);
        assert_eq!(sh(&f.base.join("remote.git"), &["rev-parse", "main"]), r.head.unwrap());

        // The user's own setting is kept.
        f.incoming("b.txt", "theirs 3\n");
        std::fs::write(f.me.join("a.txt"), "mine 3\n").unwrap();
        ok(&f.me, Op::Commit { message: "mine 3".into(), amend: false, stage_all: true, then: After::Nothing });
        sh(&f.me, &["config", "pull.rebase", "true"]);
        ok(&f.me, Op::Sync);
        assert_eq!(details(&f.me).commits[0].parents.len(), 1, "rebased");
    }

    #[test]
    fn a_rejected_push_says_why() {
        let f = Fixture::new("rejected");
        f.incoming("b.txt", "theirs\n");
        std::fs::write(f.me.join("a.txt"), "mine\n").unwrap();
        ok(&f.me, Op::Commit { message: "mine".into(), amend: false, stage_all: true, then: After::Nothing });
        let why = run(&f.me, &Op::Push, &no_trash).unwrap_err();
        assert!(why.contains("rejected"), "{why}");
    }

    #[test]
    fn a_push_that_fails_after_a_commit_leaves_the_commit_made() {
        let f = Fixture::new("commit-push-fails");
        f.incoming("b.txt", "theirs\n");
        std::fs::write(f.me.join("a.txt"), "mine\n").unwrap();
        let op = Op::Commit { message: "mine".into(), amend: false, stage_all: true, then: After::Push };
        // The commit alone works, and says what follows it.
        ok(&f.me, op.clone());
        assert_eq!(next(&op), Some(Op::Push));
        assert_eq!(next(&Op::Commit { message: "x".into(), amend: false, stage_all: false, then: After::Sync }), Some(Op::Sync));
        assert_eq!(next(&commit_op("x")), None);
        assert_eq!(next(&Op::Push), None);
        assert!(!op.touches_files());
        // The push is rejected; the commit stays, and is not made again.
        let why = run(&f.me, &Op::Push, &no_trash).unwrap_err();
        assert!(why.contains("rejected"), "{why}");
        assert_eq!(sh(&f.me, &["log", "-1", "--format=%s"]), "mine");
        assert_eq!(sh(&f.me, &["rev-list", "--count", "origin/main..HEAD"]), "1");
        assert!(f.repo().staged.is_empty() && f.repo().changes.is_empty());
    }

    #[test]
    fn a_conflict_can_be_seen_resolved_or_aborted() {
        let f = Fixture::new("conflict");
        f.incoming("a.txt", "theirs\n");
        std::fs::write(f.me.join("a.txt"), "mine\n").unwrap();
        ok(&f.me, Op::Commit { message: "mine".into(), amend: false, stage_all: true, then: After::Nothing });
        let why = run(&f.me, &Op::Pull, &no_trash).unwrap_err();
        assert!(why.contains("CONFLICT"), "{why}");
        let r = f.repo();
        assert_eq!(r.operation, Operation::Merge);
        assert_eq!(r.merge, [entry("a.txt", GitStatus::Conflict)]);
        assert!(r.merge_message.as_deref().is_some_and(|m| m.starts_with("Merge branch")), "{:?}", r.merge_message);
        assert_eq!(r.decorations()[&f.me.join("a.txt")], GitStatus::Conflict);

        ok(&f.me, Op::Abort(Operation::Merge));
        let r = f.repo();
        assert_eq!(r.operation, Operation::None);
        assert!(r.merge.is_empty());
        assert_eq!(std::fs::read_to_string(f.me.join("a.txt")).unwrap(), "mine\n");

        // Again, resolved this time: staging marks it, the commit finishes.
        assert!(run(&f.me, &Op::Pull, &no_trash).is_err());
        std::fs::write(f.me.join("a.txt"), "both\n").unwrap();
        ok(&f.me, Op::Stage(vec!["a.txt".into()]));
        let r = f.repo();
        assert!(r.merge.is_empty() && r.staged.len() == 1);
        ok_and_next(&f.me, Op::Commit { message: r.merge_message.unwrap(), amend: false, stage_all: false, then: After::Sync });
        let r = f.repo();
        assert_eq!((r.operation, r.ahead, r.behind), (Operation::None, 0, 0));
    }

    #[test]
    fn branches_stash_publish_and_ignore() {
        let f = Fixture::new("branches");
        ok(&f.me, Op::CreateBranch("feature".into()));
        let r = f.repo();
        assert_eq!((r.branch.as_deref(), r.upstream.as_deref()), (Some("feature"), None));
        std::fs::write(f.me.join("f.txt"), "f\n").unwrap();
        ok(&f.me, Op::Commit { message: "f".into(), amend: false, stage_all: true, then: After::Nothing });

        // Publish: now it has an upstream, and others see it.
        ok(&f.me, Op::Publish("origin".into()));
        assert_eq!(f.repo().upstream.as_deref(), Some("origin/feature"));
        sh(&f.other, &["fetch", "-q"]);
        let d = details(&f.other);
        let names: Vec<(&str, bool, bool)> = d.branches.iter().map(|b| (b.name.as_str(), b.remote, b.current)).collect();
        assert_eq!(names, [("main", false, true), ("origin/feature", true, false), ("origin/main", true, false)]);
        assert_eq!(d.remotes, ["origin"]);
        ok(&f.other, Op::Track("origin/feature".into()));
        assert_eq!(status(&f.other).unwrap().upstream.as_deref(), Some("origin/feature"));

        // Back, and merged in.
        ok(&f.me, Op::Checkout("main".into()));
        assert!(!f.me.join("f.txt").exists());
        ok(&f.me, Op::Merge("feature".into()));
        assert!(f.me.join("f.txt").exists());

        // Stash, with new files or without.
        std::fs::write(f.me.join("a.txt"), "wip\n").unwrap();
        std::fs::write(f.me.join("wip.txt"), "wip\n").unwrap();
        ok(&f.me, Op::Stash { untracked: false });
        assert!(f.me.join("wip.txt").exists());
        assert_eq!(std::fs::read_to_string(f.me.join("a.txt")).unwrap(), "one\n");
        ok(&f.me, Op::StashPop);
        ok(&f.me, Op::Stash { untracked: true });
        assert!(!f.me.join("wip.txt").exists());
        assert!(f.repo().changes.is_empty());
        ok(&f.me, Op::StashPop);
        assert_eq!(std::fs::read_to_string(f.me.join("a.txt")).unwrap(), "wip\n");
        assert!(f.me.join("wip.txt").exists());

        // Ignored: off the list; .gitignore itself is new.
        ok(&f.me, Op::Ignore(vec!["wip.txt".into()]));
        let paths: Vec<String> = f.repo().changes.iter().map(|e| e.path.clone()).collect();
        assert_eq!(paths, [".gitignore", "a.txt"]);
    }

    #[test]
    fn ignoring_a_path_ignores_that_path_only() {
        let f = Fixture::new("ignore-exact");
        std::fs::create_dir_all(f.me.join("sub")).unwrap();
        let odd = ["foo.txt", "#notes", "!keep", "a[1].txt", "star*.log", "end "];
        for name in odd {
            std::fs::write(f.me.join(name), "x").unwrap();
        }
        // Names the patterns would also match, were they read as patterns.
        for other in ["sub/foo.txt", "a1.txt", "starry.log", "end"] {
            std::fs::write(f.me.join(other), "x").unwrap();
        }
        ok(&f.me, Op::Ignore(odd.iter().map(|n| format!("/{n}")).collect()));
        let ignored = |path: &str| run_git(&f.me, &["check-ignore", "-q", path], None).is_ok();
        for name in odd {
            assert!(ignored(name), "{name:?} is not ignored");
        }
        for other in ["sub/foo.txt", "a1.txt", "starry.log", "end"] {
            assert!(!ignored(other), "{other:?} is ignored too");
        }
        assert_eq!(ignore_pattern("dir/a b\\c?.txt"), "/dir/a b\\\\c\\?.txt");
    }

    #[test]
    fn a_new_repository_and_a_remote() {
        let f = Fixture::new("init");
        // (Outside the fixture: it isn't a repository, but be sure.)
        let fresh = std::env::temp_dir().join(format!("eshvi-git-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fresh);
        std::fs::create_dir_all(&fresh).unwrap();
        let fresh = fresh.canonicalize().unwrap();
        assert!(status(&fresh).is_none());
        ok(&fresh, Op::Init);
        sh(&fresh, &["config", "user.name", "T"]);
        sh(&fresh, &["config", "user.email", "t@t"]);
        std::fs::write(fresh.join("a.txt"), "a\n").unwrap();
        // No commit yet: stages and unstages all the same.
        ok(&fresh, Op::Stage(vec!["a.txt".into()]));
        let r = status(&fresh).unwrap();
        assert_eq!((r.head.as_deref(), r.staged.len()), (None, 1));
        ok(&fresh, Op::Unstage(vec!["a.txt".into()]));
        assert!(status(&fresh).unwrap().staged.is_empty());
        assert!(details(&fresh).commits.is_empty());
        ok(&fresh, Op::Commit { message: "first".into(), amend: false, stage_all: true, then: After::Nothing });

        // No remote: publishing says so; with one added it works.
        assert!(run(&fresh, &Op::Publish("origin".into()), &no_trash).is_err());
        let url = f.base.join("remote.git");
        ok(&fresh, Op::AddRemote { name: "origin".into(), url: url.display().to_string() });
        assert_eq!(details(&fresh).remotes, ["origin"]);
        sh(&fresh, &["checkout", "-q", "-b", "fresh-branch"]);
        ok(&fresh, Op::Publish("origin".into()));
        assert_eq!(status(&fresh).unwrap().upstream.as_deref(), Some("origin/fresh-branch"));
        let _ = std::fs::remove_dir_all(&fresh);
    }

    #[test]
    fn files_of_a_commit() {
        let f = Fixture::new("files");
        let first = details(&f.me).commits[0].clone();
        let names = |es: Vec<Entry>| es.into_iter().map(|e| (e.path, e.orig, e.status)).collect::<Vec<_>>();
        assert_eq!(
            names(commit_files(&f.me, &first)),
            [("a.txt".to_owned(), None, GitStatus::Added), ("b.txt".to_owned(), None, GitStatus::Added)]
        );
        std::fs::write(f.me.join("a.txt"), "two\n").unwrap();
        std::fs::remove_file(f.me.join("b.txt")).unwrap();
        ok(&f.me, Op::Commit { message: "x".into(), amend: false, stage_all: true, then: After::Nothing });
        assert_eq!(
            names(commit_files(&f.me, &details(&f.me).commits[0])),
            [("a.txt".to_owned(), None, GitStatus::Modified), ("b.txt".to_owned(), None, GitStatus::Deleted)]
        );
    }

    #[test]
    fn workers_report_status_and_operations() {
        let f = Fixture::new("workers");
        let mut status = GitStatusWorker::new(|| {});
        let mut ops = GitOpsWorker::new(|| {});
        let wait_status = |w: &GitStatusWorker| match w.results.recv_timeout(Duration::from_secs(20)).expect("status") {
            StatusResult::Status(repo, details) => (repo, details),
        };
        status.refresh(&f.me, true, true);
        let (repo, details) = wait_status(&status);
        assert_eq!(repo.unwrap().branch.as_deref(), Some("main"));
        assert_eq!(details.expect("details first").commits.len(), 1);
        // Nothing moved: the details aren't read again, unless forced.
        std::thread::sleep(MIN_INTERVAL);
        status.refresh(&f.me, false, true);
        assert!(wait_status(&status).1.is_none());
        status.refresh(&f.me, true, true);
        assert!(wait_status(&status).1.is_some());

        std::fs::write(f.me.join("a.txt"), "two\n").unwrap();
        ops.run(&f.me, Op::StageAll);
        ops.run(&f.me, commit_op("second"));
        assert_eq!(ops.running, ["Staging", "Committing"]);
        let mut finished = Vec::new();
        for _ in 0..1000 {
            finished.extend(ops.poll());
            if finished.len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(&finished[..], [OpDone::Ran(Op::StageAll, Ok(()), _), OpDone::Ran(Op::Commit { .. }, Ok(()), _)]));
        assert!(ops.running.is_empty());
        status.refresh(&f.me, true, true);
        let (repo, details) = wait_status(&status);
        assert_eq!(repo.unwrap().ahead, 1);
        assert_eq!(details.unwrap().commits[0].subject, "second");

        // Undoing a commit hands back its whole message.
        std::fs::write(f.me.join("a.txt"), "three\n").unwrap();
        ops.run(&f.me, Op::StageAll);
        ops.run(&f.me, commit_op("third\n\nwith a body\nand lines"));
        ops.run(&f.me, Op::UndoCommit);
        let mut finished = Vec::new();
        for _ in 0..1000 {
            finished.extend(ops.poll());
            if finished.len() == 4 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(&finished[2..], [OpDone::Ran(Op::UndoCommit, Ok(()), _), OpDone::Undone(m)] if m == "third\n\nwith a body\nand lines"));
        assert!(ops.running.is_empty());

        // Not a repository.
        status.refresh(Path::new("/"), true, true);
        let (repo, details) = wait_status(&status);
        assert!(repo.is_none() && details.is_none());
    }
}
