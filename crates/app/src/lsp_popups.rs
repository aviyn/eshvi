//! Native popups for LSP data sent by the `gui_bridge` Lua plugin: hover
//! (rendered markdown), signature help and diagnostics.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Id, Order, Pos2, Rect, ScrollArea, Stroke, Vec2};
use std::sync::Arc;

use grid::Editor;
use nvim_bridge::gui::{Diagnostic, GuiEvent, PopupKind, Severity, SignatureHelp, SyntaxStyle};

use crate::fonts::GridFonts;
use crate::popup::{mix, place, PopupTheme};
use crate::renderer::{rgb, CellMetrics};

/// Popup size limits, in points.
#[derive(Clone, Copy, Debug)]
pub struct PopupLimits {
    pub hover_max_width: f32,
    pub hover_max_height: f32,
    pub signature_max_width: f32,
}

impl Default for PopupLimits {
    fn default() -> Self {
        let p = crate::config::PopupConfig::default();
        Self { hover_max_width: p.hover_max_width, hover_max_height: p.hover_max_height, signature_max_width: p.signature_max_width }
    }
}

struct Hover {
    markdown: crate::markdown::Rendered,
    /// Shown for the first time: start scrolled to the top.
    fresh: bool,
}

/// A completion item's documentation: the hover's look, beside the menu.
struct Docs {
    markdown: crate::markdown::Rendered,
    /// The menu's window.
    win: i64,
    /// Scrolled this far (points), and as far as it goes.
    offset: f32,
    max: f32,
}

/// Where the documentation goes around the menu `menu`: `(top-left, most
/// width, most height)`. Beside it where there's room (right first), else
/// on the far side of it from the cursor or on the cursor's other side,
/// whichever is roomier; never over the cursor's row.
fn place_docs(screen: Rect, menu: Rect, cursor: Rect, width: f32, height: f32) -> (Pos2, f32, f32) {
    const GAP: f32 = 4.0;
    const MIN_SIDE: f32 = 320.0;
    let right = screen.max.x - menu.max.x - GAP;
    let left = menu.min.x - screen.min.x - GAP;
    let tall = height.min(screen.max.y - menu.min.y);
    if right >= MIN_SIDE.min(width) {
        return (Pos2::new(menu.max.x + GAP, menu.min.y), width.min(right), tall);
    }
    if left >= MIN_SIDE.min(width) {
        let w = width.min(left);
        return (Pos2::new(menu.min.x - GAP - w, menu.min.y), w, tall);
    }
    let w = width.min(screen.width());
    let x = menu.min.x.min(screen.max.x - w).max(screen.min.x);
    // Rows above and below what mustn't be covered.
    let (upper, lower) = (menu.min.y.min(cursor.min.y), menu.max.y.max(cursor.max.y));
    let (below, above) = (screen.max.y - lower - GAP, upper - screen.min.y - GAP);
    if below >= above {
        (Pos2::new(x, lower + GAP), w, height.min(below))
    } else {
        let h = height.min(above);
        (Pos2::new(x, upper - GAP - h), w, h)
    }
}

#[derive(Default)]
pub struct LspPopups {
    hover: Option<Hover>,
    docs: Option<Docs>,
    signature: Option<SignatureHelp>,
    diagnostics: Option<Vec<Diagnostic>>,
    /// Last measured size per popup, used to decide above/below placement.
    sizes: [Option<Vec2>; 3],
    /// Rows to scroll the documentation by, for its next frame.
    docs_scroll: i64,
    /// Syntax theme for code in popups, from the colorscheme.
    syntax: Option<Arc<syntect::highlighting::Theme>>,
    pub limits: PopupLimits,
}

/// Everything a popup needs to position and style itself.
pub struct PopupCtx<'a> {
    pub editor: &'a Editor,
    pub fonts: &'a GridFonts,
    pub metrics: CellMetrics,
    pub screen: Rect,
    /// Top-left of the cursor cell, in points.
    pub cursor: Pos2,
}

