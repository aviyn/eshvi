//! The picker's preview: lines of a file beside the list, drawn here with the
//! colors Neovim worked out (`Eshvi.picker.preview` answers with a window of
//! text and highlight spans). It knows nothing of grep: it takes a file or a
//! buffer, a line to show, and, on the target line, stretches to mark.
//!
//! The previous preview stays on screen until the next arrives. Asking waits
//! until the selection has been still for `DEBOUNCE`; an answer to an older
//! request is dropped; moving inside the window already sent needs no request.

use std::ops::Range;
use std::time::{Duration, Instant};

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use grid::Editor;
use nvim_bridge::gui::PickerPreview;

use crate::fonts::GridFonts;
use crate::popup::{mix, PopupTheme};
use crate::renderer::{rgb, CellMetrics};
use crate::style;

/// How long the selection stays still before a preview is asked for.
pub const DEBOUNCE: Duration = Duration::from_millis(60);
/// A target nearer than this to the end of the window sent (that isn't the
/// end of the file) is asked for again, centered.
const EDGE: usize = 40;
/// Spaces to a tab stop.
const TAB: usize = 4;
/// A longer line is cut: only its start fits anyway.
const MAX_LINE: usize = 1000;

/// What a preview shows: a file by its path, or a buffer (whose text, saved
/// or not, is shown).
#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    Path(String),
    Buf(i64),
}

/// What to show: where, the line to put a third of the way down, a title (the
/// file's path as the list shows it) and the stretches of that line to mark
/// (bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub key: Subject,
    pub lnum: usize,
    pub title: String,
    pub ranges: Vec<Range<usize>>,
}

/// A request for Neovim.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub req: i64,
    pub key: Subject,
    /// The line the window of text should be around.
    pub lnum: usize,
}

/// The editor's colors for code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodeTheme {
    pub bg: Color32,
    pub fg: Color32,
    /// Line numbers.
    pub line_nr: Color32,
    /// The target line's fill.
    pub cursor_line: Color32,
    /// What a match is drawn with.
    pub search_bg: Color32,
    pub search_fg: Color32,
}

impl CodeTheme {
    /// The colors with nothing from the colorscheme but its background,
    /// text and accent.
    pub fn new(bg: Color32, fg: Color32, accent: Color32) -> Self {
        Self {
            bg,
            fg,
            line_nr: mix(fg, bg, style::FAINT),
            cursor_line: mix(bg, fg, style::SUBTLE),
            search_bg: mix(bg, accent, style::TINT),
            search_fg: fg,
        }
    }

    pub fn from_editor(editor: &Editor, accent: Color32) -> Self {
        let d = &editor.default_colors;
        let group = |name: &str| {
            editor.hl_groups.get(name).map(|id| {
                let attr = editor.hl_attr(*id);
                (attr.foreground.map(rgb), attr.background.map(rgb))
            })
        };
        let base = Self::new(rgb(d.bg), rgb(d.fg), accent);
        let search = group("Search");
        Self {
            line_nr: group("LineNr").and_then(|g| g.0).unwrap_or(base.line_nr),
            cursor_line: group("CursorLine").and_then(|g| g.1).unwrap_or(base.cursor_line),
            search_bg: search.and_then(|g| g.1).unwrap_or(base.search_bg),
            search_fg: search.and_then(|g| g.0).unwrap_or(base.search_fg),
            ..base
        }
    }
}

/// What the preview is drawn with.
pub struct PreviewLook<'a> {
    pub theme: &'a PopupTheme,
    pub code: &'a CodeTheme,
    pub fonts: &'a GridFonts,
    pub metrics: CellMetrics,
}

/// A colored stretch of a line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    pub fg: Option<Color32>,
    pub bold: bool,
    pub italic: bool,
}

