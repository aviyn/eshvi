//! Translation of egui keyboard/mouse input to Neovim key notation and
//! `nvim_input_mouse` arguments.

use egui::{Key, Modifiers};

/// Turns egui `Key` and `Text` events into strings for `nvim_input`.
///
/// Printable characters normally arrive as `Text` events. When a key
/// is sent with modifiers (`<C-x>`, `<M-x>`), the `Text` event that the
/// platform may also emit for it (e.g. Option+x → "≈" on macOS) is
/// swallowed so characters aren't doubled.
pub struct KeyTranslator {
    /// Treat Alt/Option as Meta (`<M-x>`), like a terminal configured
    /// with "Option as Meta".
    pub alt_is_meta: bool,
    suppress_text: bool,
}

impl Default for KeyTranslator {
    fn default() -> Self {
        Self { alt_is_meta: true, suppress_text: false }
    }
}

/// Neovim names for keys that never produce text.
fn special_name(key: Key) -> Option<&'static str> {
    use Key::*;
    Some(match key {
        ArrowDown => "Down",
        ArrowLeft => "Left",
        ArrowRight => "Right",
        ArrowUp => "Up",
        Escape => "Esc",
        Tab => "Tab",
        Backspace => "BS",
        Enter => "CR",
        Insert => "Insert",
        Delete => "Del",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        F13 => "F13",
        F14 => "F14",
        F15 => "F15",
        F16 => "F16",
        F17 => "F17",
        F18 => "F18",
        F19 => "F19",
        F20 => "F20",
        _ => return None,
    })
}

/// Character a key produces (US layout), optionally shifted.
pub(crate) fn key_char(key: Key, shift: bool) -> Option<char> {
    use Key::*;
    let (plain, shifted) = match key {
        Space => (' ', ' '),
        Colon => (':', ':'),
        Comma => (',', '<'),
        Backslash => ('\\', '|'),
        Slash => ('/', '?'),
        Pipe => ('|', '|'),
        Questionmark => ('?', '?'),
        Exclamationmark => ('!', '!'),
        OpenBracket => ('[', '{'),
        CloseBracket => (']', '}'),
        OpenCurlyBracket => ('{', '{'),
        CloseCurlyBracket => ('}', '}'),
        Backtick => ('`', '~'),
        Minus => ('-', '_'),
        Period => ('.', '>'),
        Plus => ('+', '+'),
        Equals => ('=', '+'),
        Semicolon => (';', ':'),
        Quote => ('\'', '"'),
        Num0 => ('0', ')'),
        Num1 => ('1', '!'),
        Num2 => ('2', '@'),
        Num3 => ('3', '#'),
        Num4 => ('4', '$'),
        Num5 => ('5', '%'),
        Num6 => ('6', '^'),
        Num7 => ('7', '&'),
        Num8 => ('8', '*'),
        Num9 => ('9', '('),
        _ => {
            let name = key.name();
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() || !c.is_ascii_alphabetic() {
                return None;
            }
            let lower = c.to_ascii_lowercase();
            (lower, lower.to_ascii_uppercase())
        }
    };
    Some(if shift { shifted } else { plain })
}

/// Key notation for a character inside `<...>`.
fn char_name(c: char) -> String {
    match c {
        '<' => "lt".into(),
        '\\' => "Bslash".into(),
        '|' => "Bar".into(),
        ' ' => "Space".into(),
        c => c.to_string(),
    }
}

/// Escape literal text for `nvim_input`.
pub fn escape_text(text: &str) -> String {
    text.replace('<', "<lt>")
}

impl KeyTranslator {
    /// Call at the start of each batch of events.
    pub fn begin(&mut self) {
        self.suppress_text = false;
    }

    /// A key press. `physical` is the layout-independent key, used when
    /// Alt is held because the logical key is then the composed symbol.
    pub fn key(&mut self, key: Key, physical: Option<Key>, m: Modifiers) -> Option<String> {
        let cmd = m.mac_cmd || (m.command && !m.ctrl);
        let alt = m.alt && self.alt_is_meta;

        if let Some(name) = special_name(key) {
            let mut prefix = String::new();
            if m.ctrl {
                prefix.push_str("C-");
            }
            if m.shift {
                prefix.push_str("S-");
            }
            if alt {
                prefix.push_str("M-");
            }
            if cmd {
                prefix.push_str("D-");
            }
            return Some(format!("<{prefix}{name}>"));
        }

        if !(m.ctrl || alt || cmd) {
            // Plain or shifted printable key: wait for the Text event.
            return None;
        }

        let key = if m.alt { physical.unwrap_or(key) } else { key };
        let c = key_char(key, m.shift)?;
        if c == ' ' && !(m.ctrl || alt || cmd) {
            return None;
        }
        // Ctrl+letter ignores Shift, like a terminal (Ctrl+Shift+A == Ctrl+A).
        let c = if m.ctrl && c.is_ascii_alphabetic() { c.to_ascii_lowercase() } else { c };
        let mut prefix = String::new();
        if m.ctrl {
            prefix.push_str("C-");
        }
        if alt {
            prefix.push_str("M-");
        }
        if cmd {
            prefix.push_str("D-");
        }
        // Option+key composes a character that arrives as `Text`. Ctrl and Cmd
        // combos send none, so the flag would swallow the next key's text.
        self.suppress_text = alt && !m.ctrl && !cmd;
        Some(format!("<{prefix}{}>", char_name(c)))
    }

