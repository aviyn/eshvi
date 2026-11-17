//! Terminals for the GUI: a process on a PTY and its screen, on
//! `alacritty_terminal`. The UI owns any number of `Terminal`s; each has its
//! own size, and the UI draws a `Snapshot` of it and feeds it keys, text, the
//! mouse and the wheel. Nothing here draws or knows egui.
//!
//! Output is parsed on alacritty's reader thread, which calls the `wake`
//! function (the UI's repaint) and queues `Event`s for `Terminal::events`.

mod core;
pub mod keys;
pub mod mouse;
mod palette;
pub mod proc;

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::event::{Event as AlacrittyEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Osc52, Term};
use alacritty_terminal::tty::{self, Shell};

pub use crate::core::{CursorDraw, Dims, Snapshot, Span, ViKey, ViOutcome};
pub use crate::palette::Palette;

/// Where a terminal's view is in its scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollPos {
    /// Lines scrolled back from the bottom (0: at the bottom).
    pub offset: usize,
    /// Lines of scrollback there are.
    pub history: usize,
    /// Lines on the screen.
    pub rows: usize,
    /// The alternate screen (a full-screen program) has no scrollback of its own.
    pub alt_screen: bool,
}
pub use alacritty_terminal::index::{Direction, Point, Side};
pub use alacritty_terminal::grid::Scroll;
pub use alacritty_terminal::selection::SelectionType;
pub use alacritty_terminal::term::color::Colors;
pub use alacritty_terminal::term::cell::{Cell, Flags};
pub use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Processor;
pub use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

use crate::core::{Search, ViState};
use crate::keys::{KeyEvent, KeyOptions, Mods};
use crate::mouse::{MouseButton, MouseKind};

/// A terminal's size: its grid, and a cell in pixels (programs may ask).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl Size {
    fn window(self) -> WindowSize {
        WindowSize { num_lines: self.rows, num_cols: self.cols, cell_width: self.cell_w, cell_height: self.cell_h }
    }
}

/// Called from the terminal's threads when it has something new to draw.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct SpawnOptions {
    /// The program and its arguments; see `login_shell`.
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Added to the environment (over `TERM`, `COLORTERM` and `TERM_PROGRAM`).
    pub env: Vec<(String, String)>,
    pub size: Size,
    /// Lines kept above the screen.
    pub scrollback: usize,
    pub palette: Palette,
    pub keys: KeyOptions,
    pub wake: Option<Wake>,
}

impl SpawnOptions {
    pub fn new(argv: Vec<String>, size: Size) -> Self {
        Self {
            argv,
            cwd: None,
            env: Vec::new(),
            size,
            scrollback: 10_000,
            palette: Palette::default(),
            keys: KeyOptions::default(),
            wake: None,
        }
    }
}

/// The variable to set so programs speak UTF-8 when the environment doesn't
/// say so (an app opened from the Finder has no `LANG`): `LC_CTYPE`, unless
/// `LC_ALL` is set (it would win over it) or the locale already is UTF-8.
/// `get` reads the environment.
fn utf8_locale(get: impl Fn(&str) -> Option<String>) -> Option<(&'static str, &'static str)> {
    let set = |name: &str| get(name).filter(|v| !v.is_empty());
    if set("LC_ALL").is_some() {
        return None;
    }
    let effective = set("LC_CTYPE").or_else(|| set("LANG"));
    if effective.is_some_and(|v| v.to_ascii_lowercase().replace('-', "").contains("utf8")) {
        return None;
    }
    Some(("LC_CTYPE", if cfg!(target_os = "macos") { "UTF-8" } else { "C.UTF-8" }))
}

/// The command for a shell terminal: `program` (`[terminal] shell`; none or
/// empty: the user's shell, `$SHELL`, else the password file's, else `sh`)
/// and its arguments as given.
pub fn shell_argv(program: Option<&str>, args: &[String]) -> Vec<String> {
    let program = program
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty()))
        .or_else(passwd_shell)
        .unwrap_or_else(|| "/bin/sh".into());
    std::iter::once(program).chain(args.iter().cloned()).collect()
}

/// A program run through a shell (`shell_argv`'s), so it gets what the
/// user's startup files set (PATH, variables): that shell, interactive (as a
/// terminal's is, so `.bashrc` and `.zshrc` are read too), running
/// `exec <argv>` with each word single-quoted. The program replaces the
/// shell, so the terminal's process is the program's.
pub fn in_shell(shell: Vec<String>, argv: &[String]) -> Vec<String> {
    let quote = |w: &String| format!("'{}'", w.replace('\'', r"'\''"));
    let command = format!("exec {}", argv.iter().map(quote).collect::<Vec<_>>().join(" "));
    let mut out = shell;
    if !out[1..].iter().any(|a| a == "-i") {
        out.push("-i".into());
    }
    out.extend(["-c".into(), command]);
    out
}

/// The user's login shell: `$SHELL -l` (else the password file's, else `sh`).
pub fn login_shell() -> Vec<String> {
    shell_argv(None, &["-l".into()])
}

