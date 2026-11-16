//! What the agents changed: a snapshot of the working folder taken when the
//! first agent starts, and the files that differ from it now, with line
//! counts. A snapshot is a git tree of every file git would add (tracked
//! ones, and untracked ones not ignored), written through a temporary
//! index (a copy of the real one, so only changed files are hashed; the
//! real index is never touched). So the user's own uncommitted work before
//! the agents doesn't count, and files untracked then are followed too.
//! Each time an agent goes quiet (a turn ended), the working folder is
//! recorded again as a snap point. Rejecting a file undoes its latest
//! turn: it goes back to the newest snap point where it differs from now
//! (again: one more turn back, down to the snapshot). Accepting a file puts
//! its current version into the snapshot and every snap point; "Accept
//! all" takes a new snapshot. Run on a background thread like `git status`.
//!
//! Trees (and so git objects) are written only for the snapshot, at a turn's
//! end and for an accept. The list between those, read again whenever the
//! disk changes, is a diff of the snapshot against the working folder that
//! writes nothing (`changed_now`), at most once a second.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// The repository.
    pub top: PathBuf,
    /// The tree compared against.
    pub base: String,
    /// The working folder after each agent turn since, oldest first.
    pub points: Vec<String>,
    /// When it was taken ("14:05").
    pub since: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChangedFile {
    /// Relative to the repository.
    pub path: String,
    /// 'A', 'M' or 'D'.
    pub status: char,
    pub added: usize,
    pub removed: usize,
    pub binary: bool,
}

fn git_cmd(dir: &Path, index: Option<&Path>, args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd.output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    git_cmd(dir, None, args)
}

fn git_text(dir: &Path, args: &[&str]) -> Option<String> {
    git(dir, args).map(|o| String::from_utf8_lossy(&o).trim().to_owned())
}

fn nul_fields(out: &[u8]) -> Vec<String> {
    out.split(|&b| b == 0).filter(|f| !f.is_empty()).map(|f| String::from_utf8_lossy(f).into_owned()).collect()
}

/// A temporary index file, removed when dropped.
struct TempIndex(PathBuf);

impl TempIndex {
    /// A copy of the repository's index (its stat cache spares hashing
    /// unchanged files), or none to start from. In the repository's own git
    /// folder, where only its user writes: a name in the shared temp folder
    /// could be guessed and made a link by someone else. (The temp folder
    /// when git doesn't say where that is.)
    fn new(top: &Path, copy: bool) -> TempIndex {
        static N: AtomicU32 = AtomicU32::new(0);
        let name = format!("eshvi-index-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
        let dir = git_text(top, &["rev-parse", "--path-format=absolute", "--git-dir"]).map(PathBuf::from).filter(|d| d.is_dir());
        let path = dir.unwrap_or_else(std::env::temp_dir).join(name);
        let _ = std::fs::remove_file(&path);
        if copy {
            if let Some(real) = git_text(top, &["rev-parse", "--path-format=absolute", "--git-path", "index"]) {
                let _ = std::fs::copy(real, &path);
            }
        }
        TempIndex(path)
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("lock"));
    }
}

/// The working folder as a tree: every file `git add -A` would take.
fn worktree_tree(top: &Path) -> Option<String> {
    let index = TempIndex::new(top, true);
    git_cmd(top, Some(&index.0), &["add", "-A", "--", "."])?;
    git_cmd(top, Some(&index.0), &["write-tree"]).map(|o| String::from_utf8_lossy(&o).trim().to_owned())
}

/// Snapshot the repository containing `dir`; `Err`: why there is none
/// (not in one, or no working git).
pub fn snapshot(dir: &Path) -> std::result::Result<Snapshot, String> {
    let Some(top) = git_text(dir, &["rev-parse", "--show-toplevel"]) else {
        // (macOS's /usr/bin/git without the developer tools fails too.)
        return Err(match Command::new("git").arg("--version").output() {
            Ok(out) if out.status.success() => format!("{} isn't in a git repository", dir.display()),
            _ => format!("git doesn't run (PATH: {})", std::env::var("PATH").unwrap_or_default()),
        });
    };
    let top = PathBuf::from(top);
    let base = worktree_tree(&top).ok_or_else(|| format!("git couldn't read {}", top.display()))?;
    let since = chrono::Local::now().format("%H:%M").to_string();
    Ok(Snapshot { top, base, points: Vec::new(), since })
}

/// The files that differ from the snapshot now, by path, and the tree of
/// the working folder they were found in. Writes that tree (every changed
/// file as an object): for a turn's end, which keeps it as a snap point.
pub fn changed(s: &Snapshot) -> (Vec<ChangedFile>, Option<String>) {
    let Some(now) = worktree_tree(&s.top) else { return (Vec::new(), None) };
    let diff = |kind: &str| git(&s.top, &["diff-tree", "-r", "--no-renames", kind, "-z", &s.base, &now]).unwrap_or_default();
    (changed_files(&diff("--name-status"), &diff("--numstat")), Some(now))
}

