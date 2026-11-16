//! The language server: answers `workspace/executeCommand` from the elaborated design, and builds
//! that design in the background, again when a file of it is saved.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use serde_json::{json, Value};

use crate::elaborate::{Limits, DEFAULT_TIMEOUT};
use crate::BuildChoice;
use crate::graph::Graph;
use crate::hier::{self, Hier};
use crate::source::{canonical, uri_path};
use crate::trace;

/// How long saves are collected before the design is elaborated again.
const DEBOUNCE: Duration = Duration::from_millis(300);

const RELOAD: &str = "svtrace.reload";
const STATUS: &str = "svtrace.status";
const LIST_SIGNALS: &str = "svtrace.listSignals";
const USE_BUILD: &str = "svtrace.useBuild";

/// Source files a save of which can change the design, other than the ones in the dump (an include
/// that only defines macros doesn't show up in it).
const SOURCE_EXTENSIONS: [&str; 5] = ["f", "sv", "svh", "v", "vh"];

/// The environment variable that sets how many seconds slang may run (default 120): a workspace's
/// `server.json` is shared with slang-server, which has no such key, so this is the user's to set.
pub const TIMEOUT_VARIABLE: &str = "SVTRACE_SLANG_TIMEOUT";

/// How long a slang run may take: `SVTRACE_SLANG_TIMEOUT` (seconds), else two minutes.
pub fn slang_timeout() -> Duration {
    std::env::var(TIMEOUT_VARIABLE)
        .ok()
        .and_then(|seconds| seconds.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map_or(DEFAULT_TIMEOUT, Duration::from_secs_f64)
}

/// Serves LSP on `connection` until the client exits. `slang` is the slang to run.
pub fn serve(connection: Connection, slang: PathBuf) -> Result<()> {
    serve_with(connection, slang, slang_timeout())
}

/// [`serve`] with the time slang may run before it is killed.
pub fn serve_with(connection: Connection, slang: PathBuf, timeout: Duration) -> Result<()> {
    let (id, params) = connection.initialize_start()?;
    let root = workspace_root(&params).ok_or_else(|| anyhow!("the client sent no workspace folder"))?;
    let capabilities = json!({
        "capabilities": {
            // (`svtrace.step` and `svtrace.endpoints` take `paths`, the instances of a module, and merge them.)
            "experimental": { "svtrace": { "paths": true } },
            "textDocumentSync": { "openClose": false, "change": 0, "save": { "includeText": false } },
            "executeCommandProvider": {
                "commands": [
                    trace::COMMANDS[0], trace::COMMANDS[1], trace::COMMANDS[2], trace::COMMANDS[3], trace::COMMANDS[4], trace::COMMANDS[5],
                    RELOAD, STATUS, LIST_SIGNALS, USE_BUILD,
                    hier::GET_SCOPE, hier::GET_SCOPES,
                ],
            },
        },
        "serverInfo": { "name": "svtrace", "version": env!("CARGO_PKG_VERSION") },
    });
    connection.initialize_finish(id, capabilities)?;
    log::info!("svtrace serving {}", root.display());
    // Files that change without a save in this editor (a checkout, a generated build file, another
    // editor): the client is asked to watch them, as slang-server asks it to.
    if params["capabilities"]["workspace"]["didChangeWatchedFiles"]["dynamicRegistration"] == json!(true) {
        let _ = connection.sender.send(Message::Request(Request::new(WATCH_ID.to_string().into(), "client/registerCapability".to_string(), watch_registration(&root))));
    }

    let shared = Arc::new(Shared::default());
    shared.state.lock().unwrap().status.state = "building";
    let (jobs, queue) = mpsc::channel();
    let (worker_done, worker_ended) = mpsc::channel::<()>();
    {
        let (shared, root, client) = (shared.clone(), root.clone(), connection.sender.clone());
        thread::spawn(move || {
            worker(&queue, &shared, &slang, &root, timeout, &|message| drop(client.send(message)));
            drop(worker_done);
        });
    }
    shared.state.lock().unwrap().configured_build = configured_build(&root);
    let core = Arc::new(Core { canonical_root: canonical(&root), root, shared, jobs, timeout });
    // (`initialize_finish` has taken the client's `initialized`, so that is the first build's cue.)
    core.rebuild(false, None);
    let result = run(&connection, &core);
    // Whatever ended the session, slang must not outlive it: stop the build and wait for its kill.
    core.stop();
    let _ = worker_ended.recv_timeout(Duration::from_secs(3));
    result
}

/// Reads messages until the client is done. Requests are answered on threads of their own, so that
/// a question waiting for a build holds up neither the others, nor `shutdown` and `exit`.
fn run(connection: &Connection, core: &Arc<Core>) -> Result<()> {
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if request.method == "shutdown" {
                    core.stop();
                }
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                let (core, sender) = (core.clone(), connection.sender.clone());
                thread::spawn(move || {
                    let response = core.respond(request);
                    let _ = sender.send(Message::Response(response));
                });
            }
            Message::Notification(notification) if notification.method == "exit" => return Ok(()),
            Message::Notification(notification) => core.notified(&notification),
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// The id of the request that asks the client to watch files.
const WATCH_ID: &str = "svtrace-file-watcher";

/// What the client is asked to watch: the workspace's configuration, build files and sources (the
/// patterns slang-server registers).
fn watch_registration(root: &Path) -> Value {
    let base = crate::source::file_uri(root);
    let patterns = [".slang/server.json", ".slang/local/server.json"].into_iter().map(str::to_string);
    let patterns = patterns.chain(SOURCE_EXTENSIONS.iter().map(|extension| format!("**/*.{extension}")));
    let watchers: Vec<Value> = patterns.map(|pattern| json!({ "globPattern": { "baseUri": base, "pattern": pattern } })).collect();
    json!({ "registrations": [{ "id": WATCH_ID, "method": "workspace/didChangeWatchedFiles", "registerOptions": { "watchers": watchers } }] })
}

/// The workspace folder from the `initialize` request: `rootUri`, else the first workspace
/// folder, else `rootPath`.
fn workspace_root(params: &Value) -> Option<PathBuf> {
    let uri = params["rootUri"]
        .as_str()
        .or_else(|| params["workspaceFolders"][0]["uri"].as_str())
        .and_then(uri_path);
    uri.or_else(|| params["rootPath"].as_str().map(PathBuf::from))
}

/// What the worker reports and the requests read.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when the first build is over, successful or not, and when the server stops.
    first_build: Condvar,
    /// Set when the client has shut the server down: builds and the questions waiting for them end.
    cancelled: AtomicBool,
}

