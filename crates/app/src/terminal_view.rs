//! A terminal (`crates/term`) drawn by the GUI: an egui widget painting the
//! screen's cells the way Neovim's grids are painted (the same fonts, Nerd
//! Font fallback, wide characters and `⏺` substitute: `renderer`'s
//! helpers), and taking the keyboard, the mouse and the wheel for it.
//!
//! Each `TerminalView` owns one terminal and has its own font (so its own
//! cell size), focus is the host's to give (`show`'s `focused`), and its
//! process's pid, title and foreground process are asked of it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Context, Event, Galley, Id, ImeEvent, Key, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use term::keys::{self, KeyEvent, KeyKind, KeyOptions, Mods};
use term::mouse::{MouseButton, MouseKind};
use term::{Cell, Flags, Palette, Side, SelectionType, Snapshot, Span, Terminal, ViKey, ViOutcome};

use crate::fonts::GridFonts;
use crate::links::{self, Link, Target};
use crate::popup::mix;
use crate::renderer::{self, CellMetrics, Deco, DecoKind, EguiGridRenderer};
use crate::style::{self, Chrome};

/// What the view needs from its host to draw: the font, and the colors of the
/// colorscheme.
pub struct Look<'a> {
    /// The terminal's font: its own, else the editor's (`GridFonts::terminal`).
    pub fonts: &'a GridFonts,
    pub linespace: f32,
    /// Normal's foreground and the panel's shade of its background.
    pub fg: Color32,
    pub bg: Color32,
    pub accent: Color32,
    /// `g:terminal_color_0..15`, where set.
    pub ansi: [Option<u32>; 16],
    /// Option sends ESC before the key (`[terminal] option_as_meta`).
    pub option_as_meta: bool,
    /// Whether cursors blink (`cursor_blink`).
    pub blink: bool,
    /// A selection made with the mouse goes on the clipboard when the button
    /// is let go (`[terminal] copy_on_select`), as in X11 terminals.
    pub copy_on_select: bool,
    /// A middle click pastes the clipboard (`[terminal] middle_click_paste`).
    pub middle_click_paste: bool,
}

impl Look<'_> {
    fn palette(&self) -> Palette {
        let c = |c: Color32| u32::from_be_bytes([0, c.r(), c.g(), c.b()]);
        Palette::from_theme(c(self.fg), c(self.bg), &self.ansi)
    }

    fn chrome(&self) -> Chrome {
        Chrome::new(self.bg, self.fg, self.accent)
    }
}