fn severity_color(editor: &Editor, sev: Severity) -> Color32 {
    let (group, fallback) = match sev {
        Severity::Error => ("DiagnosticError", Color32::from_rgb(0xf1, 0x4c, 0x4c)),
        Severity::Warning => ("DiagnosticWarn", Color32::from_rgb(0xcc, 0xa7, 0x00)),
        Severity::Info => ("DiagnosticInfo", Color32::from_rgb(0x37, 0x94, 0xff)),
        Severity::Hint => ("DiagnosticHint", Color32::from_rgb(0x9a, 0xa0, 0xa6)),
    };
    editor
        .hl_groups
        .get(group)
        .and_then(|id| editor.hl_attr(*id).foreground)
        .map(rgb)
        .unwrap_or(fallback)
}

impl LspPopups {
    /// The code theme from the colorscheme, for markdown in other popups.
    pub fn syntax(&self) -> Option<Arc<syntect::highlighting::Theme>> {
        self.syntax.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.hover.is_none() && self.signature.is_none() && self.diagnostics.is_none() && self.docs.is_none()
    }

    pub fn handle(&mut self, ev: GuiEvent) {
        match ev {
            GuiEvent::Hover { contents, kind } => {
                let markdown = if kind == "markdown" { contents } else { format!("```\n{contents}\n```") };
                self.hover = Some(Hover { markdown: crate::markdown::Rendered::new(&markdown), fresh: true });
                self.diagnostics = None;
            }
            GuiEvent::Docs { contents, win } => {
                self.docs = (!contents.is_empty()).then(|| Docs {
                    markdown: crate::markdown::Rendered::new(&contents),
                    win,
                    offset: 0.0,
                    max: 0.0,
                });
            }
            GuiEvent::DocsScroll(rows) => self.docs_scroll = rows,
            GuiEvent::Syntax(colors) => {
                let xml = syntax_theme(&colors);
                match syntect::highlighting::ThemeSet::load_from_reader(&mut std::io::Cursor::new(xml.as_bytes())) {
                    Ok(theme) => self.syntax = Some(Arc::new(theme)),
                    Err(e) => log::warn!("syntax theme: {e}"),
                }
            }
            GuiEvent::Signature(help) => self.signature = (!help.signatures.is_empty()).then_some(help),
            GuiEvent::Diagnostic(d) => self.diagnostics = (!d.is_empty()).then_some(d),
            GuiEvent::PopupClose(None) => self.close_all(),
            GuiEvent::PopupClose(Some(PopupKind::Hover)) => self.hover = None,
            GuiEvent::PopupClose(Some(PopupKind::Signature)) => self.signature = None,
            GuiEvent::PopupClose(Some(PopupKind::Diagnostic)) => self.diagnostics = None,
            GuiEvent::Sidebar(_)
            | GuiEvent::DecoColors(_)
            | GuiEvent::DiffColors(_)
            | GuiEvent::TermColors(_)
            | GuiEvent::Copy(_)
            | GuiEvent::Preview(_)
            | GuiEvent::PreviewText { .. }
            | GuiEvent::Progress(_)
            | GuiEvent::PumDoc { .. }
            | GuiEvent::Options { .. }
            | GuiEvent::Status(_)
            | GuiEvent::Cwd(_)
            | GuiEvent::Buffer { .. }
            | GuiEvent::Explorer(_)
            | GuiEvent::Git(_)
            | GuiEvent::TermDebug(_)
            | GuiEvent::GitDiff { .. }
            | GuiEvent::Diagnostics(_)
            | GuiEvent::Problems(_)
            | GuiEvent::Panel { .. }
            | GuiEvent::Navigate(_)
            | GuiEvent::Windows(_)
            | GuiEvent::Saved(_)
            | GuiEvent::DiffSaved(_)
            | GuiEvent::DiffEdit { .. }
            | GuiEvent::AgentEnv(_)
            | GuiEvent::Session(_)
            | GuiEvent::Menu(_)
            | GuiEvent::PopupMenu(_)
            | GuiEvent::PickerOpen(_)
            | GuiEvent::PickerClose(_)
            | GuiEvent::PickerPreview(_)
            | GuiEvent::PickerItems { .. }
            | GuiEvent::PickerTyped { .. }
            | GuiEvent::TreeSet(_)
            | GuiEvent::TreeSelect { .. }
            | GuiEvent::TreeMenu { .. }
            | GuiEvent::TreeUnregister(_)
            | GuiEvent::TreeRegister(_)
            | GuiEvent::TreeShow { .. }
            | GuiEvent::Outline(_)
            | GuiEvent::Action(_) => {}
        }
    }

