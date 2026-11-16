//! The GUI's own look in one place: the shades its strips, tabs, pills
//! and buttons are drawn in, the corner radii, the small text sizes, the
//! space inside popups, and the buttons every strip shares ("×", "+").
//! Everything is a shade of the theme's colors (`Chrome`), so it follows
//! the colorscheme. Draw like things with these, not with numbers of their
//! own: `tests::chrome_uses_style_tokens` fails on a literal shade, radius
//! or stroke in the chrome's files.

use egui::{Color32, CornerRadius, FontId, Margin, Painter, Pos2, Rect, Shadow, Stroke, Vec2};
use grid::Editor;

use crate::popup::{mix, PopupTheme};
use crate::renderer::rgb;

// Shades: how far toward the text color a fill is from the surface it's
// on (a strip, a pill, a popup).
/// Dividers between tabs, under strips.
pub const LINE: f32 = 0.10;
/// A fill at rest: a pill, a chip, a selected segment, a hovered tab.
pub const SUBTLE: f32 = 0.07;
/// A button under the pointer; a text button at rest.
pub const HOVER: f32 = 0.10;
/// Lit: the current pill, a toggle that's on, a hovered chip or text button.
pub const ON: f32 = 0.16;
/// Selected rows in the explorer: toward the accent.
pub const SELECTED: f32 = 0.30;
/// A badge holding text in the text color: toward its color.
pub const TINT: f32 = 0.35;
/// A badge holding a letter in its own color: a wash of it.
pub const WASH: f32 = 0.22;
/// Halfway: an unfocused current tab, a hovered (not selected) row, a file
/// name's git color.
pub const HALF: f32 = 0.5;

// Text and icons: how far toward the surface they fade.
/// Not current, not focused, at rest (tabs, labels, icons, toggles off).
pub const DIM: f32 = 0.4;
/// Secondary: a tab's folder, a note.
pub const FAINT: f32 = 0.55;

/// Buttons, fields, rows, badges.
pub const RADIUS: f32 = 4.0;
/// A severity bar along a popup's edge.
pub const BAR_RADIUS: f32 = 1.5;

/// Icon lines ("×", "+", the layout and preview toggles, glyphs).
pub const STROKE: f32 = 1.3;
/// Hairlines: dividers, frames, a field's edge.
pub const HAIRLINE: f32 = 1.0;

/// Secondary text (labels, counts, a field's text) to its strip's or
/// popup's text size.
pub const SMALL: f32 = 0.9;
/// A count in a badge, an uppercase header.
pub const BADGE: f32 = 0.8;
/// A count over the corner of an icon button: small, so the icon shows.
pub const TINY: f32 = 0.65;

/// A tab's padding before its icon and after its close button.
pub const TAB_PAD: f32 = 10.0;
/// Between an icon and its label.
pub const GAP: f32 = 6.0;

/// Inside a popup of text (hover, signature, completion docs, toasts).
pub const POPUP_PAD: Margin = Margin::symmetric(10, 6);
/// Inside a dialog (messages, the Changes list).
pub const DIALOG_PAD: Margin = Margin::symmetric(12, 10);
/// Inside a chip (the showcmd and macro pills).
pub const CHIP_PAD: Margin = Margin::symmetric(8, 2);

/// A popup's and a float's shadow.
pub fn shadow() -> Shadow {
    Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(110) }
}

/// The darker shade of the editor background used for GUI panels: the
/// explorer, the tab strips, the title bar and the status bar.
pub fn panel_bg(bg: Color32) -> Color32 {
    mix(bg, Color32::BLACK, 0.22)
}

/// `font` at a fraction of its size (`SMALL`, `BADGE`), whole points.
pub fn scaled(font: &FontId, by: f32) -> FontId {
    FontId::new((font.size * by).round(), font.family.clone())
}

/// A fully round corner for a shape `height` tall (chips, badges).
pub fn round(height: f32) -> f32 {
    height / 2.0
}

