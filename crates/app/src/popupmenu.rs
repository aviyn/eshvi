//! Native completion menu for `ext_popupmenu`.

use egui::{Align2, Color32, FontFamily, FontId, Id, Order, Pos2, Rect, Sense, Vec2};
use grid::{Editor, PopupMenu};

use crate::cmdline::CmdlineLayout;
use crate::fonts::GridFonts;
use crate::popup::{place, PopupTheme};
use crate::renderer::CellMetrics;

/// Rows a list shows before it scrolls (the picker's too).
pub const MAX_VISIBLE: usize = 12;

/// A menu row's height (the picker's rows are as tall).
pub fn row_height(m: CellMetrics) -> f32 {
    m.height + 4.0
}

/// Short label and color for an LSP / Vim completion kind.
pub fn kind_badge(kind: &str) -> Option<(&'static str, Color32)> {
    let purple = Color32::from_rgb(0xc5, 0x86, 0xc0);
    let blue = Color32::from_rgb(0x4f, 0xa6, 0xed);
    let cyan = Color32::from_rgb(0x56, 0xb6, 0xc2);
    let orange = Color32::from_rgb(0xe5, 0xa0, 0x4c);
    let green = Color32::from_rgb(0x98, 0xc3, 0x79);
    let grey = Color32::from_rgb(0x9a, 0xa0, 0xa6);
    Some(match kind {
        "Function" | "f" => ("fn", purple),
        "Method" | "m" => ("m", purple),
        "Constructor" => ("new", purple),
        "Field" | "Property" => ("fd", blue),
        "Variable" | "v" => ("v", blue),
        "Constant" | "d" => ("c", blue),
        "Class" | "Struct" | "t" => ("S", orange),
        "Enum" => ("E", orange),
        "EnumMember" => ("e", orange),
        "Interface" => ("T", cyan),
        "TypeParameter" => ("<>", cyan),
        "Module" => ("mod", grey),
        "Keyword" => ("kw", blue),
        "Snippet" => ("sn", green),
        "Text" => ("ab", grey),
        "File" => ("fi", grey),
        "Folder" => ("dir", grey),
        "Operator" => ("op", grey),
        "Unit" | "Value" => ("val", blue),
        "Event" => ("ev", orange),
        "Color" => ("col", green),
        "Reference" => ("ref", grey),
        "" => return None,
        _ => ("?", grey),
    })
}

/// A kind's badge in `badge`: its label in its color on a wash of it, as the
/// Changes list's status letters (also the symbols picker's).
pub fn paint_badge(p: &egui::Painter, theme: &PopupTheme, badge: Rect, label: &str, color: Color32, selected: bool, small: &egui::FontId) {
    paint_badge_on(p, if selected { theme.sel_bg } else { theme.bg }, badge, label, color, small);
}

/// The same on whatever `fill` the row is (the Outline's rows are the side bar's).
pub fn paint_badge_on(p: &egui::Painter, fill: Color32, badge: Rect, label: &str, color: Color32, small: &egui::FontId) {
    p.rect_filled(badge, crate::style::RADIUS, crate::popup::mix(fill, color, crate::style::WASH));
    p.text(badge.center(), Align2::CENTER_CENTER, label, small.clone(), color);
}

#[derive(Default)]
pub struct PopupMenuView {
    last_selected: i64,
    last_len: usize,
    /// The selected item's documentation (`info`), rendered once.
    doc: Option<(String, crate::markdown::Rendered)>,
    /// How far it is scrolled, and how far it can be (points).
    doc_offset: f32,
    doc_max: f32,
    /// How tall its box's text is (a page, for Ctrl-F/B); 0 before shown.
    doc_page: f32,
    /// Documentation shows only on request (`gui_pum_doc`, native
    /// completion): the items' own `info` isn't shown.
    on_request: bool,
    /// What was asked for: the item's index and its Markdown.
    requested: Option<(i64, String)>,
    /// Where the editor's text ends (the native status bar's top), when the
    /// grid goes on under it: the menu and its documentation stay above.
    pub bottom: Option<f32>,
}