/// The files that differ from the snapshot now, writing no objects: the
/// snapshot's tree against the working folder, through a temporary index
/// where untracked files are entered as "to be added" (`add -N`, which
/// hashes nothing). The same list as `changed`, for the refreshes between
/// turns: those come with every change on disk, and writing the tree each
/// time stored a new copy of every changing file (a log, a build output)
/// until `git gc`.
pub fn changed_now(s: &Snapshot) -> Option<Vec<ChangedFile>> {
    let index = TempIndex::new(&s.top, true);
    git_cmd(&s.top, Some(&index.0), &["add", "-A", "-N", "--", "."])?;
    let diff = |kind: &str| git_cmd(&s.top, Some(&index.0), &["diff", "--no-renames", "--no-ext-diff", kind, "-z", &s.base, "--"]);
    Some(changed_files(&diff("--name-status")?, &diff("--numstat")?))
}

/// The list from git's `--name-status -z` and `--numstat -z` of one diff.
fn changed_files(names: &[u8], numstat: &[u8]) -> Vec<ChangedFile> {
    let names = nul_fields(names);
    let mut files: Vec<ChangedFile> = Vec::new();
    for item in nul_fields(numstat) {
        let mut parts = item.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let status = names
            .chunks(2)
            .find(|c| c.len() == 2 && c[1] == path)
            .and_then(|c| c[0].chars().next())
            .unwrap_or('M');
        files.push(ChangedFile {
            path: path.to_owned(),
            status,
            added: a.parse().unwrap_or(0),
            removed: d.parse().unwrap_or(0),
            binary: a == "-",
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// `tree` with `path` as it is in tree `now` (gone if it is gone there).
fn with_file(top: &Path, tree: &str, now: &str, path: &str) -> Option<String> {
    let index = TempIndex::new(top, false);
    git_cmd(top, Some(&index.0), &["read-tree", tree])?;
    let entry = git(top, &["ls-tree", "-z", now, "--", path]).unwrap_or_default();
    let entry = String::from_utf8_lossy(&entry);
    let entry = entry.trim_end_matches('\0');
    if entry.is_empty() {
        git_cmd(top, Some(&index.0), &["update-index", "--force-remove", "--", path])?;
    } else {
        // "<mode> blob <id>\t<path>"
        let (meta, _) = entry.split_once('\t')?;
        let mut meta = meta.split(' ');
        let (mode, _, id) = (meta.next()?, meta.next()?, meta.next()?);
        git_cmd(top, Some(&index.0), &["update-index", "--add", "--cacheinfo", &format!("{mode},{id},{path}")])?;
    }
    git_cmd(top, Some(&index.0), &["write-tree"]).map(|o| String::from_utf8_lossy(&o).trim().to_owned())
}

/// Accept `path` as it is in tree `now`: into the snapshot and every snap
/// point, so undoing turns never goes back past it.
pub fn accept_file(s: &Snapshot, now: &str, path: &str) -> Option<Snapshot> {
    let base = with_file(&s.top, &s.base, now, path)?;
    let points = s.points.iter().map(|p| with_file(&s.top, p, now, path)).collect::<Option<Vec<_>>>()?;
    Some(Snapshot { base, points, ..s.clone() })
}

/// Accept every changed file whose working version is the one `HEAD` has
/// (gone from both counts), as `accept_file` does: after a commit. A file
/// committed while further changes of it are unstaged differs from `HEAD`
/// and stays. `None`: none to accept (or git failed).
pub fn accept_committed(s: &Snapshot) -> Option<(Snapshot, Vec<String>)> {
    let (files, now) = changed(s);
    let now = now?;
    let done: Vec<String> = files
        .into_iter()
        .filter(|f| blob_in(&s.top, &now, &f.path) == blob_in(&s.top, "HEAD", &f.path))
        .map(|f| f.path)
        .collect();
    let mut next = s.clone();
    for path in &done {
        next = accept_file(&next, &now, path)?;
    }
    (!done.is_empty()).then_some((next, done))
}

/// The blob of `path` in `tree`, if it is there.
fn blob_in(top: &Path, tree: &str, path: &str) -> Option<String> {
    git_text(top, &["rev-parse", "--verify", "-q", &format!("{tree}:{path}")]).filter(|b| !b.is_empty())
}

/// A file put back by `reject_file`.
#[derive(Debug, PartialEq)]
pub struct PutBack {
    /// What was there, stored as this git blob first, when it was in no snap
    /// point (edits since the last turn, maybe the user's own): the only
    /// copy of it once the file is replaced.
    pub kept: Option<String>,
}

/// Undo `path`'s latest turn: find where it is now in its history (the
/// newest snap point it matches; none: changed since the last one), and
/// put it back as it was at the newest point before that where it differs —
/// its old contents, or no file if it wasn't there. `Ok(None)`: nothing to
/// undo. Contents that are in no snap point are kept as a blob before they
/// are replaced (`PutBack::kept`); if that fails the file is left alone.
pub fn reject_file(s: &Snapshot, path: &str) -> std::io::Result<Option<PutBack>> {
    let full = s.top.join(path);
    let current = full
        .is_file()
        .then(|| git_text(&s.top, &["hash-object", "--", path]))
        .flatten();
    let history: Vec<(&String, Option<String>)> =
        std::iter::once(&s.base).chain(&s.points).map(|tree| (tree, blob_in(&s.top, tree, path))).collect();
    let at = history.iter().rposition(|(_, blob)| *blob == current).unwrap_or(history.len());
    let target = history[..at].iter().rev().find(|(_, blob)| *blob != current);
    let Some((tree, blob)) = target else { return Ok(None) };
    let kept = match &current {
        Some(_) if at == history.len() => Some(
            git_text(&s.top, &["hash-object", "-w", "--", path])
                .filter(|id| !id.is_empty())
                .ok_or_else(|| std::io::Error::other("no copy of its contents now could be kept, so it is left as it is"))?,
        ),
        _ => None,
    };
    match blob {
        Some(_) => {
            let bytes = git(&s.top, &["cat-file", "blob", &format!("{tree}:{path}")])
                .ok_or_else(|| std::io::Error::other(format!("git cat-file {tree}:{path} failed")))?;
            if let Some(dir) = full.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&full, bytes)?;
        }
        None => match std::fs::remove_file(&full) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        },
    }
    Ok(Some(PutBack { kept }))
}

pub enum Request {
    /// Take a snapshot of the repository containing this folder.
    Snapshot(PathBuf),
    /// The changed files now; `turn`: an agent's turn ended (a snap point).
    Refresh(Arc<Snapshot>, bool),
    /// Accept a file as it is now.
    Accept(Arc<Snapshot>, String),
    /// `HEAD` moved: accept the files that now equal it.
    Committed(Arc<Snapshot>),
    Reject(Arc<Snapshot>, String),
}

pub enum Result {
    /// `Err`: why there is none (not in a git repository). `accepted`: a
    /// files were accepted into the previous snapshot (the others keep
    /// their changes).
    Snapshot(std::result::Result<Arc<Snapshot>, String>, Option<Vec<String>>),
    /// The changed files, for the snapshot they were computed against,
    /// the tree they were found in, and whether a turn ended.
    Files(Arc<Snapshot>, Vec<ChangedFile>, Option<String>, bool),
    /// A file was put back as it was (`Some`: the blob its contents were
    /// kept as first); `Err`: why not.
    Rejected(String, std::result::Result<Option<String>, String>),
}

/// The least time between two reads of the list for a change on disk.
const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// Runs snapshots and diffs off the UI thread; refreshes that pile up
/// while one runs collapse into one, and those for a change on disk run at
/// most once per `REFRESH_EVERY` (a build writing files asks many times a
/// second).
pub struct ChangesWorker {
    requests: mpsc::Sender<Request>,
    pub results: mpsc::Receiver<Result>,
}

impl ChangesWorker {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("changes".into())
            .spawn(move || {
                let mut queue: Vec<Request> = Vec::new();
                let mut last_refresh: Option<std::time::Instant> = None;
                while let Ok(req) = req_rx.recv() {
                    queue.push(req);
                    queue.extend(req_rx.try_iter());
                    // Only the disk changed, and the list was read a moment
                    // ago: wait out the rest, taking what else comes.
                    while queue.iter().all(|r| matches!(r, Request::Refresh(_, false))) {
                        let Some(wait) = last_refresh.and_then(|t| REFRESH_EVERY.checked_sub(t.elapsed())) else { break };
                        match req_rx.recv_timeout(wait) {
                            Ok(req) => queue.push(req),
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    // One refresh, after the rest, for the latest snapshot
                    // (a turn's end, if any of them was one).
                    let turn = queue.iter().any(|r| matches!(r, Request::Refresh(_, true)));
                    let refresh = queue.iter().rposition(|r| matches!(r, Request::Refresh(..)));
                    let refresh = refresh.map(|i| match queue.remove(i) {
                        Request::Refresh(s, _) => Request::Refresh(s, turn),
                        other => other,
                    });
                    queue.retain(|r| !matches!(r, Request::Refresh(..)));
                    queue.extend(refresh);
                    for req in queue.drain(..) {
                        let result = match req {
                            Request::Snapshot(dir) => Result::Snapshot(snapshot(&dir).map(Arc::new), None),
                            // A turn's end keeps the working folder as a
                            // tree; otherwise nothing is written.
                            Request::Refresh(s, true) => {
                                let (files, now) = changed(&s);
                                last_refresh = Some(std::time::Instant::now());
                                Result::Files(s, files, now, true)
                            }
                            Request::Refresh(s, false) => {
                                let files = changed_now(&s).unwrap_or_default();
                                last_refresh = Some(std::time::Instant::now());
                                Result::Files(s, files, None, false)
                            }
                            Request::Accept(s, path) => {
                                match worktree_tree(&s.top).and_then(|now| accept_file(&s, &now, &path)) {
                                    Some(next) => Result::Snapshot(Ok(Arc::new(next)), Some(vec![path])),
                                    None => continue,
                                }
                            }
                            Request::Committed(s) => match accept_committed(&s) {
                                Some((next, paths)) => Result::Snapshot(Ok(Arc::new(next)), Some(paths)),
                                None => continue,
                            },
                            Request::Reject(s, path) => {
                                let done = reject_file(&s, &path).map(|put| put.and_then(|p| p.kept)).map_err(|e| e.to_string());
                                Result::Rejected(path, done)
                            }
                        };
                        if res_tx.send(result).is_err() {
                            return;
                        }
                        wake();
                    }
                }
            })
            .expect("changes thread");
        Self { requests: req_tx, results: res_rx }
    }

    pub fn send(&self, req: Request) {
        let _ = self.requests.send(req);
    }
}

/// What the Changes list needs to know of an agent: where it runs, and whether it is
/// working (output arriving).
pub struct AgentInfo {
    pub id: u64,
    pub cwd: String,
    pub active: bool,
}

/// The GUI's side: the snapshot, the changed files, whether they changed
/// unseen, and the popup.
pub struct Changes {
    worker: ChangesWorker,
    /// Folders a snapshot was asked for: the first agent's, and while
    /// there's none (not a repository), each new agent's.
    tried: HashSet<PathBuf>,
    /// A snapshot was asked for and hasn't come.
    pending: bool,
    /// The answer: `Some(None)` when not in a git repository.
    pub snapshot: Option<Option<Arc<Snapshot>>>,
    pub files: Vec<ChangedFile>,
    /// The files as last looked at.
    seen: Vec<ChangedFile>,
    /// Changed since they were last looked at: a dot on the chip.
    pub unseen: bool,
    pub popup: bool,
    /// A file whose "Reject" was clicked once: the next click discards.
    pub confirm_reject: Option<String>,
    /// Agents (by buffer) working at the last report.
    active: HashSet<u64>,
    /// The repository's `HEAD` at the last status read (none: no commit yet).
    head: Option<(PathBuf, Option<String>)>,
}

/// What the GUI must do after a poll.
pub enum Done {
    /// A file was put back: Neovim reads changed files again.
    Rejected,
    Failed(String),
    /// Worth saying, not an error (why changes aren't followed).
    Note(String),
}

impl Changes {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        Self {
            worker: ChangesWorker::new(wake),
            tried: HashSet::new(),
            pending: false,
            snapshot: None,
            files: Vec::new(),
            seen: Vec::new(),
            unseen: false,
            popup: false,
            confirm_reject: None,
            active: HashSet::new(),
            head: None,
        }
    }

    /// The agents were looked at (each frame): the first agent takes the snapshot
    /// (or, until one is in a repository, the next one in another folder);
    /// one that went quiet (a turn ended) refreshes the list.
    pub fn on_agents(&mut self, agents: &[AgentInfo]) {
        if self.current().is_none() && !self.pending {
            if let Some(tab) = agents.iter().find(|t| !self.tried.contains(Path::new(&t.cwd))) {
                self.tried.insert(PathBuf::from(&tab.cwd));
                self.pending = true;
                self.worker.send(Request::Snapshot(PathBuf::from(&tab.cwd)));
            }
        }
        let active: HashSet<u64> = agents.iter().filter(|t| t.active).map(|t| t.id).collect();
        if self.active.iter().any(|b| !active.contains(b)) {
            if let Some(s) = self.current() {
                self.worker.send(Request::Refresh(s.clone(), true));
            }
        }
        self.active = active;
    }

    /// Take the worker's results.
    pub fn poll(&mut self) -> Vec<Done> {
        let mut done = Vec::new();
        for result in self.worker.results.try_iter().collect::<Vec<_>>() {
            match result {
                Result::Snapshot(Err(why), _) => {
                    self.pending = false;
                    if self.current().is_none() {
                        self.snapshot = Some(None);
                    }
                    done.push(Done::Note(format!("Agent changes aren't followed: {why}")));
                }
                Result::Snapshot(Ok(s), accepted) => {
                    self.pending = false;
                    self.snapshot = Some(Some(s));
                    match accepted {
                        // The other files keep their changes until the
                        // refresh confirms them.
                        Some(paths) => {
                            self.files.retain(|f| !paths.contains(&f.path));
                            self.refresh();
                        }
                        None => {
                            self.files.clear();
                            self.seen.clear();
                            self.unseen = false;
                        }
                    }
                }
                Result::Files(s, files, now, turn) => {
                    // (Not for a snapshot replaced since; snap points
                    // added since don't matter.)
                    if self.current().is_some_and(|c| c.base == s.base && c.top == s.top) {
                        // A turn ended: the working folder is a snap point.
                        if let (true, Some(now), Some(c)) = (turn, &now, self.current()) {
                            if c.points.last().unwrap_or(&c.base) != now {
                                let mut next = (**c).clone();
                                next.points.push(now.clone());
                                self.snapshot = Some(Some(Arc::new(next)));
                            }
                        }
                        if self.popup {
                            self.seen = files.clone();
                        } else if !files.is_empty() && files != self.seen {
                            self.unseen = true;
                        }
                        self.files = files;
                    }
                }
                Result::Rejected(path, Ok(kept)) => {
                    self.files.retain(|f| f.path != path);
                    done.push(Done::Rejected);
                    if let Some(id) = kept {
                        done.push(Done::Note(kept_note(&path, &id)));
                    }
                    self.refresh();
                }
                Result::Rejected(path, Err(e)) => done.push(Done::Failed(format!("Couldn't put {path} back: {e}"))),
            }
        }
        done
    }

    pub fn current(&self) -> Option<&Arc<Snapshot>> {
        self.snapshot.as_ref().and_then(Option::as_ref)
    }

    pub fn refresh(&self) {
        if let Some(s) = self.current() {
            self.worker.send(Request::Refresh(s.clone(), false));
        }
    }

    pub fn open(&mut self) {
        self.popup = true;
        self.unseen = false;
        self.confirm_reject = None;
        self.seen = self.files.clone();
        self.refresh();
    }

    /// "Accept all": the changes stay on disk; count from here.
    pub fn accept_all(&mut self) {
        if let Some(s) = self.current() {
            self.worker.send(Request::Snapshot(s.top.clone()));
        }
        self.popup = false;
    }

    /// Keep a file's changes: it drops off the list until changed again.
    pub fn accept(&mut self, path: &str) {
        if let Some(s) = self.current() {
            self.worker.send(Request::Accept(s.clone(), path.to_owned()));
        }
        self.confirm_reject = None;
    }

    /// A file was saved with its diff open (`do` on the unwanted hunks, then
    /// `:w`): what is on disk is accepted, as the popup's "Accept" does.
    /// `path`: as Neovim names it. False: not a listed file.
    pub fn accept_saved(&mut self, path: &Path) -> bool {
        let Some(rel) = self.relative(path) else { return false };
        if !self.files.iter().any(|f| f.path == rel) {
            return false;
        }
        self.accept(&rel);
        true
    }

    /// `path` as the repository names it.
    fn relative(&self, path: &Path) -> Option<String> {
        let s = self.current()?;
        let full = path.canonicalize().ok()?;
        let top = s.top.canonicalize().unwrap_or_else(|_| s.top.clone());
        full.strip_prefix(&top).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
    }

    /// A status read said `top`'s `HEAD` is `head`. When it moved (a commit,
    /// here or in a terminal; a checkout too) in the repository followed,
    /// the files that now equal it are accepted.
    pub fn on_head(&mut self, top: &Path, head: Option<&str>) {
        let moved = self.head.as_ref().is_some_and(|(t, h)| t == top && h.as_deref() != head);
        self.head = Some((top.to_path_buf(), head.map(str::to_owned)));
        if let (true, Some(s)) = (moved, self.current()) {
            if s.top == top || s.top.canonicalize().ok() == top.canonicalize().ok() {
                self.worker.send(Request::Committed(s.clone()));
            }
        }
    }

    /// Undo a file's changes (after a confirming second click).
    pub fn reject(&mut self, path: &str) {
        if let Some(s) = self.current() {
            self.worker.send(Request::Reject(s.clone(), path.to_owned()));
        }
        self.confirm_reject = None;
    }

    /// A file was saved from its diff view: if the agents created it (since
    /// the snapshot) and it's now empty, every line of it was undone, so it
    /// goes, as it was before them. Returns its path in the repository when
    /// removed. (Not for any save: an empty file the user made stays.)
    pub fn remove_if_emptied(&self, path: &Path) -> Option<String> {
        let rel = self.relative(path)?;
        let full = path.canonicalize().ok()?;
        let added = self.files.iter().any(|f| f.path == rel && f.status == 'A');
        let empty = std::fs::metadata(&full).is_ok_and(|m| m.is_file() && m.len() == 0);
        (added && empty && std::fs::remove_file(&full).is_ok()).then_some(rel)
    }

    /// Lines added and removed in all the files.
    pub fn totals(&self) -> (usize, usize) {
        self.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed))
    }
}