/// What a frame of the view tells its host.
pub struct Output {
    /// The pointer went down in it: it is to be focused.
    pub pressed: bool,
    /// A link was opened with ⌘-click.
    pub link: Option<Target>,
    /// What the terminal reported (title, bell, exit).
    pub events: Vec<term::Event>,
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A cell as drawn: colors resolved, so rows are compared (and cached) as drawn.
#[derive(Clone, Debug, PartialEq)]
struct RCell {
    ch: char,
    /// Combining characters.
    zw: Vec<char>,
    fg: Color32,
    bg: Color32,
    bold: bool,
    italic: bool,
    wide: bool,
    /// The second half of a wide character: nothing to draw.
    spacer: bool,
    underline: Option<(Ul, Color32)>,
    strike: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ul {
    Line,
    Double,
    Curl,
    Dotted,
    Dashed,
}

fn color(c: term::Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// A cell's colors and style as drawn: inverse swaps them, dim fades the
/// text, hidden text is its background.
fn resolve_cell(cell: &Cell, palette: &Palette, overrides: &term::Colors) -> RCell {
    let f = cell.flags;
    let mut fg = color(palette.resolve(cell.fg, overrides, f.contains(Flags::DIM)));
    let mut bg = color(palette.resolve(cell.bg, overrides, false));
    if f.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if f.contains(Flags::HIDDEN) {
        fg = bg;
    }
    let ul_color = cell.underline_color().map_or(fg, |c| color(palette.resolve(c, overrides, false)));
    let underline = [
        (Flags::UNDERLINE, Ul::Line),
        (Flags::DOUBLE_UNDERLINE, Ul::Double),
        (Flags::UNDERCURL, Ul::Curl),
        (Flags::DOTTED_UNDERLINE, Ul::Dotted),
        (Flags::DASHED_UNDERLINE, Ul::Dashed),
    ]
    .into_iter()
    .find(|(flag, _)| f.contains(*flag))
    .map(|(_, ul)| (ul, ul_color));
    RCell {
        ch: cell.c,
        zw: cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
        fg,
        bg,
        bold: f.contains(Flags::BOLD),
        italic: f.contains(Flags::ITALIC),
        wide: f.contains(Flags::WIDE_CHAR),
        spacer: f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
        underline,
        strike: f.contains(Flags::STRIKEOUT),
    }
}

/// A row laid out: backgrounds, text and underlines, as Neovim's rows are.
struct RowLayout {
    cells: Vec<RCell>,
    backgrounds: Vec<(f32, f32, Color32)>,
    runs: Vec<(Vec2, Arc<Galley>)>,
    decos: Vec<Deco>,
}

/// Lay a row out. ASCII cells of one style share a run; anything else gets
/// its own, so fallback-font advances can't shift the grid; a run shaping
/// moved off its cells is laid out one character per cell (as the grids' are).
fn layout_row(ctx: &Context, cells: Vec<RCell>, default_bg: Color32, fonts: &GridFonts, m: CellMetrics) -> RowLayout {
    let format = |fg: Color32, bold: bool, italic: bool| {
        let (font_id, fake_italic) = fonts.font_id(bold, italic);
        TextFormat { font_id, color: fg, italics: fake_italic, line_height: Some(m.height), ..Default::default() }
    };
    let mut backgrounds: Vec<(f32, f32, Color32)> = Vec::new();
    let mut decos: Vec<Deco> = Vec::new();
    let mut jobs: Vec<(f32, LayoutJob)> = Vec::new();
    // The run being built: its start, style and text.
    let mut run: Option<(usize, (Color32, bool, bool), String)> = None;
    let flush = |run: &mut Option<(usize, (Color32, bool, bool), String)>, jobs: &mut Vec<(f32, LayoutJob)>| {
        if let Some((col, (fg, bold, italic), text)) = run.take() {
            if !text.trim().is_empty() {
                jobs.push((col as f32 * m.width, LayoutJob::single_section(text, format(fg, bold, italic))));
            }
        }
    };
    let mut col = 0;
    while col < cells.len() {
        let cell = &cells[col];
        if cell.spacer {
            col += 1;
            continue;
        }
        let width = if cell.wide { 2 } else { 1 };
        let (x0, x1) = (col as f32 * m.width, (col + width) as f32 * m.width);
        if cell.bg != default_bg {
            match backgrounds.last_mut() {
                Some(last) if last.2 == cell.bg && (last.1 - x0).abs() < 0.01 => last.1 = x1,
                _ => backgrounds.push((x0, x1, cell.bg)),
            }
        }
        let lines = cell.underline.map(|(ul, c)| {
            let kind = match ul {
                Ul::Line => DecoKind::Underline,
                Ul::Double => DecoKind::Underdouble,
                Ul::Curl => DecoKind::Undercurl,
                Ul::Dotted => DecoKind::Underdotted,
                Ul::Dashed => DecoKind::Underdashed,
            };
            (kind, c)
        });
        for (kind, color) in lines.into_iter().chain(cell.strike.then_some((DecoKind::Strikethrough, cell.fg))) {
            match decos.last_mut() {
                Some(d) if std::mem::discriminant(&d.kind) == std::mem::discriminant(&kind) && d.color == color && (d.x1 - x0).abs() < 0.01 => d.x1 = x1,
                _ => decos.push(Deco { kind, x0, x1, color }),
            }
        }
        let mut text = String::new();
        text.push(cell.ch);
        text.extend(cell.zw.iter());
        let style = (cell.fg, cell.bold, cell.italic);
        let simple = text.len() == 1 && text.is_ascii() && width == 1;
        match &mut run {
            Some((_, s, t)) if simple && *s == style && t.is_ascii() => t.push_str(&text),
            _ => {
                flush(&mut run, &mut jobs);
                run = Some((col, style, renderer::text_glyph(&text).to_owned()));
                if !simple {
                    flush(&mut run, &mut jobs);
                }
            }
        }
        col += width;
    }
    flush(&mut run, &mut jobs);

    let runs = ctx.fonts_mut(|f| {
        let mut runs = Vec::with_capacity(jobs.len());
        for (x, job) in jobs {
            let galley = f.layout_job(job.clone());
            if renderer::on_cells(&galley, m.width) {
                runs.push((Vec2::new(x, 0.0), galley));
                continue;
            }
            let format = job.sections.first().map(|s| s.format.clone()).unwrap_or_default();
            for (i, ch) in job.text.chars().enumerate() {
                let one = LayoutJob::single_section(ch.to_string(), format.clone());
                runs.push((Vec2::new(x + i as f32 * m.width, 0.0), f.layout_job(one)));
            }
        }
        runs
    });
    RowLayout { cells, backgrounds, runs, decos }
}

/// The cell under a point, and which half of it: for placing selections.
/// Points off the grid are clamped to its edge cells (and outside the first
/// column, the left half of it).
fn hit_cell(pos: Pos2, origin: Pos2, cell: Vec2, cols: usize, rows: usize) -> (usize, usize, Side) {
    let p = pos - origin;
    let (x, y) = (p.x / cell.x, p.y / cell.y);
    let col = (x.floor().max(0.0) as usize).min(cols.saturating_sub(1));
    let row = (y.floor().max(0.0) as usize).min(rows.saturating_sub(1));
    let side = if x < 0.0 {
        Side::Left
    } else if x >= cols as f32 || x.fract() >= 0.5 {
        Side::Right
    } else {
        Side::Left
    };
    (row, col, side)
}

/// How a click is counted: the same cell again quickly is a double click, a
/// third a triple.
struct Clicks {
    at: Instant,
    cell: (usize, usize),
    count: u8,
}

const MULTI_CLICK: Duration = Duration::from_millis(400);

impl Clicks {
    fn count(&mut self, cell: (usize, usize), now: Instant) -> u8 {
        if self.cell == cell && now.duration_since(self.at) < MULTI_CLICK && self.count > 0 {
            self.count = self.count % 3 + 1;
        } else {
            self.count = 1;
        }
        self.at = now;
        self.cell = cell;
        self.count
    }
}

/// A press being dragged.
#[derive(Clone, Copy, PartialEq)]
enum Drag {
    /// Selecting text.
    Select,
    /// A button the program takes: its motion is reported.
    Report(MouseButton),
}

/// The find bar: the pattern and where the keyboard is.
struct Find {
    text: String,
    /// Take the keyboard when next drawn.
    focus: bool,
    /// It had the keyboard when last drawn. (egui lets go of a field when Esc
    /// comes in, before the frame is drawn, so asking then is too late.)
    focused: bool,
    /// `?` in Vi mode: Enter looks backwards.
    forward: bool,
}

/// One terminal and how it is drawn.
pub struct TerminalView {
    term: Terminal,
    id: u64,
    metrics: CellMetrics,
    /// What the cached rows were laid out with (fonts, scale, colors): another
    /// one drops them.
    cache_key: Option<u64>,
    atlas_fill: f32,
    rows: Vec<Option<RowLayout>>,
    find: Option<Find>,
    clicks: Clicks,
    drag: Option<Drag>,
    /// The cell last reported to the program while dragging or moving.
    reported: Option<(usize, usize)>,
    /// Wheel travel not yet a whole line.
    wheel: f32,
    /// The key just sent also makes a text event, which isn't to be sent too.
    drop_text: bool,
    last_input: Instant,
    focused: bool,
    /// The cells and size of the last frame, to place the pointer.
    origin: Pos2,
    grid: (usize, usize),
}

impl TerminalView {
    pub fn new(term: Terminal) -> Self {
        Self {
            term,
            id: next_id(),
            metrics: CellMetrics { width: 8.0, height: 16.0 },
            cache_key: None,
            atlas_fill: 0.0,
            rows: Vec::new(),
            find: None,
            clicks: Clicks { at: Instant::now(), cell: (usize::MAX, 0), count: 0 },
            drag: None,
            reported: None,
            wheel: 0.0,
            drop_text: false,
            last_input: Instant::now(),
            focused: false,
            origin: Pos2::ZERO,
            grid: (80, 24),
        }
    }

    pub fn terminal(&self) -> &Terminal {
        &self.term
    }

    /// The view's own number (a tab is known by it).
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn pid(&self) -> u32 {
        self.term.pid()
    }

    pub fn title(&self) -> Option<String> {
        self.term.title()
    }

    pub fn foreground_process(&self) -> Option<String> {
        self.term.foreground_process()
    }

    /// This terminal's own cell, in points (its font's).
    pub fn cell_size(&self) -> Vec2 {
        Vec2::new(self.metrics.width, self.metrics.height)
    }

    /// The columns and rows it was last drawn with (the panels show them).
    #[allow(dead_code)]
    pub fn grid_size(&self) -> (usize, usize) {
        self.grid
    }

    /// The find field's id: the field keeps the keyboard when the app lets go
    /// of other widgets'.
    pub fn find_id(&self) -> Id {
        Id::unique(("eshvi-term-find", self.id))
    }

    /// Open the find bar and give it the keyboard.
    pub fn open_find(&mut self) {
        match &mut self.find {
            Some(f) => f.focus = true,
            None => self.find = Some(Find { text: String::new(), focus: true, focused: false, forward: true }),
        }
    }

    fn close_find(&mut self, ctx: &Context) {
        self.find = None;
        self.term.search_clear();
        ctx.memory_mut(|m| m.surrender_focus(self.find_id()));
    }

    /// Draw the terminal in the room `ui` has, and handle its input. `focused`
    /// is whether it has the keyboard.
    pub fn show(&mut self, ui: &mut Ui, look: &Look, focused: bool) -> Output {
        let ctx = ui.ctx().clone();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::CLICK | Sense::DRAG);
        let mut out = Output { pressed: false, link: None, events: Vec::new() };
        self.measure(&ctx, look);
        // Whole cells, from the top left inside a margin (the text doesn't touch
        // the panel's edge); the sliver left is the background.
        let (cw, ch) = (self.metrics.width, self.metrics.height);
        let area = rect.shrink2(Vec2::new(style::GAP, style::GAP / 2.0));
        let cols = ((area.width() / cw).floor() as usize).clamp(2, u16::MAX as usize);
        let rows = ((area.height() / ch).floor() as usize).clamp(1, u16::MAX as usize);
        self.origin = area.min;
        self.grid = (cols, rows);
        let ppp = ctx.pixels_per_point();
        self.term.resize(term::Size { cols: cols as u16, rows: rows as u16, cell_w: (cw * ppp) as u16, cell_h: (ch * ppp) as u16 });
        self.term.set_key_options(KeyOptions { option_as_meta: look.option_as_meta });
        self.term.set_palette(look.palette());
        if focused != self.focused {
            self.focused = focused;
            self.term.set_focused(focused);
        }

        for ev in self.term.events() {
            match &ev {
                term::Event::ClipboardStore(text) => ctx.copy_text(text.clone()),
                term::Event::CursorBlinking => {}
                _ => {}
            }
            out.events.push(ev);
        }

        let events = ctx.input(|i| i.events.clone());
        let hover = ctx.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p) && ctx.layer_id_at(*p) == Some(ui.layer_id()));
        self.handle_pointer(&ctx, &events, rect, look, &mut out);
        if focused && !self.find.as_ref().is_some_and(|f| f.focused) {
            self.handle_keys(&ctx, &events);
        }
        self.show_find_consumes(&ctx);

