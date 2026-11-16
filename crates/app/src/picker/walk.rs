//! The picker's files: a walk of the working directory on a thread of its
//! own (`ignore`: `.gitignore` and the like are respected, hidden files are
//! skipped as `fd` does, symlinks are not followed), or `git ls-files`.
//! Paths arrive in batches, relative to the folder, as the walk goes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Found {
    /// Paths relative to the folder, with "/" between the parts.
    Files(Vec<String>),
    /// Nothing more is coming.
    Done,
}

/// A batch goes out when it has this many paths or has been collecting this long.
const BATCH: usize = 2000;
const BATCH_TIME: Duration = Duration::from_millis(40);

pub struct Walk {
    pub found: Receiver<Found>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Walk {
    /// The picker closed: the walk stops where it is.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Collects paths and hands them on in batches.
struct Batches<'a> {
    out: &'a Sender<Found>,
    wake: &'a dyn Fn(),
    batch: Vec<String>,
    since: Instant,
}

impl Batches<'_> {
    fn push(&mut self, path: String) -> bool {
        self.batch.push(path);
        if self.batch.len() >= BATCH || self.since.elapsed() >= BATCH_TIME {
            return self.flush();
        }
        true
    }

    /// Whether anyone still listens.
    fn flush(&mut self) -> bool {
        self.since = Instant::now();
        if self.batch.is_empty() {
            return true;
        }
        let sent = self.out.send(Found::Files(std::mem::take(&mut self.batch))).is_ok();
        (self.wake)();
        sent
    }
}

impl Walk {
    /// The files under `cwd`.
    pub fn files(cwd: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        Self::start(wake, move |out, wake, cancel| walk(&cwd, out, wake, cancel))
    }

    /// The files git tracks under `cwd`; the walk instead when git has no
    /// answer (not a repository).
    pub fn git(cwd: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        Self::start(wake, move |out, wake, cancel| {
            if !tracked(&cwd, out, wake, cancel) {
                walk(&cwd, out, wake, cancel);
            }
        })
    }

    /// The flag that stops the walk (set when this is dropped).
    #[cfg(test)]
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    fn start(
        wake: impl Fn() + Send + 'static,
        run: impl FnOnce(&Sender<Found>, &dyn Fn(), &AtomicBool) + Send + 'static,
    ) -> Self {
        let (tx, found) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::Builder::new()
            .name("picker-walk".into())
            .spawn(move || {
                run(&tx, &wake, &stop);
                let _ = tx.send(Found::Done);
                wake();
            })
            .expect("picker walk thread");
        Self { found, cancel }
    }
}

/// The walk of a folder, for the file list and for grep alike: `.gitignore`
/// and the like respected, hidden files skipped, links not followed.
pub fn builder(cwd: &Path) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(cwd);
    builder
        .follow_links(false)
        // (A folder with a .gitignore and no .git: the user's own checkout in a tarball, a test.)
        .require_git(false);
    builder
}

fn walk(cwd: &Path, out: &Sender<Found>, wake: &dyn Fn(), cancel: &AtomicBool) {
    let mut batches = Batches { out, wake, batch: Vec::new(), since: Instant::now() };
    let walker = builder(cwd).sort_by_file_name(|a, b| a.cmp(b)).build();
    for entry in walker.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let kind = entry.file_type();
        // A link to a file is a file; a link to a folder is not followed.
        let file = kind.is_some_and(|k| k.is_file() || (k.is_symlink() && entry.path().is_file()));
        if !file {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(cwd) else { continue };
        if !batches.push(rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/")) {
            return;
        }
    }
    batches.flush();
}

/// `git ls-files` in `cwd`: whether git answered.
fn tracked(cwd: &Path, out: &Sender<Found>, wake: &dyn Fn(), cancel: &AtomicBool) -> bool {
    let Ok(output) = std::process::Command::new("git").arg("ls-files").arg("-z").current_dir(cwd).output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let mut batches = Batches { out, wake, batch: Vec::new(), since: Instant::now() };
    for path in output.stdout.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        let path = String::from_utf8_lossy(path).into_owned();
        // (Tracked, but deleted from the folder: nothing to open.)
        if cwd.join(&path).is_file() && !batches.push(path) {
            return true;
        }
    }
    batches.flush();
    true
}