/// What is said when a rejected file held contents that were in no snap
/// point: where they are now, and how to get them.
fn kept_note(path: &str, id: &str) -> String {
    let short = &id[..id.len().min(12)];
    log::info!("changes: {path} as it was before Reject is git object {id}");
    format!("{path} had changed since the agents' last turn. What Reject replaced is kept: git cat-file -p {short}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loose objects in the repository (`git count-objects`).
    fn objects(repo: &Path) -> String {
        git_text(repo, &["count-objects"]).unwrap().split(' ').next().unwrap().to_owned()
    }

    #[test]
    fn the_list_between_turns_is_the_same_and_writes_no_objects() {
        let repo = repo("nowrite");
        std::fs::write(repo.join("notes.txt"), "mine\n").unwrap();
        let index = std::fs::read(repo.join(".git/index")).unwrap();
        let s = snapshot(&repo).unwrap();
        assert_eq!(changed_now(&s), Some(Vec::new()));
        // An edit, a new file (untracked), a binary one, a deletion, and a
        // file that was untracked at the snapshot changed and another gone.
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.join("c.txt"), "new\nfile\n").unwrap();
        std::fs::write(repo.join("bin.dat"), [0u8, 1, 2, 0]).unwrap();
        std::fs::remove_file(repo.join("gone.txt")).unwrap();
        std::fs::write(repo.join("notes.txt"), "mine\nmore\n").unwrap();
        let first = changed_now(&s).unwrap();
        // (`add -N` stores git's empty blob once; nothing after that.)
        let before = objects(&repo);
        std::fs::write(repo.join("build.log"), "line\n".repeat(100)).unwrap();
        std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let files = changed_now(&s).unwrap();
        std::fs::write(repo.join("build.log"), "line\n".repeat(200)).unwrap();
        assert_eq!(changed_now(&s).unwrap().len(), files.len());
        assert_eq!(objects(&repo), before, "a refresh wrote objects");
        assert_eq!(summary(&first)[0], ("a.txt", 'M', 1, 0));
        assert_eq!(
            summary(&files),
            [("a.txt", 'M', 2, 0), ("bin.dat", 'A', 0, 0), ("build.log", 'A', 100, 0), ("c.txt", 'A', 2, 0), ("gone.txt", 'D', 0, 2), ("notes.txt", 'M', 1, 0)]
        );
        assert!(files[1].binary);
        // The user's index is untouched, and no temporary one is left.
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index);
        let left: Vec<_> = std::fs::read_dir(repo.join(".git")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("eshvi-index")).collect();
        assert!(left.is_empty(), "{left:?}");
        // The same list as the one that writes the tree (a turn's end).
        std::fs::write(repo.join("build.log"), "line\n".repeat(100)).unwrap();
        assert_eq!(changed(&s).0, files);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn rejecting_keeps_a_copy_of_what_is_in_no_snap_point() {
        let repo = repo("kept");
        let mut s = snapshot(&repo).unwrap();
        // The agent's turn, then the user's own fix on top of it.
        std::fs::write(repo.join("a.txt"), "one\nagent\n").unwrap();
        s.points.push(changed(&s).1.unwrap());
        std::fs::write(repo.join("a.txt"), "one\nagent\nmine, by hand\n").unwrap();
        // Reject goes back past the hand edit: it is kept as a blob first.
        let put = reject_file(&s, "a.txt").unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one\nagent\n");
        let id = put.kept.expect("a copy of the hand edit");
        assert_eq!(git(&repo, &["cat-file", "-p", &id]).map(|b| String::from_utf8_lossy(&b).into_owned()).as_deref(), Some("one\nagent\nmine, by hand\n"));
        assert!(kept_note("a.txt", &id).contains(&id[..12]));
        // Now it is as the turn left it, which the snap point has: nothing more to keep.
        assert_eq!(reject_file(&s, "a.txt").unwrap(), Some(PutBack { kept: None }));
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one\n");
        // A new file edited since the turn that made it: kept, then gone.
        std::fs::write(repo.join("c.txt"), "new\n").unwrap();
        s.points.push(changed(&s).1.unwrap());
        std::fs::write(repo.join("c.txt"), "new\nmine\n").unwrap();
        assert!(reject_file(&s, "c.txt").unwrap().unwrap().kept.is_some());
        assert_eq!(std::fs::read_to_string(repo.join("c.txt")).unwrap(), "new\n");
        assert_eq!(reject_file(&s, "c.txt").unwrap(), Some(PutBack { kept: None }));
        assert!(!repo.join("c.txt").exists());
        let _ = std::fs::remove_dir_all(repo);
    }

    fn repo(name: &str) -> PathBuf {
        let repo = std::env::temp_dir().join(format!("eshvi-changes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| assert!(git(&repo, args).is_some(), "git {args:?}");
        run(&["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        std::fs::write(repo.join("b.txt"), "keep\n").unwrap();
        std::fs::write(repo.join("gone.txt"), "x\ny\n").unwrap();
        run(&["add", "."]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
        repo
    }

    fn summary(files: &[ChangedFile]) -> Vec<(&str, char, usize, usize)> {
        files.iter().map(|f| (f.path.as_str(), f.status, f.added, f.removed)).collect()
    }

    #[test]
    fn counts_what_changed_since_the_snapshot_only() {
        let repo = repo("count");
        // Before the snapshot: the user's own change, an untracked file and
        // the index as the user left it.
        std::fs::write(repo.join("b.txt"), "keep\nmine\n").unwrap();
        std::fs::write(repo.join("notes.txt"), "mine\n").unwrap();
        let index = std::fs::read(repo.join(".git/index")).unwrap();

        let s = snapshot(&repo).unwrap();
        assert!(changed(&s).0.is_empty());

        // After: an edit, a new file, a deletion, and the untracked file
        // changed (followed too).
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.join("c.txt"), "new\nfile\n").unwrap();
        std::fs::remove_file(repo.join("gone.txt")).unwrap();
        std::fs::write(repo.join("notes.txt"), "mine\nmore\n").unwrap();
        let (files, _) = changed(&s);
        assert_eq!(
            summary(&files),
            [("a.txt", 'M', 1, 0), ("c.txt", 'A', 2, 0), ("gone.txt", 'D', 0, 2), ("notes.txt", 'M', 1, 0)]
        );
        // The user's index is untouched.
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index);

        // Not a repository.
        let plain = std::env::temp_dir().join(format!("eshvi-changes-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        assert!(snapshot(&plain).unwrap_err().contains("isn't in a git repository"));
        let _ = std::fs::remove_dir_all(plain);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn rejecting_undoes_one_turn_at_a_time() {
        let repo = repo("turns");
        let read = |f: &str| std::fs::read_to_string(repo.join(f)).ok();
        let mut s = snapshot(&repo).unwrap();
        let end_turn = |s: &mut Snapshot| {
            let (_, now) = changed(s);
            s.points.push(now.unwrap());
        };
        // Turn 1 edits a.txt; turn 2 edits it again and adds c.txt.
        std::fs::write(repo.join("a.txt"), "one\nturn 1\n").unwrap();
        end_turn(&mut s);
        std::fs::write(repo.join("a.txt"), "one\nturn 1\nturn 2\n").unwrap();
        std::fs::write(repo.join("c.txt"), "new\n").unwrap();
        end_turn(&mut s);

        // Rejecting a.txt undoes turn 2, then turn 1, then nothing is left.
        assert_eq!(reject_file(&s, "a.txt").unwrap(), Some(PutBack { kept: None }));
        assert_eq!(read("a.txt").as_deref(), Some("one\nturn 1\n"));
        assert!(reject_file(&s, "a.txt").unwrap().is_some());
        assert_eq!(read("a.txt").as_deref(), Some("one\n"));
        assert!(reject_file(&s, "a.txt").unwrap().is_none());
        // c.txt came in turn 2: gone.
        assert!(reject_file(&s, "c.txt").unwrap().is_some());
        assert_eq!(read("c.txt"), None);

        // Accepted, a file's turns before it can't be undone.
        std::fs::write(repo.join("b.txt"), "keep\nturn 3\n").unwrap();
        end_turn(&mut s);
        let (_, now) = changed(&s);
        let s = accept_file(&s, &now.unwrap(), "b.txt").unwrap();
        assert!(reject_file(&s, "b.txt").unwrap().is_none());
        assert_eq!(read("b.txt").as_deref(), Some("keep\nturn 3\n"));
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn accepting_keeps_a_file_and_rejecting_puts_it_back() {
        let repo = repo("decide");
        let s = snapshot(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.join("b.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("c.txt"), "new\n").unwrap();
        std::fs::remove_file(repo.join("gone.txt")).unwrap();
        let (_, now) = changed(&s);
        let now = now.unwrap();

        // Accepted: a.txt and the deletion stop counting; until changed again.
        let s = accept_file(&s, &now, "a.txt").unwrap();
        let s = accept_file(&s, &now, "gone.txt").unwrap();
        assert_eq!(summary(&changed(&s).0), [("b.txt", 'M', 1, 1), ("c.txt", 'A', 1, 0)]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        assert_eq!(summary(&changed(&s).0)[0], ("a.txt", 'M', 1, 0));
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();

        // Rejected: b.txt as it was, c.txt gone.
        reject_file(&s, "b.txt").unwrap();
        reject_file(&s, "c.txt").unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("b.txt")).unwrap(), "keep\n");
        assert!(!repo.join("c.txt").exists());
        assert!(changed(&s).0.is_empty());
        let _ = std::fs::remove_dir_all(repo);
    }

    /// Waits for the worker's answer to what was just sent.
    fn settle(c: &mut Changes) {
        for _ in 0..500 {
            c.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_saved_file_is_accepted_as_the_popup_does_and_only_a_listed_one() {
        let repo = repo("saved");
        let s = snapshot(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(repo.join("b.txt"), "changed\n").unwrap();
        let mut c = Changes::new(|| {});
        c.files = changed(&s).0;
        c.snapshot = Some(Some(Arc::new(s)));
        // Partial undo, then :w: a.txt as it is on disk is what counts as
        // saved; the other file keeps its changes.
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        assert!(!c.accept_saved(&repo.join("c.txt")), "not listed");
        assert!(!c.accept_saved(&std::env::temp_dir().join("elsewhere.txt")), "outside the repository");
        assert!(c.accept_saved(&repo.join("a.txt")));
        for _ in 0..500 {
            c.poll();
            if c.files.iter().all(|f| f.path != "a.txt") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let s = c.current().unwrap().clone();
        assert_eq!(summary(&changed(&s).0), [("b.txt", 'M', 1, 1)]);
        // Changed again after the save: listed again.
        std::fs::write(repo.join("a.txt"), "one\ntwo\nmore\n").unwrap();
        assert_eq!(summary(&changed(&s).0)[0], ("a.txt", 'M', 1, 0));
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn a_commit_accepts_the_files_that_now_equal_head_and_keeps_the_rest() {
        let repo = repo("commit");
        let run = |args: &[&str]| assert!(git(&repo, args).is_some(), "git {args:?}");
        let s = snapshot(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.join("b.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("c.txt"), "new\n").unwrap();
        std::fs::remove_file(repo.join("gone.txt")).unwrap();
        // Nothing committed: nothing equals HEAD.
        assert!(accept_committed(&s).is_none());
        // Commit a.txt, c.txt and the deletion; b.txt gets a further change
        // after it was staged, so its working version isn't what went in.
        run(&["add", "a.txt", "c.txt", "gone.txt", "b.txt"]);
        std::fs::write(repo.join("b.txt"), "changed\nand more\n").unwrap();
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "agents"]);
        let (next, done) = accept_committed(&s).unwrap();
        assert_eq!(done, ["a.txt", "c.txt", "gone.txt"]);
        assert_eq!(summary(&changed(&next).0), [("b.txt", 'M', 2, 1)]);
        // The same through the GUI's side: a HEAD that moved asks the worker.
        let mut c = Changes::new(|| {});
        c.files = changed(&s).0;
        c.snapshot = Some(Some(Arc::new(s)));
        let head = git_text(&repo, &["rev-parse", "HEAD"]).unwrap();
        c.on_head(&repo, Some("before"));
        c.on_head(&repo, Some(&head));
        settle(&mut c);
        assert_eq!(c.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b.txt"]);
        // HEAD unchanged on the next read: nothing asked, and a repository
        // other than the followed one doesn't count.
        let other = repo.join("elsewhere");
        c.on_head(&other, Some("x"));
        c.on_head(&other, Some("y"));
        assert_eq!(c.current().map(|s| changed(s).0.len()), Some(1));
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn a_new_file_emptied_and_saved_goes() {
        let repo = repo("emptied");
        let s = snapshot(&repo).unwrap();
        std::fs::write(repo.join("new.rs"), "fn t() {}\n").unwrap();
        std::fs::write(repo.join("a.txt"), "").unwrap();
        let mut c = Changes::new(|| {});
        c.files = changed(&s).0;
        c.snapshot = Some(Some(Arc::new(s)));
        // Not emptied yet; and a file that was there before stays, empty.
        assert_eq!(c.remove_if_emptied(&repo.join("new.rs")), None);
        assert_eq!(c.remove_if_emptied(&repo.join("a.txt")), None);
        assert!(repo.join("a.txt").exists());
        std::fs::write(repo.join("new.rs"), "").unwrap();
        assert_eq!(c.remove_if_emptied(&repo.join("new.rs")).as_deref(), Some("new.rs"));
        assert!(!repo.join("new.rs").exists());
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn an_agent_outside_a_repository_says_so_and_the_next_one_is_followed() {
        let repo = repo("later");
        let plain = std::env::temp_dir().join(format!("eshvi-changes-plain2-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        let tab = |id, dir: &Path| AgentInfo { id, cwd: dir.display().to_string(), active: false };
        let mut panel = vec![tab(1, &plain)];
        let mut c = Changes::new(|| {});
        let wait = |c: &mut Changes| {
            for _ in 0..500 {
                let done = c.poll();
                if !c.pending {
                    return done;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("no snapshot");
        };
        c.on_agents(&panel);
        let notes: Vec<String> = wait(&mut c).into_iter().filter_map(|d| if let Done::Note(n) = d { Some(n) } else { None }).collect();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("isn't in a git repository"), "{notes:?}");
        assert!(c.current().is_none());
        // The same agent reported again: not asked again.
        c.on_agents(&panel);
        assert!(!c.pending);
        // A new agent in a repository: followed from now on.
        panel.push(tab(2, &repo));
        c.on_agents(&panel);
        wait(&mut c);
        assert_eq!(c.current().map(|s| s.top.canonicalize().unwrap()), Some(repo.canonicalize().unwrap()));
        let _ = std::fs::remove_dir_all(plain);
        let _ = std::fs::remove_dir_all(repo);
    }
}
