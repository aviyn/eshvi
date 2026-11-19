//! Markdown previews (`:EshviPreview`, `preview.lua`): a buffer's Markdown
//! rendered over a window, its own (in place) or one beside it. The text
//! is split into its top-level blocks with their source lines, so the view
//! can follow the cursor: the block holding its line is scrolled to the
//! top when the line changes (else the wheel scrolls freely). Prose is
//! rendered by egui_commonmark, code as in LSP hovers (`markdown.rs`),
//! tables here (egui_commonmark's don't wrap: they scroll sideways).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, FontId, Pos2, Rect, TextStyle, Ui, UiBuilder, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use nvim_bridge::gui::PreviewWin;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag};
use syntect::highlighting::Theme;

use crate::fonts::GridFonts;
use crate::icons::IconTextures;
use crate::style::{self, Chrome};

/// The room left and right of the text; the scroll bar is in the right one.
const MARGIN: i8 = 28;

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Code { lang: String, text: String },
    Markdown(String),
    /// Each row's cells as Markdown, the header first.
    Table { align: Vec<Alignment>, rows: Vec<Vec<String>> },
}

/// A top-level block and the source lines it spans (1-based).
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub lines: Range<usize>,
    pub body: Body,
}

/// Split Markdown into its top-level blocks. Link reference definitions
/// (`[x]: url`) aren't blocks; they are added to each Markdown block so its
/// links still resolve.
pub fn blocks(text: &str) -> Vec<Block> {
    let line_of = |offset: usize| text[..offset.min(text.len())].matches('\n').count() + 1;
    let refs: String = text
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            l.len() - t.len() < 4 && t.starts_with('[') && t.contains("]:") && !t.starts_with("[^")
        })
        .map(|l| format!("\n{l}"))
        .collect();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut code: Option<(String, String)> = None;
    let mut table: Option<(Vec<Alignment>, Vec<Vec<String>>)> = None;
    for (event, range) in Parser::new_ext(text, Options::all()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    start = range.start;
                    match &tag {
                        Tag::CodeBlock(kind) => {
                            let lang = match kind {
                                CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
                                CodeBlockKind::Indented => String::new(),
                            };
                            code = Some((lang, String::new()));
                        }
                        Tag::Table(align) => table = Some((align.clone(), Vec::new())),
                        _ => {}
                    }
                } else if let Some((_, rows)) = &mut table {
                    match tag {
                        Tag::TableHead | Tag::TableRow => rows.push(Vec::new()),
                        Tag::TableCell if depth == 2 => {
                            if let Some(row) = rows.last_mut() {
                                row.push(format!("{}{refs}", cell_markdown(&text[range.clone()])));
                            }
                        }
                        _ => {}
                    }
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    // (A list's range takes the blank line after it.)
                    let end = start + text[start..range.end].trim_end().len();
                    let lines = line_of(start)..line_of(end.saturating_sub(1)) + 1;
                    let body = match (code.take(), table.take()) {
                        (Some((lang, text)), _) => Body::Code { lang, text: text.trim_end_matches('\n').to_owned() },
                        (_, Some((align, rows))) => Body::Table { align, rows },
                        _ => Body::Markdown(format!("{}{refs}", &text[start..range.end])),
                    };
                    out.push(Block { lines, body });
                }
            }
            Event::Text(t) if depth == 1 => {
                if let Some((_, text)) = &mut code {
                    text.push_str(&t);
                }
            }
            // Rules and the like: blocks without a start and end.
            Event::Rule | Event::Html(_) if depth == 0 => {
                let lines = line_of(range.start)..line_of(range.end.saturating_sub(1)) + 1;
                out.push(Block { lines, body: Body::Markdown(text[range].to_owned()) });
            }
            _ => {}
        }
    }
    out
}

