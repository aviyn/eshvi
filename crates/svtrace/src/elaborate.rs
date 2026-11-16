//! Runs slang on the design and returns its elaborated AST as JSON.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::config::Config;

/// How long slang may run before it is killed, unless `SVTRACE_SLANG_TIMEOUT` (seconds) says otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// How many of slang's last lines of messages are kept.
const STDERR_LINES: usize = 50;

pub struct Elaboration {
    pub json: Value,
    /// slang's exit status. 1 means the design has errors; the dump is complete all the same.
    pub status: Option<i32>,
    pub stderr_tail: Vec<String>,
}

/// Runs `slang <flags> -f <build> --ast-json - ...` in the workspace folder.
///
/// Instance caching is off so that every instance body is written out in full: with it on, the
/// second instance of a module only links to the first one's body, and the signals inside it would
/// be missing from the dump. Detailed types give the width of named types, which tells a plain
/// copy from a change of width. `--ast-json-compact` is ours (`scripts/slang-ast-json-compact.patch`,
/// applied by `scripts/build-slang.sh`): without indentation the dump is a quarter of the size, and
/// slang writes it in a third of the time.
pub fn run(slang: &Path, config: &Config) -> Result<Elaboration> {
    run_with(slang, config, &Limits::default())
}

/// When to give up on a slang that doesn't finish.
pub struct Limits<'a> {
    /// How long slang may run; it is killed after that.
    pub timeout: Duration,
    /// Set to stop the run early (the server is shutting down); slang is killed.
    pub cancel: Option<&'a AtomicBool>,
}

impl Default for Limits<'_> {
    fn default() -> Self {
        Limits { timeout: DEFAULT_TIMEOUT, cancel: None }
    }
}

/// [`run`] with a time limit and a way to stop it.
pub fn run_with(slang: &Path, config: &Config, limits: &Limits) -> Result<Elaboration> {
    let mut command = Command::new(slang);
    command
        .current_dir(&config.root)
        // (Only variables that don't change how the process loads or runs: see `Config::child_env`.)
        .envs(config.child_env())
        .args(arguments(config))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().with_context(|| format!("starting {}", slang.display()))?;

    // slang's messages and its dump are read as they come, on threads, so that a full pipe can't
    // stall it and a slang that never finishes can't stall us (the threads are left to end when
    // the pipes close if slang is killed).
    let stderr = child.stderr.take().expect("stderr is piped");
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let messages = {
        let tail = tail.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut tail = tail.lock().unwrap();
                if tail.len() == STDERR_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        })
    };
    // The dump is parsed as it arrives: its text is never held whole beside what is read from it.
    let stdout = child.stdout.take().expect("stdout is piped");
    let (sent, received) = mpsc::channel();
    let dump_bytes = Arc::new(AtomicU64::new(0));
    {
        let dump_bytes = dump_bytes.clone();
        thread::spawn(move || {
            // (The parser takes its input a byte at a time, which is only quick from a `BufReader` it
            // owns. When it gives up early the pipe closes with it, and slang ends on its next write.)
            let dump = BufReader::with_capacity(1 << 16, Counted { inner: stdout, bytes: dump_bytes });
            let _ = sent.send(parse_from(serde_json::de::IoRead::new(dump)));
        });
    }
    let tail_lines = |tail: &Mutex<VecDeque<String>>| Vec::from(tail.lock().unwrap().clone());

    let started = Instant::now();
    let json = loop {
        match received.recv_timeout(Duration::from_millis(50)) {
            Ok(json) => break json,
            Err(RecvTimeoutError::Disconnected) => bail!("reading slang's output failed"),
            Err(RecvTimeoutError::Timeout) => {}
        }
        let cancelled = limits.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed));
        if cancelled || started.elapsed() >= limits.timeout {
            let _ = child.kill();
            let _ = child.wait();
            if cancelled {
                bail!("stopped");
            }
            let tail = tail_lines(&tail).join("\n");
            bail!(
                "slang did not finish in {} s and was stopped{}{tail}",
                limits.timeout.as_secs_f64(),
                if tail.is_empty() { "" } else { ": " }
            );
        }
    };
    let status = child.wait()?.code();
    let _ = messages.join();
    let stderr_tail = tail_lines(&tail);

    // Exit status 1 is a design with errors, which still dumps in full.
    let wrote = dump_bytes.load(Ordering::Relaxed) > 0;
    let finished = matches!(status, Some(0) | Some(1));
    let json = match json {
        Ok(json) if wrote && finished => json,
        // What slang wrote is not a dump (slang may have been stopped by the pipe closing on it), or
        // slang ended well and its dump is cut short.
        Err(error) if wrote && (finished || !error.is_eof()) => {
            return Err(anyhow::Error::new(error).context("reading slang's AST dump"));
        }
        _ => return Err(failure(slang, status, &stderr_tail)),
    };
    Ok(Elaboration { json, status, stderr_tail })
}