/// The theme's colors as the chrome uses them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chrome {
    /// The editor's background.
    pub bg: Color32,
    pub fg: Color32,
    /// Strips, the title bar, the explorer.
    pub panel: Color32,
    pub accent: Color32,
}

impl Chrome {
    pub fn from_editor(editor: &Editor) -> Self {
        let bg = rgb(editor.default_colors.bg);
        Self::new(bg, rgb(editor.default_colors.fg), PopupTheme::from_editor(editor).accent)
    }

    pub fn new(bg: Color32, fg: Color32, accent: Color32) -> Self {
        Self { bg, fg, panel: panel_bg(bg), accent }
    }

    /// The divider color on the panel.
    pub fn line(&self) -> Color32 {
        mix(self.panel, self.fg, LINE)
    }

    /// A fill `level` (`SUBTLE`, `HOVER`, `ON`) toward the text on `surface`.
    pub fn shade(&self, surface: Color32, level: f32) -> Color32 {
        mix(surface, self.fg, level)
    }

    /// Text: full while `lit` (current, focused, hovered), else dimmed.
    pub fn text(&self, lit: bool) -> Color32 {
        if lit { self.fg } else { self.dim(self.fg) }
    }

    /// `color` dimmed (an exited terminal's title is dimmed twice).
    pub fn dim(&self, color: Color32) -> Color32 {
        mix(color, self.panel, DIM)
    }

    /// Secondary text.
    pub fn faint(&self) -> Color32 {
        mix(self.fg, self.panel, FAINT)
    }
}

/// A file icon's size beside a label in `font` (tabs, pills, the picker).
pub fn file_icon_size(font: &FontId) -> f32 {
    (font.size * 1.15).round()
}

/// A square button's side in a row `row_h` tall ("×" on a tab, "+").
pub fn button_size(row_h: f32) -> f32 {
    (row_h * 0.75).round()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Cross,
    Plus,
    Minus,
}

