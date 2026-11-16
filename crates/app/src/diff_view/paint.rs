//! Drawing the diff view: a bar of controls on top, then the sections, each
//! with a header that sticks to the top while you're inside it. Only the
//! rows in view are laid out and drawn. Colors are the colorscheme's
//! (`DiffAdd` and the others, when it has them) and the syntax theme's.

use std::ops::Range;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Id, Painter, Pos2, Rect, Sense, Stroke, Vec2};
use grid::Editor;

use super::model::{self, Kind, Row, SplitRow};
use super::source::{Span, Status};
use super::{Action, Cursor, DiffView, Do, Geometry, Layout, Rows};
use crate::decorations::DecoColors;
use crate::popup::mix;
use crate::renderer::rgb;
use crate::style::{self, Chrome, Glyph, Icon};

/// Tab stops of the code shown.
const TAB: usize = 4;

/// The colors of the lines, from the colorscheme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiffColors {
    pub added: Color32,
    pub deleted: Color32,
    pub add_line: Color32,
    pub del_line: Color32,
    /// The changed words of a line: brighter than the line.
    pub add_word: Color32,
    pub del_word: Color32,
    /// The "⋯ N unchanged lines" rows.
    pub fold: Color32,
    pub modified: Color32,
    /// A match of the find field, and the one stepped to.
    pub found: Color32,
    pub found_now: Color32,
}

impl DiffColors {
    /// From the colorscheme: `DiffAdd` and the others' backgrounds when it has
    /// them, else the git colors tinted into the editor's background.
    pub fn from_editor(editor: &Editor) -> Self {
        let chrome = Chrome::from_editor(editor);
        let deco = DecoColors::from_editor(editor);
        let theme = |role: &str| editor.diff_theme.get(role).copied().map(rgb);
        Self::new(&chrome, &deco, [theme("add"), theme("delete"), theme("change"), theme("text")])
    }

    /// `scheme`: the colorscheme's `DiffAdd`, `DiffDelete`, `DiffChange` and
    /// `DiffText` backgrounds, where it has them.
    pub fn new(chrome: &Chrome, deco: &DecoColors, scheme: [Option<Color32>; 4]) -> Self {
        let [add, delete, change, text] = scheme;
        let add_line = add.unwrap_or_else(|| mix(chrome.bg, deco.added, style::ON));
        let del_line = delete.unwrap_or_else(|| mix(chrome.bg, deco.deleted, style::ON));
        // `DiffText` marks changed text in a changed line: the brighter words.
        let word = |line: Color32, signal: Color32| match text {
            Some(t) => mix(line, t, style::HALF),
            None => mix(line, signal, style::TINT),
        };
        Self {
            added: deco.added,
            deleted: deco.deleted,
            add_line,
            del_line,
            add_word: word(add_line, deco.added),
            del_word: word(del_line, deco.deleted),
            fold: change.unwrap_or_else(|| mix(chrome.bg, deco.modified, style::SUBTLE)),
            modified: deco.modified,
            found: mix(chrome.bg, deco.warning, style::TINT),
            found_now: mix(chrome.bg, chrome.accent, style::HALF),
        }
    }
}

/// What the view is drawn with.
pub struct Look<'a> {
    pub chrome: Chrome,
    pub colors: DiffColors,
    /// The font of the bar and the headers.
    pub ui: FontId,
    /// The code font for (bold, italic), and whether italics are faked.
    pub code: &'a dyn Fn(bool, bool) -> (FontId, bool),
    /// A cell's size.
    pub row_h: f32,
    pub char_w: f32,
}

/// What was under a press.
#[derive(Clone, Copy, Debug)]
enum Hit {
    Header(usize),
    Fold(usize, usize),
    Neovim(usize),
}

/// A button drawn this frame: on a change's first row, or in a header.
#[derive(Clone, Copy)]
struct Btn {
    rect: Rect,
    what: Do,
    section: usize,
    /// The change's first row; none: the file's header.
    row: Option<usize>,
}

/// The drawn rects that take presses this frame, and the buttons.
#[derive(Default)]
struct Hits(Vec<(Rect, Hit)>, Vec<Btn>);

impl Hits {
    fn at(&self, pos: Pos2) -> Option<Hit> {
        self.0.iter().rev().find(|(r, _)| r.contains(pos)).map(|(_, h)| *h)
    }
}

fn digits(n: usize) -> usize {
    n.max(1).to_string().len()
}

