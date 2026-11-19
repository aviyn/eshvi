//! Keys as the bytes a terminal program reads: the legacy xterm sequences,
//! and the kitty keyboard protocol when the program turned it on (the
//! `TermMode` flags). Pure functions: the UI maps its key events to `KeyEvent`
//! and writes what comes back.

use alacritty_terminal::term::TermMode;

/// A key, by what it is and not by what it types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A key that types a character: the character it types with no modifier
    /// on a US layout (`a`, `1`, `;`, space), whatever the layout.
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1 to F20.
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    /// Option on a Mac.
    pub alt: bool,
    pub ctrl: bool,
    /// Command on a Mac.
    pub sup: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, sup: false };

    pub fn any(self) -> bool {
        self.shift || self.alt || self.ctrl || self.sup
    }

    /// The protocols' modifier number: 1 plus a bit each for shift, alt, ctrl
    /// and super.
    fn param(self) -> u32 {
        1 + self.shift as u32 + 2 * self.alt as u32 + 4 * self.ctrl as u32 + 8 * self.sup as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Mods,
    pub kind: KeyKind,
    /// What the key types with Shift held (US layout), for the kitty
    /// protocol's alternate keys and for Shift with Ctrl or Alt. `None` for
    /// letters: the uppercase one.
    pub shifted: Option<char>,
}

impl KeyEvent {
    pub fn press(key: Key, mods: Mods) -> Self {
        Self { key, mods, kind: KeyKind::Press, shifted: None }
    }
}

/// How keys are sent, from the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyOptions {
    /// Option is Meta (ESC before the key) and doesn't compose characters.
    pub option_as_meta: bool,
}

impl Default for KeyOptions {
    fn default() -> Self {
        Self { option_as_meta: true }
    }
}

const ESC: u8 = 0x1b;

fn kitty_on(mode: TermMode) -> bool {
    mode.intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC)
}

/// Whether the platform also sends the text of this key as a text event, so
/// that one is to be dropped once the key itself was sent: a character key
/// with no Ctrl or Command (Option composes a character).
pub fn types_text(ev: &KeyEvent) -> bool {
    matches!(ev.key, Key::Char(_)) && !ev.mods.ctrl && !ev.mods.sup
}

/// The bytes for a key, or `None` when it sends nothing here: a plain or
/// shifted character key (its text arrives as a text event), a Command
/// shortcut, or a release nobody asked for.
pub fn encode_key(ev: &KeyEvent, mode: TermMode, opts: KeyOptions) -> Option<Vec<u8>> {
    // Command's chords are the app's (⌘P, ⌘S…) under the kitty protocol too,
    // so a program never gets one the menu also runs; only the line-editing
    // ones are sent, as legacy bytes.
    if ev.mods.sup {
        return if ev.kind == KeyKind::Release { None } else { legacy(ev, mode, opts) };
    }
    if kitty_on(mode) {
        return kitty(ev, mode, opts);
    }
    if ev.kind == KeyKind::Release {
        return None;
    }
    legacy(ev, mode, opts)
}

/// The control character for Ctrl with a key, as xterm sends it.
fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | ' ' | '2' | '`' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '~' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// The character a key types with its modifiers (Shift).
fn typed(ev: &KeyEvent, c: char) -> char {
    if ev.mods.shift { ev.shifted.unwrap_or_else(|| c.to_ascii_uppercase()) } else { c }
}

/// `ESC` before `bytes` when Alt is Meta.
fn meta(alt: bool, mut bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        bytes.insert(0, ESC);
    }
    bytes
}

fn cursor_letter(key: Key) -> Option<char> {
    Some(match key {
        Key::Up => 'A',
        Key::Down => 'B',
        Key::Right => 'C',
        Key::Left => 'D',
        Key::Home => 'H',
        Key::End => 'F',
        _ => return None,
    })
}