    /// Whether the diagnostics popup is open.
    pub fn has_diagnostics(&self) -> bool {
        self.diagnostics.is_some()
    }

    pub fn close_all(&mut self) {
        self.hover = None;
        self.signature = None;
        self.diagnostics = None;
    }

    /// The window of the menu the documentation goes with, while it's open.
    pub fn docs_win(&self) -> Option<i64> {
        self.docs.as_ref().map(|d| d.win)
    }

    /// Draw the documentation around `menu`; returns its rect.
    pub fn show_docs(&mut self, ctx: &egui::Context, pc: &PopupCtx, menu: Rect) -> Option<Rect> {
        let mut docs = self.docs.take()?;
        let theme = PopupTheme::from_editor(pc.editor);
        // Short of the status bar, less the frame.
        let screen = Rect::from_min_max(pc.screen.min, pc.screen.max - Vec2::new(0.0, pc.metrics.height)).shrink(4.0);
        let cursor = Rect::from_min_size(pc.cursor, Vec2::new(pc.metrics.width, pc.metrics.height));
        let (pos, width, height) = place_docs(screen, menu, cursor, self.limits.hover_max_width, self.limits.hover_max_height);
        docs.offset = (docs.offset + std::mem::take(&mut self.docs_scroll) as f32 * pc.metrics.height).clamp(0.0, docs.max.max(0.0));
        let syntax = self.syntax.clone();
        let frame = crate::style::POPUP_PAD;
        let (inner_w, inner_h) = (width - frame.sum().x - 2.0, height - frame.sum().y - 2.0);
        let rect = egui::Area::new(Id::unique("eshvi-completion-docs"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .show(ctx, |ui| {
                theme.frame().inner_margin(frame).show(ui, |ui| {
                    Self::style_popup_ui(ui, &theme);
                    ui.set_max_width(inner_w);
                    // (An Area's room is its size last frame: the scroll area
                    // wouldn't grow for longer text.)
                    ui.set_max_height(inner_h);
                    let out = ScrollArea::vertical()
                        .max_height(inner_h)
                        .auto_shrink([true, true])
                        .vertical_scroll_offset(docs.offset)
                        .show(ui, |ui| docs.markdown.show(ui, syntax.as_deref(), pc.fonts, theme.fg));
                    docs.offset = out.state.clamped_offset().y;
                    docs.max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                });
            })
            .response
            .rect;
        self.docs = Some(docs);
        Some(rect)
    }

    /// Draw open popups; returns their screen rects.
    pub fn show(&mut self, ctx: &egui::Context, pc: &PopupCtx) -> Vec<Rect> {
        let theme = PopupTheme::from_editor(pc.editor);
        let mut rects = Vec::new();
        if let Some(r) = self.show_signature(ctx, pc, &theme) {
            rects.push(r);
        }
        // Hover prefers above the cursor, unless signature help is there.
        let hover_above = self.signature.is_none();
        if let Some(r) = self.show_hover(ctx, pc, &theme, hover_above) {
            rects.push(r);
        }
        if let Some(r) = self.show_diagnostics(ctx, pc, &theme) {
            rects.push(r);
        }
        rects
    }

    fn area(
        &mut self,
        slot: usize,
        ctx: &egui::Context,
        pc: &PopupCtx,
        prefer_above: bool,
        estimate: Vec2,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) -> Rect {
        let size = self.sizes[slot].unwrap_or(estimate);
        let pos = place(pc.screen.shrink(4.0), pc.cursor, pc.metrics.height, size, prefer_above);
        let resp = egui::Area::new(Id::unique(("eshvi-lsp-popup", slot)))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .show(ctx, add_contents)
            .response;
        let measured = resp.rect.size();
        if self.sizes[slot].is_none_or(|s| (s - measured).length() > 0.5) {
            self.sizes[slot] = Some(measured);
            ctx.request_repaint();
        }
        resp.rect
    }

    /// Markdown's look in a popup's colors (also the Markdown preview's).
    pub fn style_popup_ui(ui: &mut egui::Ui, theme: &PopupTheme) {
        crate::popup::thin_scrollbars(ui, theme);
        let hairline = 1.0 / ui.ctx().pixels_per_point();
        let v = ui.visuals_mut();
        // Not `override_text_color`: it would take links' color too.
        v.override_text_color = None;
        v.widgets.noninteractive.fg_stroke.color = theme.fg;
        v.widgets.inactive.fg_stroke.color = theme.fg;
        // Bold text, brighter.
        v.widgets.active.fg_stroke.color = mix(theme.fg, Color32::WHITE, crate::style::TINT);
        // Code blocks: the popup's own color with a thin, quiet border (the
        // syntax theme has no background, so this one is kept).
        v.extreme_bg_color = theme.bg;
        v.code_bg_color = mix(theme.bg, theme.fg, crate::style::SUBTLE);
        v.widgets.noninteractive.bg_stroke = Stroke::new(hairline, mix(theme.bg, theme.fg, crate::style::ON));
        v.widgets.noninteractive.corner_radius = egui::CornerRadius::same(crate::style::RADIUS as u8);
        v.hyperlink_color = theme.accent;
    }

    fn show_hover(&mut self, ctx: &egui::Context, pc: &PopupCtx, theme: &PopupTheme, above: bool) -> Option<Rect> {
        let mut hover = self.hover.take()?;
        let limits = self.limits;
        let max_w = limits.hover_max_width.min(pc.screen.width() - 16.0);
        // No taller than the room on the roomier side of the cursor (less
        // the frame), so it fits there and scrolls instead of running off.
        let room_above = pc.cursor.y - pc.screen.min.y;
        let room_below = pc.screen.max.y - (pc.cursor.y + pc.metrics.height);
        let room = room_above.max(room_below) - 28.0;
        let max_h = limits.hover_max_height.min(pc.screen.height() * 0.6).min(room).max(pc.metrics.height * 3.0);
        let syntax = self.syntax.clone();
        let rect = self.area(0, ctx, pc, above, Vec2::new(max_w, 120.0), |ui| {
            theme.frame().inner_margin(crate::style::POPUP_PAD).show(ui, |ui| {
                Self::style_popup_ui(ui, theme);
                ui.set_max_width(max_w);
                ui.set_max_height(max_h);
                let mut scroll = ScrollArea::vertical().max_height(max_h).auto_shrink([true, true]);
                if std::mem::take(&mut hover.fresh) {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                scroll.show(ui, |ui| {
                    hover.markdown.show(ui, syntax.as_deref(), pc.fonts, theme.fg);
                });
            });
        });
        self.hover = Some(hover);
        Some(rect)
    }

    fn show_signature(&mut self, ctx: &egui::Context, pc: &PopupCtx, theme: &PopupTheme) -> Option<Rect> {
        let help = self.signature.clone()?;
        let (sig, active) = help.active()?;
        let max_w = self.limits.signature_max_width.min(pc.screen.width() - 16.0);
        let (mono, _) = pc.fonts.font_id(false, false);
        let (mono_bold, _) = pc.fonts.font_id(true, false);
        let accent = theme.accent;

        let mut job = LayoutJob::default();
        job.wrap.max_width = max_w - 20.0;
        let plain = TextFormat { font_id: mono.clone(), color: theme.fg, ..Default::default() };
        let param = active.and_then(|i| sig.parameters.get(i)).filter(|p| {
            p.start < p.end && p.end <= sig.label.len() && sig.label.is_char_boundary(p.start) && sig.label.is_char_boundary(p.end)
        });
        match param {
            Some(p) => {
                job.append(&sig.label[..p.start], 0.0, plain.clone());
                job.append(
                    &sig.label[p.start..p.end],
                    0.0,
                    TextFormat {
                        font_id: mono_bold,
                        color: accent,
                        underline: Stroke::new(crate::style::HAIRLINE, accent),
                        ..Default::default()
                    },
                );
                job.append(&sig.label[p.end..], 0.0, plain.clone());
            }
            None => job.append(&sig.label, 0.0, plain.clone()),
        }
        if help.signatures.len() > 1 {
            job.append(
                &format!("  {}/{}", help.active_signature + 1, help.signatures.len()),
                0.0,
                TextFormat { font_id: crate::style::scaled(&FontId::new(pc.fonts.ui_size, FontFamily::Proportional), crate::style::SMALL), color: theme.dim_fg, ..Default::default() },
            );
        }
        let doc = param.map(|p| p.documentation.clone()).unwrap_or_default();

        let estimate = Vec2::new(max_w * 0.6, pc.metrics.height + 16.0);
        let mut doc = crate::markdown::Rendered::new(&doc);
        let syntax = self.syntax.clone();
        let rect = self.area(1, ctx, pc, true, estimate, |ui| {
            theme.frame().inner_margin(crate::style::POPUP_PAD).show(ui, |ui| {
                Self::style_popup_ui(ui, theme);
                ui.set_max_width(max_w);
                ui.label(job);
                if !doc.is_empty() {
                    ui.separator();
                    doc.show(ui, syntax.as_deref(), pc.fonts, theme.fg);
                }
            });
        });
        Some(rect)
    }

    fn show_diagnostics(&mut self, ctx: &egui::Context, pc: &PopupCtx, theme: &PopupTheme) -> Option<Rect> {
        let diags = self.diagnostics.clone()?;
        let max_w = self.limits.hover_max_width.min(pc.screen.width() - 16.0);
        let editor = pc.editor;
        let rect = self.area(2, ctx, pc, false, Vec2::new(max_w * 0.6, 60.0), |ui| {
            theme.frame().inner_margin(crate::style::POPUP_PAD).show(ui, |ui| {
                Self::style_popup_ui(ui, theme);
                ui.set_max_width(max_w);
                for (i, d) in diags.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(4.0);
                    }
                    let color = severity_color(editor, d.severity);
                    ui.horizontal_top(|ui| {
                        let (bar, _) = ui.allocate_exact_size(Vec2::new(3.0, pc.metrics.height), egui::Sense::hover());
                        ui.painter().rect_filled(bar, crate::style::BAR_RADIUS, color);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&d.message).color(theme.fg));
                            let mut meta = d.source.clone();
                            if !d.code.is_empty() {
                                meta = if meta.is_empty() { d.code.clone() } else { format!("{meta}({})", d.code) };
                            }
                            if !meta.is_empty() {
                                ui.label(egui::RichText::new(meta).small().color(theme.dim_fg));
                            }
                        });
                    });
                }
            });
        });
        Some(rect)
    }
}

