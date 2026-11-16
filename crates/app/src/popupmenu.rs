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
    /// Documentation shows only on request (`gui_pum_doc`, native
    /// completion): the items' own `info` isn't shown.
    on_request: bool,
    /// What was asked for: the item's index and its Markdown.
    requested: Option<(i64, String)>,
}

/// Widest and tallest the documentation beside the menu gets.
const DOC_MAX: Vec2 = Vec2::new(440.0, 360.0);

pub enum Action {
    Select(i64),
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
        let word_chars = pm.items.iter().map(|i| i.word.chars().count()).max().unwrap_or(1).min(60);
        let menu_chars = pm.items.iter().map(|i| i.menu.chars().count()).max().unwrap_or(0).min(30);
        let pad = 6.0;
        let row_h = row_height(m);
        let menu_w = if menu_chars > 0 { menu_chars as f32 * m.width * 0.85 + 2.0 * pad } else { 0.0 };
        let width = (pad + badge_w + word_chars as f32 * m.width + pad + menu_w).max(120.0);
        let visible = pm.items.len().min(MAX_VISIBLE);
        let frame_margin = 4.0 + 1.0;
        let size = Vec2::new(width + 2.0 * frame_margin, visible as f32 * row_h + 2.0 * frame_margin);

        // Align the item text with the word being completed.
        let anchor = anchor_cell - Vec2::new(frame_margin + pad + badge_w, 0.0);
        let pos = place(grid_rect, anchor, m.height, size, prefer_above);

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
                                let word = p.text(Pos2::new(x, cy), Align2::LEFT_CENTER, &item.word, mono.clone(), fg);
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

        // The selected item's documentation: right of the menu, or left
        // when there's no room.
        let info = self.info_for(pm).to_owned();
        if info.is_empty() {
            self.doc = None;
        } else {
            if self.doc.as_ref().is_none_or(|(text, _)| *text != info) {
                self.doc = Some((info.clone(), crate::markdown::Rendered::new(&info)));
                self.doc_offset = 0.0;
            }
            let room_right = grid_rect.max.x - menu.max.x - 8.0;
            let room_left = menu.min.x - grid_rect.min.x - 8.0;
            let width = DOC_MAX.x.min(room_right.max(room_left)).max(160.0);
            let (pos, pivot) = if room_right >= width.min(DOC_MAX.x) || room_right >= room_left {
                (Pos2::new(menu.max.x + 4.0, menu.min.y), Align2::LEFT_TOP)
            } else {
                (Pos2::new(menu.min.x - 4.0, menu.min.y), Align2::RIGHT_TOP)
            };
            let (_, doc) = self.doc.as_mut().unwrap();
            let offset = &mut self.doc_offset;
            let max = &mut self.doc_max;
            let area = egui::Area::new(Id::unique("eshvi-popupmenu-doc"))
                .order(Order::Foreground)
                .fixed_pos(pos)
                .pivot(pivot)
                .constrain(false)
                .show(ctx, |ui| {
                    theme.frame().inner_margin(crate::style::POPUP_PAD).show(ui, |ui| {
                        ui.set_max_width(width);
                        ui.set_max_height(DOC_MAX.y);
                        ui.style_mut().visuals.override_text_color = Some(theme.fg);
                        // The bar shows while the pointer is over it;
                        // Ctrl+F/B or the wheel scroll it.
                        crate::popup::thin_scrollbars(ui, &theme);
                        let out = egui::ScrollArea::vertical()
                            .max_height(DOC_MAX.y)
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
        self.doc_offset = (self.doc_offset + pages * DOC_MAX.y * 0.8).clamp(0.0, self.doc_max);
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
