//! Tab strips: the window tabs (winbar) and native tab pages, and what the bottom panel's strip shares.

use super::*;

impl VimApp {
    /// A window's winbar (its first row when the top margin is 1) drawn as
    /// an editor tab: a darker strip with one tab holding the file name, the
    /// directory dimmed beside it, and `●` for unsaved changes. The focused
    /// window's tab has an accent line and blends into the editor below.
    pub(super) fn paint_winbar(&mut self, painter: &egui::Painter, grid: u64, origin: Pos2) {
        if !self.statusline_gui {
            return;
        }
        let Some(w) = self.editor.windows.get(&grid) else { return };
        let win = w.win;
        let above = self.tab_in_separator(w);
        if (w.margins.top == 0 && !above) || w.kind != WindowKind::Normal || self.sidebar_wins.contains(&w.win) {
            return;
        }
        let Some(g) = self.editor.grids.get(&grid) else { return };
        // In the separator row above (no winbar): the name from the
        // window's path, unsaved changes from its report.
        let (origin, mut tab) = if above {
            let m = self.renderer.metrics();
            let modified = self.win_modified.contains(&w.win);
            (origin - Vec2::new(0.0, m.height), TabLabel { name: String::new(), dir: String::new(), modified })
        } else {
            let text: String = g.row(0).iter().map(|c| c.text.as_str()).collect();
            (origin, TabLabel::parse(&text))
        };
        // With the window's real path (reported by Neovim), show the folder
        // relative to the project and decorate by that file.
        let path = self.win_paths.get(&w.win).cloned();
        let mut deco = None;
        if let Some(path) = &path {
            if let Some(n) = path.file_name() {
                tab.name = n.to_string_lossy().into_owned();
            }
            let parent = path.parent().unwrap_or(path);
            tab.dir = match self.explorer.root().and_then(|r| parent.strip_prefix(r).ok()) {
                Some(rel) => rel.to_string_lossy().into_owned(),
                None => parent.to_string_lossy().into_owned(),
            };
            deco = Some(self.decos.get(path, false));
        }
        let deco_colors = DecoColors::from_editor(&self.editor);

        let m = self.renderer.metrics();
        let mut strip = Rect::from_min_size(origin, Vec2::new(g.width() as f32 * m.width, m.height));
        // From the separator's line on its left to the one on its right
        // (see `paint_separators`): the tab's edge is the edge of the
        // window's color below it, and the strip meets its neighbor's.
        strip.min.x -= self.separator_overlap(w);
        let right_overlap = self.separator_overlap_right(w);
        strip.max.x += right_overlap;
        let chrome = Chrome::from_editor(&self.editor);
        let (bg, fg, strip_bg) = (chrome.bg, chrome.fg, chrome.panel);
        let line = egui::Stroke::new(style::HAIRLINE, chrome.line());
        let focused = self.editor.cursor.grid == grid;

        let painter = painter.with_clip_rect(strip.intersect(painter.clip_rect()));
        painter.rect_filled(strip, 0.0, strip_bg);
        painter.hline(strip.x_range(), strip.max.y - 0.5, line);

        let font = {
            let mut f = self.grid_fonts.sidebar_font_id(false);
            f.size = self.status_font_size + 1.0;
            f
        };
        let base_color = chrome.text(focused);
        let name_color = deco
            .and_then(|d| deco_colors.name(&d))
            .map_or(base_color, |c| if focused { c } else { chrome.dim(c) });
        // Problem count after the name, like VS Code.
        let count = deco.map(|d| d.problems).filter(|p| p.errors + p.warnings > 0).map(|p| {
            if p.errors > 0 { (p.errors, deco_colors.error) } else { (p.warnings, deco_colors.warning) }
        });
        let count_galley = count.map(|(n, c)| painter.layout_no_wrap(n.to_string(), style::scaled(&font, style::SMALL), c));
        let count_w = count_galley.as_ref().map_or(0.0, |g| g.size().x + 8.0);
        let pad = style::TAB_PAD;
        let icon_size = file_icon_size(&font);
        let icon_w = icon_size + style::GAP;
        // The close slot: "×" (on the current tab, or hovered), or the
        // modified dot (which hovering turns into the "×"), as in VS Code.
        let close_w = style::button_size(m.height);
        // In a window too narrow for the whole tab, it takes the whole strip
        // (no second edge before the separator's) and the name gives up
        // only what's missing, ending in "…". The close slot keeps its room
        // only while it shows something: the "×" (focused, hovered) or the
        // modified dot.
        let hover = painter.ctx().input(|i| i.pointer.hover_pos());
        let slot = focused || tab.modified || hover.is_some_and(|p| strip.contains(p));
        let slot_w = if slot { 6.0 + close_w } else { 0.0 };
        let fixed = icon_w + count_w + slot_w + pad * 1.5;
        let natural = painter.layout_no_wrap(tab.name.clone(), font.clone(), name_color);
        let fits = fixed + natural.size().x <= strip.width();
        let name = if fits {
            natural
        } else {
            // The longest start that fits with "…" ("NOTES…", not "NOTES.…").
            let room = (strip.width() - fixed).max(font.size);
            let chars: Vec<char> = tab.name.chars().collect();
            (1..chars.len())
                .rev()
                .map(|n| format!("{}…", chars[..n].iter().collect::<String>().trim_end_matches(['.', ' ', '-', '_'])))
                .map(|label| painter.layout_no_wrap(label, font.clone(), name_color))
                .find(|g| g.size().x <= room)
                .unwrap_or_else(|| painter.layout_no_wrap("…".into(), font.clone(), name_color))
        };
        let width = if fits { (fixed + name.size().x + if slot { 0.0 } else { 6.0 + close_w }).round() } else { strip.width() };
        let tab_rect = Rect::from_min_size(strip.min, Vec2::new(width.min(strip.width()), strip.height()));
        let close = Rect::from_center_size(
            Pos2::new(tab_rect.max.x - pad * 0.5 - close_w / 2.0, tab_rect.center().y),
            Vec2::splat(close_w),
        );
        self.winbar_tabs.push((tab_rect, close, w.win, above));
        let tab_hovered = hover.is_some_and(|p| tab_rect.contains(p));
        let close_hovered = hover.is_some_and(|p| close.contains(p));
        // Like every strip's current tab: it joins the editor below, with
        // an accent on top while focused.
        style::paint_current_tab(&painter, tab_rect, focused, &chrome, bg);
        if above {
            // The edge with the window above (over the tab too; a focused
            // one's accent covers it).
            painter.hline(strip.x_range(), strip.min.y + 0.5, line);
            if focused {
                style::paint_tab_edges(&painter, tab_rect, chrome.accent, bg);
            }
        }
        if fits {
            style::paint_tab_divider(&painter, tab_rect, &chrome);
        }
        let text_y = tab_rect.center().y - name.size().y / 2.0;
        let icon_rect = Rect::from_center_size(
            Pos2::new(tab_rect.min.x + pad + icon_size / 2.0, tab_rect.center().y),
            Vec2::splat(icon_size),
        );
        let name_x = tab_rect.min.x + pad + icon_w;
        self.paint_file_icon(&painter, std::path::Path::new(&tab.name), icon_rect);
        if let Some(g) = count_galley {
            let x = name_x + name.size().x + 8.0;
            painter.galley(Pos2::new(x, tab_rect.center().y - g.size().y / 2.0), g, name_color);
        }
        painter.galley(Pos2::new(name_x, text_y), name.clone(), name_color);
        if tab.modified && !tab_hovered {
            painter.circle_filled(close.center(), 3.5, name_color);
        } else if focused || tab_hovered {
            let surface = style::tab_fill(&chrome, bg, true, focused);
            style::glyph_button(&painter, Glyph::Cross, close.center(), m.height, &chrome, surface, close_hovered);
        }
        // A Markdown file's previews, at the right end: in place (lit while
        // shown) and to the side; like the title bar's toggles. Also on a
        // tab in a separator row (a window below another).
        let markdown = path
            .as_ref()
            .and_then(|p| p.extension())
            .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        let mut right = strip.max.x - 4.0;
        if markdown {
            let in_place = self.preview.win(win).is_some_and(|p| !p.side);
            let size = Vec2::new((m.height * 1.1).round(), style::button_size(m.height));
            for (button, on) in [(WinbarButton::PreviewSide, false), (WinbarButton::Preview, in_place)] {
                let r = Rect::from_center_size(Pos2::new(right - size.x / 2.0, strip.center().y), size);
                right = r.min.x - 2.0;
                let hovered = hover.is_some_and(|p| r.contains(p));
                style::paint_button_fill(&painter, r, &chrome, strip_bg, hovered, on);
                let color = chrome.text(on || hovered);
                paint_winbar_button(&painter, button, r.center(), (m.height * 0.5).round(), color);
                self.winbar_buttons.push((r, win, button));
            }
        }
        if !tab.dir.is_empty() {
            // Ending in "…" where the strip ends, not cut through a letter.
            let room = right - (tab_rect.max.x + 10.0) - 6.0;
            if room > font.size {
                let mut job = egui::text::LayoutJob::single_section(
                    tab.dir.clone(),
                    egui::TextFormat::simple(font, chrome.faint()),
                );
                job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                painter.galley(Pos2::new(tab_rect.max.x + 10.0, text_y), painter.layout_job(job), fg);
            }
        }
    }