/// `glyph` centered at `c`, sized for a row `row_h` tall. On whole pixels:
/// the "+" as two bars of the same pixel width and length crossing in
/// their middles (lines between pixels came out one bar thicker, or off
/// center), the "−" as the "+"'s horizontal bar, the "×" around the same
/// center.
pub fn paint_glyph(p: &Painter, glyph: Glyph, c: Pos2, row_h: f32, color: Color32) {
    let ppp = p.ctx().pixels_per_point();
    let width = (STROKE * ppp).round().max(1.0);
    let mut len = (row_h * 0.32 * ppp).round().max(5.0);
    // Even leftovers either side of the crossing bar.
    if (len - width) % 2.0 != 0.0 {
        len += 1.0;
    }
    let min = ((c.to_vec2() * ppp - Vec2::splat(len / 2.0)).round()).to_pos2();
    let px = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(x, y) / ppp, Vec2::new(w, h) / ppp);
    let arm = (len - width) / 2.0;
    match glyph {
        Glyph::Plus => {
            p.rect_filled(px(min.x, min.y + arm, len, width), 0.0, color);
            p.rect_filled(px(min.x + arm, min.y, width, arm), 0.0, color);
            p.rect_filled(px(min.x + arm, min.y + arm + width, width, arm), 0.0, color);
        }
        Glyph::Minus => {
            p.rect_filled(px(min.x, min.y + arm, len, width), 0.0, color);
        }
        Glyph::Cross => {
            // From corner to corner of the same square, through the line's
            // middle: the ends inset by half the line so they stay inside.
            let (a, b) = (min.to_vec2() / ppp, (min.to_vec2() + Vec2::splat(len)) / ppp);
            let inset = width / ppp / 2.0;
            let stroke = Stroke::new(width / ppp, color);
            p.line_segment([Pos2::new(a.x + inset, a.y + inset), Pos2::new(b.x - inset, b.y - inset)], stroke);
            p.line_segment([Pos2::new(a.x + inset, b.y - inset), Pos2::new(b.x - inset, a.y + inset)], stroke);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Two pages, one behind the other (the explorer).
    Files,
    /// Source control's mark: a line with a branch off it.
    Branch,
    Check,
    Refresh,
    Undo,
    /// A page (open the file).
    OpenFile,
    Sync,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    /// Lines stacked: the diff view's unified layout.
    Unified,
    /// Two columns: the diff view's side by side layout.
    Split,
}

/// How far a button that can't be used now fades.
pub const DISABLED: f32 = 0.45;

/// The side of the square an icon is drawn in, in a row `row_h` tall (a
/// bit more than the "+"'s, as VS Code's are).
pub fn icon_size(row_h: f32) -> f32 {
    (row_h * 0.6).round()
}

/// The points of an arc of the circle at `c`, from angle `from` to `to`
/// (radians, clockwise on screen, 0 to the right).
fn arc(c: Pos2, r: f32, from: f32, to: f32) -> Vec<Pos2> {
    let steps = ((to - from).abs() * r).ceil().max(6.0) as usize;
    (0..=steps)
        .map(|i| {
            let a = from + (to - from) * i as f32 / steps as f32;
            c + Vec2::angled(a) * r
        })
        .collect()
}

/// An arrow head at `tip`, for a line arriving along `dir`.
fn arrow_head(p: &Painter, tip: Pos2, dir: Vec2, len: f32, stroke: Stroke) {
    let back = -dir.normalized() * len;
    let arm = |turn: f32| tip + egui::emath::Rot2::from_angle(turn) * back;
    p.add(egui::Shape::line(vec![arm(0.8), tip, arm(-0.8)], stroke));
}

/// A page with its corner folded, `s` points tall, centered at `c`: the
/// current file (the Problems toggle, "open file").
pub fn paint_page(p: &Painter, c: Pos2, s: f32, color: Color32) {
    let stroke = Stroke::new(STROKE, color);
    let page = Rect::from_center_size(c, Vec2::new((s * 0.8).round(), s)).shrink(0.5);
    let fold = (s * 0.3).round();
    let corner = Pos2::new(page.max.x - fold, page.min.y);
    let side = Pos2::new(page.max.x, page.min.y + fold);
    let outline = vec![page.left_top(), corner, side, page.right_bottom(), page.left_bottom()];
    p.add(egui::Shape::closed_line(outline, stroke));
    p.add(egui::Shape::line(vec![corner, Pos2::new(corner.x, side.y), side], stroke));
}

/// `icon` centered at `c` in a square of side `s` (`icon_size`), in `color`.
pub fn paint_icon(p: &Painter, icon: Icon, c: Pos2, s: f32, color: Color32) {
    use std::f32::consts::{PI, TAU};
    let stroke = Stroke::new(STROKE, color);
    let line = |points: Vec<Pos2>| {
        p.add(egui::Shape::line(points, stroke));
    };
    let at = |x: f32, y: f32| c + Vec2::new(x, y) * s;
    match icon {
        Icon::OpenFile => paint_page(p, c, s, color),
        Icon::Files => {
            // The front page whole; the one behind it shows above and to
            // the right of it.
            let side = (s * 0.8).round();
            let off = (s * 0.16).round().max(2.0);
            let front = Rect::from_center_size(c + Vec2::new(-off, off) / 2.0, Vec2::new((side * 0.8).round(), side)).shrink(0.5);
            p.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Middle);
            let back = front.translate(Vec2::new(off, -off));
            line(vec![
                Pos2::new(back.min.x, front.min.y),
                back.left_top(),
                back.right_top(),
                back.right_bottom(),
                Pos2::new(front.max.x, back.max.y),
            ]);
        }
        Icon::Branch => {
            let r = (s * 0.13).max(1.5);
            let (top, bottom) = (at(-0.24, -0.34), at(-0.24, 0.34));
            let node = at(0.26, -0.14);
            for center in [top, bottom, node] {
                p.circle_stroke(center, r, stroke);
            }
            line(vec![top + Vec2::new(0.0, r), bottom - Vec2::new(0.0, r)]);
            // From the node down and round into the line.
            let from = node + Vec2::new(0.0, r);
            let to = at(-0.24, 0.12);
            let bend = Pos2::new(from.x, to.y);
            let curve: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let t = i as f32 / 10.0;
                    let (a, b) = (from.lerp(bend, t), bend.lerp(to, t));
                    a.lerp(b, t)
                })
                .collect();
            line(curve);
        }
        Icon::Check => line(vec![at(-0.34, 0.02), at(-0.1, 0.26), at(0.36, -0.26)]),
        Icon::ChevronDown => line(vec![at(-0.3, -0.12), at(0.0, 0.18), at(0.3, -0.12)]),
        Icon::ChevronUp => line(vec![at(-0.3, 0.12), at(0.0, -0.18), at(0.3, 0.12)]),
        Icon::ChevronRight => line(vec![at(-0.12, -0.3), at(0.18, 0.0), at(-0.12, 0.3)]),
        Icon::Unified => {
            for y in [-0.28, 0.0, 0.28] {
                line(vec![at(-0.34, y), at(0.34, y)]);
            }
        }
        Icon::Split => {
            let frame = Rect::from_center_size(c, Vec2::new(s * 0.76, s * 0.6)).shrink(0.5);
            p.rect_stroke(frame, 1.0, stroke, egui::StrokeKind::Middle);
            line(vec![Pos2::new(c.x, frame.min.y), Pos2::new(c.x, frame.max.y)]);
        }
        Icon::Refresh => {
            let r = s * 0.36;
            let end = TAU * 0.625;
            let mut points = arc(c, r, -PI / 4.0, end);
            let tip = points.pop().unwrap_or(c);
            points.push(tip);
            line(points);
            // (The arc runs clockwise: it arrives along the tangent.)
            arrow_head(p, tip, Vec2::new(-end.sin(), end.cos()), s * 0.3, stroke);
        }
        Icon::Sync => {
            let r = s * 0.36;
            for start in [-2.7, 0.45] {
                let end = start + 2.3;
                let points = arc(c, r, start, end);
                let tip = *points.last().unwrap_or(&c);
                line(points);
                arrow_head(p, tip, Vec2::new(-end.sin(), end.cos()), s * 0.28, stroke);
            }
        }
        Icon::Undo => {
            // A shaft out of the arrow head, round the right, and back.
            let (top, bottom, bend) = (at(0.0, -0.2).y - c.y, at(0.0, 0.2).y - c.y, s * 0.2);
            let (left, shaft_end) = (c.x - s * 0.38, c.x + s * 0.1);
            let mut points = vec![Pos2::new(left, c.y + top)];
            points.extend(arc(Pos2::new(shaft_end, c.y + (top + bottom) / 2.0), bend, -PI / 2.0, PI / 2.0));
            points.push(Pos2::new(c.x - s * 0.2, c.y + bottom));
            line(points);
            arrow_head(p, Pos2::new(left, c.y + top), Vec2::new(-1.0, 0.0), s * 0.26, stroke);
        }
    }
}