#[derive(Default)]
struct State {
    /// The last design that was built: it keeps answering while the next one is built.
    graph: Option<Arc<Graph>>,
    /// Its hierarchy, for `slang.getScope` and `slang.getScopes`.
    hier: Option<Arc<Hier>>,
    /// The files whose save rebuilds the design.
    watched: HashSet<PathBuf>,
    status: Status,
    first_build_done: bool,
    /// The build file the design comes from: the configured one unless the client chose (`svtrace.useBuild`).
    choice: BuildChoice,
    /// The next build is of a build file just chosen: if it fails, the old design is of another build
    /// and is dropped rather than kept to answer.
    fresh_choice: bool,
    /// No design because slang-server is in explore mode (`useBuild { file = null, explore = true }`), not
    /// on a top file: a change of the configuration takes slang-server back to the configured `build`
    /// (its `loadConfig`), and svtrace with it; a top file it keeps.
    explore: bool,
    /// The `build` of the configuration as it was last read (resolved), to tell a chosen build that is
    /// the configured one from one that is not when the configuration changes.
    configured_build: Option<PathBuf>,
    /// Whether the last build that ended gave a design.
    last_build_ok: bool,
}

#[derive(Clone, Default)]
struct Status {
    state: &'static str,
    built_at: Option<u128>,
    duration_ms: Option<u128>,
    slang_exit: Option<i32>,
    stderr_tail: Vec<String>,
    signals: usize,
    error: Option<String>,
}