    /// Errors', warnings' and infos' colors: the theme's diagnostic colors as the GUI knows them.
    pub(super) fn problem_colors(&self) -> [egui::Color32; 3] {
        let deco = crate::decorations::DecoColors::from_editor(&self.editor);
        [deco.error, deco.warning, deco.info]
    }

    /// Paint the Material icon for a file name into `rect`.
    /// A file's icon in `tint` without its own colors (a tab that isn't the
    /// current one).
    pub(super) fn paint_file_icon_mono(&mut self, painter: &egui::Painter, file: &std::path::Path, rect: Rect, tint: egui::Color32) {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let icon = IconTheme::get().file_icon(&name);
        let px = (rect.width() * painter.ctx().pixels_per_point()).round() as u32;
        if let Some(tex) = self.icons.mono(painter.ctx(), icon, px) {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(tex.id(), rect, uv, tint);
        }
    }

    pub(super) fn paint_file_icon(&mut self, painter: &egui::Painter, file: &std::path::Path, rect: Rect) {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let icon = IconTheme::get().file_icon(&name);
        let px = (rect.width() * painter.ctx().pixels_per_point()).round() as u32;
        if let Some(tex) = self.icons.get(painter.ctx(), icon, px) {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(tex.id(), rect, uv, egui::Color32::WHITE);
        }
    }