/// A button's fill on `surface`: none at rest, `HOVER` under the pointer,
/// `ON` while lit.
pub fn paint_button_fill(p: &Painter, rect: Rect, chrome: &Chrome, surface: Color32, hovered: bool, on: bool) {
    let level = if on { ON } else if hovered { HOVER } else { return };
    p.rect_filled(rect, RADIUS, chrome.shade(surface, level));
}

/// A glyph button ("×", "+") centered at `c` in a row `row_h` tall on
/// `surface`: a fill while hovered, the glyph lit then. Returns the
/// button's rect (where it takes clicks).
pub fn glyph_button(p: &Painter, glyph: Glyph, c: Pos2, row_h: f32, chrome: &Chrome, surface: Color32, hovered: bool) -> Rect {
    let rect = Rect::from_center_size(c, Vec2::splat(button_size(row_h)));
    paint_button_fill(p, rect, chrome, surface, hovered, false);
    paint_glyph(p, glyph, c, row_h, chrome.text(hovered));
    rect
}

/// Whether the pointer is over `rect`.
pub fn hovered(p: &Painter, rect: Rect) -> bool {
    p.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|pos| rect.contains(pos))
}

/// A strip's current tab, as every strip draws it: in its content's color,
/// joined to it with an accent on top while focused; halfway to the
/// strip's color while not.
pub fn paint_current_tab(p: &Painter, tab: Rect, focused: bool, chrome: &Chrome, content: Color32) {
    p.rect_filled(tab, 0.0, tab_fill(chrome, content, true, focused));
    if focused {
        paint_tab_edges(p, tab, chrome.accent, content);
    }
}