/// Widest and tallest the documentation beside the menu gets (its text).
const DOC_MAX: Vec2 = Vec2::new(440.0, 360.0);
/// Narrower than this (with its frame), the documentation goes under the
/// menu (over it, for a menu over the cursor) instead of beside it.
const DOC_MIN_WIDTH: f32 = 280.0;
/// Between the menu and its documentation.
const DOC_GAP: f32 = 4.0;
/// Less room than this under (over) the menu for its text, and the
/// documentation goes beside it after all.
const DOC_MIN_HEIGHT: f32 = 80.0;
/// The words' column and the menu column, at most (characters); the whole
/// menu, at most this share of the editor's width. (A language server's
/// labels and signatures run long: rust-analyzer's `max_by_key(…) (as
/// Iterator)` beside `fn(self, f: F) -> Option<…>` took half the editor, and
/// its documentation then had no room beside it.)
const WORD_MAX_CHARS: usize = 40;
const MENU_MAX_CHARS: usize = 24;
const MENU_MAX_SHARE: f32 = 0.45;

pub enum Action {
    Select(i64),
}

/// Where the documentation goes: its corner `pos` (which corner: `pivot`)
/// and the most room its text gets.
#[derive(Debug)]
struct DocPlace {
    pos: Pos2,
    pivot: Align2,
    text: Vec2,
}

/// The documentation's place by a `menu` in the editor's `area`: right of
/// it, else left, with the most room it can have up to `DOC_MAX`; with less
/// than `DOC_MIN_WIDTH` on both sides, under the menu (over it if the menu is
/// `above` the cursor), else on the wider side after all. Beside a menu over
/// the cursor it lines up with the menu's bottom and grows up, off the
/// cursor's line. `frame`: what its frame adds to its text's size.
fn doc_place(area: Rect, menu: Rect, frame: Vec2, above: bool) -> DocPlace {
    let widest = DOC_MAX.x + frame.x;
    let right = area.max.x - menu.max.x - DOC_GAP;
    let left = menu.min.x - area.min.x - DOC_GAP;
    let beside = |on_right: bool| {
        let (x, room) = if on_right { (menu.max.x + DOC_GAP, right) } else { (menu.min.x - DOC_GAP, left) };
        let (y, tall) = if above { (menu.max.y, menu.max.y - area.min.y) } else { (menu.min.y, area.max.y - menu.min.y) };
        let pivot = match (on_right, above) {
            (true, false) => Align2::LEFT_TOP,
            (false, false) => Align2::RIGHT_TOP,
            (true, true) => Align2::LEFT_BOTTOM,
            (false, true) => Align2::RIGHT_BOTTOM,
        };
        DocPlace { pos: Pos2::new(x, y), pivot, text: Vec2::new(room.min(widest) - frame.x, (tall - frame.y).min(DOC_MAX.y)) }
    };
    if right >= widest || (right >= left && right >= DOC_MIN_WIDTH) {
        return beside(true);
    }
    if left >= DOC_MIN_WIDTH {
        return beside(false);
    }
    let outer = widest.min(area.width());
    let x = menu.min.x.min(area.max.x - outer).max(area.min.x);
    let (pos, pivot, room) = if above {
        (Pos2::new(x, menu.min.y - DOC_GAP), Align2::LEFT_BOTTOM, menu.min.y - DOC_GAP - area.min.y)
    } else {
        (Pos2::new(x, menu.max.y + DOC_GAP), Align2::LEFT_TOP, area.max.y - menu.max.y - DOC_GAP)
    };
    // (Too little room there for a few lines: beside it, narrow.)
    if room - frame.y < DOC_MIN_HEIGHT {
        return beside(right >= left);
    }
    DocPlace { pos, pivot, text: Vec2::new(outer - frame.x, (room - frame.y).min(DOC_MAX.y)) }
}