fn legacy(ev: &KeyEvent, mode: TermMode, opts: KeyOptions) -> Option<Vec<u8>> {
    let m = ev.mods;
    // Command's chords are the app's (and a few are line editing, below).
    if m.sup {
        return match (ev.key, m.shift || m.alt || m.ctrl) {
            // As iTerm does: Command with Left, Right and Backspace edit the line.
            (Key::Left, false) => Some(vec![0x01]),
            (Key::Right, false) => Some(vec![0x05]),
            (Key::Backspace, false) => Some(vec![0x15]),
            _ => None,
        };
    }
    let alt = m.alt && opts.option_as_meta;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    match ev.key {
        Key::Char(c) => {
            if !(m.ctrl || alt) {
                return None;
            }
            let c = typed(ev, c);
            let bytes = if m.ctrl {
                // Shift is ignored: Ctrl+Shift+A is Ctrl+A.
                match ctrl_byte(c) {
                    Some(b) => vec![b],
                    None => c.to_string().into_bytes(),
                }
            } else {
                c.to_string().into_bytes()
            };
            Some(meta(alt, bytes))
        }
        // Shift+Enter is a newline in the line editors and agents that take
        // Alt+Enter for one.
        Key::Enter => Some(meta(m.shift || alt, vec![b'\r'])),
        Key::Tab if m.shift => Some(meta(alt, b"\x1b[Z".to_vec())),
        Key::Tab => Some(meta(alt, vec![b'\t'])),
        Key::Backspace => Some(meta(alt, vec![if m.ctrl { 0x08 } else { 0x7f }])),
        Key::Escape => Some(meta(alt, vec![ESC])),
        // Option with the arrows moves by words, as in the Terminal app.
        Key::Left if alt && !m.ctrl && !m.shift => Some(b"\x1bb".to_vec()),
        Key::Right if alt && !m.ctrl && !m.shift => Some(b"\x1bf".to_vec()),
        Key::Up | Key::Down | Key::Right | Key::Left | Key::Home | Key::End => {
            let letter = cursor_letter(ev.key)?;
            Some(if m.any() {
                format!("\x1b[1;{}{letter}", m.param()).into_bytes()
            } else if app_cursor {
                vec![ESC, b'O', letter as u8]
            } else {
                vec![ESC, b'[', letter as u8]
            })
        }
        Key::Insert | Key::Delete | Key::PageUp | Key::PageDown => Some(tilde(tilde_number(ev.key), m)),
        Key::F(n) => function(n, m, false),
    }
}

fn tilde_number(key: Key) -> u32 {
    match key {
        Key::Insert => 2,
        Key::Delete => 3,
        Key::PageUp => 5,
        _ => 6,
    }
}

/// `ESC [ n ~`, with a modifier number as `ESC [ n ; m ~`.
fn tilde(n: u32, m: Mods) -> Vec<u8> {
    if m.any() { format!("\x1b[{n};{}~", m.param()) } else { format!("\x1b[{n}~") }.into_bytes()
}

/// A function key's number and final: F1, F2 and F4 are `P`, `Q` and `S` (F3
/// is `13 ~`, as `R` would be a cursor position report), the rest `n ~`.
fn function_parts(n: u8) -> Option<(u32, char)> {
    Some(match n {
        1 => (1, 'P'),
        2 => (1, 'Q'),
        3 => (13, '~'),
        4 => (1, 'S'),
        5 => (15, '~'),
        6 => (17, '~'),
        7 => (18, '~'),
        8 => (19, '~'),
        9 => (20, '~'),
        10 => (21, '~'),
        11 => (23, '~'),
        12 => (24, '~'),
        13 => (25, '~'),
        14 => (26, '~'),
        15 => (28, '~'),
        16 => (29, '~'),
        17 => (31, '~'),
        18 => (32, '~'),
        19 => (33, '~'),
        20 => (34, '~'),
        _ => return None,
    })
}

/// F1 to F20. Legacy F1 to F4 are `ESC O P` to `S`; with modifiers (and in
/// the kitty protocol) `ESC [ 1 ; m P`.
fn function(n: u8, m: Mods, kitty: bool) -> Option<Vec<u8>> {
    let (number, final_byte) = function_parts(n)?;
    Some(
        match (final_byte, m.any()) {
            ('~', _) => return Some(tilde(number, m)),
            (letter, true) => format!("\x1b[1;{}{letter}", m.param()),
            (letter, false) if kitty => format!("\x1b[{letter}"),
            (letter, false) => format!("\x1bO{letter}"),
        }
        .into_bytes(),
    )
}