    /// Whether the tab pages' strip is shown over the editor: Neovim's `'showtabline'` says
    /// (0 never, 1 with two or more tab pages, 2 always), and with the pills in the title
    /// bar only while it's hidden or the window is full screen.
    pub(super) fn tab_strip_shown(&self, ctx: &egui::Context) -> bool {
        let showtabline = match self.editor.options.get("showtabline") {
            Some(grid::OptionValue::Int(n)) => *n,
            _ => 1,
        };
        let strip = self.config.window.tabs == crate::config::TitleTabs::Strip;
        strip_shown(showtabline, self.editor.tabs.len(), strip || !self.tabs_in_titlebar(ctx))
    }

    /// Neovim's tab pages as GUI tabs at the top of the editor column (`ext_tabline`), as
    /// the agent panel's strip draws its tabs (`paint_tab`): the current one joined to the
    /// text below, with the accent while the editor has the keyboard, "×" while hovered
    /// (a dot while its window has unsaved changes), "+" after them. A middle click
    /// closes one, a click selects it.
    pub(super) fn show_tabline(&mut self, ui: &mut egui::Ui) {
        if !self.tab_strip_shown(ui.ctx()) {
            return;
        }
        let chrome = Chrome::from_editor(&self.editor);
        let (bg, panel) = (chrome.bg, chrome.panel);
        let font = {
            let mut f = self.grid_fonts.sidebar_font_id(false);
            f.size = self.status_font_size + 1.0;
            f
        };
        let height = self.tab_height();
        let tabs = self.editor.tabs.clone();
        let current = self.editor.current_tab;
        let focused = self.focused && matches!(self.focus, Focus::Editor | Focus::Diff);
        // Unsaved changes are known for the window with the cursor: the current tab's.
        let modified = self.editor.windows.get(&self.editor.cursor.grid).is_some_and(|w| self.win_modified.contains(&w.win));
        let mut command = None;
        let mut icons = Vec::new();
        let mut tab_painter = None;
        egui::Panel::top("eshvi-tabline")
            .exact_size(height)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(panel))
            .show(ui, |ui| {
                let strip = ui.max_rect();
                let painter = ui.painter_at(strip);
                tab_painter = Some(painter.clone());
                painter.hline(strip.x_range(), strip.max.y - 0.5, egui::Stroke::new(style::HAIRLINE, chrome.line()));
                let hover = ui.ctx().input(|i| i.pointer.hover_pos()).filter(|p| strip.contains(*p));
                let look = TabLook { chrome: &chrome, content_bg: bg, focused, height };
                let icon_w = file_icon_size(&font);
                // The tabs share the strip (less "+"); a title too long for its share ends in "…".
                let share = (strip.width() - height - 4.0) / tabs.len().max(1) as f32;
                let title_max = (share - tab_width(icon_w, 0.0, style::button_size(height))).max(font.size * 1.5);
                let mut x = strip.min.x;
                for (n, (tab, name)) in tabs.iter().enumerate() {
                    let is_current = *tab == current;
                    let label = std::path::Path::new(name)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "[No Name]".into());
                    let color = chrome.text(is_current && focused);
                    let title = cut_title(&painter, &label, &font, color, title_max);
                    let rect = Rect::from_min_size(
                        Pos2::new(x, strip.min.y),
                        Vec2::new(tab_width(icon_w, title.size().x, style::button_size(height)), strip.height()),
                    );
                    x = rect.max.x;
                    let dot = (is_current && modified).then_some(color);
                    let hit = paint_tab(&painter, &look, rect, is_current, title, color, icon_w, hover, dot, &mut |_, icon, _| {
                        icons.push((icon, std::path::PathBuf::from(name)));
                    });
                    let resp = ui.interact(rect, egui::Id::unique(("eshvi-tab", n)), egui::Sense::click());
                    let on_close = hover.is_some_and(|p| hit.slot.contains(p));
                    if resp.middle_clicked() || (resp.clicked() && on_close) {
                        command = Some(format!("lua pcall(vim.cmd.tabclose, vim.api.nvim_tabpage_get_number({tab}))"));
                    } else if resp.clicked() {
                        command = Some(format!("lua pcall(vim.api.nvim_set_current_tabpage, {tab})"));
                    }
                }
                // "+": a new tab.
                let plus = Rect::from_min_size(Pos2::new(x + 2.0, strip.min.y), Vec2::splat(strip.height()));
                let resp = ui.interact(plus, egui::Id::unique("eshvi-tab-new"), egui::Sense::click());
                style::glyph_button(&painter, Glyph::Plus, plus.center(), height, &chrome, panel, resp.hovered());
                if resp.clicked() {
                    command = Some("tabnew".into());
                }
            });
        // (The icons need `self`, which the panel's closure borrows.)
        if let Some(painter) = tab_painter {
            for (rect, path) in icons {
                self.paint_file_icon(&painter, &path, rect);
            }
        }
        if let Some(c) = command {
            self.bridge.send(UiCommand::Command(c));
        }
    }

    /// The filter field itself, over `rect`: its text edited in place. Returns
    /// whether the text changed and whether Esc was pressed in it. The bottom
    /// panel's Problems filter.
    pub(super) fn problems_filter_field(&self, ctx: &egui::Context, rect: Rect, text: &mut String) -> (bool, bool) {
        let focused = ctx.memory(|m| m.has_focus(problems_filter_id()));
        let theme = crate::popup::PopupTheme::from_editor(&self.editor);
        let bg = rgb(self.editor.default_colors.bg);
        let mut changed = false;
        let mut escaped = false;
        // In the scope switch's and severity toggles' text size. Its frame
        // is drawn here: as tall as the text's line and a little, but never
        // past the strip (1pt short of its edges), on the buttons' middle.
        // egui's own frame grows with the line, past the strip.
        let mut font = self.grid_fonts.sidebar_font_id(false);
        font.size = self.status_font_size + 1.0;
        let font = style::scaled(&font, style::SMALL);
        let row = ctx.fonts_mut(|f| f.row_height(&font));
        let rect = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), (row + 4.0).min(rect.height() + 6.0)));
        let size = Vec2::new(rect.width(), rect.height().max(row));
        let pad = ((size.y - row) / 2.0).floor() as i8;
        let hovered = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
        let stroke = style::field_stroke(&Chrome::from_editor(&self.editor), focused, hovered);
        egui::Area::new(egui::Id::unique("eshvi-problems-filter-area"))
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(rect.min.x, rect.center().y - size.y / 2.0))
            .show(ctx, |ui| {
                ui.painter().rect(rect, style::RADIUS, bg, stroke, egui::StrokeKind::Inside);
                let v = ui.visuals_mut();
                v.override_text_color = Some(theme.fg);
                v.selection.stroke = egui::Stroke::new(style::HAIRLINE, theme.accent);
                // (egui's minimum control height would make it taller.)
                ui.spacing_mut().interact_size.y = 0.0;
                let resp = ui.add_sized(
                    size,
                    egui::TextEdit::singleline(text)
                        .id(problems_filter_id())
                        .hint_text(egui::RichText::new("Filter").font(font.clone()))
                        .font(font)
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(style::GAP as i8, pad))),
                );
                // Focus by a click only: Tab is Neovim's (completion), but
                // egui gives it to the first field when none has focus, and
                // the keys after it would go here.
                if resp.gained_focus() && ui.input(|i| i.key_pressed(egui::Key::Tab)) {
                    resp.surrender_focus();
                }
                changed = resp.changed();
                escaped = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        (changed, escaped)
    }
}

