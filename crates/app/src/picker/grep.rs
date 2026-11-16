//! The picker's grep: ripgrep's libraries (`grep-searcher`, `grep-regex`) run
//! in this process over `ignore`'s parallel walk, so no `rg` is needed. A
//! search runs on threads of its own and sends its matches in batches; dropping
//! it stops it (every file, and every match, checks).
//!
//! The query is a regular expression, or text when it doesn't compile (or when
//! asked for); smart case. Files `.gitignore` and the like leave out, hidden
//! files, binary files (a NUL byte) and files over 2 MB are skipped.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkState;

/// A search stops with this many matches.
pub const MAX_MATCHES: usize = 10_000;
/// A bigger file isn't searched.
pub const MAX_FILE: u64 = 2 * 1024 * 1024;
/// A line is shown cut to this many bytes around its first match.
pub const CUT: usize = 400;
/// Matches of one line kept (a minified file's line holds thousands).
const RANGES: usize = 64;
/// A batch goes out when it has this many matches or has waited this long.
const BATCH: usize = 200;
const BATCH_TIME: Duration = Duration::from_millis(30);

/// One matching line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hit {
    /// The file, relative to the folder (the full path for a file outside it).
    pub path: String,
    /// The line, from 1.
    pub lnum: usize,
    /// The first match's byte in the line, from 1.
    pub col: usize,
    /// The line to show: trimmed, and cut around the first match.
    pub text: String,
    /// The matches in `text` (bytes).
    pub ranges: Vec<Range<usize>>,
    /// The matches in the whole line (bytes), for the preview.
    pub line_ranges: Vec<Range<usize>>,
}

pub enum Msg {
    Hits(Vec<Hit>),
    /// Nothing more is coming; `capped` when the limit stopped it.
    Done { capped: bool },
}

/// Where to search.
#[derive(Clone, Debug)]
pub enum Scope {
    /// The files under the folder.
    Tree(PathBuf),
    /// The files under the folder with one of these extensions (`sv`, no dot).
    Extensions { cwd: PathBuf, extensions: Vec<String> },
    /// These files only (the listed buffers'), shown relative to the folder.
    Files { cwd: PathBuf, files: Vec<PathBuf> },
}

impl Scope {
    fn cwd(&self) -> &Path {
        match self {
            Self::Tree(cwd) | Self::Extensions { cwd, .. } | Self::Files { cwd, .. } => cwd,
        }
    }
}

/// A running search. Dropping it stops it.
pub struct Grep {
    pub found: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    /// Files searched so far.
    #[cfg(test)]
    pub searched: Arc<AtomicUsize>,
}

impl Drop for Grep {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The matcher for `query`: a smart-case regex, or, when it isn't one (or
/// `literal` says so), the text itself. None for an empty query.
pub fn matcher(query: &str, literal: bool) -> Option<RegexMatcher> {
    if query.is_empty() {
        return None;
    }
    let build = |fixed: bool| RegexMatcherBuilder::new().case_smart(true).fixed_strings(fixed).build(query);
    if literal {
        return build(true).ok();
    }
    build(false).or_else(|_| build(true)).ok()
}

/// The hit for line `line` of `path`, if the matcher finds anything in it.
pub fn hit(path: &str, lnum: usize, line: &str, matcher: &RegexMatcher) -> Hit {
    let line = line.trim_end_matches(['\n', '\r']);
    let mut line_ranges = Vec::new();
    let _ = matcher.find_iter(line.as_bytes(), |m| {
        line_ranges.push(m.start()..m.end());
        line_ranges.len() < RANGES
    });
    let col = line_ranges.first().map_or(1, |r| r.start + 1);
    // What is shown: without the indent, and cut around the first match.
    let lead = line.len() - line.trim_start().len();
    let shown = line[lead..].trim_end();
    let ranges: Vec<Range<usize>> = line_ranges
        .iter()
        .filter_map(|r| {
            let (s, e) = (r.start.saturating_sub(lead), r.end.saturating_sub(lead).min(shown.len()));
            (s < e).then_some(s..e)
        })
        .collect();
    let (text, ranges) = cut_window(shown, &ranges, CUT, |c| c.len_utf8());
    Hit { path: path.to_owned(), lnum, col, text, ranges, line_ranges }
}

/// `text` cut to a window of `budget` (each character costs `cost`) that
/// has its first range in it, with "…" where it is cut, and the ranges that
/// are in the window, moved to where they are in the new text.
pub fn cut_window(text: &str, ranges: &[Range<usize>], budget: usize, cost: impl Fn(char) -> usize) -> (String, Vec<Range<usize>>) {
    let total: usize = text.chars().map(&cost).sum();
    if total <= budget {
        return (text.to_owned(), ranges.to_vec());
    }
    let anchor = ranges.first().map_or(0, |r| r.start);
    // A little of what comes before the match, as far as the budget allows.
    let mut start = anchor;
    let mut before = 0;
    for (i, c) in text[..anchor].char_indices().rev() {
        if before + cost(c) > budget / 6 {
            break;
        }
        before += cost(c);
        start = i;
    }
    let mut end = start;
    let mut used = 0;
    for (i, c) in text[start..].char_indices() {
        if used + cost(c) > budget {
            break;
        }
        used += cost(c);
        end = start + i + c.len_utf8();
    }
    let (lead, tail) = (if start > 0 { "…" } else { "" }, if end < text.len() { "…" } else { "" });
    let shown = format!("{lead}{}{tail}", &text[start..end]);
    let moved = ranges
        .iter()
        .filter_map(|r| {
            let (s, e) = (r.start.max(start), r.end.min(end));
            (s < e).then(|| (s - start + lead.len())..(e - start + lead.len()))
        })
        .collect();
    (shown, moved)
}

/// What the workers of one search share.
struct Shared {
    out: Sender<Msg>,
    wake: Box<dyn Fn() + Send + Sync>,
    cancel: Arc<AtomicBool>,
    searched: Arc<AtomicUsize>,
    /// Matches sent, counting up to the limit.
    count: AtomicUsize,
    matcher: RegexMatcher,
    cwd: PathBuf,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.count.load(Ordering::Relaxed) >= MAX_MATCHES
    }
}

