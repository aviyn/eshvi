//! Ranking the picker's rows: nucleo's fuzzy matching (smart case; spaces
//! make several words, and `!`, `^`, `$`, `'` work as in fzf), on a thread of
//! its own that drops a query when a newer one arrives.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// What a buffer's row gets over the rest: enough to win between similar
/// matches, not to beat a clearly better one (nucleo scores a match's letters
/// in sixteens: one more matched letter, or the name instead of a folder,
/// is worth more than this).
pub const BUFFER_BONUS: i32 = 8;
/// A recent file's.
pub const RECENT_BONUS: i32 = 4;
/// A match inside the file's name rather than its folders.
pub const NAME_BONUS: i32 = 20;
/// How many of the best matches `rank` gives the matched positions of; the
/// view finds the others' when they are drawn (`highlight`).
pub const INDICES_FOR: usize = 64;

/// A row as ranking sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct RankItem {
    pub text: String,
    /// Added to the match's score.
    pub bonus: i32,
    /// A path: matched with path bonuses, the name preferred.
    pub path: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ranked {
    /// The row's place in the items.
    pub index: usize,
    pub score: i32,
    /// The characters matched (positions in `text`), for the first
    /// `INDICES_FOR` of the result; empty after that, and for a query with
    /// only negations.
    pub indices: Vec<u32>,
}

pub fn pattern(query: &str) -> Pattern {
    Pattern::parse(query, CaseMatching::Smart, Normalization::Smart)
}

/// The name of a path-like text: after its last "/".
fn name_of(text: &str) -> &str {
    text.rsplit('/').next().unwrap_or(text)
}

/// A matcher for paths or for plain text.
pub fn matcher(path: bool) -> Matcher {
    Matcher::new(if path { Config::DEFAULT.match_paths() } else { Config::DEFAULT })
}

/// The positions in `text` that `query` matches (sorted, once each).
pub fn highlight(pattern: &Pattern, matcher: &mut Matcher, text: &str) -> Vec<u32> {
    let mut buf = Vec::new();
    let mut indices = Vec::new();
    if pattern.indices(Utf32Str::new(text, &mut buf), matcher, &mut indices).is_none() {
        return Vec::new();
    }
    indices.sort_unstable();
    indices.dedup();
    indices
}

/// The rows matching `query`, best first: by score (with the row's bonus),
/// then shorter text, then source order. An empty query matches all, in
/// order. `cancel` is asked now and then: once it says so, what is left is
/// dropped and the result is empty.
pub fn rank(items: &[RankItem], query: &str, cancel: &dyn Fn() -> bool) -> Vec<Ranked> {
    let pattern = pattern(query);
    let (mut paths, mut plain) = (matcher(true), matcher(false));
    let mut buf = Vec::new();
    let mut out = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index % 512 == 0 && cancel() {
            return Vec::new();
        }
        let matcher = if item.path { &mut paths } else { &mut plain };
        let Some(score) = pattern.score(Utf32Str::new(&item.text, &mut buf), matcher) else { continue };
        let mut score = score as i32 + item.bonus;
        // A path matched by its name alone (not by letters spread over its folders).
        if item.path && !pattern.atoms.is_empty() {
            // (A name with no folder is all name.)
            let name = name_of(&item.text);
            if pattern.score(Utf32Str::new(name, &mut buf), matcher).is_some() {
                score += NAME_BONUS;
            }
        }
        out.push(Ranked { index, score, indices: Vec::new() });
    }
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| items[a.index].text.chars().count().cmp(&items[b.index].text.chars().count()))
            .then_with(|| a.index.cmp(&b.index))
    });
    for r in out.iter_mut().take(INDICES_FOR) {
        let item = &items[r.index];
        r.indices = highlight(&pattern, if item.path { &mut paths } else { &mut plain }, &item.text);
    }
    out
}

/// What the ranking thread is told.
enum Msg {
    /// More rows (the walk's), appended.
    Add(Vec<RankItem>),
    /// Rank for this query. The number says which query it is.
    Query(u64, String),
}

/// What it answers.
pub struct Done {
    /// The query's number.
    pub version: u64,
    /// Every row, in order (the query was empty): not listed.
    pub matches: Option<Vec<Ranked>>,
}

/// The ranking thread: the rows are copied to it as they arrive, and it
/// answers every query (and every batch of rows after one) with a `Done`. A
/// query that a newer one has overtaken is dropped half way.
pub struct RankWorker {
    tx: Sender<Msg>,
    pub results: Receiver<Done>,
    latest: Arc<AtomicU64>,
}

impl RankWorker {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let (res_tx, results) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(0));
        let seen = latest.clone();
        std::thread::Builder::new()
            .name("picker-rank".into())
            .spawn(move || {
                let mut items: Vec<RankItem> = Vec::new();
                let mut query = (0, String::new());
                fn apply(msg: Msg, items: &mut Vec<RankItem>, query: &mut (u64, String)) {
                    match msg {
                        Msg::Add(more) => items.extend(more),
                        Msg::Query(version, text) => *query = (version, text),
                    }
                }
                while let Ok(msg) = rx.recv() {
                    apply(msg, &mut items, &mut query);
                    // Everything waiting counts before the work starts.
                    while let Ok(msg) = rx.try_recv() {
                        apply(msg, &mut items, &mut query);
                    }
                    let (version, text) = query.clone();
                    let matches = if text.trim().is_empty() {
                        None
                    } else {
                        let ranked = rank(&items, &text, &|| seen.load(Ordering::Relaxed) != version);
                        // (Overtaken: the newer query's answer is coming.)
                        if seen.load(Ordering::Relaxed) != version {
                            continue;
                        }
                        Some(ranked)
                    };
                    if res_tx.send(Done { version, matches }).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .expect("picker ranking thread");
        Self { tx, results, latest }
    }

    pub fn add(&self, items: Vec<RankItem>) {
        let _ = self.tx.send(Msg::Add(items));
    }

    pub fn query(&self, version: u64, text: &str) {
        self.latest.store(version, Ordering::Relaxed);
        let _ = self.tx.send(Msg::Query(version, text.to_owned()));
    }
}