        // Paint.
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, look.bg);
        let snap = self.term.snapshot();
        let palette = look.palette();
        self.paint(&ctx, &painter, &snap, look, &palette, focused);
        let cmd = ctx.input(|i| i.modifiers.command);
        if let Some(link) = hover.filter(|_| cmd).and_then(|p| self.link_at(&snap, p)) {
            for (row, c0, c1) in link.spans {
                let y = self.origin.y + (row + 1) as f32 * ch - 1.0;
                let (x0, x1) = (self.origin.x + c0 as f32 * cw, self.origin.x + (c1 + 1) as f32 * cw);
                painter.hline(x0..=x1, y, Stroke::new(style::HAIRLINE, look.accent));
            }
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        } else if hover.is_some() && !(self.term.reports_mouse() && !ctx.input(|i| i.modifiers.shift)) {
            ctx.set_cursor_icon(egui::CursorIcon::Text);
        }
        if let Some(p) = self.show_find(ui, rect, look) {
            out.pressed |= p;
        }
        // The cursor blinks as the program asked, and the view keeps painting for it.
        if look.blink && focused && snap.cursor.is_some_and(|c| c.blinking) {
            ctx.request_repaint_after(BLINK);
        }
        let _ = response;
        out
    }

    /// The cell size of the font (again when the font, scale or colors changed),
    /// dropping rows laid out with the old.
    fn measure(&mut self, ctx: &Context, look: &Look) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let (family, _) = look.fonts.font_id(false, false);
        format!("{:?}", family.family).hash(&mut h);
        look.fonts.size.to_bits().hash(&mut h);
        look.linespace.to_bits().hash(&mut h);
        ctx.pixels_per_point().to_bits().hash(&mut h);
        (look.fg, look.bg, look.ansi).hash(&mut h);
        let fill = ctx.fonts(|f| f.font_atlas_fill_ratio());
        // (egui empties its glyph atlas when it fills; cached galleys would
        // point at stale texture coordinates.)
        if self.cache_key != Some(h.finish()) || fill < self.atlas_fill {
            self.cache_key = Some(h.finish());
            self.metrics = EguiGridRenderer::measure(ctx, look.fonts, look.linespace);
            self.rows.clear();
        }
        self.atlas_fill = fill;
    }

    fn paint(&mut self, ctx: &Context, painter: &egui::Painter, snap: &Snapshot, look: &Look, palette: &Palette, focused: bool) {
        let m = self.metrics;
        self.rows.resize_with(snap.rows, || None);
        let selection_fill = mix(look.bg, look.accent, style::SELECTED);
        let match_fill = mix(look.bg, look.accent, style::TINT);
        let current_fill = mix(look.bg, look.accent, style::HALF);
        let fill_spans = |painter: &egui::Painter, spans: &[Span], row: usize, fill: Color32, top: f32| {
            for s in spans.iter().filter(|s| s.row == row) {
                let x0 = self.origin.x + s.c0 as f32 * m.width;
                let x1 = self.origin.x + (s.c1 + 1) as f32 * m.width;
                painter.rect_filled(Rect::from_min_max(Pos2::new(x0, top), Pos2::new(x1, top + m.height)), 0.0, fill);
            }
        };
        let mut layouts = Vec::with_capacity(snap.rows);
        for row in 0..snap.rows {
            let cells: Vec<RCell> = snap.row(row).iter().map(|c| resolve_cell(c, palette, &snap.colors)).collect();
            let stale = !self.rows[row].as_ref().is_some_and(|l| l.cells == cells);
            if stale {
                self.rows[row] = Some(layout_row(ctx, cells, look.bg, look.fonts, m));
            }
            layouts.push(row);
        }
        for row in layouts {
            let Some(layout) = &self.rows[row] else { continue };
            let top = self.origin.y + row as f32 * m.height;
            let top_left = Pos2::new(self.origin.x, top);
            for &(x0, x1, color) in &layout.backgrounds {
                painter.rect_filled(Rect::from_min_max(Pos2::new(top_left.x + x0, top), Pos2::new(top_left.x + x1, top + m.height)), 0.0, color);
            }
            fill_spans(painter, &snap.matches, row, match_fill, top);
            fill_spans(painter, &snap.current_match, row, current_fill, top);
            fill_spans(painter, &snap.selection, row, selection_fill, top);
            for (offset, galley) in &layout.runs {
                painter.galley(top_left + *offset, galley.clone(), Color32::WHITE);
            }
            renderer::paint_decos_at(painter, top_left, m.height, &layout.decos);
        }
        // Cursors: the terminal's (a hollow block while it hasn't the keyboard,
        // and in Vi mode), the Vi mode's a block in the accent.
        if let Some(c) = snap.cursor {
            let on = !(look.blink && focused && c.blinking) || self.blink_on();
            if on || !focused {
                let shape = if !focused || snap.vi_cursor.is_some() { term::CursorShape::HollowBlock } else { c.shape };
                self.paint_cursor(painter, snap, look, c, shape, look.fg);
            }
        }
        if let Some(c) = snap.vi_cursor {
            self.paint_cursor(painter, snap, look, c, term::CursorShape::Block, look.accent);
        }
    }

    fn paint_cursor(&self, painter: &egui::Painter, snap: &Snapshot, look: &Look, c: term::CursorDraw, shape: term::CursorShape, color: Color32) {
        let m = self.metrics;
        let width = if c.wide { 2.0 } else { 1.0 } * m.width;
        let min = self.origin + Vec2::new(c.col as f32 * m.width, c.row as f32 * m.height);
        let cell = Rect::from_min_size(min, Vec2::new(width, m.height));
        match shape {
            term::CursorShape::Block => {
                painter.rect_filled(cell, 0.0, color);
                // The character in the cell, in the background's color.
                if let Some(cell_data) = snap.cell(c.row, c.col).filter(|x| x.c != ' ' && x.c != '\0') {
                    let r = resolve_cell(cell_data, &look.palette(), &snap.colors);
                    let mut text = String::new();
                    text.push(r.ch);
                    text.extend(r.zw.iter());
                    let (font_id, fake_italic) = look.fonts.font_id(r.bold, r.italic);
                    let format = TextFormat { font_id, color: look.bg, italics: fake_italic, line_height: Some(m.height), ..Default::default() };
                    let galley = painter.layout_job(LayoutJob::single_section(renderer::text_glyph(&text).to_owned(), format));
                    painter.galley(min, galley, look.bg);
                }
            }
            term::CursorShape::Underline => {
                painter.rect_filled(Rect::from_min_max(Pos2::new(cell.min.x, cell.max.y - 2.0), cell.max), 0.0, color);
            }
            term::CursorShape::Beam => {
                painter.rect_filled(Rect::from_min_size(cell.min, Vec2::new(2.0, m.height)), 0.0, color);
            }
            term::CursorShape::HollowBlock => {
                painter.rect_stroke(cell.shrink(0.5), 0.0, Stroke::new(style::HAIRLINE, color), egui::StrokeKind::Inside);
            }
            term::CursorShape::Hidden => {}
        }
    }

    fn blink_on(&self) -> bool {
        // Solid while typing, then on and off.
        let idle = self.last_input.elapsed();
        idle < BLINK || (idle.as_millis() / BLINK.as_millis()) % 2 == 1
    }

    // Input.

    /// A key, as the terminal's bytes (or the Vi mode's key, or a ⌘ shortcut).
    fn handle_keys(&mut self, ctx: &Context, events: &[Event]) {
        self.drop_text = false;
        for ev in events {
            match ev {
                Event::Key { key, physical_key, pressed, repeat, modifiers } => {
                    let mods = Mods { shift: modifiers.shift, alt: modifiers.alt, ctrl: modifiers.ctrl, sup: modifiers.mac_cmd };
                    if mods.sup && *pressed {
                        match key {
                            Key::A => self.term.select_all(),
                            Key::K => self.term.clear(),
                            Key::F => self.open_find(),
                            // (Copy, paste and cut arrive as their own events.)
                            Key::C | Key::V | Key::X => continue,
                            _ => {}
                        }
                        if matches!(key, Key::A | Key::K | Key::F) {
                            self.last_input = Instant::now();
                            continue;
                        }
                    }
                    // ⇧⌃Space: Vi mode on or off.
                    if *key == Key::Space && mods.ctrl && mods.shift && *pressed {
                        self.term.toggle_vi();
                        self.last_input = Instant::now();
                        continue;
                    }
                    if self.term.vi_active() {
                        if *pressed {
                            self.vi_key(ctx, key, mods);
                        }
                        continue;
                    }
                    // With Option the logical key is the composed character.
                    let which = if modifiers.alt { physical_key.unwrap_or(*key) } else { *key };
                    let Some(k) = term_key(which) else { continue };
                    let kind = match (pressed, repeat) {
                        (false, _) => KeyKind::Release,
                        (true, true) => KeyKind::Repeat,
                        (true, false) => KeyKind::Press,
                    };
                    let shifted = crate::input::key_char(which, true).filter(|_| matches!(k, keys::Key::Char(_)));
                    let ev = KeyEvent { key: k, mods, kind, shifted };
                    if self.term.key(&ev) {
                        self.drop_text = keys::types_text(&ev);
                        self.began_input();
                    }
                }
                Event::Text(text) => {
                    if std::mem::take(&mut self.drop_text) || text.chars().all(char::is_control) {
                        continue;
                    }
                    self.typed(ctx, text);
                }
                Event::Ime(ImeEvent::Commit(text)) => self.typed(ctx, text),
                Event::Paste(text) => {
                    self.term.paste(text);
                    self.began_input();
                }
                Event::Copy => {
                    if let Some(text) = self.term.selection_text() {
                        ctx.copy_text(text);
                    }
                }
                _ => {}
            }
        }
    }

    /// Typed text: to the program, or the Vi mode's keys.
    fn typed(&mut self, ctx: &Context, text: &str) {
        self.last_input = Instant::now();
        if self.term.vi_active() {
            for c in text.chars() {
                self.vi_outcome(ctx, self.term.vi_key(ViKey::Char(c)));
            }
            return;
        }
        self.term.write_text(text);
        self.began_input();
    }

    /// Something was sent to the program: the view is at the bottom again, the
    /// selection is gone (as in Alacritty), the cursor shows.
    fn began_input(&mut self) {
        self.last_input = Instant::now();
        self.term.clear_selection();
        self.term.scroll(term::Scroll::Bottom);
    }

    fn vi_key(&mut self, ctx: &Context, key: &Key, mods: Mods) {
        let vi = match key {
            Key::Escape => ViKey::Escape,
            Key::ArrowUp => ViKey::Up,
            Key::ArrowDown => ViKey::Down,
            Key::ArrowLeft => ViKey::Left,
            Key::ArrowRight => ViKey::Right,
            Key::Home => ViKey::Home,
            Key::End => ViKey::End,
            Key::PageUp => ViKey::PageUp,
            Key::PageDown => ViKey::PageDown,
            // (Plain letters come as text.)
            k if mods.ctrl => match crate::input::key_char(*k, false) {
                Some(c) => ViKey::Ctrl(c),
                None => return,
            },
            _ => return,
        };
        self.last_input = Instant::now();
        let outcome = self.term.vi_key(vi);
        self.vi_outcome(ctx, outcome);
    }

    fn vi_outcome(&mut self, ctx: &Context, outcome: ViOutcome) {
        match outcome {
            ViOutcome::Copy(text) => ctx.copy_text(text),
            ViOutcome::Search(forward) => {
                self.open_find();
                if let Some(f) = &mut self.find {
                    f.forward = forward;
                }
            }
            ViOutcome::Left => self.close_find(ctx),
            _ => {}
        }
    }

    /// The pointer: reporting to a program that takes the mouse, else
    /// selecting; the wheel; ⌘-click on a link.
    fn handle_pointer(&mut self, ctx: &Context, events: &[Event], rect: Rect, look: &Look, out: &mut Output) {
        let (cw, ch) = (self.metrics.width, self.metrics.height);
        let (cols, rows) = self.grid;
        let cell_size = Vec2::new(cw, ch);
        let over = |p: Pos2| rect.contains(p) && ctx.layer_id_at(p).is_none_or(|l| l.order == egui::Order::Middle || l.order == egui::Order::Foreground || l.order == egui::Order::Background);
        for ev in events {
            match ev {
                Event::PointerButton { pos, button, pressed: true, modifiers } if rect.contains(*pos) && over(*pos) => {
                    out.pressed = true;
                    let (row, col, side) = hit_cell(*pos, self.origin, cell_size, cols, rows);
                    let mods = Mods { shift: modifiers.shift, alt: modifiers.alt, ctrl: modifiers.ctrl, sup: modifiers.mac_cmd };
                    if *button == PointerButton::Primary && modifiers.command {
                        let snap = self.term.snapshot();
                        if let Some(link) = self.link_at(&snap, *pos) {
                            out.link = Some(link.target);
                            continue;
                        }
                    }
                    let mouse_button = match button {
                        PointerButton::Primary => Some(MouseButton::Left),
                        PointerButton::Middle => Some(MouseButton::Middle),
                        PointerButton::Secondary => Some(MouseButton::Right),
                        _ => None,
                    };
                    // A program that takes the mouse gets it, unless Shift is held.
                    if self.term.reports_mouse() && !mods.shift {
                        if let Some(b) = mouse_button {
                            self.term.mouse(b, MouseKind::Press, mods, col, row);
                            self.drag = Some(Drag::Report(b));
                            self.reported = Some((row, col));
                        }
                        continue;
                    }
                    if *button == PointerButton::Middle && look.middle_click_paste {
                        if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                            self.term.paste(&text);
                            self.began_input();
                        }
                        continue;
                    }
                    if *button != PointerButton::Primary {
                        continue;
                    }
                    let count = self.clicks.count((row, col), Instant::now());
                    if mods.shift && self.term.has_selection() {
                        self.term.select_update(row, col, side);
                    } else {
                        let ty = match count {
                            2 => SelectionType::Semantic,
                            3 => SelectionType::Lines,
                            _ if mods.alt => SelectionType::Block,
                            _ => SelectionType::Simple,
                        };
                        self.term.select_begin(ty, row, col, side);
                    }
                    self.drag = Some(Drag::Select);
                }
                Event::PointerMoved(pos) => match self.drag {
                    Some(Drag::Select) => {
                        let (row, col, side) = hit_cell(*pos, self.origin, cell_size, cols, rows);
                        self.term.select_update(row, col, side);
                    }
                    Some(Drag::Report(b)) => {
                        let (row, col, _) = hit_cell(*pos, self.origin, cell_size, cols, rows);
                        if self.reported != Some((row, col)) {
                            self.reported = Some((row, col));
                            self.term.mouse(b, MouseKind::Motion, Mods::NONE, col, row);
                        }
                    }
                    None if rect.contains(*pos) && over(*pos) => {
                        let (row, col, _) = hit_cell(*pos, self.origin, cell_size, cols, rows);
                        if self.reported != Some((row, col)) {
                            self.reported = Some((row, col));
                            self.term.mouse(MouseButton::None, MouseKind::Motion, Mods::NONE, col, row);
                        }
                    }
                    None => {}
                },
                Event::PointerButton { pos, button, pressed: false, modifiers } => {
                    let (row, col, _) = hit_cell(*pos, self.origin, cell_size, cols, rows);
                    match self.drag.take() {
                        Some(Drag::Report(b)) => {
                            let mods = Mods { shift: modifiers.shift, alt: modifiers.alt, ctrl: modifiers.ctrl, sup: modifiers.mac_cmd };
                            self.term.mouse(b, MouseKind::Release, mods, col, row);
                        }
                        // A drag, double or triple click, or Shift-click made a selection.
                        Some(Drag::Select) if look.copy_on_select => {
                            if let Some(text) = self.term.selection_text().filter(|t| !t.is_empty()) {
                                ctx.copy_text(text);
                            }
                        }
                        _ => {}
                    }
                    let _ = button;
                }
                Event::MouseWheel { unit, delta, modifiers, .. } => {
                    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p) && over(*p)) else { continue };
                    let lines = match unit {
                        MouseWheelUnit::Point => delta.y / ch,
                        MouseWheelUnit::Line => delta.y,
                        MouseWheelUnit::Page => delta.y * rows as f32,
                    };
                    self.wheel += lines;
                    let whole = self.wheel.trunc();
                    if whole != 0.0 {
                        self.wheel -= whole;
                        let (row, col, _) = hit_cell(pos, self.origin, cell_size, cols, rows);
                        let mods = Mods { shift: modifiers.shift, alt: modifiers.alt, ctrl: modifiers.ctrl, sup: modifiers.mac_cmd };
                        self.term.wheel(whole as i32, col, row, mods);
                    }
                }
                _ => {}
            }
        }
        // Dragging a selection past the top or bottom scrolls (and goes on scrolling).
        if self.drag == Some(Drag::Select) {
            if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
                let delta = if pos.y < rect.min.y { 1 } else if pos.y > rect.max.y { -1 } else { 0 };
                if delta != 0 {
                    self.term.scroll(term::Scroll::Delta(delta));
                    let (row, col, side) = hit_cell(pos, self.origin, cell_size, cols, rows);
                    self.term.select_update(row, col, side);
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
        }
    }

    // Links.

    /// The link under a point: a file reference or a URL in the text (as in
    /// the agent panel), or an OSC 8 hyperlink.
    fn link_at(&self, snap: &Snapshot, pos: Pos2) -> Option<Link> {
        let (row, col, _) = hit_cell(pos, self.origin, self.cell_size(), snap.cols, snap.rows);
        if pos.x < self.origin.x || pos.y < self.origin.y {
            return None;
        }
        if let Some(link) = hyperlink_at(snap, row, col) {
            return Some(link);
        }
        let grid = text_grid(snap);
        let cwd = self.term.current_dir();
        let exists = |p: &str| {
            let path = std::path::Path::new(p);
            if path.is_absolute() { path.is_file() } else { cwd.as_ref().is_some_and(|d| d.join(p).is_file()) }
        };
        links::link_at(&grid, row, col, &exists)
    }

    /// The folder relative paths of its links are in.
    pub fn link_dir(&self) -> Option<std::path::PathBuf> {
        self.term.current_dir()
    }

    // The find bar.

    /// Enter, Shift+Enter and Esc in the find field, taken before the field
    /// is added (it would let go of the keyboard itself).
    fn show_find_consumes(&mut self, ctx: &Context) {
        let Some(find) = &self.find else { return };
        if !find.focused {
            return;
        }
        let forward = find.forward;
        let (enter, shift_enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::SHIFT, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if esc {
            self.close_find(ctx);
        } else if enter || shift_enter {
            self.term.search_step(forward == enter);
        }
    }

    /// The bar over the terminal's top right. `Some(true)` when it was clicked.
    fn show_find(&mut self, ui: &mut Ui, rect: Rect, look: &Look) -> Option<bool> {
        let id = self.find_id();
        let chrome = look.chrome();
        let row_h = self.metrics.height;
        let find = self.find.as_mut()?;
        let width = (rect.width() - 16.0).clamp(120.0, 340.0);
        let height = row_h + 8.0;
        let bar = Rect::from_min_size(Pos2::new(rect.max.x - width - 8.0, rect.min.y + 6.0), Vec2::new(width, height));
        let mut clicked = false;
        let mut step: Option<bool> = None;
        let mut close = false;
        let mut changed = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            ui.painter().rect_filled(bar, style::RADIUS, chrome.panel);
            ui.painter().rect_stroke(bar, style::RADIUS, Stroke::new(style::HAIRLINE, chrome.line()), egui::StrokeKind::Inside);
            let buttons = 3.0 * (row_h + 2.0);
            let field = Rect::from_min_max(Pos2::new(bar.min.x + 4.0, bar.min.y + 3.0), Pos2::new(bar.max.x - buttons - 4.0, bar.max.y - 3.0));
            let valid = self.term.search_valid();
            let font = look.fonts.font_id(false, false).0;
            let resp = ui
                .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                    let margin = egui::Margin::symmetric(style::GAP as i8, 0);
                    style::text_field(ui, &chrome, id, margin, |ui| {
                        let edit = egui::TextEdit::singleline(&mut find.text)
                            .id(id)
                            .font(font.clone())
                            .text_color(if valid { chrome.fg } else { Color32::from_rgb(0xf1, 0x4c, 0x4c) })
                            .hint_text(egui::RichText::new("Find").font(font.clone()).color(chrome.dim(chrome.fg)))
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE);
                        ui.add(edit)
                    })
                    .0
                })
                .inner;
            if std::mem::take(&mut find.focus) {
                resp.request_focus();
            }
            find.focused = resp.has_focus();
            changed = resp.changed();
            clicked |= resp.clicked() || resp.gained_focus();
            // ↑ ↓ ×
            let mut x = field.max.x + 6.0;
            for (label, action) in [("↑", 0), ("↓", 1), ("×", 2)] {
                let b = Rect::from_min_size(Pos2::new(x, bar.min.y + 3.0), Vec2::new(row_h, height - 6.0));
                x += row_h + 2.0;
                let r = ui.interact(b, id.with(label), Sense::CLICK);
                style::paint_button_fill(ui.painter(), b, &chrome, chrome.panel, r.hovered(), false);
                ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, label, font.clone(), chrome.text(r.hovered()));
                if r.clicked() {
                    clicked = true;
                    match action {
                        0 => step = Some(false),
                        1 => step = Some(true),
                        _ => close = true,
                    }
                }
            }
        });
        if changed {
            self.term.search(&find.text);
        }
        if let Some(forward) = step {
            self.term.search_step(forward);
        }
        if close {
            let ctx = ui.ctx().clone();
            self.close_find(&ctx);
        }
        Some(clicked)
    }
}