fn passwd_shell() -> Option<String> {
    // SAFETY: getpwuid returns a pointer to static storage or null; the
    // fields are read at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_shell.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*pw).pw_shell).to_str().ok().map(str::to_owned)
    }
}

/// What the terminal tells the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The program set the title (OSC 0 and 2).
    Title(String),
    /// The program asked for the default title back.
    ResetTitle,
    Bell,
    /// OSC 52: the program stores this text on the clipboard.
    ClipboardStore(String),
    /// The cursor starts or stops blinking.
    CursorBlinking,
    /// The process ended: its exit code, or the signal that ended it.
    Exit { code: Option<i32>, signal: Option<i32> },
}

/// State the reader thread and the UI share.
struct Shared {
    palette: Mutex<Palette>,
    size: Mutex<WindowSize>,
    title: Mutex<Option<String>>,
    exited: AtomicBool,
    exit: Mutex<Option<(Option<i32>, Option<i32>)>>,
    bells: AtomicU32,
    /// Times output was read and parsed (alacritty's wakeups): a counter that moves while the
    /// program prints.
    output: AtomicU64,
    /// Set once the event loop exists: how the terminal answers the program.
    sender: OnceLock<EventLoopSender>,
    wake: Option<Wake>,
}

impl Shared {
    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return;
        }
        if let Some(sender) = self.sender.get() {
            let _ = sender.send(Msg::Input(bytes));
        }
    }

    fn wake(&self) {
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

/// Alacritty's event listener: runs on its threads.
#[derive(Clone)]
struct Proxy {
    shared: Arc<Shared>,
    events: Sender<Event>,
}

impl Proxy {
    fn send(&self, event: Event) {
        let _ = self.events.send(event);
        self.shared.wake();
    }
}

impl EventListener for Proxy {
    fn send_event(&self, event: AlacrittyEvent) {
        match event {
            AlacrittyEvent::Wakeup => {
                self.shared.output.fetch_add(1, Ordering::Relaxed);
                self.shared.wake();
            }
            AlacrittyEvent::MouseCursorDirty => self.shared.wake(),
            AlacrittyEvent::Title(title) => {
                *self.shared.title.lock().unwrap() = Some(title.clone());
                self.send(Event::Title(title));
            }
            AlacrittyEvent::ResetTitle => {
                *self.shared.title.lock().unwrap() = None;
                self.send(Event::ResetTitle);
            }
            AlacrittyEvent::Bell => {
                self.shared.bells.fetch_add(1, Ordering::Relaxed);
                self.send(Event::Bell);
            }
            AlacrittyEvent::ClipboardStore(_, text) => self.send(Event::ClipboardStore(text)),
            // The clipboard isn't readable by programs (OSC 52 `?`).
            AlacrittyEvent::ClipboardLoad(..) => {}
            AlacrittyEvent::ColorRequest(index, format) => {
                let rgb = self.shared.palette.lock().unwrap().indexed(index);
                self.shared.write(format(rgb).into_bytes());
            }
            AlacrittyEvent::PtyWrite(text) => self.shared.write(text.into_bytes()),
            AlacrittyEvent::TextAreaSizeRequest(format) => {
                let size = *self.shared.size.lock().unwrap();
                self.shared.write(format(size).into_bytes());
            }
            AlacrittyEvent::CursorBlinkingChange => self.send(Event::CursorBlinking),
            AlacrittyEvent::ChildExit(status) => self.exited(status.code(), status.signal()),
            // The loop ends after the child (a read error ends it too).
            AlacrittyEvent::Exit => self.exited(None, None),
        }
    }
}

impl Proxy {
    fn exited(&self, code: Option<i32>, signal: Option<i32>) {
        if self.shared.exited.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.shared.exit.lock().unwrap() = Some((code, signal));
        self.send(Event::Exit { code, signal });
    }
}

/// One terminal: a process on a PTY and what it has drawn, or (`Terminal::output`)
/// only what it was `feed`.
pub struct Terminal {
    term: Arc<FairMutex<Term<Proxy>>>,
    shared: Arc<Shared>,
    /// How the loop is told to resize and end; none for an output terminal.
    sender: Option<EventLoopSender>,
    events: Mutex<Receiver<Event>>,
    /// The process's id (0 for an output terminal).
    pid: u32,
    /// A duplicate of the PTY's master side, for the foreground process.
    master: Option<OwnedFd>,
    /// What `feed` hasn't finished reading: an escape sequence cut in two.
    parser: Mutex<Processor>,
    cwd: Option<PathBuf>,
    size: Mutex<Size>,
    search: Mutex<Search>,
    vi: Mutex<ViState>,
    keys: Mutex<KeyOptions>,
}

impl Terminal {
    pub fn spawn(opts: SpawnOptions) -> io::Result<Terminal> {
        let (program, args) = opts.argv.split_first().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no program to run"))?;
        let mut env: HashMap<String, String> = HashMap::from([
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("TERM_PROGRAM".into(), "eshvi".into()),
        ]);
        if let Some((name, value)) = utf8_locale(|n| std::env::var(n).ok()) {
            env.insert(name.into(), value.into());
        }
        env.extend(opts.env);
        let pty_options = tty::Options {
            shell: Some(Shell::new(program.clone(), args.to_vec())),
            working_directory: opts.cwd.clone(),
            // A short-lived program's last output is read before it is over.
            drain_on_exit: true,
            env,
        };
        let size = opts.size;
        // Now and then the forked child fails while it sets up the terminal (a raw "-6",
        // seen only with many processes starting at once, as in the full test run; not
        // explained): try twice more before giving up, and say what was being started.
        let mut tries = 0;
        let pty = loop {
            match tty::new(&pty_options, size.window(), 0) {
                Ok(pty) => break pty,
                Err(e) if tries < 2 => {
                    tries += 1;
                    log::warn!("terminal: starting {program} failed ({e}), trying again");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(io::Error::new(e.kind(), format!("starting {program}: {e}"))),
            }
        };
        let pid = pty.child().id();
        // SAFETY: duplicates a descriptor that is open; the copy is ours.
        let master = unsafe {
            let fd = libc::dup(std::os::fd::AsRawFd::as_raw_fd(pty.file()));
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(fd)
        };

        let shared = Arc::new(Shared {
            palette: Mutex::new(opts.palette),
            size: Mutex::new(size.window()),
            title: Mutex::new(None),
            exited: AtomicBool::new(false),
            exit: Mutex::new(None),
            bells: AtomicU32::new(0),
            output: AtomicU64::new(0),
            sender: OnceLock::new(),
            wake: opts.wake,
        });
        let (tx, rx) = channel();
        let proxy = Proxy { shared: shared.clone(), events: tx };
        let config = Config {
            scrolling_history: opts.scrollback,
            kitty_keyboard: true,
            // Programs may store on the clipboard, not read it.
            osc52: Osc52::OnlyCopy,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &Dims { cols: size.cols as usize, rows: size.rows as usize }, proxy.clone())));
        let event_loop = EventLoop::new(term.clone(), proxy, pty, true, false)?;
        let sender = event_loop.channel();
        let _ = shared.sender.set(sender.clone());
        // The thread ends with the process (or at `Drop`), and takes the PTY with it.
        drop(event_loop.spawn());
        Ok(Terminal {
            term,
            shared,
            sender: Some(sender),
            events: Mutex::new(rx),
            pid,
            master: Some(master),
            parser: Mutex::new(Processor::default()),
            cwd: opts.cwd,
            size: Mutex::new(size),
            search: Mutex::new(Search::default()),
            vi: Mutex::new(ViState::default()),
            keys: Mutex::new(opts.keys),
        })
    }

    /// A terminal with no process: it shows what `feed` is given, with its
    /// scrollback, selection, search and Vi mode as any other. Nothing typed
    /// or pasted goes anywhere, and it never ends.
    pub fn output(size: Size, palette: Palette, scrollback: usize, wake: Option<Wake>) -> Terminal {
        let shared = Arc::new(Shared {
            palette: Mutex::new(palette),
            size: Mutex::new(size.window()),
            title: Mutex::new(None),
            exited: AtomicBool::new(false),
            exit: Mutex::new(None),
            bells: AtomicU32::new(0),
            output: AtomicU64::new(0),
            sender: OnceLock::new(),
            wake,
        });
        let (tx, rx) = channel();
        let proxy = Proxy { shared: shared.clone(), events: tx };
        let config = Config { scrolling_history: scrollback, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &Dims { cols: size.cols as usize, rows: size.rows as usize }, proxy)));
        Terminal {
            term,
            shared,
            sender: None,
            events: Mutex::new(rx),
            pid: 0,
            master: None,
            parser: Mutex::new(Processor::default()),
            cwd: None,
            size: Mutex::new(size),
            search: Mutex::new(Search::default()),
            vi: Mutex::new(ViState::default()),
            keys: Mutex::new(KeyOptions::default()),
        }
    }

    /// Show `bytes` as a program's output would be shown: escape sequences
    /// (colors, bold) are read, and a line ends with "\r\n" (a bare "\n" only
    /// moves down, as on any terminal). A no-op on a terminal with a process.
    pub fn feed(&self, bytes: &[u8]) {
        if self.sender.is_some() {
            return;
        }
        self.parser.lock().unwrap().advance(&mut *self.term.lock(), bytes);
        self.shared.output.fetch_add(1, Ordering::Relaxed);
        self.shared.wake();
    }

    /// Whether anything typed reaches a process (not for an output terminal).
    pub fn takes_input(&self) -> bool {
        self.sender.is_some()
    }

    /// The process's id (0 when there is none).
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Where the process was started.
    pub fn cwd(&self) -> Option<&std::path::Path> {
        self.cwd.as_deref()
    }

    pub fn size(&self) -> Size {
        *self.size.lock().unwrap()
    }

    /// The title the program set (`None`: it hasn't, or reset it).
    pub fn title(&self) -> Option<String> {
        self.shared.title.lock().unwrap().clone()
    }

    /// `None` while the process runs; then its exit code (`None` if a signal
    /// ended it) and that signal.
    pub fn exit_status(&self) -> Option<(Option<i32>, Option<i32>)> {
        *self.shared.exit.lock().unwrap()
    }

    pub fn is_running(&self) -> bool {
        !self.shared.exited.load(Ordering::SeqCst)
    }

    /// Bells so far, to tell a new one.
    pub fn bells(&self) -> u32 {
        self.shared.bells.load(Ordering::Relaxed)
    }

    /// How many times output has come in so far: a tab is active while this moves.
    pub fn output_count(&self) -> u64 {
        self.shared.output.load(Ordering::Relaxed)
    }

    /// The name of the process in the PTY's foreground (`zsh` at its prompt,
    /// `vim` in an editor), from its foreground process group.
    pub fn foreground_process(&self) -> Option<String> {
        use std::os::fd::AsRawFd;
        if !self.is_running() {
            return None;
        }
        let pgid = proc::foreground_pgid(self.master.as_ref()?.as_raw_fd() as RawFd)?;
        proc::process_name(pgid)
    }

    /// The folder the foreground process (else the shell) is in now, else the
    /// one the terminal was started in.
    pub fn current_dir(&self) -> Option<PathBuf> {
        use std::os::fd::AsRawFd;
        let Some(master) = &self.master else { return self.cwd.clone() };
        let foreground = proc::foreground_pgid(master.as_raw_fd() as RawFd);
        foreground.and_then(proc::process_cwd).or_else(|| proc::process_cwd(self.pid)).or_else(|| self.cwd.clone())
    }

    /// Whether the shell itself is in the foreground, as at its prompt.
    pub fn at_shell_prompt(&self) -> bool {
        use std::os::fd::AsRawFd;
        self.master.as_ref().is_some_and(|m| proc::foreground_pgid(m.as_raw_fd() as RawFd) == Some(self.pid))
    }

    /// What happened since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().try_iter().collect()
    }

    // Output to the process.

    /// Bytes to the program, as typed.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if self.is_running() {
            self.shared.write(bytes);
        }
    }

    /// Text to the program (what a key types, an IME's commit).
    pub fn write_text(&self, text: &str) {
        self.write(text.as_bytes().to_vec());
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn set_key_options(&self, opts: KeyOptions) {
        *self.keys.lock().unwrap() = opts;
    }

    /// A key: encoded for the mode the program set, and sent. `true` if it
    /// sent anything (else the key makes text, which comes separately).
    pub fn key(&self, ev: &KeyEvent) -> bool {
        let mode = self.mode();
        let opts = *self.keys.lock().unwrap();
        match keys::encode_key(ev, mode, opts) {
            Some(bytes) => {
                self.write(bytes);
                true
            }
            None => false,
        }
    }

    /// Pasted text, in brackets if the program asked for them.
    pub fn paste(&self, text: &str) {
        self.write(keys::encode_paste(text, self.mode()));
    }

    /// The window gained or lost focus: the program hears of it if it asked.
    pub fn set_focused(&self, focused: bool) {
        let mut term = self.term.lock();
        if term.is_focused == focused {
            return;
        }
        term.is_focused = focused;
        let focus_events = term.mode().contains(TermMode::FOCUS_IN_OUT);
        drop(term);
        if focus_events {
            self.write(if focused { &b"\x1b[I"[..] } else { &b"\x1b[O"[..] });
        }
    }

    // The mouse.

    /// A mouse event for the program, if it asked for these (and `true` if it
    /// was sent). `col` and `row` are screen cells.
    pub fn mouse(&self, button: MouseButton, kind: MouseKind, mods: Mods, col: usize, row: usize) -> bool {
        match mouse::encode_mouse(self.mode(), button, kind, mods, col, row) {
            Some(bytes) => {
                self.write(bytes);
                true
            }
            None => false,
        }
    }

    /// Whether the program takes the mouse (clicks select when Shift is held).
    pub fn reports_mouse(&self) -> bool {
        self.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// The wheel, `lines` notches up (negative: down) at a cell: reported to a
    /// program that takes the mouse, as arrow keys on the alternate screen
    /// without one, else scrolls the scrollback. Shift scrolls the scrollback
    /// anyway.
    pub fn wheel(&self, lines: i32, col: usize, row: usize, mods: Mods) {
        if lines == 0 {
            return;
        }
        let mode = self.mode();
        let button = if lines > 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
        if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift {
            for _ in 0..lines.abs() {
                self.mouse(button, MouseKind::Press, mods, col, row);
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) && !mode.contains(TermMode::VI) {
            for _ in 0..lines.abs() {
                self.write(keys::arrow(lines > 0, mode));
            }
        } else {
            self.scroll(Scroll::Delta(lines));
        }
    }

    // Size and scrolling.

    /// A new size. The text reflows, and the process hears of it (SIGWINCH).
    pub fn resize(&self, size: Size) {
        if size.cols == 0 || size.rows == 0 {
            return;
        }
        let mut current = self.size.lock().unwrap();
        if *current == size {
            return;
        }
        *current = size;
        *self.shared.size.lock().unwrap() = size.window();
        self.term.lock().resize(Dims { cols: size.cols as usize, rows: size.rows as usize });
        if let Some(sender) = self.sender.as_ref().filter(|_| self.is_running()) {
            let _ = sender.send(Msg::Resize(size.window()));
        }
    }

    /// Scroll the view through the scrollback (`Delta`: positive is up).
    pub fn scroll(&self, scroll: Scroll) {
        self.term.lock().scroll_display(scroll);
    }

    /// Where the view is in the scrollback, without a snapshot's copy of the screen.
    pub fn scroll_position(&self) -> ScrollPos {
        use alacritty_terminal::grid::Dimensions;
        let term = self.term.lock();
        ScrollPos {
            offset: term.grid().display_offset(),
            history: term.history_size(),
            rows: term.screen_lines(),
            alt_screen: term.mode().contains(TermMode::ALT_SCREEN),
        }
    }

    /// Scroll the view to `offset` lines back from the bottom (as far as there
    /// are lines): what dragging a scroll bar does.
    pub fn scroll_to(&self, offset: usize) {
        let mut term = self.term.lock();
        let now = term.grid().display_offset() as i32;
        term.scroll_display(Scroll::Delta(offset as i32 - now));
    }

    /// Lines of scrollback.
    pub fn history_size(&self) -> usize {
        use alacritty_terminal::grid::Dimensions;
        self.term.lock().history_size()
    }

    /// The colors of the colorscheme; cells already drawn follow.
    pub fn set_palette(&self, palette: Palette) {
        *self.shared.palette.lock().unwrap() = palette;
    }

    pub fn palette(&self) -> Palette {
        *self.shared.palette.lock().unwrap()
    }

    /// Clear the screen and the scrollback (⌘K).
    pub fn clear(&self) {
        let mut term = self.term.lock();
        core::clear_history(&mut term);
        let on_primary = !term.mode().contains(TermMode::ALT_SCREEN);
        drop(term);
        // The shell clears the screen and draws its prompt again.
        if on_primary && self.at_shell_prompt() {
            self.write(&b"\x0c"[..]);
        }
    }

    /// The screen, to draw.
    pub fn snapshot(&self) -> Snapshot {
        let term = self.term.lock();
        let mut search = self.search.lock().unwrap();
        core::snapshot(&term, &mut search)
    }

    // Selecting.

    /// Start selecting at a cell of the screen: `Simple` for a click and drag,
    /// `Semantic` (a word) for a double click, `Lines` for a triple click,
    /// `Block` for Option-drag.
    pub fn select_begin(&self, ty: SelectionType, row: usize, col: usize, side: Side) {
        core::select_begin(&mut self.term.lock(), ty, row, col, side);
    }

    /// The selection reaches a cell (dragging, or Shift-click).
    pub fn select_update(&self, row: usize, col: usize, side: Side) {
        core::select_update(&mut self.term.lock(), row, col, side);
    }

    pub fn select_all(&self) {
        core::select_all(&mut self.term.lock());
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    pub fn has_selection(&self) -> bool {
        core::selection_text(&self.term.lock()).is_some()
    }

    /// The selected text.
    pub fn selection_text(&self) -> Option<String> {
        core::selection_text(&self.term.lock())
    }

    /// The text between two cells of the screen (a link's).
    pub fn text_between(&self, from: (usize, usize), to: (usize, usize)) -> String {
        let term = self.term.lock();
        term.bounds_to_string(core::grid_point(&term, from.0, from.1), core::grid_point(&term, to.0, to.1))
    }

    // Searching.

    /// Search for a pattern (a regex): the first match from the top of the
    /// screen scrolls into view. `false` for an invalid regex.
    pub fn search(&self, pattern: &str) -> bool {
        let mut term = self.term.lock();
        let ok = self.search.lock().unwrap().set(&mut term, pattern);
        self.vi_follow_search(&mut term);
        ok
    }

    /// The next (or previous) match.
    pub fn search_step(&self, forward: bool) -> bool {
        let mut term = self.term.lock();
        let found = self.search.lock().unwrap().step(&mut term, forward).is_some();
        self.vi_follow_search(&mut term);
        found
    }

    pub fn search_clear(&self) {
        self.search.lock().unwrap().clear();
    }

    /// Whether the pattern searched for is a valid regex.
    pub fn search_valid(&self) -> bool {
        self.search.lock().unwrap().is_valid()
    }

    pub fn search_active(&self) -> bool {
        self.search.lock().unwrap().is_active()
    }

    /// In Vi mode the cursor goes to the current match.
    fn vi_follow_search(&self, term: &mut Term<Proxy>) {
        if !term.mode().contains(TermMode::VI) {
            return;
        }
        if let Some(m) = self.search.lock().unwrap().current.clone() {
            core::vi_goto(term, *m.start());
        }
    }

    // Vi mode.

    pub fn vi_active(&self) -> bool {
        self.mode().contains(TermMode::VI)
    }

    /// Enter or leave Vi mode (⇧⌃Space).
    pub fn toggle_vi(&self) {
        *self.vi.lock().unwrap() = ViState::default();
        core::toggle_vi(&mut self.term.lock());
    }

    /// A key in Vi mode. For `/` and `?` the UI asks for a pattern; `n` and
    /// `N` step through the matches here.
    pub fn vi_key(&self, key: ViKey) -> ViOutcome {
        let outcome = core::vi_key(&mut self.term.lock(), &mut self.vi.lock().unwrap(), key);
        match outcome {
            ViOutcome::Step(forward) => {
                self.search_step(forward);
                ViOutcome::Handled
            }
            other => other,
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The loop ends and drops the PTY, which hangs up the process.
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn size(cols: u16, rows: u16) -> Size {
        Size { cols, rows, cell_w: 8, cell_h: 16 }
    }

    fn sh(script: &str, cols: u16, rows: u16) -> Terminal {
        let argv = vec!["/bin/sh".into(), "-c".into(), script.into()];
        Terminal::spawn(SpawnOptions::new(argv, size(cols, rows))).unwrap()
    }

    /// The screen's text, a row each, trailing blanks and empty rows cut.
    fn screen(t: &Terminal) -> Vec<String> {
        let snap = t.snapshot();
        let mut rows: Vec<String> = (0..snap.rows)
            .map(|r| snap.row(r).iter().filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER)).map(|c| c.c).collect::<String>().trim_end().to_owned())
            .collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_program_s_output_is_on_the_screen() {
        let t = sh("printf 'hello\\r\\nworld'", 20, 5);
        wait_for("the output", || screen(&t) == ["hello", "world"]);
    }

    #[test]
    fn the_output_counter_moves_while_the_program_prints() {
        let t = sh("sleep 0.3; printf one; sleep 0.3; printf two; sleep 5", 20, 5);
        let start = t.output_count();
        wait_for("the first output", || t.output_count() > start);
        let first = t.output_count();
        wait_for("the second output", || t.output_count() > first);
        // Quiet: it stays.
        let quiet = t.output_count();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(t.output_count(), quiet);
    }

    #[test]
    fn the_environment_and_start_folder_are_the_callers() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let mut opts = SpawnOptions::new(vec!["/bin/sh".into(), "-c".into(), "printf \"$FOO $TERM $COLORTERM\\r\\n\"; pwd -P".into()], size(60, 5));
        opts.cwd = Some(dir.clone());
        opts.env = vec![("FOO".into(), "bar".into())];
        let t = Terminal::spawn(opts).unwrap();
        wait_for("the output", || screen(&t).len() == 2);
        assert_eq!(screen(&t), ["bar xterm-256color truecolor".to_string(), dir.display().to_string()]);
    }

    #[test]
    fn the_program_s_title_and_exit_arrive_as_events() {
        let t = sh("printf '\\033]0;hello there\\007'; exit 3", 20, 5);
        let mut seen = Vec::new();
        wait_for("the exit", || {
            seen.extend(t.events());
            seen.iter().any(|e| matches!(e, Event::Exit { .. }))
        });
        assert!(seen.contains(&Event::Title("hello there".into())));
        assert!(seen.contains(&Event::Exit { code: Some(3), signal: None }));
        assert_eq!(t.title().as_deref(), Some("hello there"));
        assert_eq!(t.exit_status(), Some((Some(3), None)));
        assert!(!t.is_running());
    }

    #[test]
    fn a_killed_program_reports_its_signal() {
        let t = sh("kill -9 $$", 20, 5);
        wait_for("the exit", || t.exit_status().is_some());
        assert_eq!(t.exit_status(), Some((None, Some(9))));
    }

    #[test]
    fn bells_and_clipboard_stores_arrive_and_loads_are_refused() {
        let t = sh("printf '\\007\\033]52;c;aGk=\\007\\033]52;c;?\\007'; sleep 0.3; printf done", 30, 5);
        let mut seen = Vec::new();
        wait_for("the output", || {
            seen.extend(t.events());
            screen(&t) == ["done"]
        });
        assert!(seen.contains(&Event::Bell));
        assert!(seen.contains(&Event::ClipboardStore("hi".into())));
        assert_eq!(seen.iter().filter(|e| matches!(e, Event::ClipboardStore(_))).count(), 1);
        assert_eq!(t.bells(), 1);
    }

    #[test]
    fn keys_reach_the_program_and_it_answers() {
        // `cat` echoes what it reads: the PTY's line discipline shows it too.
        let t = Terminal::spawn(SpawnOptions::new(vec!["/bin/cat".into()], size(30, 5))).unwrap();
        t.write_text("typed\n");
        wait_for("the echo", || screen(&t).iter().filter(|r| *r == "typed").count() == 2);
        assert!(t.is_running());
        assert!(t.pid() > 0);
        // Resizing tells the program: stty reports the new size.
        let t = sh("sleep 0.5; stty size", 30, 5);
        t.resize(size(40, 7));
        wait_for("stty", || screen(&t).iter().any(|r| r == "7 40"));
    }

    #[test]
    fn resizing_reflows_the_text() {
        let t = sh("printf '0123456789abcdefghij'; sleep 5", 20, 4);
        wait_for("the output", || screen(&t) == ["0123456789abcdefghij"]);
        t.resize(size(10, 4));
        // (The rows below the text stay, so its first row goes into the scrollback.)
        t.scroll(Scroll::Top);
        assert_eq!(screen(&t), ["0123456789", "abcdefghij"]);
        t.resize(size(20, 4));
        t.scroll(Scroll::Top);
        assert_eq!(screen(&t), ["0123456789abcdefghij"]);
    }

    #[test]
    fn the_foreground_process_is_the_shell_then_what_it_runs() {
        let t = Terminal::spawn(SpawnOptions::new(vec!["/bin/sh".into()], size(40, 5))).unwrap();
        // (macOS's sh starts bash, so either name.)
        wait_for("the prompt", || t.at_shell_prompt() && matches!(t.foreground_process().as_deref(), Some("sh" | "bash" | "dash")));
        t.write_text("sleep 30\n");
        wait_for("sleep", || t.foreground_process().as_deref() == Some("sleep"));
        assert!(!t.at_shell_prompt());
    }

    #[test]
    fn the_mode_the_program_sets_changes_the_keys() {
        let t = sh("printf '\\033[>1u\\033[?2004h\\033[?1h'; sleep 5", 20, 5);
        wait_for("the modes", || t.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
        assert!(t.mode().contains(TermMode::BRACKETED_PASTE | TermMode::APP_CURSOR));
    }

    #[test]
    fn the_scrollback_and_the_wheel() {
        let t = sh("for i in 1 2 3 4 5 6 7 8 9 10; do echo line$i; done; sleep 5", 20, 4);
        wait_for("the output", || t.history_size() >= 6);
        assert_eq!(t.snapshot().display_offset, 0);
        t.wheel(3, 0, 0, Mods::NONE);
        assert_eq!(t.snapshot().display_offset, 3);
        t.wheel(-2, 0, 0, Mods::NONE);
        assert_eq!(t.snapshot().display_offset, 1);
        t.selection_text();
    }

    #[test]
    fn the_scroll_position_is_read_and_set() {
        let t = sh("for i in 1 2 3 4 5 6 7 8 9 10; do echo line$i; done; sleep 5", 20, 4);
        wait_for("the output", || t.history_size() >= 6);
        let at = t.scroll_position();
        assert_eq!((at.offset, at.rows, at.alt_screen), (0, 4, false));
        assert!(at.history >= 6);
        t.scroll_to(3);
        assert_eq!(t.scroll_position().offset, 3);
        assert_eq!(t.snapshot().display_offset, 3);
        t.scroll_to(1);
        assert_eq!(t.scroll_position().offset, 1);
        // Past the top: the top. Zero: the bottom.
        t.scroll_to(10_000);
        assert_eq!(t.scroll_position().offset, at.history);
        t.scroll_to(0);
        assert_eq!(t.scroll_position().offset, 0);
    }

    fn output_terminal(cols: u16, rows: u16) -> Terminal {
        Terminal::output(size(cols, rows), Palette::default(), 100, None)
    }

    #[test]
    fn an_output_terminal_shows_what_it_is_fed() {
        let t = output_terminal(20, 4);
        t.feed(b"one\r\ntwo\r\n");
        t.feed(b"\x1b[1mbold\x1b[0m and \x1b[31mred\x1b[0m");
        assert_eq!(screen(&t), ["one", "two", "bold and red"]);
        let snap = t.snapshot();
        let bold = &snap.row(2)[0];
        assert!(bold.flags.contains(Flags::BOLD));
        assert_eq!(t.text_between((0, 0), (0, 2)), "one");
        // An escape sequence cut between two feeds is still read as one.
        t.feed(b"\r\n\x1b[3");
        t.feed(b"1mcut");
        let snap = t.snapshot();
        assert_eq!(snap.row(3)[0].c, 'c');
        assert_ne!(snap.row(3)[0].fg, snap.row(2)[0].fg);
    }

    #[test]
    fn an_output_terminal_keeps_a_scrollback_that_can_be_scrolled() {
        let t = output_terminal(20, 4);
        for i in 1..=10 {
            t.feed(format!("line{i}\r\n").as_bytes());
        }
        let at = t.scroll_position();
        assert_eq!((at.offset, at.rows, at.alt_screen), (0, 4, false));
        assert!(at.history >= 7);
        t.scroll_to(3);
        assert_eq!(t.scroll_position().offset, 3);
        t.scroll(Scroll::Bottom);
        assert_eq!(t.scroll_position().offset, 0);
    }

    #[test]
    fn an_output_terminal_has_no_process_and_takes_no_input() {
        let t = output_terminal(20, 4);
        assert_eq!(t.pid(), 0);
        assert!(!t.takes_input() && sh("sleep 5", 20, 4).takes_input());
        assert!(t.is_running() && t.exit_status().is_none());
        assert_eq!((t.foreground_process(), t.current_dir(), t.at_shell_prompt()), (None, None, false));
        t.write_text("typed");
        t.paste("pasted");
        t.resize(size(30, 6));
        assert_eq!(t.snapshot().cols, 30);
        assert!(screen(&t).is_empty());
        // Selecting and searching work as in any terminal.
        t.feed(b"find me");
        assert!(t.search("find"));
        t.select_all();
        assert_eq!(t.selection_text().as_deref(), Some("find me"));
        // A terminal with a process takes no feed.
        let real = sh("sleep 5", 20, 4);
        real.feed(b"not shown");
        assert!(screen(&real).is_empty());
    }

    #[test]
    fn the_alternate_screen_is_said_so() {
        let t = sh("printf '\\033[?1049h'; sleep 5", 20, 4);
        wait_for("the alternate screen", || t.scroll_position().alt_screen);
    }

    #[test]
    fn a_utf8_locale_is_set_when_the_environment_has_none() {
        let env = |vars: &'static [(&'static str, &'static str)]| move |n: &str| vars.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string());
        let fallback = Some(("LC_CTYPE", if cfg!(target_os = "macos") { "UTF-8" } else { "C.UTF-8" }));
        assert_eq!(utf8_locale(env(&[])), fallback, "nothing set (opened from the Finder)");
        assert_eq!(utf8_locale(env(&[("LANG", "C")])), fallback);
        assert_eq!(utf8_locale(env(&[("LANG", "en_US.ISO8859-1")])), fallback);
        assert_eq!(utf8_locale(env(&[("LANG", "en_US.UTF-8")])), None);
        assert_eq!(utf8_locale(env(&[("LANG", "de_DE.utf8")])), None);
        assert_eq!(utf8_locale(env(&[("LANG", "C"), ("LC_CTYPE", "en_US.UTF-8")])), None, "LC_CTYPE comes first");
        assert_eq!(utf8_locale(env(&[("LANG", "en_US.UTF-8"), ("LC_CTYPE", "C")])), fallback);
        assert_eq!(utf8_locale(env(&[("LC_ALL", "C")])), None, "LC_ALL wins: left as it is");
    }

    #[test]
    fn the_configured_shell_and_its_arguments() {
        let login = ["-l".to_string()];
        assert_eq!(shell_argv(Some("/opt/homebrew/bin/bash"), &login), ["/opt/homebrew/bin/bash", "-l"]);
        assert_eq!(shell_argv(Some("/bin/zsh"), &[]), ["/bin/zsh"]);
        assert_eq!(shell_argv(Some("bash"), &["--login".into(), "-i".into()]), ["bash", "--login", "-i"]);
        // Not set: the user's shell.
        assert_eq!(shell_argv(None, &login), login_shell());
        assert_eq!(shell_argv(Some("  "), &login), login_shell());
    }

    #[test]
    fn a_program_runs_through_the_shell_with_its_words_quoted() {
        let bash = vec!["/opt/homebrew/bin/bash".to_string(), "-l".into()];
        let argv = ["claude".to_string(), "--resume".into(), "it's".into()];
        assert_eq!(in_shell(bash.clone(), &argv), ["/opt/homebrew/bin/bash", "-l", "-i", "-c", r"exec 'claude' '--resume' 'it'\''s'"]);
        // Already interactive: not twice.
        let zsh = vec!["/bin/zsh".to_string(), "-i".into()];
        assert_eq!(in_shell(zsh, &argv[..1]), ["/bin/zsh", "-i", "-c", "exec 'claude'"]);
    }

    /// The shell runs the program in its place, its words as given: the terminal's process is
    /// the program.
    #[test]
    fn a_program_in_the_shell_is_the_terminals_process() {
        let t = Terminal::spawn(SpawnOptions::new(in_shell(vec!["/bin/sh".into()], &["/bin/echo".into(), "a  b".into(), "it's".into()]), size(40, 5))).unwrap();
        wait_for("the output", || screen(&t).iter().any(|r| r == "a  b it's"));
        let t = Terminal::spawn(SpawnOptions::new(in_shell(vec!["/bin/sh".into()], &["/bin/sleep".into(), "5".into()]), size(40, 5))).unwrap();
        wait_for("exec", || proc::process_name(t.pid()).as_deref() == Some("sleep"));
    }

    #[test]
    fn the_login_shell_is_a_login_shell() {
        let argv = login_shell();
        assert_eq!(argv.len(), 2);
        assert_eq!(argv[1], "-l");
        assert!(argv[0].starts_with('/'));
    }
}