/// A cell's source, to be parsed on its own: what would start a block
/// there (`# x`, `- x`, `1. x`, `---`) is escaped, being text in a cell.
fn cell_markdown(source: &str) -> String {
    let mut cell = source.trim().replace("\\|", "|");
    let inline = cell.is_empty() || matches!(Parser::new_ext(&cell, Options::all()).next(), Some(Event::Start(Tag::Paragraph)));
    if !inline {
        if let Some(mark) = cell.find(|c: char| c.is_ascii_punctuation()) {
            cell.insert(mark, '\\');
        }
    }
    cell
}

/// A cell's width on one line, and that of its longest word.
fn cell_widths(ui: &Ui, cell: &str, font: &FontId) -> (f32, f32) {
    let code = TextStyle::Monospace.resolve(ui.style());
    let width = |text: &str, font: &FontId| ui.painter().layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x;
    let (mut line, mut word) = (0.0f32, 0.0f32);
    for event in Parser::new_ext(cell, Options::all()) {
        let (text, font) = match &event {
            Event::Text(t) => (t, font),
            Event::Code(t) => (t, &code),
            _ => continue,
        };
        line += width(text, font);
        for w in text.split_whitespace() {
            word = word.max(width(w, font));
        }
    }
    (line, word)
}

/// Columns in `room`, as a browser does: as wide as their text when that
/// fits; else each gets its longest word, and shares what's left by how much
/// more it would like. A word over an equal share (a URL) doesn't count as
/// more than that: it breaks, and the other columns keep theirs.
fn column_widths(natural: &[f32], least: &[f32], room: f32) -> Vec<f32> {
    let total: f32 = natural.iter().sum();
    if total <= room {
        return natural.to_vec();
    }
    let fair = room / natural.len() as f32;
    let least: Vec<f32> = least.iter().map(|l| l.min(fair)).collect();
    let floor: f32 = least.iter().sum();
    let share = (room - floor) / (total - floor);
    natural.iter().zip(&least).map(|(n, l)| l + (n - l) * share).collect()
}

/// A table in the width there is, its cells wrapping, with lines between its
/// rows and a bold header (as VS Code's preview). Returns where it went.
fn show_table(ui: &mut Ui, viewer: impl Fn() -> CommonMarkViewer<'static>, cache: &mut CommonMarkCache, align: &[Alignment], rows: &[Vec<String>]) -> Rect {
    let pad = Vec2::new(f32::from(crate::style::POPUP_PAD.left), f32::from(crate::style::POPUP_PAD.top));
    let body = TextStyle::Body.resolve(ui.style());
    let mut bold = TextStyle::Heading.resolve(ui.style());
    bold.size = body.size;
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    // (A little over: a cell that fits mustn't wrap on rounding.)
    let slack = pad.x * 2.0 + 2.0;
    let (mut natural, mut least) = (vec![slack; columns], vec![slack; columns]);
    let mut lines = Vec::with_capacity(rows.len());
    for (r, row) in rows.iter().enumerate() {
        let font = if r == 0 { &bold } else { &body };
        let mut in_row = Vec::with_capacity(row.len());
        for (c, cell) in row.iter().enumerate() {
            let (line, word) = cell_widths(ui, cell, font);
            natural[c] = natural[c].max(line + slack);
            least[c] = least[c].max(word + slack);
            in_row.push(line);
        }
        lines.push(in_row);
    }
    let widths = column_widths(&natural, &least, ui.available_width());
    let origin = ui.cursor().min;
    let right = origin.x + widths.iter().sum::<f32>();
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    let mut y = origin.y;
    for (r, row) in rows.iter().enumerate() {
        let mut x = origin.x;
        let mut bottom = y + pad.y * 2.0 + ui.text_style_height(&TextStyle::Body);
        for (c, cell) in row.iter().enumerate() {
            let room = widths[c] - pad.x * 2.0;
            let free = (room - lines[r][c]).max(0.0);
            let shift = match align.get(c) {
                Some(Alignment::Right) => free,
                Some(Alignment::Center) => free / 2.0,
                _ => 0.0,
            };
            let place = Rect::from_min_size(Pos2::new(x + pad.x + shift, y + pad.y), Vec2::new(room - shift, f32::INFINITY));
            let mut ui = ui.new_child(UiBuilder::new().id_salt((r, c)).max_rect(place));
            if r == 0 {
                let style = ui.style_mut();
                style.text_styles.insert(TextStyle::Body, bold.clone());
                style.visuals.widgets.noninteractive.fg_stroke.color = style.visuals.widgets.active.fg_stroke.color;
                style.visuals.widgets.inactive.fg_stroke.color = style.visuals.widgets.active.fg_stroke.color;
            }
            viewer().show(&mut ui, cache, cell);
            bottom = bottom.max(ui.min_rect().max.y + pad.y);
            x += widths[c];
        }
        if r + 1 < rows.len() {
            ui.painter().hline(origin.x..=right, bottom, stroke);
        }
        y = bottom;
    }
    let rect = Rect::from_min_max(origin, Pos2::new(right, y));
    ui.advance_cursor_after_rect(rect);
    rect
}