/// How long the cursor stays on or off while blinking.
const BLINK: Duration = Duration::from_millis(530);

/// egui's key as the terminal's: what a character key types is its US-layout
/// character, so layouts and Option's composed characters don't matter.
fn term_key(key: Key) -> Option<keys::Key> {
    use keys::Key as K;
    Some(match key {
        Key::ArrowUp => K::Up,
        Key::ArrowDown => K::Down,
        Key::ArrowLeft => K::Left,
        Key::ArrowRight => K::Right,
        Key::Enter => K::Enter,
        Key::Tab => K::Tab,
        Key::Backspace => K::Backspace,
        Key::Escape => K::Escape,
        Key::Insert => K::Insert,
        Key::Delete => K::Delete,
        Key::Home => K::Home,
        Key::End => K::End,
        Key::PageUp => K::PageUp,
        Key::PageDown => K::PageDown,
        Key::F1 => K::F(1),
        Key::F2 => K::F(2),
        Key::F3 => K::F(3),
        Key::F4 => K::F(4),
        Key::F5 => K::F(5),
        Key::F6 => K::F(6),
        Key::F7 => K::F(7),
        Key::F8 => K::F(8),
        Key::F9 => K::F(9),
        Key::F10 => K::F(10),
        Key::F11 => K::F(11),
        Key::F12 => K::F(12),
        Key::F13 => K::F(13),
        Key::F14 => K::F(14),
        Key::F15 => K::F(15),
        Key::F16 => K::F(16),
        Key::F17 => K::F(17),
        Key::F18 => K::F(18),
        Key::F19 => K::F(19),
        Key::F20 => K::F(20),
        other => K::Char(crate::input::key_char(other, false)?),
    })
}