/// `CSI code ; mods : event ; text u` (or the key's own final), leaving out
/// what has its default.
fn kitty_seq(code: &str, final_byte: char, ev: &KeyEvent, text: Option<&str>) -> Vec<u8> {
    let event = match ev.kind {
        KeyKind::Press => 1,
        KeyKind::Repeat => 2,
        KeyKind::Release => 3,
    };
    let mods = ev.mods.param();
    let mut out = format!("\x1b[{code}");
    if mods != 1 || event != 1 || text.is_some() {
        out.push(';');
        out.push_str(&mods.to_string());
        if event != 1 {
            out.push_str(&format!(":{event}"));
        }
    }
    if let Some(text) = text {
        let codes: Vec<String> = text.chars().map(|c| (c as u32).to_string()).collect();
        out.push(';');
        out.push_str(&codes.join(":"));
    }
    out.push(final_byte);
    out.into_bytes()
}

/// The kitty keyboard protocol's progressive enhancement: the flags the
/// program set decide which keys are escape codes and what they carry.
fn kitty(ev: &KeyEvent, mode: TermMode, opts: KeyOptions) -> Option<Vec<u8>> {
    let all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let m = ev.mods;
    // Repeats and releases are sent only when asked for. Keys that make text
    // (and Enter, Tab and Backspace) report them only with every key an
    // escape code. A repeat that isn't reported is a press.
    let makes_text = matches!(ev.key, Key::Char(_) | Key::Enter | Key::Tab | Key::Backspace);
    if ev.kind != KeyKind::Press && !(events && (all || !makes_text)) {
        if ev.kind == KeyKind::Release {
            return None;
        }
        return kitty(&KeyEvent { kind: KeyKind::Press, ..*ev }, mode, opts);
    }
    // Option composes characters when it isn't Meta: the text is sent.
    let composing = m.alt && !opts.option_as_meta && !m.ctrl && !m.sup;
    match ev.key {
        Key::Char(c) => {
            if (composing || !m.any() || (m.shift && !(m.ctrl || m.alt || m.sup))) && !all {
                return None;
            }
            let alternate = if mode.contains(TermMode::REPORT_ALTERNATE_KEYS) && m.shift {
                format!(":{}", typed(ev, c) as u32)
            } else {
                String::new()
            };
            let text = (all && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) && !(m.ctrl || m.alt || m.sup) && ev.kind != KeyKind::Release)
                .then(|| typed(ev, c).to_string());
            Some(kitty_seq(&format!("{}{alternate}", c as u32), 'u', ev, text.as_deref()))
        }
        Key::Enter | Key::Tab | Key::Backspace => {
            if !all && !m.any() {
                return legacy(ev, TermMode::empty(), opts);
            }
            let code = match ev.key {
                Key::Enter => 13,
                Key::Tab => 9,
                _ => 127,
            };
            Some(kitty_seq(&code.to_string(), 'u', ev, None))
        }
        Key::Escape => Some(kitty_seq("27", 'u', ev, None)),
        Key::Up | Key::Down | Key::Right | Key::Left | Key::Home | Key::End => {
            if ev.kind == KeyKind::Press && !m.any() {
                return legacy(ev, mode, opts);
            }
            Some(kitty_seq("1", cursor_letter(ev.key)?, ev, None))
        }
        Key::Insert | Key::Delete | Key::PageUp | Key::PageDown => {
            if ev.kind == KeyKind::Press && !m.any() {
                return legacy(ev, mode, opts);
            }
            Some(kitty_seq(&tilde_number(ev.key).to_string(), '~', ev, None))
        }
        Key::F(n) => {
            if ev.kind == KeyKind::Press && !m.any() {
                return function(n, m, true);
            }
            let (number, final_byte) = function_parts(n)?;
            Some(kitty_seq(&number.to_string(), final_byte, ev, None))
        }
    }
}