/// Whether the tab pages' strip shows: `'showtabline'` 0 never, 1 with two or more tab pages,
/// 2 always; and only `allowed` (the pills aren't in the title bar).
pub(super) fn strip_shown(showtabline: i64, tab_pages: usize, allowed: bool) -> bool {
    allowed
        && match showtabline {
            0 => false,
            2 => tab_pages >= 1,
            _ => tab_pages >= 2,
        }
}

/// What the strips' tabs are drawn with (the agent panel's, the bottom panel's and the tab
/// pages'): the theme, the color of what the current tab joins, whether the strip's
/// owner has the keyboard (the accent), and the strip's height.
pub(super) struct TabLook<'a> {
    pub chrome: &'a Chrome,
    pub content_bg: egui::Color32,
    pub focused: bool,
    pub height: f32,
}

/// Where a painted tab's "×" slot is, and whether the pointer is on the tab.
pub(super) struct TabHit {
    pub slot: Rect,
    pub hovered: bool,
}

/// A tab's width: its padding, icon, gap, title and the slot after it (the dot, or the
/// "×" while hovered).
pub(super) fn tab_width(icon_w: f32, title_w: f32, slot_w: f32) -> f32 {
    (style::TAB_PAD + icon_w + style::GAP + 4.0 + slot_w + style::TAB_PAD * 0.5 + title_w).round()
}

