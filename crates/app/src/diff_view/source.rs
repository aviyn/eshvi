//! Where a diff's two sides come from, and the thread that reads them: `git
//! show <ref>:<path>`, the index (`git show :<path>`), the file on disk, or
//! nothing (a new file's left side). No round trip through Neovim. Reading,
//! diffing and syntax coloring a file run on worker threads, those the view
//! wants now first; a long file's colors follow its diff in a message of
//! their own.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Arc, Condvar, Mutex};

use egui::Color32;
use syntect::highlighting::Theme;

use super::model::{self, Body, Content};
use crate::git::Commit;

/// Where one side of a file is read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Side {
    /// `git show <ref>:<path>`: a commit, `HEAD`, or the tree of the agents' snapshot.
    Ref(String),
    /// The index (`git show :<path>`).
    Index,
    /// The file as it is now.
    Disk,
    /// Not there (a new file's left, a deleted file's right).
    Missing,
}

/// A file's change as the Git panel or the agents' list names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl Status {
    pub fn letter(self) -> &'static str {
        match self {
            Status::Added => "A",
            Status::Modified => "M",
            Status::Deleted => "D",
            Status::Renamed => "R",
        }
    }
}

/// One file of a diff view: its path, and what is compared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSpec {
    pub path: String,
    /// Where it was, if renamed: the left side is read from there.
    pub old_path: Option<String>,
    pub status: Status,
    pub left: Side,
    pub right: Side,
}

impl FileSpec {
    /// The file's name for a heading: "old → new" when renamed.
    pub fn title(&self) -> String {
        match &self.old_path {
            Some(old) if *old != self.path => format!("{old} → {}", self.path),
            _ => self.path.clone(),
        }
    }

    /// The file is read from the disk, so a change on disk changes the diff.
    pub fn reads_disk(&self) -> bool {
        self.left == Side::Disk || self.right == Side::Disk
    }
}

/// What a side is called in the bar: "index", "working tree", "HEAD", or a
/// commit's short id.
pub fn side_name(side: &Side) -> String {
    match side {
        Side::Index => "index".into(),
        Side::Disk => "working tree".into(),
        Side::Missing => "nothing".into(),
        Side::Ref(r) if r == "HEAD" => "HEAD".into(),
        Side::Ref(r) => r.chars().take(7).collect(),
    }
}

/// "HEAD → working tree".
pub fn compared(left: &Side, right: &Side) -> String {
    format!("{} → {}", side_name(left), side_name(right))
}

/// What a view can do to the changes it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Nothing: a commit's diff, or one from Neovim's own history.
    #[default]
    ReadOnly,
    /// The agents' changes: revert a change or a file, accept a file.
    Agents,
    /// The index against the working tree: stage or discard.
    Stage,
    /// `HEAD` against the index: unstage.
    Unstage,
}

impl Mode {
    /// What a comparison of Source Control's is (read-only for any other).
    pub fn of(left: &Side, right: &Side) -> Mode {
        match (left, right) {
            (Side::Index | Side::Missing, Side::Disk) => Mode::Stage,
            (Side::Ref(r), Side::Index | Side::Missing) if r == "HEAD" => Mode::Unstage,
            (Side::Missing, Side::Index) => Mode::Unstage,
            _ => Mode::ReadOnly,
        }
    }
}

/// What a view opens: the files, what is compared, and the file to scroll to.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub top: PathBuf,
    /// "HEAD → working tree".
    pub compared: String,
    pub files: Files,
    /// Where to start: this file's section at the top.
    pub focus: Option<String>,
    pub mode: Mode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Files {
    List(Vec<FileSpec>),
    /// A commit's files against its first parent: listed by the worker.
    Commit(Commit),
}

/// The bytes of one side (`git show` failing is a side that isn't there).
fn read(top: &Path, side: &Side, path: &str) -> Content {
    let git = |spec: String| {
        let out = Command::new("git").arg("-C").arg(top).args(["show", &spec]).output().ok()?;
        out.status.success().then(|| Content::from_bytes(&out.stdout))
    };
    match side {
        Side::Missing => Content::Missing,
        Side::Disk => match std::fs::read(top.join(path)) {
            Ok(bytes) => Content::from_bytes(&bytes),
            Err(_) => Content::Missing,
        },
        Side::Index => git(format!(":{path}")).unwrap_or(Content::Missing),
        Side::Ref(r) => git(format!("{r}:{path}")).unwrap_or(Content::Missing),
    }
}