/// A previewed buffer: its blocks, code laid out once.
struct Doc {
    blocks: Vec<Block>,
    code: Vec<Option<LayoutJob>>,
    cache: CommonMarkCache,
}

#[derive(Default)]
pub struct Previews {
    /// This tab page's preview windows.
    pub wins: Vec<PreviewWin>,
    docs: HashMap<i64, Doc>,
    /// The source line each window last scrolled to.
    scrolled: HashMap<i64, usize>,
    /// What each window shows, as the scroll bar wants it (`viewport`).
    views: HashMap<i64, (i64, i64, i64)>,
    /// Where the scroll bar was dragged to, for the next draw.
    goto: HashMap<i64, f32>,
    /// The source line at the top of a preview the user scrolled (with the wheel
    /// or the scroll bar, not the cursor moving it), and its buffer, until read.
    scrolled_top: Option<(i64, usize)>,
    /// The size the code in `docs` was laid out at (it is again when that changes).
    code_size: f32,
}

impl Previews {
    /// (Texts stay: Neovim sends one again only when it changed.)
    pub fn set_wins(&mut self, wins: Vec<PreviewWin>) {
        self.scrolled.retain(|w, _| wins.iter().any(|p| p.win == *w));
        self.views.retain(|w, _| wins.iter().any(|p| p.win == *w));
        self.wins = wins;
    }

    /// The preview's part in view, for its window's scroll bar: its first
    /// pixel, the one after its last, and its height. The window's own
    /// viewport is that of the text behind the preview.
    pub fn viewport(&self, win: i64) -> Option<(i64, i64, i64)> {
        self.views.get(&win).copied()
    }

    /// The scroll bar moved: show from pixel `top` on.
    pub fn scroll_to(&mut self, win: i64, top: i64) {
        self.goto.insert(win, top as f32);
    }

    /// The buffer and source line a user's scroll left at the top of its
    /// preview, once.
    pub fn take_scrolled(&mut self) -> Option<(i64, usize)> {
        self.scrolled_top.take()
    }

    pub fn set_text(&mut self, buf: i64, text: &str) {
        let blocks = blocks(text);
        let doc = Doc { code: vec![None; blocks.len()], blocks, cache: CommonMarkCache::default() };
        self.docs.insert(buf, doc);
    }

    pub fn win(&self, win: i64) -> Option<&PreviewWin> {
        self.wins.iter().find(|p| p.win == win)
    }