/// `text` as one line in `color`, ending in "…" where it would pass `max`.
pub(super) fn cut_title(painter: &egui::Painter, text: &str, font: &egui::FontId, color: egui::Color32, max: f32) -> std::sync::Arc<egui::Galley> {
    let mut title = painter.layout_no_wrap(text.to_owned(), font.clone(), color);
    let mut keep = text.chars().count();
    while title.size().x > max && keep > 1 {
        keep -= 1;
        title = painter.layout_no_wrap(truncate_chars(text, keep), font.clone(), color);
    }
    title
}

/// One tab of a strip, in `rect`: the current one joined to the content below (the accent
/// while `look.focused`), a divider after it, the icon (`draw_icon`, given its square and
/// the text color), the title, and at the end "×" while the pointer is on the tab, else
/// `dot` if there is one. `hover` is the pointer, if over the strip.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_tab(
    painter: &egui::Painter,
    look: &TabLook,
    rect: Rect,
    current: bool,
    title: std::sync::Arc<egui::Galley>,
    color: egui::Color32,
    icon_w: f32,
    hover: Option<Pos2>,
    dot: Option<egui::Color32>,
    draw_icon: &mut dyn FnMut(&egui::Painter, Rect, egui::Color32),
) -> TabHit {
    let chrome = look.chrome;
    let pad = style::TAB_PAD;
    if current {
        style::paint_current_tab(painter, rect, look.focused, chrome, look.content_bg);
    }
    style::paint_tab_divider(painter, rect, chrome);
    let icon = Rect::from_min_size(Pos2::new(rect.min.x + pad, rect.center().y - icon_w / 2.0), Vec2::splat(icon_w));
    draw_icon(painter, icon, color);
    painter.galley(Pos2::new(icon.max.x + style::GAP, rect.center().y - title.size().y / 2.0), title, color);
    let close_w = style::button_size(look.height);
    let slot = Rect::from_center_size(Pos2::new(rect.max.x - pad * 0.5 - close_w / 2.0, rect.center().y), Vec2::splat(close_w));
    let hovered = hover.is_some_and(|p| rect.contains(p));
    if hovered {
        let on = hover.is_some_and(|p| slot.contains(p));
        let surface = style::tab_fill(chrome, look.content_bg, current, look.focused);
        style::glyph_button(painter, Glyph::Cross, slot.center(), look.height, chrome, surface, on);
    } else if let Some(c) = dot {
        painter.circle_filled(slot.center(), TAB_DOT, c);
    }
    TabHit { slot, hovered }
}

