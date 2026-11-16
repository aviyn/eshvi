//! Apps opened from the Finder or the Dock get launchd's PATH
//! (`/usr/bin:/bin:/usr/sbin:/sbin`): no Homebrew, no `~/.local/bin`, so
//! `nvim` isn't found, nor the tools Neovim runs (LSP servers, formatters,
//! git). Started without a terminal, take PATH from the user's shell, the
//! one a terminal would have. They also start in `/`: work from the home
//! folder instead, as a terminal would (relative paths, `:e`, terminals).

use std::io::{IsTerminal, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MARKER: &str = "__ESHVI_PATH__";

pub fn adopt() {
    if !cfg!(unix) || std::io::stdin().is_terminal() {
        return;
    }
    match from_shell() {
        Some(path) => {
            log::info!("PATH from the login shell: {path}");
            std::env::set_var("PATH", path);
        }
        None => log::warn!("couldn't read PATH from the login shell"),
    }
    if std::env::current_dir().is_ok_and(|d| d == std::path::Path::new("/")) {
        if let Some(home) = std::env::var_os("HOME") {
            let _ = std::env::set_current_dir(home);
        }
    }
}

fn from_shell() -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    // fish keeps PATH as a list, which "$PATH" joins with spaces.
    let print = if shell.ends_with("fish") {
        format!("printf '%s%s' {MARKER} (string join : $PATH)")
    } else {
        format!("printf '%s%s' {MARKER} \"$PATH\"")
    };
    // Interactive too: PATH is often set in .zshrc or .bashrc, which a login
    // shell alone doesn't read. Its other output goes before the marker.
    let mut command = Command::new(&shell);
    command.args(["-l", "-i", "-c", &print]);
    // (Not UTF-8 somewhere in a greeting: the rest is still read.)
    parse(&String::from_utf8_lossy(&output_within(command, Duration::from_secs(5))?))
}

/// What `command` prints, if it ends within `limit` (a shell setup that
/// waits for something mustn't hold up the window). Read while it runs: a
/// setup that prints more than a pipe holds would otherwise wait for a
/// reader until the limit, and its PATH would be lost.
fn output_within(mut command: Command, limit: Duration) -> Option<Vec<u8>> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    // (Something it started may still hold the pipe open: not waited for long.)
    rx.recv_timeout(Duration::from_secs(1)).ok()
}

fn parse(out: &str) -> Option<String> {
    let path = out.rsplit_once(MARKER)?.1.trim();
    (!path.is_empty()).then(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn more_output_than_a_pipe_holds_is_read_and_a_stuck_shell_is_left() {
        let mut noisy = Command::new("/bin/sh");
        noisy.args(["-c", "head -c 300000 /dev/zero | tr '\\0' 'x'; printf '\\377\\n__ESHVI_PATH__/opt/bin:/usr/bin'"]);
        let started = Instant::now();
        let out = output_within(noisy, Duration::from_secs(5)).expect("its output");
        assert!(started.elapsed() < Duration::from_secs(4), "waited for a reader");
        assert!(out.len() > 300_000);
        assert_eq!(parse(&String::from_utf8_lossy(&out)), Some("/opt/bin:/usr/bin".into()));
        let mut stuck = Command::new("/bin/sh");
        stuck.args(["-c", "sleep 30"]);
        assert_eq!(output_within(stuck, Duration::from_millis(200)), None);
    }

    #[test]
    fn path_is_what_follows_the_marker() {
        assert_eq!(parse("motd\n__ESHVI_PATH__/opt/homebrew/bin:/usr/bin"), Some("/opt/homebrew/bin:/usr/bin".into()));
        assert_eq!(parse("__ESHVI_PATH__"), None);
        assert_eq!(parse("no marker"), None);
    }
}