/// `text` as runs: byte ranges of one style each, after applying `spans` in
/// order (a later one wins where they overlap), and whether each is part of
/// `marked`. Offsets that fall inside a character belong to it.
pub fn runs(text: &str, spans: &[(Range<usize>, Style)], marked: &[Range<usize>]) -> Vec<(Range<usize>, Style, bool)> {
    // What each byte is: the span that painted it last (+1), and whether it is marked.
    // (Within the text, and never backwards: a span that ends before it starts covers nothing.)
    let within = |range: &Range<usize>| {
        let start = range.start.min(text.len());
        start..range.end.clamp(start, text.len())
    };
    let mut owner = vec![0usize; text.len()];
    for (n, (range, _)) in spans.iter().enumerate() {
        for slot in &mut owner[within(range)] {
            *slot = n + 1;
        }
    }
    let mut mark = vec![false; text.len()];
    for range in marked {
        for slot in &mut mark[within(range)] {
            *slot = true;
        }
    }
    let mut out: Vec<(Range<usize>, usize, bool)> = Vec::new();
    for (i, c) in text.char_indices() {
        let key = (owner[i], mark[i]);
        match out.last_mut() {
            Some((range, o, m)) if (*o, *m) == key => range.end = i + c.len_utf8(),
            _ => out.push((i..i + c.len_utf8(), key.0, key.1)),
        }
    }
    out.into_iter().map(|(r, o, m)| (r, if o == 0 { Style::default() } else { spans[o - 1].1 }, m)).collect()
}