/// The radius of the dot a tab shows in place of its "×".
const TAB_DOT: f32 = 3.0;

/// A button at the right of a window's file-tab row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WinbarButton {
    /// The Markdown preview in place (`:EshviPreview`).
    Preview,
    /// The Markdown preview beside the file (`:EshviPreview side`).
    PreviewSide,
}

impl WinbarButton {
    pub(super) fn command(self) -> &'static str {
        match self {
            Self::Preview => "EshviPreview",
            Self::PreviewSide => "EshviPreview side",
        }
    }
}

/// A winbar button's icon, `s` points tall, centered at `c`: an eye (the
/// preview), or a box halved (beside, as the panel's split button).
pub(super) fn paint_winbar_button(p: &egui::Painter, button: WinbarButton, c: Pos2, s: f32, color: egui::Color32) {
    let stroke = egui::Stroke::new(style::STROKE, color);
    match button {
        WinbarButton::Preview => {
            let (w, h) = (s * 0.62, s * 0.42);
            let lid = |dir: f32| {
                egui::epaint::QuadraticBezierShape::from_points_stroke(
                    [c + Vec2::new(-w, 0.0), c + Vec2::new(0.0, dir * h * 1.6), c + Vec2::new(w, 0.0)],
                    false,
                    egui::Color32::TRANSPARENT,
                    stroke,
                )
            };
            p.add(lid(-1.0));
            p.add(lid(1.0));
            p.circle_stroke(c, s * 0.18, stroke);
        }
        WinbarButton::PreviewSide => {
            let b = Rect::from_center_size(c, Vec2::new(s * 1.1, s * 0.85));
            p.rect_stroke(b, style::BAR_RADIUS, stroke, egui::StrokeKind::Inside);
            p.vline(b.center().x, b.y_range(), stroke);
        }
    }
}

/// A tooltip under a button painted on a strip (no egui widget to hang
/// it on), after the pointer rested on it as long as egui's own wait.
pub(super) fn show_strip_tooltip(ctx: &egui::Context, button: Rect, text: &str) {
    let delay = ctx.global_style().interaction.tooltip_delay as f64;
    if ctx.input(|i| i.pointer.time_since_last_movement() as f64) < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay));
        return;
    }
    let id = egui::Id::unique(("eshvi-strip-tip", button.min.x as i32));
    egui::Tooltip::always_open(ctx.clone(), egui::LayerId::background(), id, egui::PopupAnchor::Position(button.center_bottom()))
        .show(|ui| ui.label(text));
}

/// The Problems filter field's egui id (it keeps keyboard focus).
pub(super) fn problems_filter_id() -> egui::Id {
    egui::Id::unique("eshvi-problems-filter")
}

/// Height of the digits' middle in a one-line galley: its baseline less
/// half a digit (digits are about cap height, 0.7 of the font size).
pub(super) fn digits_middle(galley: &egui::Galley, font_size: f32) -> f32 {
    let baseline = galley
        .rows
        .first()
        .and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y))
        .unwrap_or(galley.size().y * 0.8);
    baseline - 0.35 * font_size
}

/// A file icon's size beside a label in `font` (tabs, pills).
pub(super) fn file_icon_size(font: &egui::FontId) -> f32 {
    style::file_icon_size(font)
}