    /// Draw window `win`'s preview in `rect` (`ui` is already there).
    /// `scrolls`: false while something else is drawn over the pointer.
    /// `fonts` carries the preview's size (`[preview] font_size`, zoomed).
    #[allow(clippy::too_many_arguments)]
    pub fn show(&mut self, ui: &mut Ui, win: i64, theme: Option<&Arc<Theme>>, fonts: &GridFonts, fg: Color32, scrolls: bool, chrome: &Chrome, icons: &mut IconTextures) {
        let Some(p) = self.win(win).cloned() else { return };
        if self.code_size != fonts.size {
            self.code_size = fonts.size;
            for doc in self.docs.values_mut() {
                doc.code.iter_mut().for_each(|c| *c = None);
            }
        }
        let Some(doc) = self.docs.get_mut(&p.buf) else { return };
        // Images by paths relative to the file (`![](assets/a.png)`).
        let base = image_base(&p.path);
        // Scroll to the cursor's block when its line changed.
        let follow = self.scrolled.get(&win) != Some(&p.line);
        self.scrolled.insert(win, p.line);
        let target = doc.blocks.iter().rposition(|b| b.lines.start <= p.line);
        // The window's scroll bar is the one (`paint_scrollbars`): its drags
        // come through `scroll_to`.
        let mut area = egui::ScrollArea::vertical()
            .id_salt(("eshvi-preview", win))
            .auto_shrink([false, false])
            .scroll_source(if scrolls { egui::scroll_area::ScrollSource::MOUSE_WHEEL } else { egui::scroll_area::ScrollSource::NONE })
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        // The wheel over it, or the scroll bar dragged: the user's scroll.
        let mut by_user = scrolls && ui.input(|i| i.smooth_scroll_delta.y != 0.0) && ui.rect_contains_pointer(ui.max_rect());
        if let Some(top) = self.goto.remove(&win) {
            area = area.vertical_scroll_offset(top);
            by_user = true;
        }
        let mut ends = Vec::with_capacity(doc.blocks.len());
        let viewer = || CommonMarkViewer::new().default_implicit_uri_scheme(base.clone());
        let shown = area.show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(MARGIN, 0)).show(ui, |ui| {
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                for (i, block) in doc.blocks.iter().enumerate() {
                    let rect: Rect = match &block.body {
                        Body::Code { lang, text } => {
                            let job = doc.code[i]
                                .get_or_insert_with(|| crate::markdown::highlight(text, lang, theme.map(|t| &**t), fonts, fg))
                                .clone();
                            let fill = ui.visuals().code_bg_color;
                            let rect = egui::Frame::new()
                                .fill(fill)
                                .corner_radius(crate::style::RADIUS)
                                .inner_margin(crate::style::POPUP_PAD)
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Wrap));
                                })
                                .response
                                .rect;
                            copy_button(ui, icons, chrome, (win, i), rect, fill, fonts.size, text);
                            rect
                        }
                        Body::Markdown(text) => {
                            let from = next_shape(ui);
                            let rect = ui.push_id(i, |ui| viewer().show(ui, &mut doc.cache, text)).response.rect;
                            restyle_copy_buttons(ui, icons, chrome, (win, i), from);
                            rect
                        }
                        Body::Table { align, rows } => {
                            ui.push_id(i, |ui| show_table(ui, viewer, &mut doc.cache, align, rows)).inner
                        }
                    };
                    ends.push(rect.max.y);
                    if follow && Some(i) == target {
                        ui.scroll_to_rect(rect.expand(12.0), Some(egui::Align::TOP));
                    }
                }
                ui.add_space(24.0);
            });
        });
        let top = shown.state.clamped_offset().y;
        // The first block in view, for the Outline to follow: the first that doesn't
        // end above the view's top.
        if by_user && !follow {
            let at = ends.iter().position(|y| *y > shown.inner_rect.min.y + 1.0).unwrap_or(ends.len().saturating_sub(1));
            self.scrolled_top = doc.blocks.get(at).map(|b| (p.buf, b.lines.start));
        }
        self.views.insert(win, (top as i64, (top + shown.inner_rect.height()) as i64, shown.content_size.y as i64));
    }
}

/// How long a code block's copy button shows its check after a click.
const COPIED_FOR: f64 = 1.0;