/// What pasted text becomes: bracketed when the program asked for it (with
/// the escapes that could end the bracket removed), else with newlines as
/// the carriage returns Enter sends.
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let clean = text.replace(['\x1b', '\x03'], "");
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(clean.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// The arrow key a wheel notch sends on the alternate screen.
pub fn arrow(up: bool, mode: TermMode) -> Vec<u8> {
    let letter = if up { b'A' } else { b'B' };
    if mode.contains(TermMode::APP_CURSOR) { vec![ESC, b'O', letter] } else { vec![ESC, b'[', letter] }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO: Mods = Mods::NONE;
    const SHIFT: Mods = Mods { shift: true, ..Mods::NONE };
    const ALT: Mods = Mods { alt: true, ..Mods::NONE };
    const CTRL: Mods = Mods { ctrl: true, ..Mods::NONE };
    const CMD: Mods = Mods { sup: true, ..Mods::NONE };

    fn ev(key: Key, mods: Mods) -> KeyEvent {
        KeyEvent::press(key, mods)
    }

    fn shifted(c: char, mods: Mods, s: char) -> KeyEvent {
        KeyEvent { shifted: Some(s), ..ev(Key::Char(c), mods) }
    }

    /// The bytes as a readable string (`ESC` as `\x1b`).
    fn bytes(e: KeyEvent, mode: TermMode) -> Option<String> {
        encode_key(&e, mode, KeyOptions::default()).map(|b| b.iter().map(|b| std::ascii::escape_default(*b).to_string()).collect())
    }

    fn legacy_of(e: KeyEvent) -> Option<String> {
        bytes(e, TermMode::empty())
    }

    #[test]
    fn legacy_keys() {
        let table: &[(KeyEvent, Option<&str>)] = &[
            // Text comes as text events.
            (ev(Key::Char('a'), NO), None),
            (shifted('a', SHIFT, 'A'), None),
            (ev(Key::Char(' '), NO), None),
            // Control characters.
            (ev(Key::Char('a'), CTRL), Some("\\x01")),
            (ev(Key::Char('c'), CTRL), Some("\\x03")),
            (ev(Key::Char('z'), CTRL), Some("\\x1a")),
            (ev(Key::Char('a'), Mods { ctrl: true, shift: true, ..NO }), Some("\\x01")),
            (ev(Key::Char(' '), CTRL), Some("\\x00")),
            (ev(Key::Char('['), CTRL), Some("\\x1b")),
            (ev(Key::Char('\\'), CTRL), Some("\\x1c")),
            (ev(Key::Char(']'), CTRL), Some("\\x1d")),
            (shifted('6', Mods { ctrl: true, shift: true, ..NO }, '^'), Some("\\x1e")),
            (ev(Key::Char('-'), CTRL), Some("\\x1f")),
            (ev(Key::Char('/'), CTRL), Some("\\x1f")),
            // Option is Meta.
            (ev(Key::Char('x'), ALT), Some("\\x1bx")),
            (shifted('x', Mods { alt: true, shift: true, ..NO }, 'X'), Some("\\x1bX")),
            (ev(Key::Char('d'), Mods { alt: true, ctrl: true, ..NO }), Some("\\x1b\\x04")),
            // Command's chords are the app's.
            (ev(Key::Char('c'), CMD), None),
            (ev(Key::Char('k'), CMD), None),
            // The editing keys.
            (ev(Key::Enter, NO), Some("\\r")),
            (ev(Key::Enter, SHIFT), Some("\\x1b\\r")),
            (ev(Key::Enter, ALT), Some("\\x1b\\r")),
            (ev(Key::Tab, NO), Some("\\t")),
            (ev(Key::Tab, SHIFT), Some("\\x1b[Z")),
            (ev(Key::Backspace, NO), Some("\\x7f")),
            (ev(Key::Backspace, CTRL), Some("\\x08")),
            (ev(Key::Backspace, ALT), Some("\\x1b\\x7f")),
            (ev(Key::Escape, NO), Some("\\x1b")),
            // Cursor keys, with and without modifiers.
            (ev(Key::Up, NO), Some("\\x1b[A")),
            (ev(Key::Down, NO), Some("\\x1b[B")),
            (ev(Key::Right, NO), Some("\\x1b[C")),
            (ev(Key::Left, NO), Some("\\x1b[D")),
            (ev(Key::Home, NO), Some("\\x1b[H")),
            (ev(Key::End, NO), Some("\\x1b[F")),
            (ev(Key::Left, SHIFT), Some("\\x1b[1;2D")),
            (ev(Key::Right, CTRL), Some("\\x1b[1;5C")),
            (ev(Key::Up, Mods { ctrl: true, shift: true, ..NO }), Some("\\x1b[1;6A")),
            (ev(Key::Left, ALT), Some("\\x1bb")),
            (ev(Key::Right, ALT), Some("\\x1bf")),
            (ev(Key::Left, CMD), Some("\\x01")),
            (ev(Key::Right, CMD), Some("\\x05")),
            (ev(Key::Backspace, CMD), Some("\\x15")),
            (ev(Key::Insert, NO), Some("\\x1b[2~")),
            (ev(Key::Delete, NO), Some("\\x1b[3~")),
            (ev(Key::PageUp, NO), Some("\\x1b[5~")),
            (ev(Key::PageDown, NO), Some("\\x1b[6~")),
            (ev(Key::Delete, CTRL), Some("\\x1b[3;5~")),
            (ev(Key::F(1), NO), Some("\\x1bOP")),
            (ev(Key::F(4), NO), Some("\\x1bOS")),
            (ev(Key::F(5), NO), Some("\\x1b[15~")),
            (ev(Key::F(12), NO), Some("\\x1b[24~")),
            (ev(Key::F(2), SHIFT), Some("\\x1b[1;2Q")),
            (ev(Key::F(6), CTRL), Some("\\x1b[17;5~")),
        ];
        for (e, want) in table {
            assert_eq!(legacy_of(*e).as_deref(), *want, "{:?} {:?}", e.key, e.mods);
        }
    }

    #[test]
    fn application_cursor_keys() {
        let app = TermMode::APP_CURSOR;
        assert_eq!(bytes(ev(Key::Up, NO), app).as_deref(), Some("\\x1bOA"));
        assert_eq!(bytes(ev(Key::Home, NO), app).as_deref(), Some("\\x1bOH"));
        // Modifiers use the CSI form whatever the mode.
        assert_eq!(bytes(ev(Key::Up, CTRL), app).as_deref(), Some("\\x1b[1;5A"));
    }

    #[test]
    fn option_that_composes_sends_no_meta() {
        let opts = KeyOptions { option_as_meta: false };
        let e = ev(Key::Char('e'), ALT);
        assert_eq!(encode_key(&e, TermMode::empty(), opts), None);
        assert_eq!(encode_key(&ev(Key::Enter, ALT), TermMode::empty(), opts), Some(b"\r".to_vec()));
        assert!(types_text(&e) && !types_text(&ev(Key::Char('e'), CTRL)) && !types_text(&ev(Key::Up, NO)));
    }

    #[test]
    fn release_sends_nothing_in_legacy() {
        let e = KeyEvent { kind: KeyKind::Release, ..ev(Key::Up, NO) };
        assert_eq!(legacy_of(e), None);
    }

    #[test]
    fn kitty_disambiguate() {
        let k = TermMode::DISAMBIGUATE_ESC_CODES;
        let table: &[(KeyEvent, Option<&str>)] = &[
            // Text still comes as text.
            (ev(Key::Char('a'), NO), None),
            (shifted('a', SHIFT, 'A'), None),
            // Escape and the chords that legacy can't tell apart.
            (ev(Key::Escape, NO), Some("\\x1b[27u")),
            (ev(Key::Char('c'), CTRL), Some("\\x1b[99;5u")),
            (ev(Key::Char('a'), ALT), Some("\\x1b[97;3u")),
            (ev(Key::Char('a'), Mods { ctrl: true, shift: true, ..NO }), Some("\\x1b[97;6u")),
            // Command's chords stay the app's, as in legacy mode.
            (ev(Key::Char('a'), CMD), None),
            (ev(Key::Char('p'), Mods { sup: true, shift: true, ..NO }), None),
            (ev(Key::Enter, CMD), None),
            (ev(Key::Left, CMD), Some("\\x01")),
            // Enter, Tab and Backspace keep their bytes until modified.
            (ev(Key::Enter, NO), Some("\\r")),
            (ev(Key::Tab, NO), Some("\\t")),
            (ev(Key::Backspace, NO), Some("\\x7f")),
            (ev(Key::Enter, SHIFT), Some("\\x1b[13;2u")),
            (ev(Key::Enter, CTRL), Some("\\x1b[13;5u")),
            (ev(Key::Tab, SHIFT), Some("\\x1b[9;2u")),
            (ev(Key::Backspace, CTRL), Some("\\x1b[127;5u")),
            // The others as in legacy mode, with modifiers in the CSI form.
            (ev(Key::Up, NO), Some("\\x1b[A")),
            (ev(Key::Up, SHIFT), Some("\\x1b[1;2A")),
            (ev(Key::Delete, NO), Some("\\x1b[3~")),
            (ev(Key::Delete, ALT), Some("\\x1b[3;3~")),
            (ev(Key::F(1), NO), Some("\\x1b[P")),
            (ev(Key::F(3), NO), Some("\\x1b[13~")),
            (ev(Key::F(5), NO), Some("\\x1b[15~")),
            (ev(Key::F(1), CTRL), Some("\\x1b[1;5P")),
            (ev(Key::F(3), CTRL), Some("\\x1b[13;5~")),
        ];
        for (e, want) in table {
            assert_eq!(bytes(*e, k).as_deref(), *want, "{:?} {:?}", e.key, e.mods);
        }
    }

    #[test]
    fn kitty_report_all_keys_events_and_text() {
        let all = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(bytes(ev(Key::Char('a'), NO), all).as_deref(), Some("\\x1b[97u"));
        assert_eq!(bytes(shifted('a', SHIFT, 'A'), all).as_deref(), Some("\\x1b[97;2u"));
        assert_eq!(bytes(ev(Key::Enter, NO), all).as_deref(), Some("\\x1b[13u"));
        // Releases and repeats need the event types flag.
        let release = KeyEvent { kind: KeyKind::Release, ..ev(Key::Char('a'), NO) };
        assert_eq!(bytes(release, all), None);
        let with_events = all | TermMode::REPORT_EVENT_TYPES;
        assert_eq!(bytes(release, with_events).as_deref(), Some("\\x1b[97;1:3u"));
        let repeat = KeyEvent { kind: KeyKind::Repeat, ..ev(Key::Char('a'), CTRL) };
        assert_eq!(bytes(repeat, with_events).as_deref(), Some("\\x1b[97;5:2u"));
        let release_esc = KeyEvent { kind: KeyKind::Release, ..ev(Key::Escape, NO) };
        assert_eq!(bytes(release_esc, TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES).as_deref(), Some("\\x1b[27;1:3u"));
        // Enter's release only with every key reported.
        let release_enter = KeyEvent { kind: KeyKind::Release, ..ev(Key::Enter, NO) };
        assert_eq!(bytes(release_enter, TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES), None);
        // The text the key types, as codepoints.
        let text = with_events | TermMode::REPORT_ASSOCIATED_TEXT;
        assert_eq!(bytes(shifted('a', SHIFT, 'A'), text).as_deref(), Some("\\x1b[97;2;65u"));
        // Alternate keys: the shifted one after a colon.
        let alts = all | TermMode::REPORT_ALTERNATE_KEYS;
        assert_eq!(bytes(shifted('a', SHIFT, 'A'), alts).as_deref(), Some("\\x1b[97:65;2u"));
    }

    #[test]
    fn kitty_option_that_composes_still_sends_text() {
        let opts = KeyOptions { option_as_meta: false };
        let e = ev(Key::Char('a'), ALT);
        assert_eq!(encode_key(&e, TermMode::DISAMBIGUATE_ESC_CODES, opts), None);
        assert_eq!(encode_key(&ev(Key::Char('a'), CTRL), TermMode::DISAMBIGUATE_ESC_CODES, opts), Some(b"\x1b[97;5u".to_vec()));
    }

    #[test]
    fn pasting() {
        assert_eq!(encode_paste("a\nb\r\nc", TermMode::empty()), b"a\rb\rc");
        assert_eq!(encode_paste("hi", TermMode::BRACKETED_PASTE), b"\x1b[200~hi\x1b[201~");
        // (⌘V with only an image on the clipboard: Claude Code reads the image then.)
        assert_eq!(encode_paste("", TermMode::BRACKETED_PASTE), b"\x1b[200~\x1b[201~");
        assert_eq!(encode_paste("a\x1b[201~b\nc", TermMode::BRACKETED_PASTE), b"\x1b[200~a[201~b\nc\x1b[201~");
    }

    #[test]
    fn the_wheel_on_the_alternate_screen_sends_arrows() {
        assert_eq!(arrow(true, TermMode::empty()), b"\x1b[A");
        assert_eq!(arrow(false, TermMode::APP_CURSOR), b"\x1bOB");
    }
}