impl DiffView {
    /// Draw the view in `rect` and take the pointer. `focused`: the view has the keyboard.
    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, look: &Look, focused: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, look.chrome.bg);
        let header_h = (look.ui.size * 2.0).round();
        self.geom = Geometry { row_h: look.row_h, header_h, gap: (look.row_h * 0.6).round(), char_w: look.char_w };
        let bar_h = (look.ui.size * 2.4).round();
        let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), bar_h));
        // The find field's strip under the bar, while it is open.
        let find_h = if self.find.is_some() { bar_h } else { 0.0 };
        let find_bar = Rect::from_min_size(bar.left_bottom(), Vec2::new(rect.width(), find_h));
        let body = Rect::from_min_max(find_bar.left_bottom(), rect.max);
        self.viewport_h = body.height();
        self.focused = focused;
        self.ptr = ui.input(|i| i.pointer.hover_pos());
        self.prepare();
        // Enter, ⇧Enter and Esc in the field, before the field takes them.
        self.find_keys(ui.ctx());

        // The pointer over the rows scrolls them (and, sideways, the code).
        let resp = ui.interact(body, Id::unique("eshvi-diff-body"), Sense::click_and_drag());
        if resp.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            self.scroll_by(-d.y);
            self.scroll_x = (self.scroll_x - d.x).max(0.0);
        }
        if self.pin.is_none() {
            self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        }
        // The row the pointer is on (its change shows its buttons: also while the
        // pointer is on one of them, which are over the rows and take the hover).
        let over = self.ptr.filter(|p| body.contains(*p)).and_then(|p| self.locate(p.y - body.min.y + self.scroll));
        let tops = self.tops();
        let mut hits = Hits::default();
        let mut longest = 0usize;
        for (i, section_top) in tops.iter().enumerate() {
            let top = body.min.y + section_top - self.scroll;
            let height = self.section_height(i);
            if top > body.max.y || top + height < body.min.y {
                continue;
            }
            let body_clip = body.intersect(p.clip_rect());
            self.paint_rows(&p.with_clip_rect(body_clip), look, i, top, body, over.filter(|c| c.section == i).and_then(|c| c.row), &mut hits, &mut longest);
            // The header sticks to the top while the section is in view, and
            // is pushed up by the end of it.
            let y = top.max(body.min.y).min(top + height - header_h);
            let header = Rect::from_min_size(Pos2::new(body.min.x, y), Vec2::new(body.width(), header_h));
            self.paint_header(&p.with_clip_rect(body_clip), look, i, header, focused, over.is_some_and(|c| c.section == i && c.row.is_none()), &mut hits);
            hits.0.push((header, Hit::Header(i)));
        }
        if self.sections.is_empty() {
            let text = if self.awaiting_files() { "Reading the files…" } else { "No changes" };
            p.text(body.center_top() + Vec2::new(0.0, look.row_h * 3.0), Align2::CENTER_TOP, text, look.ui.clone(), look.chrome.text(false));
        }
        // (A little past the longest line, so it can be scrolled into view whole.)
        let widest = (longest as f32 + 4.0) * look.char_w;
        self.scroll_x = self.scroll_x.min((widest - body.width() / 2.0).max(0.0));
        self.pointer(ui, &resp, body, &hits, &mut actions);
        // The buttons are over the rows: they take their own presses.
        for b in std::mem::take(&mut hits.1) {
            let id = Id::unique(("eshvi-diff-button", b.section, b.what));
            let r = ui.interact(b.rect, id, Sense::click()).on_hover_text(b.what.tip());
            if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if r.clicked() {
                self.anchor = None;
                let at = Cursor { section: b.section, row: b.row };
                self.cursor = at;
                if let Some(a) = self.do_action(b.what, at) {
                    actions.push(a);
                }
            }
        }
        self.paint_bar(ui, &p, look, bar, &mut actions);
        if find_h > 0.0 {
            self.paint_find(ui, &p, look, find_bar);
        }
        // (Scrolling animates: a frame more while egui has kinetic scroll left.)
        actions
    }

    /// Enter, ⇧Enter and Esc while the find field has the keyboard: taken
    /// before the field is added (it would let go of the keyboard itself).
    fn find_keys(&mut self, ctx: &egui::Context) {
        if !self.find.as_ref().is_some_and(|f| f.focused) {
            return;
        }
        let (enter, shift_enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if esc {
            self.close_find();
            ctx.memory_mut(|m| m.surrender_focus(super::find::field_id()));
        } else if enter || shift_enter {
            self.find_step(enter);
        }
    }

    /// The find strip: the field, ↑ ↓ ×, and how it went.
    fn paint_find(&mut self, ui: &mut egui::Ui, p: &Painter, look: &Look, strip: Rect) {
        let c = &look.chrome;
        p.rect_filled(strip, 0.0, c.panel);
        p.hline(strip.x_range(), strip.max.y - style::HAIRLINE / 2.0, Stroke::new(style::HAIRLINE, c.line()));
        let id = super::find::field_id();
        let button = strip.height();
        let width = (strip.width() - style::TAB_PAD * 2.0 - button * 3.0).clamp(120.0, 340.0);
        let field = Rect::from_min_size(Pos2::new(strip.min.x + style::TAB_PAD, strip.center().y - button * 0.4), Vec2::new(width, button * 0.8));
        let none = self.find.as_ref().is_some_and(|f| f.none);
        let mut changed = false;
        if let Some(find) = self.find.as_mut() {
            let resp = ui
                .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                    // (The text sits in the middle of the field's height.)
                    let pad = ((field.height() - look.ui.size * 1.5) / 2.0).max(0.0) as i8;
                    let margin = egui::Margin::symmetric(style::GAP as i8, pad);
                    style::text_field(ui, c, id, margin, |ui| {
                        let edit = egui::TextEdit::singleline(&mut find.text)
                            .id(id)
                            .font(look.ui.clone())
                            .text_color(if none { look.colors.deleted } else { c.fg })
                            .hint_text(egui::RichText::new("Find").font(look.ui.clone()).color(c.dim(c.fg)))
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
        }
        if changed {
            self.find_changed();
        }
        let icon = style::icon_size(button);
        let mut x = field.max.x + style::GAP;
        for (kind, forward, tip) in [(Icon::ChevronUp, false, "Previous match (⇧Enter, ⇧⌘G)"), (Icon::ChevronDown, true, "Next match (Enter, ⌘G)")] {
            let r = Rect::from_min_size(Pos2::new(x, strip.center().y - button * 0.4), Vec2::splat(button * 0.8));
            x = r.max.x + style::GAP / 2.0;
            let resp = ui.interact(r, id.with(tip), Sense::click()).on_hover_text(tip);
            style::paint_button_fill(p, r, c, c.panel, resp.hovered(), false);
            style::paint_icon(p, kind, r.center(), icon, c.text(true));
            if resp.clicked() {
                self.find_step(forward);
            }
        }
        let r = Rect::from_min_size(Pos2::new(x, strip.center().y - button * 0.4), Vec2::splat(button * 0.8));
        x = r.max.x + style::GAP;
        let resp = ui.interact(r, id.with("close"), Sense::click()).on_hover_text("Close (Esc)");
        style::paint_button_fill(p, r, c, c.panel, resp.hovered(), false);
        style::paint_glyph(p, Glyph::Cross, r.center(), button, c.text(true));
        if resp.clicked() {
            self.close_find();
            ui.ctx().memory_mut(|m| m.surrender_focus(id));
            return;
        }
        // How it went: matches in the files read so far, or none at all.
        let Some(f) = self.find.as_ref().filter(|f| !f.text.is_empty()) else { return };
        let (note, color) = if f.none {
            ("No results".to_owned(), look.colors.deleted)
        } else {
            let (loaded, files) = self.loaded();
            let more = if loaded < files { "+" } else { "" };
            (format!("{}{more} matches", self.find_count()), c.text(false))
        };
        let g = p.layout_no_wrap(note, look.ui.clone(), color);
        p.galley(Pos2::new(x, strip.center().y - g.size().y / 2.0), g, color);
    }

    /// Presses on the rows: the cursor and selection, headers, folds.
    fn pointer(&mut self, ui: &egui::Ui, resp: &egui::Response, body: Rect, hits: &Hits, actions: &mut Vec<Action>) {
        let (pressed, down, pos, shift) =
            ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.modifiers.shift));
        let Some(pos) = pos else { return };
        if pressed && resp.hovered() {
            self.anchor = self.anchor.filter(|_| shift);
            match hits.at(pos) {
                Some(Hit::Header(i)) => {
                    self.cursor = Cursor { section: i, row: None };
                    self.toggle_fold(i);
                }
                Some(Hit::Fold(i, row)) => {
                    self.cursor = Cursor { section: i, row: Some(row) };
                    if let Some(a) = self.activate(self.cursor) {
                        actions.push(a);
                    }
                }
                Some(Hit::Neovim(i)) => actions.push(Action::Neovim(self.sections[i].spec.clone())),
                None => {
                    if let Some(c) = self.locate(pos.y - body.min.y + self.scroll).filter(|c| c.row.is_some()) {
                        if shift {
                            self.anchor.get_or_insert(self.cursor);
                        } else {
                            self.anchor = Some(c);
                        }
                        self.cursor = c;
                        self.dragging = true;
                        // A press in the line number gutter opens the file there.
                        if pos.x - body.min.x < self.gutter_width(c.section) {
                            if let Some(a) = self.open_at(c) {
                                actions.push(a);
                                self.dragging = false;
                            }
                        }
                    }
                }
            }
        } else if self.dragging && down {
            // Dragging up or down off the rows scrolls them.
            let past = if pos.y < body.min.y { pos.y - body.min.y } else if pos.y > body.max.y { pos.y - body.max.y } else { 0.0 };
            if past != 0.0 {
                self.scroll_by(past.signum() * self.geom.row_h * 0.5);
                ui.ctx().request_repaint();
            }
            let y = pos.y.clamp(body.min.y, body.max.y - 1.0) - body.min.y + self.scroll;
            if let Some(c) = self.locate(y).filter(|c| c.row.is_some()) {
                self.cursor = c;
            }
        }
        if !down {
            self.dragging = false;
            if self.anchor == Some(self.cursor) {
                self.anchor = None;
            }
        }
    }

    /// How far right of the view's left edge the line numbers reach.
    fn gutter_width(&self, i: usize) -> f32 {
        let (gw, _) = self.gutter(i);
        match self.layout {
            Layout::Unified => gw * 2.0,
            Layout::Split => gw,
        }
    }

    /// The width of a line number column in section `i`, and of the sign.
    fn gutter(&self, i: usize) -> (f32, f32) {
        let n = self.sections[i].diff().map_or(2, |d| digits(d.old.len().max(d.new.len())).max(2));
        let w = self.geom.char_w;
        ((n as f32 + 2.0) * w, w * 2.0)
    }

    // The bar.

    fn paint_bar(&mut self, ui: &egui::Ui, p: &Painter, look: &Look, bar: Rect, actions: &mut Vec<Action>) {
        let c = &look.chrome;
        p.rect_filled(bar, 0.0, c.panel);
        p.hline(bar.x_range(), bar.max.y - style::HAIRLINE / 2.0, Stroke::new(style::HAIRLINE, c.line()));
        let button = bar.height();
        let icon = style::icon_size(button);
        let right = std::cell::Cell::new(bar.max.x - style::GAP);
        let button_at = |ui: &egui::Ui, name: &str, tip: &str, on: bool| -> (Rect, bool) {
            let r = Rect::from_center_size(Pos2::new(right.get() - button / 2.0, bar.center().y), Vec2::splat(button * 0.8));
            let resp = ui.interact(r, Id::unique(("eshvi-diff-bar", name)), Sense::click()).on_hover_text(tip);
            style::paint_button_fill(p, r, c, c.panel, resp.hovered(), on);
            right.set(r.min.x - style::GAP / 2.0);
            (r, resp.clicked())
        };
        let (r, clicked) = button_at(ui, "close", "Close (Esc)", false);
        style::paint_glyph(p, Glyph::Cross, r.center(), button, c.text(true));
        if clicked {
            actions.push(Action::Close);
        }
        right.set(right.get() - style::GAP);
        for (layout, icon_kind, tip) in [(Layout::Split, Icon::Split, "Side by side (s)"), (Layout::Unified, Icon::Unified, "Unified (s)")] {
            let on = self.layout == layout;
            let (r, clicked) = button_at(ui, tip, tip, on);
            style::paint_icon(p, icon_kind, r.center(), icon, c.text(on));
            if clicked && !on {
                self.toggle_layout();
            }
        }
        right.set(right.get() - style::GAP);
        let (r, next) = button_at(ui, "next", "Next change (]c)", false);
        style::paint_icon(p, Icon::ChevronDown, r.center(), icon, c.text(true));
        let (r, prev) = button_at(ui, "prev", "Previous change ([c)", false);
        style::paint_icon(p, Icon::ChevronUp, r.center(), icon, c.text(true));
        if next {
            self.goto_change(true);
        }
        if prev {
            self.goto_change(false);
        }

        // On the left: what is compared, the files, how much changed; what
        // doesn't fit before the buttons is left out.
        let limit = right.get() - style::GAP;
        let y = bar.center().y;
        let mut x = bar.min.x + style::TAB_PAD;
        // (The list's arrow keeps its room.)
        let arrow = icon + style::GAP * 2.0;
        let mut job = LayoutJob::single_section(self.compared.clone(), TextFormat::simple(look.ui.clone(), c.fg));
        job.wrap = egui::text::TextWrapping::truncate_at_width((limit - x - arrow - style::TAB_PAD).max(0.0));
        let g = p.layout_job(job);
        p.galley(Pos2::new(x, y - g.size().y / 2.0), g.clone(), c.fg);
        x += g.size().x + style::TAB_PAD;
        let (loaded, files) = self.loaded();
        let label = if files == 1 { "1 file".to_owned() } else { format!("{files} files") };
        let g = p.layout_no_wrap(label, look.ui.clone(), c.text(true));
        // The button shrinks to its arrow when the room is short.
        let full = g.size().x + icon + style::GAP * 3.0;
        let width = if x + full <= limit { full } else { arrow };
        let list = Rect::from_min_size(Pos2::new(x, bar.center().y - button * 0.4), Vec2::new(width, button * 0.8));
        let resp = ui.interact(list, Id::unique("eshvi-diff-files"), Sense::click()).on_hover_text("Go to a file");
        style::paint_button_fill(p, list, c, c.panel, resp.hovered(), false);
        if width == full {
            p.galley(Pos2::new(list.min.x + style::GAP, y - g.size().y / 2.0), g, c.fg);
        }
        style::paint_icon(p, Icon::ChevronDown, Pos2::new(list.max.x - style::GAP - icon / 2.0, y), icon, c.text(true));
        x = list.max.x + style::TAB_PAD;
        let (added, removed) = self.totals();
        let totals = [(format!("+{added}"), look.colors.added), (format!("−{removed}"), look.colors.deleted)];
        let more = if loaded < files { Some(("…".to_owned(), c.text(false))) } else { None };
        let widths: f32 = totals.iter().chain(more.iter()).map(|(t, _)| p.layout_no_wrap(t.clone(), look.ui.clone(), c.fg).size().x + style::GAP).sum();
        if x + widths <= limit {
            for (text, color) in totals.into_iter().chain(more) {
                let g = p.layout_no_wrap(text, look.ui.clone(), color);
                p.galley(Pos2::new(x, y - g.size().y / 2.0), g.clone(), color);
                x += g.size().x + style::GAP;
            }
        }
        let mut go = None;
        egui::Popup::menu(&resp).show(|ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (i, s) in self.sections.iter().enumerate() {
                    let counts = s.diff().map(|d| format!("   +{} −{}", d.added, d.removed)).unwrap_or_default();
                    let text = format!("{}  {}{counts}", s.status().letter(), s.spec.title());
                    if ui.button(text).clicked() {
                        go = Some(i);
                        ui.close();
                    }
                }
            });
        });
        if let Some(i) = go {
            self.go_to_file(i);
        }
    }

    // Headers.

    #[allow(clippy::too_many_arguments)]
    fn paint_header(&self, p: &Painter, look: &Look, i: usize, r: Rect, focused: bool, hovered: bool, hits: &mut Hits) {
        let c = &look.chrome;
        let s = &self.sections[i];
        let on = self.cursor == Cursor { section: i, row: None };
        let fill = if on && focused { mix(c.panel, c.accent, style::SUBTLE) } else { c.panel };
        p.rect_filled(r, 0.0, fill);
        p.hline(r.x_range(), r.max.y - style::HAIRLINE / 2.0, Stroke::new(style::HAIRLINE, c.line()));
        let icon = style::icon_size(r.height());
        let y = r.center().y;
        let chevron = if s.folded { Icon::ChevronRight } else { Icon::ChevronDown };
        style::paint_icon(p, chevron, Pos2::new(r.min.x + style::TAB_PAD + icon / 2.0, y), icon, c.text(true));
        let mut x = r.min.x + style::TAB_PAD * 2.0 + icon;
        // The status letter, in the color of the change.
        let color = match s.status() {
            Status::Added => look.colors.added,
            Status::Deleted => look.colors.deleted,
            Status::Modified | Status::Renamed => look.colors.modified,
        };
        let badge = Rect::from_center_size(Pos2::new(x + icon / 2.0 + 1.0, y), Vec2::splat(icon + 2.0));
        p.rect_filled(badge, style::RADIUS, mix(c.panel, color, style::WASH));
        p.text(badge.center(), Align2::CENTER_CENTER, s.status().letter(), style::scaled(&look.ui, style::SMALL), color);
        x = badge.max.x + style::GAP * 2.0;
        // The path: the folder dimmed, the name bright.
        let right_text = match (&s.loaded, s.counts()) {
            (_, Some((a, rm))) if s.diff().is_some() => Some((format!("+{a}"), format!("−{rm}"))),
            _ => None,
        };
        let right_w = right_text.as_ref().map_or(0.0, |(a, rm)| {
            let w = |t: &str| p.layout_no_wrap(t.to_owned(), look.ui.clone(), c.fg).size().x;
            w(a) + w(rm) + style::GAP * 3.0
        });
        // The file's buttons at the right end, while the cursor or the pointer is in the file.
        let buttons_w = if on || hovered || self.cursor.section == i { self.paint_buttons(p, look, i, None, r, c.panel, hits) } else { 0.0 };
        let room = (r.max.x - style::TAB_PAD - right_w - buttons_w - x).max(0.0);
        let (dir, name) = match s.spec.path.rsplit_once('/') {
            Some((d, n)) => (format!("{d}/"), n.to_owned()),
            None => (String::new(), s.spec.path.clone()),
        };
        let mut job = LayoutJob::default();
        if let Some(old) = s.spec.old_path.as_ref().filter(|o| **o != s.spec.path) {
            job.append(&format!("{old} → "), 0.0, TextFormat::simple(look.ui.clone(), c.text(false)));
        }
        job.append(&dir, 0.0, TextFormat::simple(look.ui.clone(), c.text(false)));
        job.append(&name, 0.0, TextFormat::simple(look.ui.clone(), c.fg));
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let g = p.layout_job(job);
        let end = x + g.size().x;
        p.galley(Pos2::new(x, y - g.size().y / 2.0), g, c.fg);
        // The counts follow the name.
        if let Some((a, rm)) = right_text {
            let mut cx = end + style::GAP * 2.0;
            for (t, col) in [(a, look.colors.added), (rm, look.colors.deleted)] {
                let g = p.layout_no_wrap(t, look.ui.clone(), col);
                p.galley(Pos2::new(cx, y - g.size().y / 2.0), g.clone(), col);
                cx += g.size().x + style::GAP;
            }
        } else if s.loaded.is_none() {
            let g = p.layout_no_wrap("…".into(), look.ui.clone(), c.text(false));
            p.galley(Pos2::new(end + style::GAP * 2.0, y - g.size().y / 2.0), g, c.fg);
        }
    }

    /// The buttons of a change (`row`) or of a file's header (none), right
    /// aligned in `r`: the width they take. Their presses are taken by `show`.
    #[allow(clippy::too_many_arguments)]
    fn paint_buttons(&self, p: &Painter, look: &Look, i: usize, row: Option<usize>, r: Rect, surface: Color32, hits: &mut Hits) -> f32 {
        let c = &look.chrome;
        let what_for: &[Do] = if row.is_some() { self.mode.change_actions() } else { self.mode.file_actions() };
        if row.is_some() && !self.changes_editable(i) {
            return 0.0;
        }
        let small = style::scaled(&look.ui, style::SMALL);
        let mut right = r.max.x - style::TAB_PAD;
        let mut used = 0.0;
        // Drawn from the right end, so the first listed is the leftmost.
        for &what in what_for.iter().rev() {
            if row.is_none() && !self.file_action_offered(what, i) {
                continue;
            }
            let armed = self.is_armed(what, i, row);
            let label = if armed { what.confirm_label() } else { what.label() };
            let g = p.layout_no_wrap(label.to_owned(), small.clone(), c.fg);
            let rect = Rect::from_min_max(
                Pos2::new(right - g.size().x - style::TAB_PAD * 1.5, r.min.y + style::GAP / 2.0),
                Pos2::new(right, r.max.y - style::GAP / 2.0),
            );
            let hovered = self.ptr.is_some_and(|q| rect.contains(q));
            let (fill, text) = if armed {
                (style::primary_fill(look.colors.deleted, hovered), Color32::WHITE)
            } else {
                (style::text_button_fill(c, surface, hovered), c.fg)
            };
            p.rect_filled(rect, style::RADIUS, surface);
            p.rect_filled(rect, style::RADIUS, fill);
            p.rect_stroke(rect, style::RADIUS, Stroke::new(style::HAIRLINE, c.line()), egui::StrokeKind::Inside);
            p.galley(rect.center() - g.size() / 2.0, g.clone(), text);
            hits.1.push(Btn { rect, what, section: i, row });
            used += rect.width() + style::GAP;
            right = rect.min.x - style::GAP;
        }
        if used > 0.0 {
            used + style::TAB_PAD
        } else {
            0.0
        }
    }

    // Rows.

    #[allow(clippy::too_many_arguments)]
    fn paint_rows(&self, p: &Painter, look: &Look, i: usize, top: f32, body: Rect, pointer_row: Option<usize>, hits: &mut Hits, longest: &mut usize) {
        let s = &self.sections[i];
        if s.folded {
            return;
        }
        let row_h = self.geom.row_h;
        let first = top + self.geom.header_h;
        let x0 = body.min.x;
        let width = body.width();
        let c = &look.chrome;
        let message = |text: &str, button: Option<&str>, hits: &mut Hits| {
            let r = Rect::from_min_size(Pos2::new(x0, first), Vec2::new(width, row_h));
            let g = p.layout_no_wrap(text.to_owned(), look.ui.clone(), c.text(false));
            p.galley(Pos2::new(x0 + style::TAB_PAD * 2.0, r.center().y - g.size().y / 2.0), g.clone(), c.fg);
            if let Some(label) = button {
                // On the row below the message.
                let b = p.layout_no_wrap(label.to_owned(), style::scaled(&look.ui, style::SMALL), Color32::WHITE);
                let br = Rect::from_min_size(
                    Pos2::new(x0 + style::TAB_PAD * 2.0, r.max.y + 2.0),
                    Vec2::new(b.size().x + style::TAB_PAD * 2.0, row_h - 4.0),
                );
                p.rect_filled(br, style::RADIUS, style::primary_fill(c.accent, false));
                p.galley(br.center() - b.size() / 2.0, b, Color32::WHITE);
                hits.0.push((br, Hit::Neovim(i)));
            }
        };
        let Some(loaded) = &s.loaded else {
            return message("Reading…", None, hits);
        };
        let diff = match &loaded.body {
            model::Body::Binary => return message("Binary file not shown", None, hits),
            model::Body::TooLarge { lines } => {
                return message(&format!("Too large to show here ({lines} lines)"), Some("Open in Neovim's diff"), hits);
            }
            model::Body::Text(d) => d,
        };
        if diff.is_identical() {
            let text = if s.status() == Status::Renamed { "Renamed, no changes to the lines" } else { "No changes" };
            return message(text, None, hits);
        }
        let Some((.., rows)) = &s.rows else { return };
        let (r0, r1) = {
            let lo = (body.min.y - first).max(0.0);
            let hi = (body.max.y - first).max(0.0);
            ((lo / row_h) as usize, ((hi / row_h).ceil() as usize).min(rows.len()))
        };
        let (gw, sign_w) = self.gutter(i);
        let half = ((width - style::HAIRLINE) / 2.0).floor();
        // The changes the cursor or the pointer is on show their buttons on their first row.
        let hot = self.hot_runs(i, pointer_row);
        for r in r0..r1 {
            let y = first + r as f32 * row_h;
            let row_rect = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(width, row_h));
            let here = Cursor { section: i, row: Some(r) };
            let selected = self.in_selection(here);
            match rows {
                Rows::Unified(v) => {
                    match v[r] {
                        Row::Fold { op, hidden } => self.paint_fold(p, look, row_rect, hidden, self.cursor == here, i, op, hits),
                        Row::Line(l) => {
                            let (line_bg, word_bg) = kind_colors(look, l.kind);
                            let (text, spans, words) = match l.kind {
                                Kind::Removed => {
                                    let o = l.old.unwrap_or(0);
                                    (&diff.old[o], loaded.colors.as_ref().and_then(|c| c.old.get(o)), &diff.old_words[o])
                                }
                                _ => {
                                    let n = l.new.unwrap_or(0);
                                    (&diff.new[n], loaded.colors.as_ref().and_then(|c| c.new.get(n)), &diff.new_words[n])
                                }
                            };
                            if let Some(bg) = line_bg {
                                p.rect_filled(row_rect, 0.0, bg);
                            }
                            self.paint_highlight(p, look, row_rect, selected, self.cursor == here, line_bg);
                            let dim = c.text(false);
                            number(p, look, Rect::from_min_size(Pos2::new(x0, y), Vec2::new(gw, row_h)), l.old.map(|n| n + 1), dim);
                            number(p, look, Rect::from_min_size(Pos2::new(x0 + gw, y), Vec2::new(gw, row_h)), l.new.map(|n| n + 1), dim);
                            let sign_x = x0 + gw * 2.0;
                            sign(p, look, Rect::from_min_size(Pos2::new(sign_x, y), Vec2::new(sign_w, row_h)), l.kind);
                            let code = Rect::from_min_max(Pos2::new(sign_x + sign_w, y), Pos2::new(row_rect.max.x, y + row_h));
                            *longest = (*longest).max(text.chars().count());
                            self.paint_code(p, look, code, text, spans, words, word_bg, self.marks(i, r, 0, text));
                        }
                    }
                }
                Rows::Split(v) => match v[r] {
                    SplitRow::Fold { op, hidden } => self.paint_fold(p, look, row_rect, hidden, self.cursor == here, i, op, hits),
                    SplitRow::Pair { left, right, .. } => {
                        let sides = [(left, Rect::from_min_size(Pos2::new(x0, y), Vec2::new(half, row_h)), false), (right, Rect::from_min_size(Pos2::new(x0 + half + style::HAIRLINE, y), Vec2::new(width - half - style::HAIRLINE, row_h)), true)];
                        for (cell, rect, is_new) in sides {
                            let (kind, line) = cell.map_or((None, 0), |c| (Some(c.kind), c.line));
                            let line_bg = match kind {
                                Some(k) => kind_colors(look, k).0,
                                // Filler where this side has no line.
                                None => Some(c.shade(c.bg, style::SUBTLE)),
                            };
                            if let Some(bg) = line_bg {
                                p.rect_filled(rect, 0.0, bg);
                            }
                            self.paint_highlight(p, look, rect, selected, self.cursor == here, line_bg);
                            let Some(kind) = kind else { continue };
                            let (text, spans, words, word_bg) = if is_new {
                                (&diff.new[line], loaded.colors.as_ref().and_then(|c| c.new.get(line)), &diff.new_words[line], kind_colors(look, kind).1)
                            } else {
                                (&diff.old[line], loaded.colors.as_ref().and_then(|c| c.old.get(line)), &diff.old_words[line], kind_colors(look, kind).1)
                            };
                            number(p, look, Rect::from_min_size(rect.min, Vec2::new(gw, row_h)), Some(line + 1), c.text(false));
                            let sign_x = rect.min.x + gw;
                            sign(p, look, Rect::from_min_size(Pos2::new(sign_x, y), Vec2::new(sign_w, row_h)), kind);
                            let code = Rect::from_min_max(Pos2::new(sign_x + sign_w, y), rect.max);
                            *longest = (*longest).max(text.chars().count());
                            self.paint_code(p, look, code, text, spans, words, word_bg, self.marks(i, r, u8::from(is_new), text));
                        }
                        p.vline(x0 + half + style::HAIRLINE / 2.0, row_rect.y_range(), Stroke::new(style::HAIRLINE, c.line()));
                    }
                },
            }
            if hot.iter().any(|run| run.start == r) {
                self.paint_buttons(p, look, i, Some(r), row_rect, look.chrome.panel, hits);
            }
        }
    }

    /// A row is in the selection the mouse or the keys made.
    fn in_selection(&self, c: Cursor) -> bool {
        let Some(a) = self.anchor else { return false };
        let (lo, hi) = if a <= self.cursor { (a, self.cursor) } else { (self.cursor, a) };
        c >= lo && c <= hi
    }

    /// The cursor's line and the selection, over the line's own color; the
    /// cursor has a bar at the edge while the view has the keyboard.
    fn paint_highlight(&self, p: &Painter, look: &Look, rect: Rect, selected: bool, cursor: bool, under: Option<Color32>) {
        let c = &look.chrome;
        let base = under.unwrap_or(c.bg);
        if selected {
            p.rect_filled(rect, 0.0, mix(base, c.accent, style::SELECTED));
        } else if cursor {
            p.rect_filled(rect, 0.0, mix(base, c.fg, style::HOVER));
        }
        if cursor && self.focused {
            p.rect_filled(Rect::from_min_size(rect.min, Vec2::new(style::BAR_RADIUS * 2.0, rect.height())), 0.0, c.accent);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_fold(&self, p: &Painter, look: &Look, r: Rect, hidden: usize, cursor: bool, section: usize, op: usize, hits: &mut Hits) {
        let c = &look.chrome;
        p.rect_filled(r, 0.0, look.colors.fold);
        if cursor {
            p.rect_filled(r, 0.0, mix(look.colors.fold, c.fg, style::SUBTLE));
        }
        let text = format!("⋯  {hidden} unchanged lines");
        let g = p.layout_no_wrap(text, look.ui.clone(), c.text(false));
        p.galley(Pos2::new(r.min.x + style::TAB_PAD * 2.0, r.center().y - g.size().y / 2.0), g, c.fg);
        let row = self.cursor_row_of(section, op);
        hits.0.push((r, Hit::Fold(section, row)));
    }

    /// The row index of the fold for `op`.
    fn cursor_row_of(&self, section: usize, op: usize) -> usize {
        let Some((.., rows)) = &self.sections[section].rows else { return 0 };
        match rows {
            Rows::Unified(v) => v.iter().position(|r| matches!(r, Row::Fold { op: o, .. } if *o == op)).unwrap_or(0),
            Rows::Split(v) => v.iter().position(|r| matches!(r, SplitRow::Fold { op: o, .. } if *o == op)).unwrap_or(0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_code(
        &self,
        p: &Painter,
        look: &Look,
        area: Rect,
        text: &str,
        spans: Option<&Vec<Span>>,
        words: &[Range<usize>],
        word_bg: Option<Color32>,
        (marks, current): (Vec<Range<usize>>, Option<usize>),
    ) {
        let p = p.with_clip_rect(area.intersect(p.clip_rect()));
        let spans = spans.map_or(&[][..], |s| s.as_slice());
        let ranges: Vec<Range<usize>> = spans.iter().map(|s| s.start..s.end).collect();
        let fg = look.chrome.fg;
        let mut job = LayoutJob::default();
        let mut col = 0;
        for seg in model::segments_marked(text, &ranges, words, &marks) {
            let piece = &text[seg.range.clone()];
            let shown = model::expand_tabs(piece, col, TAB);
            col += shown.chars().count();
            let span = seg.span.map(|k| spans[k]);
            let (font, fake_italic) = (look.code)(span.is_some_and(|s| s.bold), span.is_some_and(|s| s.italic));
            let mut format = TextFormat { font_id: font, color: span.map_or(fg, |s| s.color), italics: fake_italic, ..Default::default() };
            if let (true, Some(bg)) = (seg.word, word_bg) {
                format.background = bg;
            }
            if let Some(k) = seg.mark {
                format.background = if current == Some(k) { look.colors.found_now } else { look.colors.found };
            }
            job.append(&shown, 0.0, format);
        }
        if job.text.is_empty() {
            return;
        }
        let g = p.layout_job(job);
        let pos = Pos2::new(area.min.x - self.scroll_x, area.center().y - g.size().y / 2.0);
        p.galley(pos, g, fg);
    }
}

/// A kind's line and word colors (none for context).
fn kind_colors(look: &Look, kind: Kind) -> (Option<Color32>, Option<Color32>) {
    match kind {
        Kind::Context => (None, None),
        Kind::Added => (Some(look.colors.add_line), Some(look.colors.add_word)),
        Kind::Removed => (Some(look.colors.del_line), Some(look.colors.del_word)),
    }
}

/// A line number, right aligned in `r`.
fn number(p: &Painter, look: &Look, r: Rect, n: Option<usize>, color: Color32) {
    let Some(n) = n else { return };
    let (font, _) = (look.code)(false, false);
    p.text(Pos2::new(r.max.x - look.char_w, r.center().y), Align2::RIGHT_CENTER, n.to_string(), font, color);
}

/// The `+` or `-` before a line.
fn sign(p: &Painter, look: &Look, r: Rect, kind: Kind) {
    let (text, color) = match kind {
        Kind::Added => ("+", look.colors.added),
        Kind::Removed => ("-", look.colors.deleted),
        Kind::Context => return,
    };
    let (font, _) = (look.code)(false, false);
    p.text(r.center(), Align2::CENTER_CENTER, text, font, color);
}