/// A TextMate theme (what syntect reads) from the colorscheme's syntax
/// colors. It has no background, so code keeps the popup's.
fn syntax_theme(colors: &std::collections::BTreeMap<String, SyntaxStyle>) -> String {
    // Role → TextMate scopes (the Sublime syntaxes syntect bundles).
    const SCOPES: &[(&str, &str)] = &[
        ("comment", "comment, punctuation.definition.comment"),
        ("string", "string, punctuation.definition.string"),
        ("escape", "constant.character.escape"),
        ("number", "constant.numeric"),
        ("constant", "constant.language, constant.other, constant.character, support.constant"),
        ("keyword", "keyword, storage, storage.type, storage.modifier"),
        ("operator", "keyword.operator"),
        ("function", "entity.name.function, support.function, variable.function"),
        ("macro", "support.macro, entity.name.macro, entity.name.function.macro"),
        (
            "type",
            "entity.name.type, entity.name.struct, entity.name.enum, entity.name.trait, entity.name.class, \
             entity.name.union, support.type, support.class, storage.type.numeric, storage.type.primitive",
        ),
        ("property", "variable.other.member, variable.other.property, entity.name.field, meta.field"),
        ("parameter", "variable.parameter"),
        ("module", "entity.name.namespace, entity.name.module, support.module"),
        ("attribute", "meta.annotation, meta.attribute, entity.other.attribute-name"),
    ];
    let settings = |s: &SyntaxStyle| {
        let style = match (s.bold, s.italic) {
            (true, true) => "bold italic",
            (true, false) => "bold",
            (false, true) => "italic",
            (false, false) => "",
        };
        format!(
            "<dict><key>foreground</key><string>#{:06x}</string><key>fontStyle</key><string>{style}</string></dict>",
            s.fg
        )
    };
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
         <key>name</key><string>eshvi</string><key>settings</key><array>",
    );
    // Global settings first: the text color, and no background.
    out.push_str("<dict><key>settings</key>");
    match colors.get("text") {
        Some(t) => out.push_str(&format!("<dict><key>foreground</key><string>#{:06x}</string></dict>", t.fg)),
        None => out.push_str("<dict/>"),
    }
    out.push_str("</dict>");
    for (role, scopes) in SCOPES {
        if let Some(style) = colors.get(*role) {
            out.push_str(&format!("<dict><key>scope</key><string>{scopes}</string><key>settings</key>{}</dict>", settings(style)));
        }
    }
    out.push_str("</array></dict></plist>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The next item's longer documentation, with the popup still open: it
    /// grows to fit, not stuck at the shorter one's height (an egui Area's
    /// room is its size last frame).
    #[test]
    fn docs_grow_when_the_next_item_has_more() {
        use egui_kittest::Harness;
        let mut popups = LspPopups::default();
        popups.handle(GuiEvent::Docs { contents: "short".into(), win: 1 });
        let editor = Editor::new();
        let fonts = GridFonts::uninstalled(14.0);
        let mut harness = Harness::builder().with_size(Vec2::new(1200.0, 800.0)).build_ui_state(
            |ui, (popups, rect): &mut (LspPopups, Option<Rect>)| {
                let pc = PopupCtx {
                    editor: &editor,
                    fonts: &fonts,
                    metrics: CellMetrics { width: 8.0, height: 20.0 },
                    screen: Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0)),
                    cursor: Pos2::new(20.0, 20.0),
                };
                let menu = Rect::from_min_size(Pos2::new(20.0, 40.0), Vec2::new(200.0, 60.0));
                *rect = popups.show_docs(ui.ctx(), &pc, menu);
            },
            (popups, None),
        );
        harness.run();
        let short = harness.state().1.unwrap().height();
        harness.state_mut().0.handle(GuiEvent::Docs { contents: "a paragraph\n\n".repeat(40), win: 1 });
        harness.run();
        let long = harness.state().1.unwrap().height();
        assert!(short < 60.0, "{short}");
        assert!(long > 400.0, "stuck at {long}");
        // And back: no taller than it needs.
        harness.state_mut().0.handle(GuiEvent::Docs { contents: "short".into(), win: 1 });
        harness.run();
        assert!(harness.state().1.unwrap().height() < 60.0);
    }

    #[test]
    fn docs_go_beside_the_menu_else_clear_of_it_and_the_cursor() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0));
        let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
        let cursor = at(100.0, 180.0, 10.0, 20.0);
        let menu = at(100.0, 200.0, 200.0, 200.0);
        // Room on the right.
        assert_eq!(place_docs(screen, menu, cursor, 500.0, 400.0), (Pos2::new(304.0, 200.0), 500.0, 400.0));
        // Only on the left: its right edge next to the menu.
        let (p, w, _) = place_docs(screen, at(700.0, 200.0, 200.0, 200.0), at(700.0, 180.0, 10.0, 20.0), 500.0, 400.0);
        assert_eq!((p, w), (Pos2::new(196.0, 200.0), 500.0));
        // Neither side: below the menu, or above the cursor if roomier.
        let narrow = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 600.0));
        let (p, _, h) = place_docs(narrow, at(200.0, 200.0, 200.0, 100.0), at(200.0, 180.0, 10.0, 20.0), 500.0, 400.0);
        assert_eq!((p.y, h), (304.0, 296.0));
        let (p, _, h) = place_docs(narrow, at(200.0, 400.0, 200.0, 100.0), at(200.0, 380.0, 10.0, 20.0), 500.0, 400.0);
        assert_eq!((p.y, h), (0.0, 376.0));
    }
    use std::collections::BTreeMap;

    #[test]
    fn syntax_theme_follows_the_colorscheme_without_a_background() {
        let style = |fg, italic| SyntaxStyle { fg, italic, bold: false };
        let colors = BTreeMap::from([
            ("text".to_owned(), style(0xcdcecf, false)),
            ("keyword".to_owned(), style(0xc678dd, false)),
            ("comment".to_owned(), style(0x60728a, true)),
        ]);
        let xml = syntax_theme(&colors);
        let theme = syntect::highlighting::ThemeSet::load_from_reader(&mut std::io::Cursor::new(xml.as_bytes())).unwrap();
        assert_eq!(theme.settings.background, None, "code blocks keep the popup's background");
        assert_eq!(theme.settings.foreground.map(|c| (c.r, c.g, c.b)), Some((0xcd, 0xce, 0xcf)));
        let keyword = theme.scopes.iter().find(|i| format!("{:?}", i.scope).contains("storage")).unwrap();
        assert_eq!(keyword.style.foreground.map(|c| (c.r, c.g, c.b)), Some((0xc6, 0x78, 0xdd)));
    }
}