impl BuildChoice {
    fn json(&self) -> Value {
        match self {
            BuildChoice::Config => json!({ "choice": "config" }),
            BuildChoice::File(file) => json!({ "choice": "file", "file": file }),
            BuildChoice::None => json!({ "choice": "none" }),
        }
    }
}

impl Status {
    fn json(&self) -> Value {
        json!({
            "state": self.state,
            "builtAt": self.built_at,
            "durationMs": self.duration_ms,
            "slangExit": self.slang_exit,
            "stderrTail": self.stderr_tail,
            "signals": self.signals,
            "error": self.error,
        })
    }
}

struct Job {
    /// Wait for more saves first, so a burst of them builds once.
    debounce: bool,
    /// Told when the build this job is part of is over.
    reply: Option<Sender<()>>,
}

fn worker(queue: &Receiver<Job>, shared: &Shared, slang: &Path, root: &Path, timeout: Duration, client: &dyn Fn(Message)) {
    let cancelled = || shared.cancelled.load(Ordering::Relaxed);
    while !cancelled() {
        let first = match queue.recv_timeout(Duration::from_millis(100)) {
            Ok(first) => first,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let mut debounce = first.debounce;
        let mut replies: Vec<Sender<()>> = first.reply.into_iter().collect();
        while debounce && !cancelled() {
            match queue.recv_timeout(DEBOUNCE) {
                Ok(job) => {
                    replies.extend(job.reply);
                    debounce = job.debounce;
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if cancelled() {
            return;
        }
        rebuild(shared, slang, root, timeout, client);
        for reply in replies {
            let _ = reply.send(());
        }
    }
}

fn rebuild(shared: &Shared, slang: &Path, root: &Path, timeout: Duration, client: &dyn Fn(Message)) {
    let (was_ready, choice) = {
        let mut state = shared.state.lock().unwrap();
        let was_ready = state.last_build_ok;
        state.status.state = "building";
        (was_ready, state.choice.clone())
    };
    let outcome = crate::build_chosen(slang, root, &Limits { timeout, cancel: Some(&shared.cancelled) }, &choice);
    let mut state = shared.state.lock().unwrap();
    match outcome {
        Ok(built) => {
            log::info!("built {} signals in {} ms", built.graph.signals().count(), built.duration.as_millis());
            state.status = Status {
                state: "ready",
                built_at: now_ms(),
                duration_ms: Some(built.duration.as_millis()),
                slang_exit: built.slang_exit,
                stderr_tail: built.stderr_tail,
                signals: built.graph.signals().count(),
                error: None,
            };
            state.watched = built.graph.sources.paths().into_iter().collect();
            state.graph = Some(Arc::new(built.graph));
            state.hier = Some(Arc::new(built.hier));
            state.last_build_ok = true;
        }
        Err(error) => {
            log::error!("build failed: {error:#}");
            // The previous design, if any, keeps answering: the user is told so, once, when a build
            // fails after one that worked (not for each save while it stays broken, nor when the server
            // is stopping). With no design at all, every question gets the error instead.
            state.status.state = "failed";
            state.status.error = Some(format!("{error:#}"));
            state.last_build_ok = false;
            let dropped = choice == BuildChoice::None || state.fresh_choice;
            if dropped {
                // No design, or a build the old design isn't of: it is gone, as the server's is.
                state.graph = None;
                state.hier = None;
                state.watched.clear();
            }
            if !dropped && was_ready && !shared.cancelled.load(Ordering::Relaxed) {
                let first_line = format!("{error:#}").lines().next().unwrap_or_default().to_string();
                let message = format!("svtrace: the design could not be rebuilt; answers come from the last one that could. {first_line}");
                let params = json!({ "type": 2, "message": message });
                client(Message::Notification(Notification::new("window/showMessage".to_string(), params)));
            }
        }
    }
    state.fresh_choice = false;
    state.first_build_done = true;
    shared.first_build.notify_all();
}

fn now_ms() -> Option<u128> {
    Some(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_millis())
}

/// The configured `build`, resolved and without `.` and `..` (`None`: none, or a configuration that can't be read).
fn configured_build(root: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    crate::config::load(root, home.as_deref()).ok().map(|config| normalize(&config.build))
}

fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// What the threads that answer requests share.
struct Core {
    /// The workspace folder.
    root: PathBuf,
    shared: Arc<Shared>,
    jobs: Sender<Job>,
    /// The workspace folder with symbolic links resolved, to compare saved files with.
    canonical_root: PathBuf,
    /// How long slang may run; the first question waits for that much, and a little more.
    timeout: Duration,
}

impl Core {
    /// Stops the build in progress (killing slang) and wakes the questions waiting for one.
    fn stop(&self) {
        self.shared.cancelled.store(true, Ordering::Relaxed);
        let _state = self.shared.state.lock().unwrap();
        self.shared.first_build.notify_all();
    }

    fn notified(&self, notification: &Notification) {
        let changed: Vec<&str> = match notification.method.as_str() {
            "textDocument/didSave" => notification.params["textDocument"]["uri"].as_str().into_iter().collect(),
            // (A save in this editor comes both ways; the pause before a build makes one of the two.)
            "workspace/didChangeWatchedFiles" => {
                let changes = notification.params["changes"].as_array().map_or(&[][..], Vec::as_slice);
                changes.iter().filter_map(|change| change["uri"].as_str()).collect()
            }
            _ => return,
        };
        let changed: Vec<PathBuf> = changed.into_iter().filter_map(uri_path).collect();
        if changed.iter().any(|path| path.file_name().is_some_and(|name| name == "server.json")) {
            self.configuration_changed();
        }
        if changed.iter().any(|path| self.changes_design(path)) {
            self.rebuild(true, None);
        }
    }

    /// `server.json` changed: a chosen build stays, unless it is the old configured `build`, which follows
    /// the new one (slang-server's `loadConfig`: "re-resolve configured builds while retaining builds selected
    /// by the client"). A configuration that can't be read changes nothing, as the server keeps its last.
    fn configuration_changed(&self) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Ok(config) = crate::config::load(&self.root, home.as_deref()) else { return };
        let mut state = self.shared.state.lock().unwrap();
        if let BuildChoice::File(file) = &state.choice {
            if state.configured_build.as_deref() == Some(normalize(&self.root.join(file)).as_path()) {
                state.choice = BuildChoice::Config;
            }
        }
        // (Explore mode has no build of its own: the server's reload takes the configured one.)
        if state.choice == BuildChoice::None && state.explore {
            state.choice = BuildChoice::Config;
            state.explore = false;
        }
        state.configured_build = Some(normalize(&config.build));
    }

    fn rebuild(&self, debounce: bool, reply: Option<Sender<()>>) {
        let _ = self.jobs.send(Job { debounce, reply });
    }

    /// Whether saving this file can change the design: one of its files, a build file or the
    /// configuration.
    fn changes_design(&self, path: &Path) -> bool {
        // (The design's files are canonical; the editor's path may come through a symbolic link.)
        let path = &canonical(path);
        if self.shared.state.lock().unwrap().watched.contains(path) {
            return true;
        }
        let extension = path.extension().and_then(|extension| extension.to_str());
        path.starts_with(&self.canonical_root)
            && (path.file_name().is_some_and(|name| name == "server.json")
                || extension.is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension)))
    }

    fn respond(&self, request: Request) -> Response {
        if request.method != "workspace/executeCommand" {
            return Response::new_err(request.id, ErrorCode::MethodNotFound as i32, request.method);
        }
        match self.execute(&request.params) {
            Ok(result) => Response::new_ok(request.id, result),
            Err(message) => Response::new_err(request.id, ErrorCode::InternalError as i32, message),
        }
    }

    /// The state once the first build is over. A question that comes in before it is ready waits
    /// for it, for as long as slang may run (it is killed then, which ends the build), and a little more.
    fn built(&self) -> Result<MutexGuard<'_, State>, String> {
        let deadline = Instant::now() + self.timeout + Duration::from_secs(10);
        let mut state = self.shared.state.lock().unwrap();
        while !state.first_build_done {
            if self.shared.cancelled.load(Ordering::Relaxed) {
                return Err("svtrace: shutting down".to_string());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("svtrace: the first build has not finished".to_string());
            }
            state = self.shared.first_build.wait_timeout(state, left.min(Duration::from_millis(200))).unwrap().0;
        }
        Ok(state)
    }

    fn graph(&self) -> Result<Option<Arc<Graph>>, String> {
        Ok(self.built()?.graph.clone())
    }

    /// The hierarchy of the last design built (waiting for the first build, as `graph` does).
    fn hier(&self) -> Result<Option<Arc<Hier>>, String> {
        Ok(self.built()?.hier.clone())
    }

    /// The status, with the build the design comes from.
    fn status_json(&self) -> Value {
        let state = self.shared.state.lock().unwrap();
        let mut status = state.status.json();
        status["build"] = state.choice.json();
        // (Whether a design answers: a failed rebuild after a good one keeps the old one.)
        status["design"] = json!(state.graph.is_some());
        status
    }

    /// The error for a question when no design could be built.
    fn no_design(&self) -> String {
        let state = self.shared.state.lock().unwrap();
        format!("svtrace: no design: {}", state.status.error.as_deref().unwrap_or("not built"))
    }

    fn execute(&self, params: &Value) -> Result<Value, String> {
        let command = params["command"].as_str().unwrap_or_default();
        let arguments = params["arguments"].as_array().map_or(&[][..], Vec::as_slice);
        match command {
            RELOAD => {
                let (done, finished) = mpsc::channel();
                self.rebuild(false, Some(done));
                finished.recv().map_err(|_| "the build stopped".to_string())?;
                Ok(self.status_json())
            }
            USE_BUILD => {
                // `{ file }`: a build file; `{ file = null }` (or none): no design, `explore = true` when
                // that is slang-server's explore mode rather than a top file.
                let file = match arguments.first().map(|argument| &argument["file"]) {
                    Some(Value::String(file)) if !file.is_empty() => Some(file.clone()),
                    Some(Value::String(_)) | Some(Value::Null) | None => None,
                    Some(_) => return Err("svtrace.useBuild takes { file = string | null, explore = bool }".to_string()),
                };
                let explore = match arguments.first().map(|argument| &argument["explore"]) {
                    Some(Value::Bool(explore)) => *explore,
                    Some(Value::Null) | None => false,
                    Some(_) => return Err("svtrace.useBuild takes { file = string | null, explore = bool }".to_string()),
                };
                {
                    let mut state = self.shared.state.lock().unwrap();
                    state.explore = file.is_none() && explore;
                    state.choice = file.map_or(BuildChoice::None, BuildChoice::File);
                    state.fresh_choice = true;
                }
                let (done, finished) = mpsc::channel();
                self.rebuild(false, Some(done));
                finished.recv().map_err(|_| "the build stopped".to_string())?;
                Ok(self.status_json())
            }
            STATUS => Ok(self.status_json()),
            LIST_SIGNALS => Ok(self.graph()?.map_or(Value::Null, |graph| json!(graph.signals().collect::<Vec<_>>()))),
            command if hier::COMMANDS.contains(&command) => match self.hier()? {
                Some(hier) => hier::run(&hier, command, arguments),
                None => Err(self.no_design()),
            },
            command if trace::COMMANDS.contains(&command) => {
                // (No design was ever built: say why, or the trace would look like a signal with no drivers.)
                match self.graph()? {
                    Some(graph) => trace::run(&graph, command, arguments),
                    None => Err(self.no_design()),
                }
            }
            // (An error, not an empty answer: a misspelt command must not look like "nothing found".)
            _ => Err(format!("Unknown command: {command}")),
        }
    }
}