impl PopupMenuView {
    /// Draw the menu, and the selected item's documentation beside it.
    /// Returns their screen rects and a click action, if any.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        editor: &Editor,
        pm: &PopupMenu,
        fonts: &GridFonts,
        m: CellMetrics,
        grid_rect: Rect,
        cmdline: Option<&CmdlineLayout>,
        syntax: Option<&syntect::highlighting::Theme>,
    ) -> (Vec<Rect>, Option<Action>) {
        let theme = PopupTheme::from_editor(editor);
        let (mono, _) = fonts.font_id(false, false);
        let small = crate::style::scaled(&FontId::new(fonts.ui_size, FontFamily::Proportional), crate::style::SMALL);

        // Top-left of the anchor cell, in points. Grid -1 means the external
        // cmdline: anchor under (above, at the bottom) the cmdline box at byte column `col`.
        let (anchor_cell, prefer_above) = match (pm.grid, cmdline) {
            (g, Some(c)) if g < 0 => {
                let p = c.pos_of_byte(pm.col.max(0) as usize);
                if c.at_bottom {
                    // Above the box; `place` goes by the anchor's top.
                    (Pos2::new(p.x, c.rect.min.y - 4.0), true)
                } else {
                    (Pos2::new(p.x, c.rect.max.y - m.height), false)
                }
            }
            (g, None) if g < 0 => {
                let rows = (grid_rect.height() / m.height).floor();
                (grid_rect.min + Vec2::new(pm.col as f32 * m.width, (rows - 1.0) * m.height), true)
            }
            (g, _) => {
                let (r0, c0) = editor.grid_origin(g as u64);
                (grid_rect.min + Vec2::new((c0 + pm.col) as f32 * m.width, (r0 + pm.row) as f32 * m.height), false)
            }
        };

        let has_kinds = pm.items.iter().any(|i| kind_badge(&i.kind).is_some());
        let badge_w = if has_kinds { 3.0 * m.width + 6.0 } else { 0.0 };
        let word_chars = pm.items.iter().map(|i| i.word.chars().count()).max().unwrap_or(1).min(WORD_MAX_CHARS);
        let menu_chars = pm.items.iter().map(|i| i.menu.chars().count()).max().unwrap_or(0).min(MENU_MAX_CHARS);
        let pad = 6.0;
        let row_h = row_height(m);
        let menu_w = if menu_chars > 0 { menu_chars as f32 * m.width * 0.85 + 2.0 * pad } else { 0.0 };
        let width = (pad + badge_w + word_chars as f32 * m.width + pad + menu_w)
            .min((grid_rect.width() * MENU_MAX_SHARE).max(240.0))
            .max(120.0);
        let frame_margin = 4.0 + 1.0;
        // (The rows under the status bar are hidden.)
        let room = Rect::from_min_max(grid_rect.min, Pos2::new(grid_rect.max.x, self.bottom.unwrap_or(f32::INFINITY).min(grid_rect.max.y)));
        // Fewer rows when the list fits neither under the cursor nor over it:
        // as many as the roomier side has.
        let most = (room.max.y - (anchor_cell.y + m.height)).max(anchor_cell.y - room.min.y);
        let fitting = ((most - 2.0 * frame_margin) / row_h).floor().max(1.0) as usize;
        let visible = pm.items.len().min(MAX_VISIBLE).min(fitting);
        let size = Vec2::new(width + 2.0 * frame_margin, visible as f32 * row_h + 2.0 * frame_margin);

        // Align the item text with the word being completed.
        let anchor = anchor_cell - Vec2::new(frame_margin + pad + badge_w, 0.0);
        let pos = place(room, anchor, m.height, size, prefer_above);

        let selection_changed = pm.selected != self.last_selected || pm.items.len() != self.last_len;
        self.last_selected = pm.selected;
        self.last_len = pm.items.len();

        let mut action = None;
        let area = egui::Area::new(Id::unique("eshvi-popupmenu"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .show(ctx, |ui| {
                theme.frame().show(ui, |ui| {
                    ui.set_width(width);
                    // (An Area's room is its size last frame: the scroll area
                    // wouldn't grow for a longer list.)
                    ui.set_max_height(visible as f32 * row_h);
                    crate::popup::thin_scrollbars(ui, &theme);
                    egui::ScrollArea::vertical()
                        .max_height(visible as f32 * row_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            for (i, item) in pm.items.iter().enumerate() {
                                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, row_h), Sense::CLICK);
                                let selected = i as i64 == pm.selected;
                                let p = ui.painter();
                                // (The one highlight, as the picker's: none for the mouse.)
                                if selected {
                                    p.rect_filled(rect, crate::style::RADIUS, theme.sel_bg);
                                }
                                let fg = if selected { theme.sel_fg } else { theme.fg };
                                let mut x = rect.min.x + pad;
                                let cy = rect.center().y;
                                if has_kinds {
                                    if let Some((label, color)) = kind_badge(&item.kind) {
                                        let badge = Rect::from_center_size(
                                            Pos2::new(x + badge_w / 2.0 - 3.0, cy),
                                            Vec2::new(badge_w - 6.0, m.height - 2.0),
                                        );
                                        paint_badge(p, &theme, badge, label, color, selected, &small);
                                    }
                                    x += badge_w;
                                }
                                // A word longer than its column is cut, keeping some of
                                // the menu column's room.
                                let room = rect.max.x - pad - x - if item.menu.is_empty() { 0.0 } else { menu_w.min((rect.max.x - x) * 0.35) };
                                let mono_width = |t: &str| p.layout_no_wrap(t.to_owned(), mono.clone(), fg).size().x;
                                let (shown, _) = crate::picker::truncate_to_width(&item.word, room, &mono_width);
                                let word = p.text(Pos2::new(x, cy), Align2::LEFT_CENTER, shown, mono.clone(), fg);
                                if !item.menu.is_empty() {
                                    let dim = if selected { theme.sel_fg.gamma_multiply(0.7) } else { theme.dim_fg };
                                    // Cut to the room after the word (the menu column is at
                                    // most 30 characters wide; a buffer name can be longer):
                                    // right-aligned whole, it ran left over the word and past
                                    // the menu's edge.
                                    let room = rect.max.x - pad - (word.max.x + pad);
                                    let width_of = |t: &str| p.layout_no_wrap(t.to_owned(), small.clone(), dim).size().x;
                                    let (menu, _) = crate::picker::truncate_to_width(&item.menu, room, &width_of);
                                    p.text(Pos2::new(rect.max.x - pad, cy), Align2::RIGHT_CENTER, menu, small.clone(), dim);
                                }
                                if selected && selection_changed {
                                    resp.scroll_to_me(None);
                                }
                                if resp.clicked() {
                                    action = Some(Action::Select(i as i64));
                                }
                            }
                        });
                });
            });
        let menu = area.response.rect;
        let mut rects = vec![menu];

        // The selected item's documentation: beside the menu, else under it;
        // within the editor either way.
        let info = self.info_for(pm).to_owned();
        if info.is_empty() {
            self.doc = None;
        } else {
            if self.doc.as_ref().is_none_or(|(text, _)| *text != info) {
                self.doc = Some((info.clone(), crate::markdown::Rendered::new(&info)));
                self.doc_offset = 0.0;
            }
            let frame = theme.frame().inner_margin(crate::style::POPUP_PAD);
            let place = doc_place(room, menu, frame.total_margin().sum(), anchor_cell.y > menu.min.y);
            let (pos, pivot, width, height) = (place.pos, place.pivot, place.text.x, place.text.y);
            self.doc_page = height;
            let (_, doc) = self.doc.as_mut().unwrap();
            let offset = &mut self.doc_offset;
            let max = &mut self.doc_max;
            let area = egui::Area::new(Id::unique("eshvi-popupmenu-doc"))
                .order(Order::Foreground)
                .fixed_pos(pos)
                .pivot(pivot)
                .constrain(false)
                .show(ctx, |ui| {
                    frame.show(ui, |ui| {
                        ui.set_max_width(width);
                        ui.set_max_height(height);
                        ui.style_mut().visuals.override_text_color = Some(theme.fg);
                        // The bar shows while the pointer is over it;
                        // Ctrl+F/B or the wheel scroll it.
                        crate::popup::thin_scrollbars(ui, &theme);
                        let out = egui::ScrollArea::vertical()
                            .max_height(height)
                            .vertical_scroll_offset(*offset)
                            .show(ui, |ui| doc.show(ui, syntax, fonts, theme.fg));
                        *offset = out.state.clamped_offset().y;
                        *max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                    });
                });
            rects.push(area.response.rect);
        }
        (rects, action)
    }

    /// A `gui_pum_doc` from Neovim: the documentation asked for item
    /// `selected` (empty: none). `selected` -1 only turns on-request mode on.
    pub fn set_requested_doc(&mut self, selected: i64, contents: String) {
        self.on_request = true;
        self.requested = (selected >= 0 && !contents.trim().is_empty()).then_some((selected, contents));
    }

    /// The documentation to show for the menu's selection: the item's `info`,
    /// or in on-request mode only what was asked for.
    fn info_for<'a>(&'a self, pm: &'a PopupMenu) -> &'a str {
        if self.on_request {
            return match &self.requested {
                Some((i, text)) if *i == pm.selected => text.trim(),
                _ => "",
            };
        }
        usize::try_from(pm.selected).ok().and_then(|i| pm.items.get(i)).map(|i| i.info.trim()).unwrap_or("")
    }

    /// Whether documentation is shown and taller than its popup.
    pub fn doc_scrolls(&self) -> bool {
        self.doc.is_some() && self.doc_max > 0.0
    }

    /// Scroll the documentation by `pages` (negative: up).
    pub fn scroll_doc(&mut self, pages: f32) {
        let page = if self.doc_page > 0.0 { self.doc_page } else { DOC_MAX.y };
        self.doc_offset = (self.doc_offset + pages * page * 0.8).clamp(0.0, self.doc_max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_list_growing_into_a_long_one_shows_all_its_rows() {
        let ctx = egui::Context::default();
        let fonts = GridFonts::uninstalled(14.0);
        let m = CellMetrics { width: 8.0, height: 20.0 };
        let editor = Editor::new();
        let grid_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let mut view = PopupMenuView::default();
        let menu = |n: usize| PopupMenu {
            items: (0..n)
                .map(|i| grid::PmItem { word: format!("item{i}"), kind: String::new(), menu: String::new(), info: String::new() })
                .collect(),
            selected: -1,
            row: 0,
            col: 0,
            grid: 1,
        };
        let height = |view: &mut PopupMenuView, n: usize| {
            let mut h = 0.0;
            for _ in 0..3 {
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    let (rects, _) = view.show(ui.ctx(), &editor, &menu(n), &fonts, m, grid_rect, None, None);
                    h = rects[0].height();
                });
                out.textures_delta.clear();
            }
            h
        };
        let short = height(&mut view, 3);
        let long = height(&mut view, 20);
        assert!(long > short * 3.0, "12 rows, not 3: {short} then {long}");
    }

    /// A long `menu` (cmp's buffer source names the buffer: a terminal's
    /// `term://…` name) is cut to its column, not drawn over the word and
    /// past the menu's left edge.
    #[test]
    fn a_long_menu_text_stays_in_its_column() {
        let ctx = egui::Context::default();
        let fonts = GridFonts::uninstalled(14.0);
        let m = CellMetrics { width: 8.0, height: 20.0 };
        let editor = Editor::new();
        let grid_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let mut view = PopupMenuView::default();
        let long = "term://~/projects/eshvi//92591:/Users/me/.local/bin/claude";
        let pm = PopupMenu {
            items: ["shell", "Switch"]
                .iter()
                .map(|w| grid::PmItem { word: w.to_string(), kind: String::new(), menu: long.into(), info: String::new() })
                .collect(),
            selected: -1,
            row: 5,
            col: 30,
            grid: 1,
        };
        let mut menu_rect = Rect::NOTHING;
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let (rects, _) = view.show(ui.ctx(), &editor, &pm, &fonts, m, grid_rect, None, None);
                menu_rect = rects[0];
            });
            out.textures_delta.clear();
            shapes = out.shapes;
        }
        let mut texts = Vec::new();
        for s in &shapes {
            if let egui::Shape::Text(t) = &s.shape {
                texts.push((t.galley.text().to_owned(), t.visual_bounding_rect()));
            }
        }
        let menus: Vec<_> = texts.iter().filter(|(t, _)| t.starts_with("term://")).collect();
        assert_eq!(menus.len(), 2, "{texts:?}");
        for (text, r) in menus {
            assert!(text.ends_with('…') && text.len() < long.len(), "cut: {text}");
            assert!(menu_rect.contains_rect(*r), "{r:?} inside the menu {menu_rect:?}");
            let word = texts.iter().find(|(t, w)| (t == "shell" || t == "Switch") && w.center().y == r.center().y);
            if let Some((_, w)) = word {
                assert!(w.max.x < r.min.x, "after the word: {w:?} {r:?}");
            }
        }
    }

    fn menu_with_info(selected: i64) -> PopupMenu {
        PopupMenu {
            items: ["a", "b"]
                .map(|w| grid::PmItem { word: w.into(), kind: String::new(), menu: String::new(), info: format!("info of {w}") })
                .to_vec(),
            selected,
            row: 0,
            col: 0,
            grid: 1,
        }
    }

    #[test]
    fn items_info_shows_until_documentation_is_on_request() {
        let mut v = PopupMenuView::default();
        assert_eq!(v.info_for(&menu_with_info(1)), "info of b", "as Neovim's own completion has it");
        assert_eq!(v.info_for(&menu_with_info(-1)), "");
        // -1 turns on-request mode on: the items' info is no longer shown.
        v.set_requested_doc(-1, String::new());
        assert_eq!(v.info_for(&menu_with_info(1)), "");
        // What was asked for shows for its item only.
        v.set_requested_doc(1, "asked *for*\n".into());
        assert_eq!(v.info_for(&menu_with_info(1)), "asked *for*");
        assert_eq!(v.info_for(&menu_with_info(0)), "");
        v.set_requested_doc(1, String::new());
        assert_eq!(v.info_for(&menu_with_info(1)), "", "emptied");
    }

    #[test]
    fn requested_documentation_is_drawn_beside_the_menu_and_scrolls() {
        let ctx = egui::Context::default();
        let fonts = GridFonts::uninstalled(14.0);
        let m = CellMetrics { width: 8.0, height: 20.0 };
        let editor = Editor::new();
        let grid_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        let mut view = PopupMenuView::default();
        let mut pm = menu_with_info(0);
        let run = |view: &mut PopupMenuView, pm: &PopupMenu| {
            let mut rects = 0;
            for _ in 0..3 {
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    rects = view.show(ui.ctx(), &editor, pm, &fonts, m, grid_rect, None, None).0.len();
                });
                out.textures_delta.clear();
            }
            rects
        };
        // On-request mode: the items' `info` isn't drawn, only what was asked for.
        view.set_requested_doc(-1, String::new());
        assert_eq!(run(&mut view, &pm), 1, "menu only");
        assert!(!view.doc_scrolls());
        view.set_requested_doc(0, "paragraph\n\n".repeat(80));
        assert_eq!(run(&mut view, &pm), 2, "menu and its docs");
        assert!(view.doc_scrolls(), "long: Ctrl-F/B scroll it");
        // The selection moved on: the docs of the old item go.
        pm.selected = 1;
        assert_eq!(run(&mut view, &pm), 1);
        assert!(!view.doc_scrolls());
    }

    /// The documentation's whole box, with its frame, for `menu` in `area`.
    fn doc_box(area: Rect, menu: Rect, above: bool) -> Rect {
        let frame = Vec2::new(30.0, 22.0);
        let d = doc_place(area, menu, frame, above);
        d.pivot.anchor_size(d.pos, d.text + frame)
    }

    #[test]
    fn documentation_goes_beside_the_menu_or_under_it_and_stays_in_the_editor() {
        let area = Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(1000.0, 600.0));
        let menu = |x: f32, w: f32| Rect::from_min_size(Pos2::new(x, 200.0), Vec2::new(w, 150.0));
        // Room on the right: there, as wide as it gets.
        let b = doc_box(area, menu(200.0, 300.0), false);
        assert_eq!((b.min.x, b.min.y, b.width()), (504.0, 200.0, DOC_MAX.x + 30.0));
        // Less than the widest on the right, more than the least and than on
        // the left: as wide as there's room for, its frame inside the editor.
        let b = doc_box(area, menu(150.0, 650.0), false);
        assert_eq!((b.min.x, b.max.x), (804.0, 1100.0));
        // Too little on the right: on the left.
        let b = doc_box(area, menu(700.0, 300.0), false);
        assert_eq!((b.max.x, b.min.x), (696.0, 696.0 - DOC_MAX.x - 30.0));
        // Neither side: under the menu, or over it.
        let b = doc_box(area, menu(300.0, 600.0), false);
        assert_eq!((b.min.x, b.min.y), (300.0, 354.0));
        let b = doc_box(area, menu(300.0, 600.0), true);
        assert_eq!(b.max.y, 196.0);
        // Beside a menu over the cursor: its bottom lined up with the menu's.
        let b = doc_box(area, menu(200.0, 300.0), true);
        assert_eq!((b.min.x, b.max.y), (504.0, 350.0));
        // No room under the menu either: beside it, on the wider side.
        let low = Rect::from_min_size(Pos2::new(350.0, 520.0), Vec2::new(600.0, 120.0));
        let b = doc_box(area, low, false);
        assert_eq!((b.max.x, b.min.y), (346.0, 520.0));
        assert!(area.contains_rect(b), "{b:?}");
        // Under a menu at the right: moved left to fit.
        let b = doc_box(area, menu(850.0, 250.0), false);
        assert!(area.contains_rect(b), "{b:?}");
        // Never past the editor's bottom.
        for (x, w, above) in [(200.0, 300.0, false), (700.0, 300.0, false), (300.0, 600.0, false), (300.0, 600.0, true)] {
            let b = doc_box(area, menu(x, w), above);
            assert!(area.contains_rect(b), "{x} {w}: {b:?}");
        }
    }

    /// Under the menu, the documentation ends above the status bar (the grid
    /// goes on under it).
    #[test]
    fn documentation_under_the_menu_stays_above_the_status_bar() {
        let ctx = egui::Context::default();
        let fonts = GridFonts::uninstalled(14.0);
        let m = CellMetrics { width: 8.0, height: 20.0 };
        let editor = Editor::new();
        let grid_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 600.0));
        let mut view = PopupMenuView { bottom: Some(540.0), ..Default::default() };
        let item = |w: &str| grid::PmItem { word: w.into(), kind: "Method".into(), menu: "fn(self) -> Option<Self::Item>".into(), info: String::new() };
        let pm = PopupMenu { items: vec![item("max_by_key(…) (as Iterator)"), item("max")], selected: 0, row: 18, col: 25, grid: 1 };
        view.set_requested_doc(-1, String::new());
        view.set_requested_doc(0, "paragraph\n\n".repeat(40));
        let mut rects = Vec::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                rects = view.show(ui.ctx(), &editor, &pm, &fonts, m, grid_rect, None, None).0;
            });
            out.textures_delta.clear();
        }
        let (menu, doc) = (rects[0], rects[1]);
        assert!(doc.min.y >= menu.max.y, "under the menu: {menu:?} {doc:?}");
        assert!(doc.max.y <= 540.0 && menu.max.y <= 540.0, "above the status bar: {menu:?} {doc:?}");
        assert!(grid_rect.contains_rect(doc), "{doc:?}");
    }

    /// A long list that fits neither under the cursor nor over it shows fewer
    /// rows, not past the status bar.
    #[test]
    fn a_menu_with_room_for_fewer_rows_shows_fewer() {
        let ctx = egui::Context::default();
        let fonts = GridFonts::uninstalled(14.0);
        let m = CellMetrics { width: 8.0, height: 20.0 };
        let editor = Editor::new();
        let grid_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 300.0));
        let mut view = PopupMenuView { bottom: Some(280.0), ..Default::default() };
        let pm = PopupMenu {
            items: (0..20).map(|i| grid::PmItem { word: format!("item{i}"), kind: String::new(), menu: String::new(), info: String::new() }).collect(),
            selected: 0,
            row: 6,
            col: 10,
            grid: 1,
        };
        let mut menu = Rect::NOTHING;
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                menu = view.show(ui.ctx(), &editor, &pm, &fonts, m, grid_rect, None, None).0[0];
            });
            out.textures_delta.clear();
        }
        assert!(menu.min.y >= 140.0 && menu.max.y <= 280.0, "under the cursor's row, above the status bar: {menu:?}");
    }

    /// Long labels and signatures (rust-analyzer's) leave room beside the menu.
    #[test]
    fn a_menu_of_long_labels_takes_at_most_its_share_of_the_editor() {
        let ctx = egui::Context::default();
        let fonts = GridFonts::uninstalled(14.0);
        let m = CellMetrics { width: 8.0, height: 20.0 };
        let editor = Editor::new();
        let grid_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 600.0));
        let mut view = PopupMenuView::default();
        let item = |w: &str| grid::PmItem {
            word: w.into(),
            kind: "Method".into(),
            menu: "fn(self, compare: F) -> Option<<Self as Iterator>::Item>".into(),
            info: String::new(),
        };
        let pm = PopupMenu {
            items: vec![item("max_by(…) (as Iterator)"), item("max_by_key(…) (as Iterator)"), item("a_label_far_longer_than_its_column_ever_gets(…) (as SomeTrait)")],
            selected: -1,
            row: 2,
            col: 10,
            grid: 1,
        };
        let mut menu = Rect::NOTHING;
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                menu = view.show(ui.ctx(), &editor, &pm, &fonts, m, grid_rect, None, None).0[0];
            });
            out.textures_delta.clear();
            shapes = out.shapes;
        }
        assert!(menu.width() <= 1200.0 * MENU_MAX_SHARE + 12.0, "{menu:?}");
        for s in &shapes {
            if let egui::Shape::Text(t) = &s.shape {
                assert!(menu.contains_rect(t.visual_bounding_rect()), "{:?} in {menu:?}", t.galley.text());
            }
        }
    }

    #[test]
    fn documentation_scrolls_by_pages_within_its_length() {
        let mut v = PopupMenuView::default();
        assert!(!v.doc_scrolls(), "no documentation shown");
        v.doc = Some(("x".into(), crate::markdown::Rendered::new("x")));
        v.doc_max = 500.0;
        assert!(v.doc_scrolls());
        v.scroll_doc(1.0);
        assert_eq!(v.doc_offset, DOC_MAX.y * 0.8);
        v.scroll_doc(5.0);
        assert_eq!(v.doc_offset, 500.0, "not past the end");
        v.scroll_doc(-9.0);
        assert_eq!(v.doc_offset, 0.0);
    }
}
