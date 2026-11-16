//! Data Neovim writes to a UI's terminal (`ui_send`, for UIs attached with
//! `stdout_tty`), read the way a terminal would: kitty graphics protocol
//! commands (snacks.image) and progress bars (OSC 9;4). Everything else
//! (queries, which the bridge answers right away; colors; clipboard) is
//! skipped.

/// Something found in the terminal data.
#[derive(Clone, Debug, PartialEq)]
pub enum TtyEvent {
    /// A kitty graphics protocol command, `ESC _ G <control> ; <payload> ESC \`.
    Graphics(GraphicsCommand),
    /// A progress bar (OSC 9;4): `None` removes it; `Some(None)` is a bar
    /// with no known amount; else the percentage.
    Progress(Option<Option<u8>>),
}

/// A graphics command's control keys (`a=p,i=1,…`) and payload (base64).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphicsCommand {
    pub keys: Vec<(String, String)>,
    pub payload: Vec<u8>,
}

impl GraphicsCommand {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn num(&self, key: &str) -> Option<u32> {
        self.get(key)?.parse().ok()
    }

    fn parse(body: &[u8]) -> Self {
        let (control, payload) = match body.iter().position(|&b| b == b';') {
            Some(i) => (&body[..i], body[i + 1..].to_vec()),
            None => (body, Vec::new()),
        };
        let keys = String::from_utf8_lossy(control)
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        Self { keys, payload }
    }
}

/// Splits terminal data into events; keeps an unfinished sequence for the
/// next call.
#[derive(Default)]
pub struct TtyParser {
    pending: Vec<u8>,
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// The most an unfinished sequence may grow to before it is given up (an
/// escape that never ends would otherwise be kept, and searched again from
/// its start, with every send).
const MAX_PENDING: usize = 32 << 20;

impl TtyParser {
    pub fn feed(&mut self, data: &[u8]) -> Vec<TtyEvent> {
        self.pending.extend_from_slice(data);
        let buf = std::mem::take(&mut self.pending);
        let mut events = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            if buf[i] != ESC {
                i += 1; // plain text: nothing to show it in
                continue;
            }
            let Some(&kind) = buf.get(i + 1) else { break };
            let body_start = i + 2;
            let end = match kind {
                // APC, OSC, DCS: up to ST (`ESC \`), or BEL for OSC.
                b'_' | b']' | b'P' => string_end(&buf[body_start..], kind == b']').map(|(len, term)| (body_start + len, term)),
                // CSI: parameters, then a final byte.
                b'[' => buf[body_start..].iter().position(|b| (0x40..=0x7e).contains(b)).map(|len| (body_start + len, 1)),
                _ => Some((body_start, 0)),
            };
            let Some((body_end, term_len)) = end else { break };
            let body = &buf[body_start..body_end];
            match kind {
                b'_' if body.first() == Some(&b'G') => events.push(TtyEvent::Graphics(GraphicsCommand::parse(&body[1..]))),
                b']' => events.extend(osc(body)),
                _ => {}
            }
            i = body_end + term_len;
        }
        if i < buf.len() && buf.len() - i <= MAX_PENDING {
            self.pending = buf[i..].to_vec();
        }
        events
    }
}

/// Length of a string sequence's body and of its terminator.
fn string_end(s: &[u8], bel: bool) -> Option<(usize, usize)> {
    s.iter().enumerate().find_map(|(j, &b)| match b {
        ESC if s.get(j + 1) == Some(&b'\\') => Some((j, 2)),
        BEL if bel => Some((j, 1)),
        _ => None,
    })
}

fn osc(body: &[u8]) -> Option<TtyEvent> {
    let text = std::str::from_utf8(body).ok()?;
    let mut parts = text.split(';');
    if (parts.next(), parts.next()) != (Some("9"), Some("4")) {
        return None;
    }
    let state: u8 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let percent = parts.next().and_then(|s| s.parse::<u32>().ok()).map(|p| p.min(100) as u8);
    // 0 removes, 3 has no amount; 1 (normal), 2 (error) and 4 (paused)
    // show the percentage.
    Some(TtyEvent::Progress(match state {
        0 => None,
        3 => Some(None),
        _ => Some(Some(percent.unwrap_or(0))),
    }))
}

/// Standard base64 (padding optional); invalid characters are skipped.
pub fn base64_decode(data: &[u8]) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for v in data.iter().filter_map(|&c| value(c)) {
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_commands_queries_and_progress() {
        let mut p = TtyParser::default();
        let data = b"\x1b_Ga=p,U=1,i=7,p=2,c=10,r=4,q=2\x1b\\\x1b]11;?\x07\x1b[5n\x1b]9;4;1;40\x1b\\";
        let ev = p.feed(data);
        let TtyEvent::Graphics(g) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!((g.get("a"), g.num("i"), g.num("c"), g.num("r")), (Some("p"), Some(7), Some(10), Some(4)));
        assert!(g.payload.is_empty());
        // Queries are skipped (the bridge answers them).
        assert_eq!(ev[1], TtyEvent::Progress(Some(Some(40))));
        assert_eq!(ev.len(), 2);
        assert_eq!(p.feed(b"\x1b]9;4;0;0\x1b\\"), [TtyEvent::Progress(None)]);
        assert_eq!(p.feed(b"\x1b]9;4;3;0\x07"), [TtyEvent::Progress(Some(None))]);
        assert!(p.feed(b"\x1b[>q").is_empty());
    }

    #[test]
    fn a_sequence_that_never_ends_is_given_up() {
        let mut p = TtyParser::default();
        assert!(p.feed(b"\x1b_Gf=100;").is_empty());
        let chunk = vec![b'A'; 1 << 20];
        for _ in 0..40 {
            assert!(p.feed(&chunk).is_empty());
            assert!(p.pending.len() <= MAX_PENDING);
        }
        // Given up, and what comes after is read as ever.
        assert!(p.pending.is_empty());
        assert_eq!(p.feed(b"\x1b]9;4;1;40\x1b\\"), [TtyEvent::Progress(Some(Some(40)))]);
    }

    #[test]
    fn a_sequence_split_across_sends_waits_for_its_end() {
        let mut p = TtyParser::default();
        assert!(p.feed(b"\x1b_Gt=f,i=1;L3RtcC9h").is_empty());
        let ev = p.feed(b"LnBuZw==\x1b\\");
        let TtyEvent::Graphics(g) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(base64_decode(&g.payload), b"/tmp/a.png");
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(base64_decode(b"aGVsbG8="), b"hello");
        assert_eq!(base64_decode(b"aGVsbG8"), b"hello");
        assert_eq!(base64_decode(b""), b"");
    }
}
