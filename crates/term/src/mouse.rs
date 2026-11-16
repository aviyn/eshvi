//! Mouse events as the bytes a program that asked for them reads: SGR
//! (`CSI < b ; x ; y M`) when it turned that on, else the legacy
//! `CSI M` (with UTF-8 coordinates if it asked for those).

use alacritty_terminal::term::TermMode;

use crate::keys::Mods;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// Motion with no button down.
    None,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Motion,
}

/// The bytes for a mouse event at a (0-based) cell, or `None` when the
/// program didn't ask for this kind: nothing is reported without a mouse mode,
/// motion only in drag mode (with a button down) or any-motion mode, and
/// releases and presses in all of them.
pub fn encode_mouse(mode: TermMode, button: MouseButton, kind: MouseKind, mods: Mods, col: usize, row: usize) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let wheel = matches!(button, MouseButton::WheelUp | MouseButton::WheelDown | MouseButton::WheelLeft | MouseButton::WheelRight);
    match kind {
        MouseKind::Motion => {
            let allowed = mode.contains(TermMode::MOUSE_MOTION) || (mode.contains(TermMode::MOUSE_DRAG) && button != MouseButton::None);
            if !allowed || wheel {
                return None;
            }
        }
        // A wheel notch is a press only.
        MouseKind::Release if wheel => return None,
        _ => {}
    }
    let mut code = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
    };
    if kind == MouseKind::Motion {
        code += 32;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;
    let (x, y) = (col + 1, row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let last = if kind == MouseKind::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{last}").into_bytes());
    }
    // Legacy: a release doesn't say which button.
    if kind == MouseKind::Release {
        code = (code & !3) | 3;
    }
    let mut out = b"\x1b[M".to_vec();
    out.push(32 + code as u8);
    for v in [x, y] {
        if mode.contains(TermMode::UTF8_MOUSE) {
            // Up to 2015, as two UTF-8 bytes from 128.
            let c = char::from_u32(32 + v as u32).filter(|_| v <= 2015)?;
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        } else {
            // One byte: columns past 222 can't be said.
            out.push(u8::try_from(32 + v).ok().filter(|_| v <= 222)?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods::NONE;

    fn s(b: Option<Vec<u8>>) -> Option<String> {
        b.map(|b| b.iter().map(|b| std::ascii::escape_default(*b).to_string()).collect())
    }

    #[test]
    fn nothing_without_a_mouse_mode() {
        let m = TermMode::SGR_MOUSE;
        assert_eq!(encode_mouse(m, MouseButton::Left, MouseKind::Press, NONE, 0, 0), None);
    }

    #[test]
    fn sgr() {
        let m = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(s(encode_mouse(m, MouseButton::Left, MouseKind::Press, NONE, 4, 9)).as_deref(), Some("\\x1b[<0;5;10M"));
        assert_eq!(s(encode_mouse(m, MouseButton::Left, MouseKind::Release, NONE, 4, 9)).as_deref(), Some("\\x1b[<0;5;10m"));
        assert_eq!(s(encode_mouse(m, MouseButton::Right, MouseKind::Press, NONE, 0, 0)).as_deref(), Some("\\x1b[<2;1;1M"));
        assert_eq!(s(encode_mouse(m, MouseButton::WheelUp, MouseKind::Press, NONE, 1, 1)).as_deref(), Some("\\x1b[<64;2;2M"));
        assert_eq!(s(encode_mouse(m, MouseButton::WheelDown, MouseKind::Release, NONE, 1, 1)), None, "a wheel notch has no release");
        // Modifiers: alt 8, ctrl 16 (shift selects instead, but is encoded).
        let ctrl_alt = Mods { ctrl: true, alt: true, ..NONE };
        assert_eq!(s(encode_mouse(m, MouseButton::Left, MouseKind::Press, ctrl_alt, 0, 0)).as_deref(), Some("\\x1b[<24;1;1M"));
        // Large coordinates are fine.
        assert_eq!(s(encode_mouse(m, MouseButton::Left, MouseKind::Press, NONE, 299, 0)).as_deref(), Some("\\x1b[<0;300;1M"));
    }

    #[test]
    fn motion_needs_drag_or_any_motion_mode() {
        let click = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(encode_mouse(click, MouseButton::Left, MouseKind::Motion, NONE, 1, 1), None);
        let drag = click | TermMode::MOUSE_DRAG;
        assert_eq!(s(encode_mouse(drag, MouseButton::Left, MouseKind::Motion, NONE, 1, 1)).as_deref(), Some("\\x1b[<32;2;2M"));
        assert_eq!(encode_mouse(drag, MouseButton::None, MouseKind::Motion, NONE, 1, 1), None, "no button, no drag");
        let any = click | TermMode::MOUSE_MOTION;
        assert_eq!(s(encode_mouse(any, MouseButton::None, MouseKind::Motion, NONE, 1, 1)).as_deref(), Some("\\x1b[<35;2;2M"));
    }

    #[test]
    fn legacy_bytes() {
        let m = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(s(encode_mouse(m, MouseButton::Left, MouseKind::Press, NONE, 0, 0)).as_deref(), Some("\\x1b[M !!"));
        // A release is button 3, whichever was down.
        assert_eq!(s(encode_mouse(m, MouseButton::Right, MouseKind::Release, NONE, 0, 0)).as_deref(), Some("\\x1b[M#!!"));
        assert_eq!(s(encode_mouse(m, MouseButton::WheelUp, MouseKind::Press, NONE, 0, 0)).as_deref(), Some("\\x1b[M`!!"));
        // Past column 222 it can't be said, unless UTF-8 coordinates are on.
        assert_eq!(encode_mouse(m, MouseButton::Left, MouseKind::Press, NONE, 300, 0), None);
        let utf8 = m | TermMode::UTF8_MOUSE;
        assert_eq!(encode_mouse(utf8, MouseButton::Left, MouseKind::Press, NONE, 300, 0), Some(vec![0x1b, b'[', b'M', 32, 0xc5, 0x8d, b'!']));
    }
}