/// The two sides of `spec`.
pub fn read_sides(top: &Path, spec: &FileSpec) -> (Content, Content) {
    let old_path = spec.old_path.as_deref().unwrap_or(&spec.path);
    (read(top, &spec.left, old_path), read(top, &spec.right, &spec.path))
}

/// A colored stretch of a line.
pub type Span = crate::markdown::Span;

/// The syntax colors of both sides' lines.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Colors {
    pub old: Vec<Vec<Span>>,
    pub new: Vec<Vec<Span>>,
}

/// A file read, diffed and colored.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub body: Body,
    pub colors: Option<Colors>,
    /// What the change turned out to be: a file with no left side is new,
    /// one with no right side is deleted (whatever the list that named it said).
    pub status: Status,
}

/// What syntax coloring needs: the colorscheme's theme and the plain text color.
#[derive(Clone)]
pub struct Coloring {
    pub theme: Arc<Theme>,
    pub fg: Color32,
}

/// The file's language as its extension (syntect finds a grammar by it).
pub fn language(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(_, ext)| ext)
}

/// A file with more lines than this (on its longer side) is sent as its diff
/// first and its colors after: coloring takes far longer than diffing (half a
/// second a side for 9,000 lines), and the rows can be shown, uncolored, in the
/// meantime. A shorter one is sent whole, so that an ordinary file doesn't
/// flash uncolored.
pub const COLORS_LATER_LINES: usize = 1500;

/// Read and diff one file, without colors.
fn diff(top: &Path, spec: &FileSpec, max_lines: usize) -> Loaded {
    let (old, new) = read_sides(top, spec);
    let body = model::body(&old, &new, max_lines);
    let status = match (&old, &new) {
        _ if spec.status == Status::Renamed => Status::Renamed,
        (Content::Missing, Content::Missing) => spec.status,
        (Content::Missing, _) => Status::Added,
        (_, Content::Missing) => Status::Deleted,
        _ => Status::Modified,
    };
    Loaded { body, colors: None, status }
}

/// The syntax colors of a text diff's two sides, made in parallel (none for
/// another kind of body, or a language that is not known).
pub fn colors(path: &str, body: &Body, coloring: &Coloring) -> Option<Colors> {
    let Body::Text(d) = body else { return None };
    let spans = |lines: &[String]| crate::markdown::spans(&lines.join("\n"), language(path), &coloring.theme, coloring.fg);
    let (old, new) = std::thread::scope(|s| {
        let old = s.spawn(|| spans(&d.old));
        let new = spans(&d.new);
        (old.join().ok().flatten(), new)
    });
    Some(Colors { old: old?, new: new? })
}

/// Read, diff and color one file, in one piece (what the worker does for a
/// short one).
#[cfg(test)]
pub fn load(top: &Path, spec: &FileSpec, coloring: Option<&Coloring>, max_lines: usize) -> Loaded {
    let mut loaded = diff(top, spec, max_lines);
    loaded.colors = coloring.and_then(|c| colors(&spec.path, &loaded.body, c));
    loaded
}

/// The files of a commit against its first parent (a root commit: all new).
pub fn commit_specs(top: &Path, commit: &Commit) -> Vec<FileSpec> {
    use crate::decorations::GitStatus;
    let parent = commit.parents.first().cloned();
    crate::git::commit_files(top, commit)
        .into_iter()
        .map(|e| {
            let status = match e.status {
                GitStatus::Added | GitStatus::Untracked => Status::Added,
                GitStatus::Deleted => Status::Deleted,
                GitStatus::Renamed => Status::Renamed,
                _ => Status::Modified,
            };
            FileSpec {
                // (A renamed file's old version is in the parent too.)
                left: match &parent {
                    Some(p) if status != Status::Added => Side::Ref(p.clone()),
                    _ => Side::Missing,
                },
                right: if status == Status::Deleted { Side::Missing } else { Side::Ref(commit.oid.clone()) },
                old_path: e.orig,
                path: e.path,
                status,
            }
        })
        .collect()
}

/// What the view asks the worker to do.
pub enum Job {
    /// Read file `index` of the view. `serial` names this request: what comes
    /// back says it, and the view drops what answers an older one.
    Load { index: usize, serial: u64, top: PathBuf, spec: FileSpec, coloring: Option<Coloring>, max_lines: usize },
    /// List a commit's files.
    ListCommit { top: PathBuf, commit: Commit },
}