/// One thread's share: its files' matches, sent in batches.
struct Worker {
    shared: Arc<Shared>,
    batch: Vec<Hit>,
    since: Instant,
}

impl Worker {
    fn new(shared: Arc<Shared>) -> Self {
        Self { shared, batch: Vec::new(), since: Instant::now() }
    }

    /// Send what is collected. Whether anyone still listens.
    fn flush(&mut self) -> bool {
        self.since = Instant::now();
        if self.batch.is_empty() {
            return true;
        }
        // (The limit is counted as batches go out: a batch that crosses it is cut.)
        let room = MAX_MATCHES.saturating_sub(self.shared.count.fetch_add(self.batch.len(), Ordering::Relaxed));
        self.batch.truncate(room);
        if self.batch.is_empty() {
            return true;
        }
        let sent = self.shared.out.send(Msg::Hits(std::mem::take(&mut self.batch))).is_ok();
        (self.shared.wake)();
        sent
    }

    /// Search one file.
    fn file(&mut self, path: &Path) {
        let shared = self.shared.clone();
        shared.searched.fetch_add(1, Ordering::Relaxed);
        let Ok(meta) = std::fs::metadata(path) else { return };
        if !meta.is_file() || meta.len() > MAX_FILE {
            return;
        }
        let Ok(bytes) = std::fs::read(path) else { return };
        // Binary: a NUL byte anywhere.
        if bytes.contains(&0) {
            return;
        }
        let rel = path.strip_prefix(&shared.cwd).unwrap_or(path).to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
        let batch = &mut self.batch;
        let since = &mut self.since;
        let mut full = false;
        let _ = searcher.search_slice(
            &shared.matcher,
            &bytes,
            Lossy(|lnum, line| {
                batch.push(hit(&rel, lnum as usize, line, &shared.matcher));
                if batch.len() >= BATCH || since.elapsed() >= BATCH_TIME {
                    full = true;
                }
                Ok(!shared.stopped())
            }),
        );
        if full || (!self.batch.is_empty() && self.since.elapsed() >= BATCH_TIME) {
            self.flush();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.flush();
    }
}

impl Grep {
    /// Start searching `scope` for `query` (None when the query is empty).
    pub fn start(scope: Scope, query: &str, literal: bool, wake: impl Fn() + Send + Sync + 'static) -> Option<Self> {
        let matcher = matcher(query, literal)?;
        let (out, found) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let searched = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            out,
            wake: Box::new(wake),
            cancel: cancel.clone(),
            searched: searched.clone(),
            count: AtomicUsize::new(0),
            matcher,
            cwd: scope.cwd().to_path_buf(),
        });
        std::thread::Builder::new()
            .name("picker-grep".into())
            .spawn(move || {
                run(&scope, &shared);
                let capped = shared.count.load(Ordering::Relaxed) >= MAX_MATCHES;
                let _ = shared.out.send(Msg::Done { capped });
                (shared.wake)();
            })
            .expect("picker grep thread");
        Some(Self { found, cancel, #[cfg(test)] searched })
    }
}

fn run(scope: &Scope, shared: &Arc<Shared>) {
    match scope {
        Scope::Tree(cwd) => walk(cwd, None, shared),
        Scope::Extensions { cwd, extensions } => walk(cwd, Some(extensions), shared),
        Scope::Files { files, .. } => {
            let mut worker = Worker::new(shared.clone());
            for file in files {
                if shared.stopped() {
                    break;
                }
                worker.file(file);
            }
        }
    }
}

/// Search the files under `cwd`, those with one of `extensions` if it is given.
fn walk(cwd: &Path, extensions: Option<&[String]>, shared: &Arc<Shared>) {
    let wanted = |path: &Path| {
        extensions.is_none_or(|list| path.extension().is_some_and(|e| list.iter().any(|x| e == x.as_str())))
    };
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8);
    let walker = super::walk::builder(cwd).threads(threads).build_parallel();
    walker.run(|| {
        let mut worker = Worker::new(shared.clone());
        Box::new(move |entry| {
            if worker.shared.stopped() {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            let file = entry.file_type().is_some_and(|k| k.is_file() || (k.is_symlink() && entry.path().is_file()));
            if file && wanted(entry.path()) {
                worker.file(entry.path());
            }
            WalkState::Continue
        })
    });
}

/// Insert `incoming` (sorted) into `rows` (sorted), by path then line. Where
/// each old row went: `moved[old] = new`.
pub fn merge_sorted<T>(rows: Vec<T>, incoming: Vec<T>, key: impl Fn(&T) -> (&str, usize, usize)) -> (Vec<T>, Vec<usize>) {
    let mut merged = Vec::with_capacity(rows.len() + incoming.len());
    let mut moved = Vec::with_capacity(rows.len());
    let mut old = rows.into_iter().peekable();
    let mut new = incoming.into_iter().peekable();
    loop {
        let take_old = match (old.peek(), new.peek()) {
            (Some(o), Some(n)) => key(o) <= key(n),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_old {
            moved.push(merged.len());
            merged.extend(old.next());
        } else {
            merged.extend(new.next());
        }
    }
    (merged, moved)
}