/// The screen's text as a grid `links` reads: a character per cell, and the
/// second cell of a wide one empty, as Neovim's grids have them.
fn text_grid(snap: &Snapshot) -> grid::Grid {
    let mut g = grid::Grid::new(snap.cols, snap.rows);
    for row in 0..snap.rows {
        let cells: Vec<grid::LineCell> = snap
            .row(row)
            .iter()
            .map(|c| {
                let mut text = String::new();
                if !c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    text.push(if c.c == '\0' { ' ' } else { c.c });
                    text.extend(c.zerowidth().into_iter().flatten());
                }
                grid::LineCell { text, hl_id: 0, repeat: 1 }
            })
            .collect();
        g.put_line(row, 0, &cells);
    }
    g
}

/// An OSC 8 hyperlink on a cell: its target, and the run of cells with it.
fn hyperlink_at(snap: &Snapshot, row: usize, col: usize) -> Option<Link> {
    let uri = snap.cell(row, col)?.hyperlink()?.uri().to_owned();
    let (mut c0, mut c1) = (col, col);
    let same = |c: usize| snap.cell(row, c).and_then(|x| x.hyperlink()).is_some_and(|h| h.uri() == uri);
    while c0 > 0 && same(c0 - 1) {
        c0 -= 1;
    }
    while same(c1 + 1) {
        c1 += 1;
    }
    let target = match uri.strip_prefix("file://") {
        // (A file URL may name a host: file://host/path.)
        Some(rest) => {
            let path = rest.find('/').map_or(rest, |at| &rest[at..]);
            Target::File { path: path.to_owned(), line: None, col: None }
        }
        None => Target::Url(uri),
    };
    Some(Link { target, spans: vec![(row, c0, c1)] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use term::SpawnOptions;

    #[test]
    fn hit_testing_finds_the_cell_and_its_half() {
        let origin = Pos2::new(10.0, 20.0);
        let cell = Vec2::new(8.0, 16.0);
        let at = |x: f32, y: f32| hit_cell(Pos2::new(x, y), origin, cell, 10, 5);
        assert_eq!(at(10.0, 20.0), (0, 0, Side::Left));
        assert_eq!(at(10.0 + 5.0, 20.0 + 17.0), (1, 0, Side::Right));
        assert_eq!(at(10.0 + 8.0 * 3.0 + 3.9, 20.0), (0, 3, Side::Left));
        assert_eq!(at(10.0 + 8.0 * 3.0 + 4.0, 20.0), (0, 3, Side::Right));
        // Off the grid: the edge cells, the side facing away.
        assert_eq!(at(0.0, 0.0), (0, 0, Side::Left));
        assert_eq!(at(500.0, 500.0), (4, 9, Side::Right));
    }

    #[test]
    fn clicks_count_up_to_three_and_start_over() {
        let t = Instant::now();
        let mut c = Clicks { at: t, cell: (usize::MAX, 0), count: 0 };
        assert_eq!(c.count((1, 1), t), 1);
        assert_eq!(c.count((1, 1), t + Duration::from_millis(100)), 2);
        assert_eq!(c.count((1, 1), t + Duration::from_millis(200)), 3);
        assert_eq!(c.count((1, 1), t + Duration::from_millis(300)), 1, "a fourth starts over");
        assert_eq!(c.count((1, 2), t + Duration::from_millis(350)), 1, "another cell");
        assert_eq!(c.count((1, 2), t + Duration::from_millis(1000)), 1, "too slow");
    }

    #[test]
    fn egui_keys_become_the_terminals() {
        assert_eq!(term_key(Key::ArrowUp), Some(keys::Key::Up));
        assert_eq!(term_key(Key::F12), Some(keys::Key::F(12)));
        assert_eq!(term_key(Key::A), Some(keys::Key::Char('a')));
        assert_eq!(term_key(Key::Space), Some(keys::Key::Char(' ')));
        assert_eq!(term_key(Key::Num6), Some(keys::Key::Char('6')));
        assert_eq!(term_key(Key::OpenBracket), Some(keys::Key::Char('[')));
        assert_eq!(term_key(Key::Enter), Some(keys::Key::Enter));
    }

    /// A terminal running a script, once its first row has text.
    fn snap_of(script: &str, cols: u16, rows: u16) -> (Terminal, Snapshot) {
        let t = Terminal::spawn(SpawnOptions::new(vec!["/bin/sh".into(), "-c".into(), script.into()], term::Size { cols, rows, cell_w: 8, cell_h: 16 })).unwrap();
        let start = Instant::now();
        while t.snapshot().row(0).iter().all(|c| c.c == ' ') && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        // (The output may arrive in pieces.)
        std::thread::sleep(Duration::from_millis(200));
        let snap = t.snapshot();
        (t, snap)
    }

    #[test]
    fn cells_resolve_to_the_themes_colors() {
        let (_t, snap) = snap_of("printf '\\033[31mred\\033[0m \\033[1;38;2;1;2;3mtrue\\033[0m \\033[7minv\\033[0m \\033[38;5;196mx'; sleep 1", 40, 3);
        let palette = Palette::from_theme(0xeeeeee, 0x101010, &{
            let mut a = [None; 16];
            a[1] = Some(0xff0000);
            a
        });
        let cell = |col: usize| resolve_cell(snap.cell(0, col).unwrap(), &palette, &snap.colors);
        assert_eq!(cell(0).fg, Color32::from_rgb(0xff, 0, 0), "ANSI red from g:terminal_color_1");
        assert_eq!(cell(0).bg, Color32::from_rgb(0x10, 0x10, 0x10), "the default background");
        assert_eq!(cell(2).ch, 'd');
        let true_color = cell(4);
        assert_eq!((true_color.fg, true_color.bold), (Color32::from_rgb(1, 2, 3), true));
        let inverse = cell(9);
        assert_eq!((inverse.fg, inverse.bg), (Color32::from_rgb(0x10, 0x10, 0x10), Color32::from_rgb(0xee, 0xee, 0xee)), "inverse swaps them");
        assert_eq!(cell(13).fg, Color32::from_rgb(255, 0, 0), "256-color 196");
        // A color the scheme doesn't set is Alacritty's.
        let default_red = resolve_cell(snap.cell(0, 0).unwrap(), &Palette::default(), &snap.colors);
        assert_eq!(default_red.fg, Color32::from_rgb(0xac, 0x42, 0x42));
    }

    #[test]
    fn links_are_found_in_the_terminals_text_and_in_hyperlinks() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        std::fs::write(dir.join("eshvi-term-link.txt"), "x").unwrap();
        let script = "printf 'see eshvi-term-link.txt:7 or https://example.com/x.\\r\\n\\033]8;;https://osc8.example/a\\033\\\\link\\033]8;;\\033\\\\ plain'; sleep 1";
        let mut opts = SpawnOptions::new(vec!["/bin/sh".into(), "-c".into(), script.into()], term::Size { cols: 60, rows: 4, cell_w: 8, cell_h: 16 });
        opts.cwd = Some(dir.clone());
        let t = Terminal::spawn(opts).unwrap();
        let start = Instant::now();
        while t.snapshot().row(1).iter().all(|c| c.c == ' ') && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut view = TerminalView::new(t);
        view.metrics = CellMetrics { width: 8.0, height: 16.0 };
        let snap = view.term.snapshot();
        let at = |row: usize, col: usize| view.link_at(&snap, view.origin + Vec2::new(col as f32 * 8.0 + 1.0, row as f32 * 16.0 + 1.0));
        let file = at(0, 6).unwrap();
        assert_eq!(file.target, Target::File { path: "eshvi-term-link.txt".into(), line: Some(7), col: None });
        assert_eq!(file.spans, [(0, 4, 24)]);
        assert_eq!(at(0, 34).unwrap().target, Target::Url("https://example.com/x".into()));
        assert_eq!(at(0, 0), None);
        let osc8 = at(1, 2).unwrap();
        assert_eq!((osc8.target, osc8.spans), (Target::Url("https://osc8.example/a".into()), vec![(1, 0, 3)]));
        assert_eq!(at(1, 6), None, "text after the hyperlink is plain");
        let _ = std::fs::remove_file(dir.join("eshvi-term-link.txt"));
    }

    /// Run the view through egui headlessly for a few frames; the text it paints.
    #[test]
    fn the_view_paints_the_terminals_text_in_its_font() {
        let ctx = Context::default();
        let spec = crate::fonts::FontSpec::default_with_size(14.0);
        let fonts = crate::fonts::install(&ctx, &spec, None, None, None, None);
        let t = Terminal::spawn(SpawnOptions::new(
            vec!["/bin/sh".into(), "-c".into(), "printf 'hello \\033[31mred\\033[0m\\r\\n日本'; sleep 2".into()],
            term::Size { cols: 40, rows: 5, cell_w: 8, cell_h: 16 },
        ))
        .unwrap();
        let mut view = TerminalView::new(t);
        let mut painted: Vec<String> = Vec::new();
        let start = Instant::now();
        while !(painted.iter().any(|s| s == "hello ") && painted.iter().any(|s| s == "red")) && start.elapsed() < Duration::from_secs(10) {
            painted.clear();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let look = Look { fonts: &fonts, linespace: 0.0, fg: Color32::WHITE, bg: Color32::BLACK, accent: Color32::BLUE, ansi: [None; 16], option_as_meta: true, blink: false, copy_on_select: true, middle_click_paste: true };
                view.show(ui, &look, true);
            });
            out.textures_delta.clear();
            for clipped in out.shapes {
                collect_text(&clipped.shape, &mut painted);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(painted.iter().any(|s| s == "hello "), "{painted:?}");
        assert!(painted.iter().any(|s| s == "red"), "a run per style: {painted:?}");
        assert!(painted.iter().any(|s| s == "日"), "a wide character is its own run: {painted:?}");
        // A cell is the font's (a size, not Neovim's): the columns fit the room inside the margin.
        assert!(view.cell_size().x > 4.0 && view.cell_size().y > 8.0);
        let (cols, rows) = view.grid_size();
        assert_eq!(cols, ((400.0 - 2.0 * style::GAP) / view.cell_size().x).floor() as usize);
        assert_eq!(rows, ((300.0 - style::GAP) / view.cell_size().y).floor() as usize);
        assert_eq!(view.terminal().size().cols as usize, cols);
    }

    /// One frame of the view with these events.
    fn frame(ctx: &Context, view: &mut TerminalView, fonts: &GridFonts, events: Vec<Event>) -> egui::FullOutput {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            let look = Look { fonts, linespace: 0.0, fg: Color32::WHITE, bg: Color32::BLACK, accent: Color32::BLUE, ansi: [None; 16], option_as_meta: true, blink: false, copy_on_select: true, middle_click_paste: true };
            view.show(ui, &look, true);
        });
        out.textures_delta.clear();
        out
    }

    fn rows_of(t: &Terminal) -> Vec<String> {
        let snap = t.snapshot();
        (0..snap.rows).map(|r| snap.row(r).iter().map(|c| c.c).collect::<String>().trim_end().to_owned()).filter(|r| !r.is_empty()).collect()
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn key(key: Key, modifiers: egui::Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    fn click(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
    }

    fn drag_again(from: Pos2, to: Pos2) -> Vec<Event> {
        vec![click(from, true, egui::Modifiers::NONE), Event::PointerMoved(to), click(to, false, egui::Modifiers::NONE)]
    }

    fn copied(out: &egui::FullOutput) -> Vec<String> {
        out.platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// Typing, the keys, selecting with the mouse and copying, through egui's events.
    #[test]
    fn typing_selecting_and_copying_through_the_view() {
        let ctx = Context::default();
        let fonts = crate::fonts::install(&ctx, &crate::fonts::FontSpec::default_with_size(14.0), None, None, None, None);
        // `cat -v` shows the bytes it reads; raw mode, so Enter's carriage return isn't turned into a newline.
        let script = "stty raw -echo; cat -v";
        let t = Terminal::spawn(SpawnOptions::new(vec!["/bin/sh".into(), "-c".into(), script.into()], term::Size { cols: 40, rows: 5, cell_w: 8, cell_h: 16 })).unwrap();
        let mut view = TerminalView::new(t);
        frame(&ctx, &mut view, &fonts, vec![]);
        std::thread::sleep(Duration::from_millis(300));
        let none = egui::Modifiers::NONE;
        // Typed text, a control key, Option as Meta, Shift+Enter, an arrow key.
        let events = vec![
            Event::Text("foo bar".into()),
            key(Key::A, egui::Modifiers { ctrl: true, ..none }),
            // Option+x arrives with the composed character as text too.
            key(Key::X, egui::Modifiers { alt: true, ..none }),
            Event::Text("≈".into()),
            key(Key::Enter, egui::Modifiers { shift: true, ..none }),
            key(Key::ArrowUp, none),
        ];
        frame(&ctx, &mut view, &fonts, events);
        until("the bytes", || rows_of(view.terminal()).first().is_some_and(|r| r == "foo bar^A^[x^[^M^[[A"));
        // Double click selects a word; a drag selects between cells; ⌘C copies.
        let (cw, ch) = (view.cell_size().x, view.cell_size().y);
        let origin = view.origin;
        let at = move |col: f32, row: f32| origin + Vec2::new(col * cw + cw * 0.3, row * ch + ch * 0.5);
        let double = [click(at(5.0, 0.0), true, none), click(at(5.0, 0.0), false, none), click(at(5.0, 0.0), true, none), click(at(5.0, 0.0), false, none)];
        // (Selecting copies too: `copy_on_select`.)
        let out = frame(&ctx, &mut view, &fonts, double.to_vec());
        assert_eq!(view.terminal().selection_text().as_deref(), Some("bar^A^"), "a word ends at a delimiter, [");
        assert_eq!(copied(&out), ["bar^A^"], "copied when the button is let go");
        let out = frame(&ctx, &mut view, &fonts, vec![Event::Copy]);
        assert_eq!(copied(&out), ["bar^A^"]);
        let drag = vec![click(at(1.0, 0.0), true, none), Event::PointerMoved(at(2.0, 0.0)), Event::PointerMoved(at(4.0, 0.0)), click(at(4.0, 0.0), false, none)];
        let out = frame(&ctx, &mut view, &fonts, drag);
        assert_eq!(view.terminal().selection_text().as_deref(), Some("oo "), "up to the left half of the cell the drag ends in");
        assert_eq!(copied(&out), ["oo "]);
        // A plain click copies nothing (there's no selection).
        let out = frame(&ctx, &mut view, &fonts, vec![click(at(9.0, 2.0), true, none), click(at(9.0, 2.0), false, none)]);
        assert!(copied(&out).is_empty(), "{:?}", copied(&out));
        frame(&ctx, &mut view, &fonts, drag_again(at(1.0, 0.0), at(4.0, 0.0)));
        // Shift-click extends it.
        let shift = egui::Modifiers { shift: true, ..none };
        frame(&ctx, &mut view, &fonts, vec![click(at(6.0, 0.0), true, shift), click(at(6.0, 0.0), false, shift)]);
        assert_eq!(view.terminal().selection_text().as_deref(), Some("oo ba"));
        // A click clears it; so does typing.
        frame(&ctx, &mut view, &fonts, vec![click(at(8.0, 0.0), true, none), click(at(8.0, 0.0), false, none)]);
        assert_eq!(view.terminal().selection_text(), None);
        frame(&ctx, &mut view, &fonts, vec![click(at(1.0, 0.0), true, none), Event::PointerMoved(at(4.0, 0.0)), click(at(4.0, 0.0), false, none)]);
        assert!(view.terminal().has_selection());
        frame(&ctx, &mut view, &fonts, vec![Event::Text("q".into())]);
        assert!(!view.terminal().has_selection(), "typing clears the selection");
        // ⌘A selects everything, ⌘F opens the find bar and ⌘K clears.
        let cmd = egui::Modifiers { mac_cmd: true, command: true, ..none };
        frame(&ctx, &mut view, &fonts, vec![key(Key::A, cmd)]);
        assert!(view.terminal().selection_text().unwrap().starts_with("foo bar"));
        frame(&ctx, &mut view, &fonts, vec![key(Key::F, cmd)]);
        assert!(view.find.is_some());
        frame(&ctx, &mut view, &fonts, vec![]);
        assert!(view.find.as_ref().is_some_and(|f| f.focused), "the bar has the keyboard");
        // (Keys go to the bar, not the program.) Esc closes it.
        frame(&ctx, &mut view, &fonts, vec![Event::Text("zzz".into())]);
        assert_eq!(view.find.as_ref().map(|f| f.text.as_str()), Some("zzz"));
        frame(&ctx, &mut view, &fonts, vec![key(Key::Escape, none)]);
        assert!(view.find.is_none());
        // Paste goes to the program (cat -v shows it).
        frame(&ctx, &mut view, &fonts, vec![Event::Paste("pasted\ntext".into())]);
        until("the paste", || rows_of(view.terminal()).iter().any(|r| r.contains("pasted")));
    }

    /// The program takes the mouse: a click is reported, and Shift selects anyway.
    #[test]
    fn a_program_that_takes_the_mouse_is_sent_it_and_shift_selects_anyway() {
        let ctx = Context::default();
        let fonts = crate::fonts::install(&ctx, &crate::fonts::FontSpec::default_with_size(14.0), None, None, None, None);
        // Button and any-motion events with SGR coordinates (mode 1000 and 1006), then show what comes in.
        let script = "printf 'text here\\033[?1000h\\033[?1006h'; stty raw -echo; cat -v";
        let t = Terminal::spawn(SpawnOptions::new(vec!["/bin/sh".into(), "-c".into(), script.into()], term::Size { cols: 40, rows: 5, cell_w: 8, cell_h: 16 })).unwrap();
        let mut view = TerminalView::new(t);
        frame(&ctx, &mut view, &fonts, vec![]);
        until("the mouse mode", || view.terminal().reports_mouse());
        let (cw, ch) = (view.cell_size().x, view.cell_size().y);
        let origin = view.origin;
        let at = move |col: f32, row: f32| origin + Vec2::new(col * cw + cw * 0.3, row * ch + ch * 0.5);
        let none = egui::Modifiers::NONE;
        frame(&ctx, &mut view, &fonts, vec![click(at(3.0, 2.0), true, none), click(at(3.0, 2.0), false, none)]);
        until("the click", || rows_of(view.terminal()).iter().any(|r| r.contains("^[[<0;4;3M^[[<0;4;3m")));
        assert_eq!(view.terminal().selection_text(), None, "reported, not selected");
        let shift = egui::Modifiers { shift: true, ..none };
        frame(&ctx, &mut view, &fonts, vec![click(at(0.0, 0.0), true, shift), Event::PointerMoved(at(3.0, 0.0)), click(at(3.0, 0.0), false, shift)]);
        assert_eq!(view.terminal().selection_text().as_deref(), Some("tex"));
        // The wheel is reported too.
        frame(&ctx, &mut view, &fonts, vec![Event::PointerMoved(at(1.0, 1.0)), Event::MouseWheel { unit: MouseWheelUnit::Line, delta: Vec2::new(0.0, 1.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: none }]);
        until("the wheel", || rows_of(view.terminal()).iter().any(|r| r.contains("^[[<64;2;2M")));
    }

    /// ⇧⌃Space turns Vi mode on, its keys are text events, `y` copies, Esc leaves.
    #[test]
    fn vi_mode_through_the_view() {
        let ctx = Context::default();
        let fonts = crate::fonts::install(&ctx, &crate::fonts::FontSpec::default_with_size(14.0), None, None, None, None);
        let script = "printf 'one two\\r\\nsecond line'; sleep 5";
        let t = Terminal::spawn(SpawnOptions::new(vec!["/bin/sh".into(), "-c".into(), script.into()], term::Size { cols: 40, rows: 5, cell_w: 8, cell_h: 16 })).unwrap();
        let mut view = TerminalView::new(t);
        frame(&ctx, &mut view, &fonts, vec![]);
        until("the output", || rows_of(view.terminal()).len() == 2);
        let none = egui::Modifiers::NONE;
        let space = key(Key::Space, egui::Modifiers { ctrl: true, shift: true, ..none });
        frame(&ctx, &mut view, &fonts, vec![space.clone()]);
        assert!(view.terminal().vi_active());
        let text = |s: &str| Event::Text(s.into());
        // Top left, a word forward, select to its end, copy.
        let out = frame(&ctx, &mut view, &fonts, vec![text("g"), text("g"), text("0"), text("w"), text("v"), text("e"), text("y")]);
        assert_eq!(copied(&out), ["two"]);
        // Keys don't reach the program in Vi mode; Esc leaves.
        frame(&ctx, &mut view, &fonts, vec![key(Key::Escape, none)]);
        assert!(!view.terminal().vi_active());
        // The same key again turns it on, and off.
        frame(&ctx, &mut view, &fonts, vec![space.clone(), space]);
        assert!(!view.terminal().vi_active());
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }
}