/// What a tab is filled with (what its "×" sits on): the strip's color,
/// or the current one's (`paint_current_tab`).
pub fn tab_fill(chrome: &Chrome, content: Color32, current: bool, focused: bool) -> Color32 {
    match (current, focused) {
        (false, _) => chrome.panel,
        (true, true) => content,
        (true, false) => mix(content, chrome.panel, HALF),
    }
}

/// A focused tab's accent on top, and its bottom edge in its content's
/// color (joining it). Filled rects, not lines: a line's soft ends spill
/// past the divider on the tab's right.
pub fn paint_tab_edges(p: &Painter, tab: Rect, accent: Color32, content: Color32) {
    let (x0, x1) = (tab.min.x, tab.max.x - 1.0);
    p.rect_filled(Rect::from_min_max(Pos2::new(x0, tab.min.y), Pos2::new(x1, tab.min.y + 2.0)), 0.0, accent);
    p.rect_filled(Rect::from_min_max(Pos2::new(x0, tab.max.y - 1.0), Pos2::new(x1, tab.max.y)), 0.0, content);
}

/// The divider on a tab's right.
pub fn paint_tab_divider(p: &Painter, tab: Rect, chrome: &Chrome) {
    p.vline(tab.max.x - 0.5, tab.y_range(), Stroke::new(HAIRLINE, chrome.line()));
}

/// A chip's or a pill's frame (the showcmd pill): the popup's colors,
/// fully round.
pub fn chip_frame(theme: &PopupTheme) -> egui::Frame {
    egui::Frame::new()
        .fill(theme.bg)
        .stroke(Stroke::new(HAIRLINE, theme.border))
        .corner_radius(CornerRadius::same(u8::MAX))
        .inner_margin(CHIP_PAD)
}

/// A text button ("Accept", "Reject") on `surface`: `HOVER` at rest, `ON`
/// under the pointer.
pub fn text_button_fill(chrome: &Chrome, surface: Color32, hovered: bool) -> Color32 {
    chrome.shade(surface, if hovered { ON } else { HOVER })
}

/// The accent button ("Accept all"): lighter under the pointer.
pub fn primary_fill(accent: Color32, hovered: bool) -> Color32 {
    if hovered { mix(accent, Color32::WHITE, HOVER) } else { accent }
}

/// A text field's edge, as the Problems filter has it: a hairline that is
/// `HOVER` at rest, `ON` under the pointer, the accent while it has the
/// keyboard. Its fill is the editor's background (`Chrome::bg`).
pub fn field_stroke(chrome: &Chrome, focused: bool, hovered: bool) -> Stroke {
    Stroke::new(HAIRLINE, match (focused, hovered) {
        (true, _) => chrome.accent,
        (false, true) => chrome.shade(chrome.bg, ON),
        (false, false) => chrome.shade(chrome.bg, HOVER),
    })
}