    /// A `Text` event.
    pub fn text(&mut self, text: &str) -> Option<String> {
        if std::mem::take(&mut self.suppress_text) {
            return None;
        }
        Some(escape_text(text))
    }
}

/// Modifier prefix for `nvim_input_mouse`, e.g. "C-S-".
pub fn mouse_modifiers(m: Modifiers) -> String {
    let mut s = String::new();
    if m.ctrl {
        s.push_str("C-");
    }
    if m.shift {
        s.push_str("S-");
    }
    if m.alt {
        s.push_str("A-");
    }
    if m.mac_cmd {
        s.push_str("D-");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, shift: bool, alt: bool) -> Modifiers {
        Modifiers { ctrl, shift, alt, mac_cmd: false, command: ctrl }
    }

    fn press(t: &mut KeyTranslator, key: Key, m: Modifiers, text: Option<&str>) -> String {
        t.begin();
        let mut out = String::new();
        if let Some(s) = t.key(key, Some(key), m) {
            out += &s;
        }
        if let Some(text) = text {
            if let Some(s) = t.text(text) {
                out += &s;
            }
        }
        out
    }

    #[test]
    fn plain_text_is_not_doubled() {
        let mut t = KeyTranslator::default();
        assert_eq!(press(&mut t, Key::A, Modifiers::NONE, Some("a")), "a");
        assert_eq!(press(&mut t, Key::A, mods(false, true, false), Some("A")), "A");
        assert_eq!(press(&mut t, Key::Space, Modifiers::NONE, Some(" ")), " ");
    }

    #[test]
    fn a_key_after_a_ctrl_combo_in_one_batch_is_kept() {
        let mut t = KeyTranslator::default();
        t.begin();
        let mut out = t.key(Key::W, Some(Key::W), mods(true, false, false)).unwrap();
        out += &t.key(Key::L, Some(Key::L), Modifiers::NONE).unwrap_or_default();
        out += &t.text("l").unwrap_or_default();
        assert_eq!(out, "<C-w>l");
    }

    #[test]
    fn option_key_does_not_also_type_the_composed_character() {
        let mut t = KeyTranslator::default();
        assert_eq!(press(&mut t, Key::E, mods(false, false, true), Some("\u{b4}")), "<M-e>");
    }

    #[test]
    fn lt_is_escaped() {
        let mut t = KeyTranslator::default();
        assert_eq!(press(&mut t, Key::Comma, mods(false, true, false), Some("<")), "<lt>");
        assert_eq!(press(&mut t, Key::Comma, mods(false, true, true), Some("≤")), "<M-lt>");
    }

    #[test]
    fn ctrl_combos() {
        let mut t = KeyTranslator::default();
        assert_eq!(press(&mut t, Key::W, mods(true, false, false), None), "<C-w>");
        assert_eq!(press(&mut t, Key::W, mods(true, true, false), None), "<C-w>");
        assert_eq!(press(&mut t, Key::CloseBracket, mods(true, false, false), None), "<C-]>");
        assert_eq!(press(&mut t, Key::Num6, mods(true, true, false), None), "<C-^>");
        assert_eq!(press(&mut t, Key::Backslash, mods(true, false, false), None), "<C-Bslash>");
        assert_eq!(press(&mut t, Key::Space, mods(true, false, false), None), "<C-Space>");
    }

    #[test]
    fn alt_is_meta_and_swallows_composed_text() {
        let mut t = KeyTranslator::default();
        assert_eq!(press(&mut t, Key::X, mods(false, false, true), Some("≈")), "<M-x>");
        assert_eq!(press(&mut t, Key::X, mods(false, true, true), Some("˛")), "<M-X>");
        t.alt_is_meta = false;
        assert_eq!(press(&mut t, Key::X, mods(false, false, true), Some("≈")), "≈");
    }

    #[test]
    fn special_keys() {
        let mut t = KeyTranslator::default();
        assert_eq!(press(&mut t, Key::Tab, mods(false, true, false), None), "<S-Tab>");
        assert_eq!(press(&mut t, Key::Escape, Modifiers::NONE, None), "<Esc>");
        assert_eq!(press(&mut t, Key::Enter, mods(true, false, false), None), "<C-CR>");
        assert_eq!(press(&mut t, Key::Enter, mods(false, true, false), None), "<S-CR>");
        assert_eq!(press(&mut t, Key::F5, Modifiers::NONE, None), "<F5>");
        assert_eq!(press(&mut t, Key::ArrowUp, mods(false, false, true), None), "<M-Up>");
        assert_eq!(press(&mut t, Key::Backspace, Modifiers::NONE, None), "<BS>");
    }

    #[test]
    fn mouse_modifier_string() {
        assert_eq!(mouse_modifiers(mods(true, true, false)), "C-S-");
        assert_eq!(mouse_modifiers(Modifiers::NONE), "");
    }
}
