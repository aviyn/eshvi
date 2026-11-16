//! svtrace: `svtrace` serves LSP on stdio; `svtrace query` and `svtrace signals` ask the same
//! questions on the command line, for debugging and for comparing with slang-server.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};

const USAGE: &str = "usage: svtrace [--slang PATH] [query WORKSPACE COMMAND [ARG] | signals WORKSPACE]

With no command, svtrace is a language server on stdio.
  query WORKSPACE COMMAND [ARG]    print the answer to COMMAND as JSON. For slang.getDriversWithLocation,
                                   slang.getLoadsWithLocation and slang.getDriversWithConstants ARG is the
                                   signal's hierarchical path; for svtrace.step, svtrace.endpoints and
                                   svtrace.atCursor it is JSON, such as
                                   '{\"path\":\"top.x\",\"direction\":\"loads\"}'; for slang.getScope and
                                   slang.getScopes a scope's path (none: the roots); svtrace.status and
                                   svtrace.listSignals take none
  signals WORKSPACE                print the path of every signal, one per line
--slang PATH is the slang to run (default ~/tools/slang/slang).";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("svtrace: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut slang = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("tools/slang/slang");
    if let Some(at) = args.iter().position(|arg| arg == "--slang") {
        if at + 1 >= args.len() {
            bail!("--slang needs a path\n{USAGE}");
        }
        slang = PathBuf::from(args.remove(at + 1));
        args.remove(at);
    }
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {
            let (connection, io_threads) = lsp_server::Connection::stdio();
            svtrace::lsp::serve(connection, slang)?;
            // (The reply to `shutdown` is written by one of these threads.)
            io_threads.join()?;
            Ok(())
        }
        ["query", workspace, command, argument @ ..] if argument.len() <= 1 => {
            let built = build(&slang, workspace)?;
            // (An argument that starts with `{` is the JSON argument of an `svtrace.*` command.)
            let arguments = match argument {
                [json] if json.starts_with('{') => vec![serde_json::from_str(json).context("the argument is not valid JSON")?],
                [path] => vec![serde_json::Value::String(path.to_string())],
                _ => Vec::new(),
            };
            let answer = built.answer(command, &arguments).map_err(anyhow::Error::msg)?;
            print(&format!("{answer}\n"))
        }
        ["signals", workspace] => {
            let paths: Vec<_> = build(&slang, workspace)?.graph.signals().map(|path| format!("{path}\n")).collect();
            print(&paths.concat())
        }
        _ => bail!("{USAGE}"),
    }
}

/// Writes to stdout, quietly giving up when the reader (`| head`) has gone.
fn print(text: &str) -> Result<()> {
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}

fn build(slang: &std::path::Path, workspace: &str) -> Result<svtrace::Built> {
    let workspace = std::fs::canonicalize(workspace).with_context(|| format!("no workspace folder {workspace}"))?;
    svtrace::build(slang, &workspace)
}