/// A code block's copy button, at its top right: the Codicons `copy` icon
/// (dim, lit under the pointer, with a fill as the other icon buttons), and
/// `check` for a second after a click. A press copies the block's text.
/// (egui_commonmark's own button, on code nested in a list or a quote, can't
/// be replaced: the blocks drawn here have this one.)
#[allow(clippy::too_many_arguments)]
fn copy_button(ui: &mut Ui, icons: &mut IconTextures, chrome: &Chrome, key: (i64, usize), block: Rect, surface: Color32, font: f32, text: &str) {
    let row = (font * 1.6).round();
    let side = style::button_size(row);
    let rect = Rect::from_center_size(Pos2::new(block.max.x - 6.0 - side / 2.0, block.min.y + 6.0 + side / 2.0), Vec2::splat(side));
    let id = ui.make_persistent_id(("eshvi-copy", key));
    let resp = ui.interact(rect, id, egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let now = ui.input(|i| i.time);
    if resp.clicked() {
        ui.ctx().copy_text(text.to_owned());
        ui.memory_mut(|m| m.data.insert_temp(id, now));
    }
    let copied = ui.memory(|m| m.data.get_temp::<f64>(id)).filter(|at| now - at < COPIED_FOR);
    if let Some(at) = copied {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((COPIED_FOR - (now - at)).max(0.0) + 0.02));
    }
    let hovered = resp.hovered();
    style::paint_button_fill(ui.painter(), rect, chrome, surface, hovered, false);
    let icon = if copied.is_some() { "check" } else { "copy" };
    icons.paint_codicon(ui.painter(), icon, rect.center(), style::icon_size(row), chrome.text(hovered || copied.is_some()));
}

/// Where the next shape painted in `ui`'s layer goes (see `restyle_copy_buttons`).
fn next_shape(ui: &Ui) -> usize {
    ui.ctx().graphics_mut(|g| g.entry(ui.layer_id()).next_idx().0)
}

/// egui_commonmark's code blocks (nested in a list or a quote, which the
/// blocks drawn here aren't) come with a copy button of its own, a "🗐" or "✔"
/// glyph that can't be turned off or replaced. Its text, painted since shape
/// `from`, is swapped for the Codicons icon in its place (the glyph's own
/// button still takes the click and copies): `copy`, `check` for a second after
/// a press, lit under the pointer.
fn restyle_copy_buttons(ui: &mut Ui, icons: &mut IconTextures, chrome: &Chrome, key: (i64, usize), from: usize) {
    let layer = ui.layer_id();
    let mut found = Vec::new();
    ui.ctx().graphics_mut(|g| {
        let list = g.entry(layer);
        for (idx, clipped) in list.all_entries().enumerate().skip(from) {
            if let egui::Shape::Text(t) = &clipped.shape {
                if matches!(t.galley.text(), "\u{1F5D0}" | "\u{2714}") {
                    found.push((idx, t.visual_bounding_rect()));
                }
            }
        }
        for &(idx, _) in &found {
            list.reset_shape(egui::layers::ShapeIdx(idx));
        }
    });
    let (now, pointer) = ui.input(|i| (i.time, i.pointer.hover_pos()));
    let pressed = ui.input(|i| i.pointer.primary_clicked());
    let surface = ui.visuals().extreme_bg_color;
    for (n, (_, glyph)) in found.into_iter().enumerate() {
        let id = ui.make_persistent_id(("eshvi-nested-copy", key, n));
        let rect = Rect::from_center_size(glyph.center(), Vec2::splat(glyph.width().max(glyph.height()) + 4.0));
        let hovered = pointer.is_some_and(|p| rect.contains(p));
        if hovered && pressed {
            ui.memory_mut(|m| m.data.insert_temp(id, now));
        }
        let copied = ui.memory(|m| m.data.get_temp::<f64>(id)).filter(|at| now - at < COPIED_FOR);
        if let Some(at) = copied {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((COPIED_FOR - (now - at)).max(0.0) + 0.02));
        }
        let side = rect.height().max(14.0);
        let rect = Rect::from_center_size(rect.center(), Vec2::splat(side));
        style::paint_button_fill(ui.painter(), rect, chrome, surface, hovered, false);
        let icon = if copied.is_some() { "check" } else { "copy" };
        icons.paint_codicon(ui.painter(), icon, rect.center(), (side * 0.75).round(), chrome.text(hovered || copied.is_some()));
    }
}