pub enum Done {
    /// The file's diff; its colors too, unless it is long (`COLORS_LATER_LINES`).
    Loaded { index: usize, serial: u64, loaded: Loaded },
    /// The colors of a file whose diff was sent without them (none: no grammar).
    Colors { index: usize, serial: u64, colors: Option<Colors> },
    Files { files: Vec<FileSpec> },
}

/// Run one job, sending what it makes (a load of a long file makes two).
fn run(job: Job, send: &dyn Fn(Done) -> bool) {
    match job {
        Job::Load { index, serial, top, spec, coloring, max_lines } => {
            let mut loaded = diff(&top, &spec, max_lines);
            let Some(c) = coloring else {
                send(Done::Loaded { index, serial, loaded });
                return;
            };
            let long = matches!(&loaded.body, Body::Text(d) if d.old.len().max(d.new.len()) > COLORS_LATER_LINES);
            if long {
                let body = loaded.body.clone();
                if !send(Done::Loaded { index, serial, loaded }) {
                    return;
                }
                send(Done::Colors { index, serial, colors: colors(&spec.path, &body, &c) });
            } else {
                loaded.colors = colors(&spec.path, &loaded.body, &c);
                send(Done::Loaded { index, serial, loaded });
            }
        }
        Job::ListCommit { top, commit } => {
            send(Done::Files { files: commit_specs(&top, &commit) });
        }
    }
}

/// The jobs waiting, and whether the worker is gone.
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    closed: bool,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;

/// A few threads: jobs run in the order sent, those marked urgent first.
pub struct Worker {
    queue: Shared,
    pub done: mpsc::Receiver<Done>,
}

impl Worker {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let queue: Shared = Default::default();
        let (done_tx, done_rx) = mpsc::channel();
        let wake = Arc::new(wake);
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4);
        for _ in 0..threads {
            let (queue, done_tx, wake) = (queue.clone(), done_tx.clone(), wake.clone());
            std::thread::Builder::new()
                .name("diff".into())
                .spawn(move || {
                    let send = |done: Done| {
                        let sent = done_tx.send(done).is_ok();
                        if sent {
                            wake();
                        }
                        sent
                    };
                    loop {
                        let job = {
                            let (lock, ready) = &*queue;
                            let mut q = lock.lock().unwrap();
                            loop {
                                if q.closed {
                                    return;
                                }
                                if let Some(job) = q.jobs.pop_front() {
                                    break job;
                                }
                                q = ready.wait(q).unwrap();
                            }
                        };
                        run(job, &send);
                    }
                })
                .expect("diff thread");
        }
        Self { queue, done: done_rx }
    }

    /// Queue `job`; `urgent` ones go before those waiting.
    pub fn send(&self, job: Job, urgent: bool) {
        let (lock, ready) = &*self.queue;
        let mut q = lock.lock().unwrap();
        if urgent {
            q.jobs.push_front(job);
        } else {
            q.jobs.push_back(job);
        }
        ready.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        if let Ok(mut q) = lock.lock() {
            q.closed = true;
            q.jobs.clear();
        }
        ready.notify_all();
    }
}

