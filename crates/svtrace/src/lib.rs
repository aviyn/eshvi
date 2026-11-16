//! svtrace: a small language server that answers the signal trace's questions (drivers and loads of
//! a signal) from the design slang elaborates, without a patched slang-server.

pub mod ast;
pub mod bits;
pub mod cursor;
pub mod config;
pub mod elaborate;
pub mod graph;
pub mod hier;
pub mod lsp;
pub mod source;
pub mod trace;
pub mod walk;

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use graph::Graph;

/// A design that has been elaborated and indexed.
pub struct Built {
    pub graph: Graph,
    pub hier: hier::Hier,
    /// slang's exit status; 1 means the design has errors.
    pub slang_exit: Option<i32>,
    pub stderr_tail: Vec<String>,
    pub duration: Duration,
}

impl Built {
    /// Answers a command from this design, as `svtrace query` does: the trace's and the hierarchy's
    /// commands, `svtrace.listSignals`, and `svtrace.status` as far as one build has a status.
    pub fn answer(&self, command: &str, arguments: &[serde_json::Value]) -> Result<serde_json::Value, String> {
        use serde_json::json;
        match command {
            "svtrace.listSignals" => Ok(json!(self.graph.signals().collect::<Vec<_>>())),
            "svtrace.status" => Ok(json!({
                "state": "ready",
                "durationMs": self.duration.as_millis(),
                "slangExit": self.slang_exit,
                "stderrTail": self.stderr_tail,
                "signals": self.graph.signals().count(),
            })),
            // (With no path: the roots.)
            command if hier::COMMANDS.contains(&command) && arguments.is_empty() => hier::run(&self.hier, command, &[json!("")]),
            command if hier::COMMANDS.contains(&command) => hier::run(&self.hier, command, arguments),
            command if trace::COMMANDS.contains(&command) => trace::run(&self.graph, command, arguments),
            _ => Err(format!("Unknown command: {command}")),
        }
    }
}

/// Which build file a design is elaborated from.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BuildChoice {
    /// The `build` of `.slang/server.json` (the default).
    #[default]
    Config,
    /// A build file chosen by the client (`svtrace.useBuild`), relative to the workspace or absolute.
    File(String),
    /// No design: the server has none (explore mode), so there is nothing to elaborate.
    None,
}

/// Elaborates the design configured in `workspace` with the slang at `slang`.
pub fn build(slang: &Path, workspace: &Path) -> Result<Built> {
    build_with(slang, workspace, &elaborate::Limits::default())
}

/// [`build`], with a time limit for slang and a way to stop it.
pub fn build_with(slang: &Path, workspace: &Path, limits: &elaborate::Limits) -> Result<Built> {
    build_chosen(slang, workspace, limits, &BuildChoice::Config)
}

/// [`build_with`], from the build file `choice` says.
pub fn build_chosen(slang: &Path, workspace: &Path, limits: &elaborate::Limits, choice: &BuildChoice) -> Result<Built> {
    let started = Instant::now();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let file = match choice {
        BuildChoice::Config => None,
        BuildChoice::File(file) => {
            if !workspace.join(file).is_file() {
                return Err(anyhow!("build file not found: {file}"));
            }
            Some(file.as_str())
        }
        BuildChoice::None => return Err(anyhow!("no build chosen")),
    };
    let config = config::load_with_build(workspace, home.as_deref(), file)?;
    let elaboration = elaborate::run_with(slang, &config, limits)?;
    // (The two readers of the dump share one copy of each source file's text.)
    let texts = source::Texts::default();
    let design = ast::read(&elaboration.json).map_err(|message| anyhow!(message))?;
    let hier = hier::Hier::read_shared(workspace, &elaboration.json, &texts);
    // (A deeply nested dump is deeply nested to drop, too.)
    deep(move || drop(elaboration.json));
    Ok(Built {
        graph: Graph::build_shared(workspace, design, &texts),
        hier,
        slang_exit: elaboration.status,
        stderr_tail: elaboration.stderr_tail,
        duration: started.elapsed(),
    })
}

/// Runs `f` on a stack of its own with room for a deep recursion: the expression trees and statement
/// chains of generated RTL nest a thousand levels, and every pass over them (reading the dump, the graph,
/// the tracers, dropping them) recurses once per level, where the thread's stack may hold a few
/// thousand frames. The 256 MB is reserved, not used, until a walk grows into it. A call inside one
/// that is already on such a stack just runs.
pub(crate) fn deep<R>(f: impl FnOnce() -> R) -> R {
    use std::cell::Cell;
    thread_local!(static ON_BIG_STACK: Cell<bool> = const { Cell::new(false) });
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ON_BIG_STACK.with(|on| on.set(false));
        }
    }
    if ON_BIG_STACK.with(Cell::get) {
        return f();
    }
    ON_BIG_STACK.with(|on| on.set(true));
    let _reset = Reset;
    stacker::grow(256 * 1024 * 1024, f)
}