/// `text` cut to `MAX_LINE` bytes at a character.
fn clipped(text: &str) -> &str {
    let mut end = text.len().min(MAX_LINE);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A window of a file as Neovim sent it.
struct Content {
    key: Subject,
    /// The file's line number of `lines[0]` (from 1).
    first: usize,
    lines: Vec<String>,
    /// For each line, its colored stretches in the order to apply.
    spans: Vec<Vec<(Range<usize>, Style)>>,
    total: usize,
    note: String,
}

impl Content {
    fn last(&self) -> usize {
        self.first + self.lines.len().saturating_sub(1)
    }

    /// Whether `lnum` is far enough inside the window to show it with its
    /// surroundings.
    fn covers(&self, lnum: usize) -> bool {
        let near_start = self.first == 1 || lnum >= self.first + EDGE;
        let near_end = self.last() >= self.total || lnum + EDGE <= self.last();
        lnum >= self.first && lnum <= self.last() && near_start && near_end
    }
}

struct Pending {
    req: i64,
    key: Subject,
    center: usize,
}

/// What `due` tells the caller.
#[derive(Debug, Default, PartialEq)]
pub struct Due {
    pub request: Option<Request>,
    /// Look again after this long (a request is waiting for the selection to settle).
    pub wait: Option<Duration>,
}

#[derive(Default)]
pub struct PreviewPane {
    content: Option<Content>,
    target: Option<Target>,
    /// What is wanted from Neovim, and since when.
    want: Option<(Subject, usize)>,
    want_since: Option<Instant>,
    pending: Option<Pending>,
    next_req: i64,
    /// The file line (from 0, fractional) at the top of the view.
    top: f32,
    /// Put the target a third of the way down, once its text is here.
    align: bool,
    /// Rows the view had last frame.
    rows: f32,
}

impl PreviewPane {
    /// What to show now (the selection). None: nothing.
    pub fn set_target(&mut self, target: Option<Target>) {
        let place = |t: &Option<Target>| t.as_ref().map(|t| (t.key.clone(), t.lnum));
        if place(&target) != place(&self.target) {
            self.align = true;
        }
        // (Nothing selected, as when a search finds nothing: nothing to show.)
        if target.is_none() {
            self.content = None;
        }
        self.target = target;
    }

    /// The text window to ask for, if the one shown doesn't do.
    fn wanted(&self) -> Option<(Subject, usize)> {
        let t = self.target.as_ref()?;
        match &self.content {
            Some(c) if c.key == t.key => {
                // (A note says all there is to say about the file.)
                if !c.note.is_empty() {
                    return None;
                }
                if !c.covers(t.lnum) {
                    return Some((t.key.clone(), t.lnum));
                }
                // Scrolled toward an end of the window: a new one around the view.
                let (first, last) = (c.first as f32 - 1.0, c.last() as f32);
                let edge = EDGE as f32 / 2.0;
                let (view_top, view_end) = (self.top, self.top + self.rows);
                // (Once it has been drawn: before that the view is not anywhere.)
                let before = c.first > 1 && view_top < first + edge;
                let after = c.last() < c.total && view_end > last - edge;
                if self.rows > 0.0 && (before || after) {
                    let center = (self.top + self.rows / 2.0).max(1.0) as usize;
                    return Some((t.key.clone(), center.min(c.total.max(1))));
                }
                None
            }
            _ => Some((t.key.clone(), t.lnum)),
        }
    }

    /// Whether to ask Neovim for something now: once what is wanted has been
    /// the same for `DEBOUNCE`.
    pub fn due(&mut self, now: Instant) -> Due {
        let want = self.wanted();
        if want != self.want {
            self.want = want.clone();
            self.want_since = Some(now);
        }
        let Some((key, center)) = want else { return Due::default() };
        if self.pending.as_ref().is_some_and(|p| p.key == key && p.center == center) {
            return Due::default();
        }
        let waited = now.saturating_duration_since(self.want_since.unwrap_or(now));
        if waited < DEBOUNCE {
            return Due { request: None, wait: Some(DEBOUNCE - waited) };
        }
        self.next_req += 1;
        self.pending = Some(Pending { req: self.next_req, key: key.clone(), center });
        Due { request: Some(Request { req: self.next_req, key, lnum: center }), wait: None }
    }

    /// Take in Neovim's answer. Whether it was used: an answer to an older
    /// request, or for what is no longer selected, is dropped.
    pub fn answer(&mut self, a: PickerPreview) -> bool {
        let key = match a.buf {
            Some(b) => Subject::Buf(b),
            None => Subject::Path(a.path.clone()),
        };
        let current = self.pending.as_ref().is_some_and(|p| p.req == a.req);
        // (A buffer is asked for by its number or by its file's path.)
        let wanted = self.target.as_ref().is_some_and(|t| t.key == key);
        if !current || !wanted {
            return false;
        }
        self.pending = None;
        let mut spans = vec![Vec::new(); a.lines.len()];
        for s in &a.spans {
            if let Some(line) = spans.get_mut(s.line) {
                let style = Style { fg: s.fg.map(rgb), bold: s.bold, italic: s.italic };
                line.push((s.start..s.end, style));
            }
        }
        self.content = Some(Content { key, first: a.first_line.max(1), lines: a.lines, spans, total: a.total, note: a.note });
        true
    }

    /// Page the view by `pages` (negative: up), as Ctrl-F and Ctrl-B do.
    pub fn page(&mut self, pages: f32) {
        self.align = false;
        self.top += pages * (self.rows - 1.0).max(1.0);
        self.clamp();
    }

    fn clamp(&mut self) {
        let total = self.content.as_ref().map_or(0, |c| c.total) as f32;
        self.top = self.top.clamp(0.0, (total - self.rows / 2.0).max(0.0));
    }

    /// The file line at the top of the view (from 1).
    #[cfg(test)]
    pub fn top_line(&self) -> usize {
        self.top.round() as usize + 1
    }

    /// The text's lines in the window that is held (from the window's first line).
    #[cfg(test)]
    pub fn window(&self) -> Option<(usize, &[String])> {
        self.content.as_ref().map(|c| (c.first, c.lines.as_slice()))
    }

    /// The line the view shows the target on, from the top (0 is the first).
    #[cfg(test)]
    pub fn rows(&self) -> f32 {
        self.rows
    }

    /// Draw it in `size`.
    pub fn show(&mut self, ui: &mut egui::Ui, size: Vec2, look: &PreviewLook) {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
        let (theme, code, m) = (look.theme, look.code, look.metrics);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, style::RADIUS, code.bg);
        painter.rect_stroke(rect, style::RADIUS, Stroke::new(style::HAIRLINE, theme.border), egui::StrokeKind::Inside);
        let small = style::scaled(&FontId::proportional(look.fonts.size), style::SMALL);
        let head_h = m.height + style::GAP;
        let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), head_h));
        let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), rect.max);
        if let Some(t) = &self.target {
            let room = rect.width() - 2.0 * style::GAP;
            let (title, _) = super::truncate_to_width(&t.title, room, &|s| painter.layout_no_wrap(s.to_owned(), small.clone(), theme.dim_fg).size().x);
            painter.text(Pos2::new(rect.min.x + style::GAP, head.center().y), Align2::LEFT_CENTER, title, small.clone(), theme.dim_fg);
        }
        painter.hline(rect.x_range(), head.max.y, Stroke::new(style::HAIRLINE, theme.border));

        self.rows = (body.height() / m.height).floor().max(1.0);
        // The wheel scrolls it while the pointer is over it.
        if resp.hovered() {
            let delta = ui.ctx().input(|i| i.smooth_scroll_delta.y);
            if delta != 0.0 {
                self.align = false;
                self.top -= delta / m.height;
            }
        }
        // The target a third of the way down, once its text is here.
        if let (true, Some(t), Some(c)) = (self.align, &self.target, &self.content) {
            if c.key == t.key && (c.covers(t.lnum) || !c.note.is_empty()) {
                self.top = t.lnum as f32 - 1.0 - (self.rows / 3.0).floor();
                self.align = false;
            }
        }
        self.clamp();

        let Some(c) = &self.content else { return };
        if !c.note.is_empty() {
            painter.text(body.center(), Align2::CENTER_CENTER, &c.note, small, theme.dim_fg);
            return;
        }
        let (font, _) = look.fonts.font_id(false, false);
        let gutter_digits = (c.first + c.lines.len()).max(c.total).to_string().len().max(3);
        let gutter = gutter_digits as f32 * m.width + style::GAP;
        let x0 = body.min.x + gutter + style::GAP;
        let painter = painter.with_clip_rect(body);
        let first_row = self.top.floor().max(0.0) as usize;
        let shift = (self.top - self.top.floor()) * m.height;
        let target = self.target.as_ref();
        for n in 0..=(self.rows as usize) {
            let line_no = first_row + n + 1;
            let Some(i) = line_no.checked_sub(c.first).filter(|&i| i < c.lines.len()) else { continue };
            let y = body.min.y + n as f32 * m.height - shift;
            let row = Rect::from_min_size(Pos2::new(body.min.x, y), Vec2::new(body.width(), m.height));
            let on_target = target.is_some_and(|t| t.lnum == line_no && t.key == c.key);
            if on_target {
                painter.rect_filled(row, 0.0, code.cursor_line);
            }
            painter.text(Pos2::new(body.min.x + gutter, row.center().y), Align2::RIGHT_CENTER, line_no, font.clone(), code.line_nr);
            let text = clipped(&c.lines[i]);
            let marked: &[Range<usize>] = if on_target { target.map_or(&[], |t| &t.ranges) } else { &[] };
            let job = line_job(text, &c.spans[i], marked, look);
            let galley = painter.layout_job(job);
            painter.galley(Pos2::new(x0, row.center().y - galley.size().y / 2.0), galley, code.fg);
        }
    }
}

/// One line's text as a layout job: tabs as spaces, each run in its color,
/// the marked stretches in the search colors.
pub fn line_job(text: &str, spans: &[(Range<usize>, Style)], marked: &[Range<usize>], look: &PreviewLook) -> LayoutJob {
    let code = look.code;
    let mut job = LayoutJob::default();
    let mut col = 0;
    for (range, style, hit) in runs(text, spans, marked) {
        let mut piece = String::new();
        for c in text[range].chars() {
            if c == '\t' {
                let to = TAB - col % TAB;
                piece.extend(std::iter::repeat_n(' ', to));
                col += to;
            } else {
                piece.push(c);
                col += 1;
            }
        }
        let (font, _) = look.fonts.font_id(style.bold, style.italic);
        job.append(
            &piece,
            0.0,
            TextFormat {
                font_id: font,
                color: if hit { code.search_fg } else { style.fg.unwrap_or(code.fg) },
                background: if hit { code.search_bg } else { Color32::TRANSPARENT },
                ..Default::default()
            },
        );
    }
    job
}