#[cfg(test)]
fn pairs(spans: &[Span]) -> Vec<std::ops::Range<usize>> {
    spans.iter().map(|s| s.start..s.end).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_view::testing::Repo;

    fn spec(left: Side, right: Side) -> FileSpec {
        FileSpec { path: "a.txt".into(), old_path: None, status: Status::Modified, left, right }
    }

    #[test]
    fn sides_come_from_a_ref_the_index_and_the_disk() {
        let repo = Repo::new("sides");
        repo.write("a.txt", "one\n");
        repo.commit("first");
        repo.write("a.txt", "two\n");
        repo.git(&["add", "a.txt"]);
        repo.write("a.txt", "three\n");
        let text = |s: &str| Content::Text(s.into());
        let read = |left, right| read_sides(&repo.dir, &spec(left, right));
        assert_eq!(read(Side::Ref("HEAD".into()), Side::Index), (text("one\n"), text("two\n")));
        assert_eq!(read(Side::Index, Side::Disk), (text("two\n"), text("three\n")));
        assert_eq!(read(Side::Ref("HEAD".into()), Side::Disk), (text("one\n"), text("three\n")));
        assert_eq!(read(Side::Missing, Side::Disk).0, Content::Missing);
        // A ref where the file isn't (a new file), and a deleted file on disk.
        let mut s = spec(Side::Ref("HEAD".into()), Side::Disk);
        s.path = "nope.txt".into();
        assert_eq!(read_sides(&repo.dir, &s), (Content::Missing, Content::Missing));
    }

    #[test]
    fn a_renamed_file_is_read_from_where_it_was() {
        let repo = Repo::new("rename");
        repo.write("old.txt", "a\nb\nc\n");
        repo.commit("first");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.write("new.txt", "a\nB\nc\n");
        let s = FileSpec {
            path: "new.txt".into(),
            old_path: Some("old.txt".into()),
            status: Status::Renamed,
            left: Side::Ref("HEAD".into()),
            right: Side::Disk,
        };
        assert_eq!(s.title(), "old.txt → new.txt");
        let loaded = load(&repo.dir, &s, None, model::MAX_LINES);
        let Body::Text(d) = loaded.body else { panic!() };
        assert_eq!((d.added, d.removed), (1, 1));
        assert_eq!(spec(Side::Disk, Side::Disk).title(), "a.txt");
    }

    #[test]
    fn binary_and_missing_files_load_as_such() {
        let repo = Repo::new("binary");
        repo.write("a.txt", "x\n");
        std::fs::write(repo.dir.join("img.bin"), [0u8, 1, 2, 3]).unwrap();
        repo.commit("first");
        let mut s = spec(Side::Ref("HEAD".into()), Side::Disk);
        s.path = "img.bin".into();
        assert_eq!(load(&repo.dir, &s, None, 100).body, Body::Binary);
        // Deleted on disk: all removed.
        std::fs::remove_file(repo.dir.join("a.txt")).unwrap();
        let Body::Text(d) = load(&repo.dir, &spec(Side::Ref("HEAD".into()), Side::Disk), None, 100).body else { panic!() };
        assert_eq!((d.added, d.removed), (0, 1));
    }

    #[test]
    fn a_big_file_is_left_alone() {
        let repo = Repo::new("big");
        repo.write("a.txt", &"x\n".repeat(50));
        let loaded = load(&repo.dir, &spec(Side::Missing, Side::Disk), None, 10);
        assert_eq!(loaded.body, Body::TooLarge { lines: 50 });
    }

    #[test]
    fn a_commits_files_are_against_its_first_parent() {
        let repo = Repo::new("commit");
        repo.write("keep.txt", "k\n");
        repo.write("gone.txt", "g\n");
        repo.write("old.txt", "one\ntwo\nthree\nfour\n");
        let root = repo.commit("first");
        repo.write("keep.txt", "k2\n");
        repo.git(&["rm", "-q", "gone.txt"]);
        repo.git(&["mv", "old.txt", "moved.txt"]);
        repo.write("added.txt", "new\n");
        let oid = repo.commit("second");
        let commit = |oid: &str, parents: Vec<String>| Commit {
            oid: oid.into(),
            parents,
            author: String::new(),
            when: String::new(),
            date: String::new(),
            subject: String::new(),
            body: String::new(),
            refs: vec![],
        };
        let mut specs = commit_specs(&repo.dir, &commit(&oid, vec![root.clone()]));
        specs.sort_by(|a, b| a.path.cmp(&b.path));
        let by = |p: &str| specs.iter().find(|s| s.path == p).unwrap().clone();
        assert_eq!(specs.len(), 4);
        assert_eq!((by("added.txt").status, by("added.txt").left), (Status::Added, Side::Missing));
        assert_eq!((by("gone.txt").status, by("gone.txt").right), (Status::Deleted, Side::Missing));
        assert_eq!(by("gone.txt").left, Side::Ref(root.clone()));
        let moved = by("moved.txt");
        assert_eq!((moved.status, moved.old_path.as_deref(), moved.left.clone()), (Status::Renamed, Some("old.txt"), Side::Ref(root.clone())));
        let Body::Text(d) = load(&repo.dir, &moved, None, 100).body else { panic!() };
        assert!(d.is_identical(), "a pure rename has no line changes");
        let Body::Text(d) = load(&repo.dir, &by("keep.txt"), None, 100).body else { panic!() };
        assert_eq!((d.added, d.removed), (1, 1));
        // The first commit: everything is new.
        let first = commit_specs(&repo.dir, &commit(&root, vec![]));
        assert!(first.iter().all(|s| s.status == Status::Added && s.left == Side::Missing));
    }

    #[test]
    fn the_worker_runs_urgent_jobs_first_and_reports_each() {
        let repo = Repo::new("worker");
        repo.write("a.txt", "x\n");
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(move || {
            let _ = tx.send(());
        });
        let job = |index| Job::Load { index, serial: index as u64, top: repo.dir.clone(), spec: spec(Side::Missing, Side::Disk), coloring: None, max_lines: 100 };
        worker.send(job(0), false);
        worker.send(job(1), false);
        let mut got = Vec::new();
        for _ in 0..2 {
            let Done::Loaded { index, loaded, .. } = worker.done.recv_timeout(std::time::Duration::from_secs(10)).unwrap() else { panic!() };
            assert!(matches!(loaded.body, Body::Text(_)));
            got.push(index);
        }
        got.sort();
        assert_eq!(got, [0, 1]);
        assert!(rx.try_iter().count() >= 1, "woken");
    }

    fn coloring() -> Coloring {
        let theme = Arc::new(syntect::highlighting::ThemeSet::load_defaults().themes["base16-ocean.dark"].clone());
        Coloring { theme, fg: Color32::WHITE }
    }

    /// What the worker sends for one `a.rs` of `lines` lines, new on disk.
    fn answers(name: &str, lines: usize, coloring: Option<Coloring>) -> Vec<Done> {
        let repo = Repo::new(name);
        repo.write("a.rs", &"let x = 1;\n".repeat(lines));
        let worker = Worker::new(|| {});
        let colored = coloring.is_some();
        let mut s = spec(Side::Missing, Side::Disk);
        s.path = "a.rs".into();
        worker.send(Job::Load { index: 3, serial: 7, top: repo.dir.clone(), spec: s, coloring, max_lines: 100_000 }, true);
        let first = worker.done.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        let mut got = vec![first];
        if colored && matches!(&got[0], Done::Loaded { loaded, .. } if loaded.colors.is_none()) {
            // (Long enough for a debug build on a slow runner to color it.)
            if let Ok(more) = worker.done.recv_timeout(std::time::Duration::from_secs(20)) {
                got.push(more);
            }
        }
        got
    }

    #[test]
    fn a_long_files_colors_come_in_a_message_after_its_diff() {
        let got = answers("long", COLORS_LATER_LINES + 1, Some(coloring()));
        assert_eq!(got.len(), 2);
        let Done::Loaded { index: 3, serial: 7, loaded } = &got[0] else { panic!("the diff first") };
        assert!(matches!(loaded.body, Body::Text(_)));
        assert!(loaded.colors.is_none());
        let Done::Colors { index: 3, serial: 7, colors: Some(colors) } = &got[1] else { panic!("then the colors") };
        assert_eq!(colors.new.len(), COLORS_LATER_LINES + 1);
        assert!(colors.old.is_empty());
    }

    #[test]
    fn a_short_file_has_its_colors_in_the_one_message() {
        let got = answers("short", COLORS_LATER_LINES, Some(coloring()));
        assert_eq!(got.len(), 1);
        let Done::Loaded { loaded, .. } = &got[0] else { panic!() };
        assert_eq!(loaded.colors.as_ref().map(|c| c.new.len()), Some(COLORS_LATER_LINES));
        // Without a theme a long file has nothing to wait for either.
        let got = answers("long-plain", COLORS_LATER_LINES + 1, None);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn extensions_name_the_language() {
        assert_eq!(language("src/main.rs"), "rs");
        assert_eq!(language("Makefile"), "Makefile");
        assert_eq!(language("a/b.test.py"), "py");
    }

    #[test]
    fn a_loaded_file_has_syntax_colors_when_there_is_a_theme() {
        let repo = Repo::new("colors");
        repo.write("a.rs", "fn main() {\n    let x = 1;\n}\n");
        let theme = Arc::new(syntect::highlighting::ThemeSet::load_defaults().themes["base16-ocean.dark"].clone());
        let coloring = Coloring { theme, fg: Color32::WHITE };
        let mut s = spec(Side::Missing, Side::Disk);
        s.path = "a.rs".into();
        let loaded = load(&repo.dir, &s, Some(&coloring), 100);
        let colors = loaded.colors.expect("rust is known");
        assert_eq!(colors.new.len(), 3);
        // "fn" and "main" don't share a color span.
        assert!(colors.new[0].len() >= 3, "{:?}", pairs(&colors.new[0]));
        assert!(load(&repo.dir, &s, None, 100).colors.is_none());
        s.path = "a.unknownext".into();
        repo.write("a.unknownext", "x\n");
        assert!(load(&repo.dir, &s, Some(&coloring), 100).colors.is_none());
    }
}