/// What a relative image path is joined to: the file's folder as a
/// `file://` URI (egui_commonmark puts it before paths without a scheme;
/// they load through egui_extras' file loader, scaled down to the width).
/// There's no http loader: web images aren't fetched.
fn image_base(file: &str) -> String {
    let dir = std::path::Path::new(file).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    format!("file://{}/", dir.trim_end_matches('/'))
}

/// Marks for `texts` (a search's matches) wherever the text drawn on `layer`
/// since its `from`th shape has them: a box in `bg` behind each, and its
/// text again on top in `fg` (the Search group's colors: the page's text
/// color may not read on its background). A match that wraps gets a box on
/// each row, its text left as it is.
pub fn search_marks(ctx: &egui::Context, layer: egui::LayerId, from: usize, texts: &[String], bg: Color32, fg: Option<Color32>) -> Vec<egui::Shape> {
    use egui::text::CCursor;
    let mut boxes = Vec::new();
    let mut words = Vec::new();
    ctx.graphics(|g| {
        let Some(list) = g.get(layer) else { return };
        for entry in list.all_entries().skip(from) {
            let egui::Shape::Text(t) = &entry.shape else { continue };
            let text = t.galley.text();
            for needle in texts.iter().filter(|n| !n.is_empty()) {
                for (at, _) in text.match_indices(needle.as_str()) {
                    let start = text[..at].chars().count();
                    let end = start + needle.chars().count();
                    let pos = |c: usize| t.galley.pos_from_cursor(CCursor::new(c));
                    // One box per row the match is on.
                    let mut rows: Vec<Rect> = Vec::new();
                    for c in start..end {
                        let (here, next) = (pos(c), pos(c + 1));
                        let right = if (next.min.y - here.min.y).abs() < 0.5 {
                            next.min.x
                        } else {
                            t.galley.rows.iter().find(|r| (r.pos.y - here.min.y).abs() < 0.5).map_or(here.max.x, |r| r.rect().max.x)
                        };
                        let cell = Rect::from_min_max(here.min, Pos2::new(right, here.max.y));
                        match rows.last_mut() {
                            Some(last) if (last.min.y - cell.min.y).abs() < 0.5 => *last = last.union(cell),
                            _ => rows.push(cell),
                        }
                    }
                    let clip = entry.clip_rect;
                    boxes.extend(rows.iter().map(|r| r.translate(t.pos.to_vec2())).filter(|r| clip.intersects(*r)));
                    // The text again, in the Search color, if on one row.
                    if let (Some(fg), [row]) = (fg, rows.as_slice()) {
                        let section = t.galley.job.sections.iter().find(|s| s.byte_range.start.0 <= at && at < s.byte_range.end.0);
                        if let Some(font) = section.map(|s| s.format.font_id.clone()) {
                            words.push((row.translate(t.pos.to_vec2()).min, needle.clone(), font, fg, clip));
                        }
                    }
                }
            }
        }
    });
    let mut shapes: Vec<egui::Shape> = boxes.into_iter().map(|r| egui::Shape::rect_filled(r.expand2(Vec2::new(1.0, 0.0)), style::RADIUS, bg)).collect();
    for (at, word, font, fg, clip) in words {
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap(word, font, fg));
        if clip.intersects(Rect::from_min_size(at, galley.size())) {
            shapes.push(egui::Shape::galley(at, galley, fg));
        }
    }
    shapes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_is_marked_where_the_drawn_text_has_it() {
        let ctx = egui::Context::default();
        let mut marks = Vec::new();
        let mut label = Rect::NOTHING;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let layer = ui.layer_id();
            let from = ui.ctx().graphics(|g| g.get(layer).map_or(0, |l| l.all_entries().len()));
            label = ui.label("say hello and hello again").rect;
            marks = search_marks(ui.ctx(), layer, from, &["hello".to_owned(), "absent".to_owned()], Color32::YELLOW, Some(Color32::BLACK));
        });
        out.textures_delta.clear();
        let boxes: Vec<Rect> = marks.iter().filter_map(|s| if let egui::Shape::Rect(r) = s { Some(r.rect) } else { None }).collect();
        let words: Vec<String> = marks.iter().filter_map(|s| if let egui::Shape::Text(t) = s { Some(t.galley.text().to_owned()) } else { None }).collect();
        assert_eq!(boxes.len(), 2, "{marks:?}");
        assert_eq!(words, ["hello", "hello"]);
        assert!(boxes[0].max.x < boxes[1].min.x, "{boxes:?}");
        for b in &boxes {
            assert!(label.expand(1.5).contains_rect(*b), "{b:?} in {label:?}");
            assert!(b.width() > 10.0, "{b:?}");
        }
    }

    /// A code block's copy button: a press copies the block, and the icon is
    /// a check for a second.
    #[test]
    fn the_copy_button_copies_the_block() {
        let md = "Intro\n\n```rust\nfn main() {}\nlet x = 1;\n```\n";
        let mut previews = Previews::default();
        previews.set_wins(vec![PreviewWin { win: 1, buf: 7, line: 1, side: false, path: "/x/n.md".into(), search: None }]);
        previews.set_text(7, md);
        let chrome = Chrome::new(Color32::from_rgb(0x28, 0x2c, 0x34), Color32::from_rgb(0xab, 0xb2, 0xbf), Color32::from_rgb(0x4f, 0xa6, 0xed));
        let mut icons = IconTextures::default();
        let mut fonts: Option<GridFonts> = None;
        let mut h = egui_kittest::Harness::builder().with_size(Vec2::new(400.0, 300.0)).with_accessibility_check(false).build_ui_state(
            move |ui, p: &mut Previews| {
                // (Fonts are live a pass after they are installed.)
                let f = fonts.get_or_insert_with(|| crate::fonts::install(ui.ctx(), &crate::fonts::FontSpec::default_with_size(13.0), None, None, None, None));
                if f.is_live(ui.ctx()) {
                    p.show(ui, 1, None, f, Color32::WHITE, true, &chrome, &mut icons);
                }
            },
            previews,
        );
        h.run();
        let copied = |h: &egui_kittest::Harness<'_, Previews>| {
            h.output().platform_output.commands.iter().find_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
        };
        assert_eq!(copied(&h), None);
        // Pressed in the code block's top right: the frame's 6 and the button's 16 (at 13 pt), 6 in.
        let x = 400.0 - f32::from(MARGIN) - 6.0 - 8.0;
        let mut text = None;
        for y in (30..90).step_by(2) {
            let at = Pos2::new(x, y as f32);
            h.hover_at(at);
            h.run();
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
                h.step();
                text = text.or_else(|| copied(&h));
            }
            if text.is_some() {
                break;
            }
        }
        assert_eq!(text.as_deref(), Some("fn main() {}\nlet x = 1;"), "no button where the block's top right is");
    }

    #[test]
    fn images_are_relative_to_the_file() {
        assert_eq!(image_base("/Users/a/proj/README.md"), "file:///Users/a/proj/");
        assert_eq!(image_base("/README.md"), "file:///");
    }

    #[test]
    fn a_table_is_a_block_of_cells() {
        let md = "Intro\n\n| Name | `a\\|b` | Right |\n|---|:-:|--:|\n| - not a list | **bold** [d] | 1. |\n| x |\n\n[d]: https://example.com\n";
        let b = blocks(md);
        assert_eq!(b[1].lines, 3..7);
        let Body::Table { align, rows } = &b[1].body else { panic!("{:?}", b[1].body) };
        assert_eq!(align, &[Alignment::None, Alignment::Center, Alignment::Right]);
        let refs = "\n[d]: https://example.com";
        let cell = |r: usize, c: usize| rows[r][c].strip_suffix(refs).unwrap_or_else(|| panic!("{}", rows[r][c]));
        assert_eq!((cell(0, 0), cell(0, 1), cell(0, 2)), ("Name", "`a|b`", "Right"));
        // Text in a cell, though it would start a block on its own.
        assert_eq!((cell(1, 0), cell(1, 1), cell(1, 2)), ("\\- not a list", "**bold** [d]", "1\\."));
        assert_eq!(cell(2, 0), "x");
        for cell in rows.iter().flatten().filter(|c| !c.starts_with(refs)) {
            assert!(matches!(Parser::new_ext(cell, Options::all()).next(), Some(Event::Start(Tag::Paragraph))), "{cell}");
        }
    }

    #[test]
    fn columns_share_the_room() {
        assert_eq!(column_widths(&[50.0, 100.0], &[20.0, 30.0], 300.0), [50.0, 100.0], "as wide as their text");
        // 100 over: each keeps its longest word, the rest goes by what's wanted.
        let w = column_widths(&[50.0, 150.0], &[50.0, 30.0], 100.0);
        assert_eq!(w, [50.0, 50.0]);
        // A word too long for its share doesn't squeeze the other columns.
        assert_eq!(column_widths(&[60.0, 900.0, 40.0], &[60.0, 800.0, 40.0], 300.0), [60.0, 200.0, 40.0]);
    }

    /// A table narrower than its text: nothing to scroll sideways, the
    /// cells wrap and the row grows.
    #[test]
    fn a_table_wraps_in_its_room() {
        use crate::style::tests::themes;
        let md = "| Option | Meaning | Default |\n|---|---|--:|\n| `scrollbar` | Whether a window has a scroll bar: shown while it scrolls and on approach, always, or never | auto |\n| **tabs** | The tab pages' look, see https://example.com/a/very/long/link/that/has/no/spaces/at/all/and/must/break and [the notes](NOTES.md) | wide |\n";
        let Body::Table { align, rows } = blocks(md).remove(0).body else { panic!() };
        let mut results = egui_kittest::SnapshotResults::new();
        for (name, chrome, theme) in themes() {
            let (mut cache, placed) = (CommonMarkCache::default(), std::cell::Cell::new(Rect::NOTHING));
            let mut h = egui_kittest::Harness::builder()
                .with_size(Vec2::new(360.0, 260.0))
                .with_pixels_per_point(2.0)
                .with_accessibility_check(false)
                .wgpu()
                .build_ui(|ui| {
                    ui.ctx().set_theme(if chrome.bg.r() < 128 { egui::Theme::Dark } else { egui::Theme::Light });
                    egui::Frame::new().fill(theme.bg).show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        crate::lsp_popups::LspPopups::style_popup_ui(ui, &theme);
                        placed.set(show_table(ui, CommonMarkViewer::new, &mut cache, &align, &rows));
                    });
                });
            h.run();
            h.snapshot(format!("preview_table_{name}"));
            results.extend_harness(&mut h);
            drop(h);
            let r = placed.get();
            assert!(r.width() <= 360.0, "{r:?}");
            assert!(r.height() > 100.0, "the long cell wraps: {r:?}");
        }
        results.unwrap();
    }

    #[test]
    fn splits_into_blocks_with_their_lines() {
        let md = "# Title\n\nSome *text*\nover two lines.\n\n```rust\nfn main() {}\n```\n\n- a\n- b\n\n---\n\nSee [docs][d].\n\n[d]: https://example.com\n";
        let b = blocks(md);
        let lines: Vec<_> = b.iter().map(|b| b.lines.clone()).collect();
        assert_eq!(lines, [1..2, 3..5, 6..9, 10..12, 13..14, 15..16]);
        assert_eq!(b[2].body, Body::Code { lang: "rust".into(), text: "fn main() {}".into() });
        // The reference definition goes with every Markdown block.
        let Body::Markdown(see) = &b[5].body else { panic!() };
        assert!(see.starts_with("See [docs][d].") && see.contains("[d]: https://example.com"), "{see}");
        assert!(matches!(&b[0].body, Body::Markdown(t) if t.starts_with("# Title")));
    }

    #[test]
    fn follows_the_cursor_to_its_block() {
        let b = blocks("# A\n\ntext\n\n## B\n\nmore\n");
        let at = |line: usize| b.iter().rposition(|b| b.lines.start <= line);
        assert_eq!(at(1), Some(0));
        assert_eq!(at(4), Some(1));
        assert_eq!(at(5), Some(2));
        assert_eq!(at(99), Some(3));
    }
}