/// `text` cut to `max` characters, with "…" when cut.
pub(super) fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_owned(),
    }
}

/// Tab rects laid out left to right in `strip` with the given widths, and
/// a square "+" button after them (`button` wide). Tabs that don't fit are
/// clipped by the strip.
pub(super) fn terminal_tab_layout(strip: Rect, widths: &[f32], button: f32) -> (Vec<Rect>, Rect) {
    let mut x = strip.min.x;
    let tabs = widths
        .iter()
        .map(|w| {
            let r = Rect::from_min_size(Pos2::new(x, strip.min.y), Vec2::new(w.round(), strip.height()));
            x += w.round();
            r
        })
        .collect();
    (tabs, Rect::from_min_size(Pos2::new(x + 2.0, strip.min.y), Vec2::new(button, strip.height())))
}

/// What a winbar tab shows, parsed from the winbar text (`%f %m`).
#[derive(Debug, PartialEq)]
pub(super) struct TabLabel {
    name: String,
    /// Directory part of the path, shown dimmed next to the tab.
    dir: String,
    modified: bool,
}

impl TabLabel {
    fn parse(winbar: &str) -> Self {
        let mut text = winbar.trim();
        let mut modified = false;
        for flag in ["[+]", "[-]", "[RO]"] {
            if let Some(rest) = text.strip_suffix(flag) {
                modified |= flag == "[+]";
                text = rest.trim_end();
            }
        }
        let (dir, name) = match text.rfind('/') {
            Some(i) if i + 1 < text.len() => (&text[..i], &text[i + 1..]),
            _ => ("", text),
        };
        Self { name: name.to_owned(), dir: dir.to_owned(), modified }
    }
}

#[cfg(test)]
mod tests {
    use super::TabLabel;
    use egui::{Pos2, Rect, Vec2};