/// The error for a slang that gave no dump.
fn failure(slang: &Path, status: Option<i32>, stderr_tail: &[String]) -> anyhow::Error {
    if stderr_tail.iter().any(|line| line.contains("--ast-json-compact")) {
        return anyhow!("{} is a slang without --ast-json-compact: build it with scripts/build-slang.sh", slang.display());
    }
    anyhow!("slang failed ({}): {}", status.map_or("killed".to_string(), |s| s.to_string()), stderr_tail.join("\n"))
}

/// svtrace's own options: how the dump is asked for.
const OWN_FLAGS: [&str; 5] =
    ["--ast-json-source-info", "--ast-json-detailed-types", "--ast-json-compact", "--disable-instance-caching", "-q"];

/// slang's arguments: the configured flags, the build file and include folders, then svtrace's own.
/// slang refuses an option given twice (`more than one value provided for argument '-q'`), and
/// slang-server takes a `flags` line with any of these, so the configured ones that svtrace passes
/// too are left out, as is an `--ast-json FILE` (the dump goes to svtrace, not to a file).
pub fn arguments(config: &Config) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = Vec::new();
    let mut flags = config.flags.iter();
    while let Some(flag) = flags.next() {
        let name = flag.split_once('=').map_or(flag.as_str(), |(name, _)| name);
        if name == "--ast-json" {
            if !flag.contains('=') {
                flags.next();
            }
        } else if !(OWN_FLAGS.contains(&name) || name == "--quiet") {
            arguments.push(flag.into());
        }
    }
    arguments.push(if config.build_relative_paths { "-F" } else { "-f" }.into());
    arguments.push(config.build.clone().into());
    for dir in &config.incdirs {
        arguments.push("-I".into());
        arguments.push(dir.into());
    }
    arguments.extend(["--ast-json", "-"].map(OsString::from));
    arguments.extend(OWN_FLAGS.map(OsString::from));
    arguments
}

/// Counts what is read through it.
struct Counted<R> {
    inner: R,
    bytes: Arc<AtomicU64>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.bytes.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

/// Reads the dump's JSON. serde_json refuses nesting past 128 levels, and slang nests a level per
/// chained operator and per `else if`, so the limit is off and the stack grows as needed.
pub fn parse(dump: &[u8]) -> Result<Value> {
    Ok(parse_from(serde_json::de::SliceRead::new(dump))?)
}

fn parse_from<'de>(read: impl serde_json::de::Read<'de>) -> serde_json::Result<Value> {
    use serde::Deserialize;
    let mut reader = serde_json::Deserializer::new(read);
    reader.disable_recursion_limit();
    let value = Value::deserialize(serde_stacker::Deserializer::new(&mut reader))?;
    reader.end()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(arguments: Vec<OsString>) -> Vec<String> {
        arguments.into_iter().map(|argument| argument.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn configured_flags_that_svtrace_passes_too_are_given_once() {
        let flags = ["-q", "-DA=1", "--ast-json", "out.json", "--quiet", "--ast-json=x.json", "--ast-json-source-info"];
        let more = ["--ast-json-detailed-types", "--ast-json-compact", "--disable-instance-caching", "--top", "t", "--ast-json-scope", "t.u"];
        let config = Config {
            flags: flags.iter().chain(&more).map(|flag| flag.to_string()).collect(),
            build: "/w/b.f".into(),
            incdirs: vec!["/w/inc".into()],
            ..Config::default()
        };
        let expected = [
            "-DA=1", "--top", "t", "--ast-json-scope", "t.u", "-f", "/w/b.f", "-I", "/w/inc", "--ast-json", "-",
            "--ast-json-source-info", "--ast-json-detailed-types", "--ast-json-compact", "--disable-instance-caching", "-q",
        ];
        assert_eq!(strings(arguments(&config)), expected);
    }

    #[test]
    fn the_build_file_can_be_relative_to_itself() {
        let config = Config { build: "/w/b.f".into(), build_relative_paths: true, ..Config::default() };
        assert_eq!(strings(arguments(&config))[..2], ["-F", "/w/b.f"]);
    }
}