/// A text field's frame around whatever `add` puts in it (a `TextEdit`
/// without a frame of its own, with the id `id`): the editor's background,
/// `field_stroke`, `RADIUS` corners. For fields that sit in the panel's
/// flow (the message box, the explorer's name field, the dialogs'); the
/// Problems filter, which is drawn in an area of its own, uses
/// `field_stroke` alone.
pub fn text_field<R>(
    ui: &mut egui::Ui,
    chrome: &Chrome,
    id: egui::Id,
    margin: Margin,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, Rect) {
    // (The stroke's width is part of the frame's margin, so it is there from
    // the start; its color is known once the content is.)
    let mut frame = egui::Frame::new()
        .fill(chrome.bg)
        .stroke(field_stroke(chrome, false, false))
        .corner_radius(CornerRadius::same(RADIUS as u8))
        .inner_margin(margin)
        .begin(ui);
    let inner = {
        let ui = &mut frame.content_ui;
        ui.visuals_mut().selection.stroke = Stroke::new(HAIRLINE, chrome.accent);
        // (egui's minimum control height would make it taller.)
        ui.spacing_mut().interact_size.y = 0.0;
        add(ui)
    };
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let over = hovered(ui.painter(), frame.frame.widget_rect(frame.content_ui.min_rect()));
    frame.frame = frame.frame.stroke(field_stroke(chrome, focused, over));
    let outer = frame.end(ui).rect;
    (inner, outer)
}

/// A dialog (`egui::Modal`) in the popups' look: their frame, with
/// `DIALOG_PAD` inside, and their text color. Enter and Esc are the
/// caller's: Esc closes (`ModalResponse::should_close`).
pub fn dialog<R>(
    ctx: &egui::Context,
    id: egui::Id,
    theme: &PopupTheme,
    width: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::ModalResponse<R> {
    let frame = theme.frame().inner_margin(DIALOG_PAD);
    egui::Modal::new(id).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        add(ui)
    })
}

/// A dialog's title, in the popups' text color (egui's strong text takes
/// the selection's, which can be as pale as the dialog's own fill).
pub fn dialog_title(ui: &mut egui::Ui, theme: &PopupTheme, text: &str) {
    ui.label(egui::RichText::new(text).strong().color(theme.fg));
}

/// A dialog's button: the accent with white text as "Accept all" is when
/// it's the main one, else the popups' text button.
pub fn dialog_button(ui: &mut egui::Ui, theme: &PopupTheme, text: &str, primary: bool) -> egui::Response {
    let color = if primary { Color32::WHITE } else { theme.fg };
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let pad = Vec2::new(GAP * 2.0, GAP);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::CLICK);
    let fill = if primary {
        primary_fill(theme.accent, resp.hovered())
    } else {
        text_button_fill(&Chrome::new(theme.bg, theme.fg, theme.accent), theme.bg, resp.hovered())
    };
    ui.painter().rect_filled(rect, RADIUS, fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// egui's own widgets (the right-click and explorer menus, the menu bar's
/// menus off macOS, tooltips) in the popups' look: their colors, corners,
/// shadow, and the completion menu's selection.
pub fn apply_to_egui(ctx: &egui::Context, theme: &PopupTheme) {
    ctx.all_styles_mut(|style| {
        style.spacing.menu_margin = Margin::same(4);
        let v = &mut style.visuals;
        v.window_fill = theme.bg;
        v.window_stroke = Stroke::new(HAIRLINE, theme.border);
        v.window_corner_radius = theme.radius();
        v.menu_corner_radius = theme.radius();
        v.window_shadow = theme.shadow();
        v.popup_shadow = theme.shadow();
        v.weak_text_color = Some(theme.dim_fg);
        let w = &mut v.widgets;
        w.noninteractive.fg_stroke.color = theme.fg;
        w.noninteractive.bg_stroke = Stroke::new(HAIRLINE, theme.border);
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.corner_radius = CornerRadius::same(RADIUS as u8);
        }
        w.inactive.fg_stroke.color = theme.fg;
        for state in [&mut w.hovered, &mut w.active, &mut w.open] {
            state.bg_fill = theme.sel_bg;
            state.weak_bg_fill = theme.sel_bg;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke.color = theme.sel_fg;
        }
    });
}

#[cfg(test)]
pub(crate) mod tests;