    #[test]
    fn terminal_tabs_layout() {
        use egui::{Pos2, Rect, Vec2};
        let strip = Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(500.0, 20.0));
        let (tabs, plus) = super::terminal_tab_layout(strip, &[80.0, 60.0], 20.0);
        assert_eq!(tabs[0], Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(80.0, 20.0)));
        assert_eq!(tabs[1].min.x, 180.0);
        assert!(tabs[1].contains(Pos2::new(200.0, 60.0)) && !tabs[0].contains(Pos2::new(200.0, 60.0)));
        assert_eq!(plus, Rect::from_min_size(Pos2::new(242.0, 50.0), Vec2::new(20.0, 20.0)));
        let (none, plus) = super::terminal_tab_layout(strip, &[], 20.0);
        assert!(none.is_empty());
        assert_eq!(plus.min.x, 102.0);
    }

    /// The Problems current-file toggle: off, on, and hovered with its
    /// tooltip (after egui's wait).
    #[test]
    fn current_file_toggle() {
        use crate::style::{self, Chrome};
        use egui::{Color32, Pos2, Rect, Vec2};
        let chrome = Chrome::new(Color32::from_rgb(0x28, 0x2c, 0x34), Color32::from_rgb(0xab, 0xb2, 0xbf), Color32::from_rgb(0x4f, 0xa6, 0xed));
        let mut results = egui_kittest::SnapshotResults::new();
        for (name, on, hover) in [("off", false, false), ("on", true, false), ("hovered", false, true)] {
            let mut harness = egui_kittest::Harness::builder()
                .with_size(Vec2::new(260.0, 70.0))
                .with_pixels_per_point(2.0)
                .with_render_cursor(false)
                .with_step_dt(0.25)
                .wgpu()
                .build_ui(move |ui| {
                    let full = ui.max_rect();
                    let p = ui.painter().clone();
                    p.rect_filled(full, 0.0, chrome.bg);
                    let strip = Rect::from_min_size(full.min, Vec2::new(full.width(), 24.0));
                    p.rect_filled(strip, 0.0, chrome.panel);
                    let h = strip.height();
                    let toggle = Rect::from_center_size(Pos2::new(strip.max.x - 30.0, strip.center().y), Vec2::new((h * 1.1).round(), style::button_size(h)));
                    let hovered = style::hovered(&p, toggle);
                    style::paint_button_fill(&p, toggle, &chrome, chrome.panel, hovered, on);
                    style::paint_page(&p, toggle.center(), (h * 0.5).round(), chrome.text(on || hovered));
                    if hovered {
                        super::show_strip_tooltip(ui.ctx(), toggle, "Show only the current file's problems");
                    }
                });
            if hover {
                harness.hover_at(Pos2::new(230.0, 12.0));
            }
            harness.run_steps(6);
            harness.snapshot(format!("current_file_toggle_{name}"));
            results.extend_harness(&mut harness);
        }
        results.unwrap();
    }

    /// 'showtabline': 0 never, 1 with two or more tab pages, 2 always; not while the pills
    /// are in the title bar.
    #[test]
    fn the_strip_follows_showtabline() {
        use super::strip_shown;
        assert!(!strip_shown(0, 3, true) && !strip_shown(0, 1, true));
        assert!(!strip_shown(1, 1, true) && strip_shown(1, 2, true));
        assert!(strip_shown(2, 1, true) && strip_shown(2, 3, true));
        assert!(!strip_shown(1, 3, false) && !strip_shown(2, 1, false));
    }

    #[test]
    fn a_tab_is_as_wide_as_its_parts() {
        // padding, icon, gap, 4, the slot, half a padding, the title: 10 + 15 + 6 + 4 + 21 + 5 + 80.
        assert_eq!(super::tab_width(15.0, 80.0, 21.0), 141.0);
    }

    /// The tab pages' strip as `show_tabline` lays it out: "×" only on the tab under the
    /// pointer, a dot on a modified one otherwise, the accent only while focused, long titles
    /// cut to their share.
    #[test]
    fn the_tab_pages_strip() {
        use crate::style;
        use egui_kittest::Harness;
        let mut results = egui_kittest::SnapshotResults::new();
        for (theme, chrome, _) in crate::style::tests::themes() {
            for (name, focused, hover) in [("focused", true, None), ("unfocused", false, None), ("hover", true, Some(Pos2::new(120.0, 14.0)))] {
                let mut harness = Harness::builder().with_size(Vec2::new(520.0, 28.0)).with_pixels_per_point(2.0).with_render_cursor(false).wgpu().build_ui(move |ui| {
                    let screen = ui.ctx().content_rect();
                    ui.painter().rect_filled(screen, 0.0, chrome.bg);
                    let painter = ui.painter().clone();
                    painter.rect_filled(screen, 0.0, chrome.panel);
                    let font = egui::FontId::proportional(12.0);
                    let look = super::TabLook { chrome: &chrome, content_bg: chrome.bg, focused, height: screen.height() };
                    let icon_w = super::file_icon_size(&font);
                    let titles = ["main.rs", "a_rather_long_file_name_for_a_tab.rs", "notes.md"];
                    let title_max = ((screen.width() - screen.height()) / 3.0 - super::tab_width(icon_w, 0.0, style::button_size(screen.height()))).max(font.size * 1.5);
                    let mut x = screen.min.x;
                    for (n, t) in titles.iter().enumerate() {
                        let color = chrome.text(n == 0 && focused);
                        let title = super::cut_title(&painter, t, &font, color, title_max);
                        let rect = Rect::from_min_size(Pos2::new(x, screen.min.y), Vec2::new(super::tab_width(icon_w, title.size().x, style::button_size(screen.height())), screen.height()));
                        x = rect.max.x;
                        // The first is current; the third has unsaved changes.
                        let dot = (n == 2).then_some(color);
                        super::paint_tab(&painter, &look, rect, n == 0, title, color, icon_w, hover, dot, &mut |p, icon, color| {
                            p.rect_filled(icon, style::RADIUS, color);
                        });
                    }
                });
                harness.run();
                harness.snapshot(format!("tab_pages_strip_{name}_{theme}"));
                results.extend_harness(&mut harness);
            }
        }
        results.unwrap();
    }

    #[test]
    fn tab_label_from_winbar() {
        let t = TabLabel::parse(" crates/app/src/app.rs [+] ");
        assert_eq!(t, TabLabel { name: "app.rs".into(), dir: "crates/app/src".into(), modified: true });
        let t = TabLabel::parse("[No Name]");
        assert_eq!(t, TabLabel { name: "[No Name]".into(), dir: String::new(), modified: false });
    }
}
