//! The title bar and the status bar.

use super::*;

impl VimApp {
    pub(super) fn paint_statuslines(&mut self, painter: &egui::Painter, cell_pos: &dyn Fn(i64, i64) -> Pos2, screen: Rect) {
        self.branch_rect = None;
        self.native_bar_rect = None;
        if !self.statusline_gui && !self.sidebar_hides_statusline {
            return;
        }
        let native = self.native_status();
        let m = self.renderer.metrics();
        let Some((rows, cols)) = self.editor.grids.get(&DEFAULT_GRID).map(|g| (g.height(), g.width())) else { return };
        let cursor_win = self.editor.windows.get(&self.editor.cursor.grid).map(|w| w.win);
        for seg in self.editor.status_segments(self.laststatus == 3 || native) {
            let origin = cell_pos(seg.row as i64, seg.col_start as i64);
            let mut bar = Rect::from_min_size(origin, Vec2::new((seg.col_end - seg.col_start) as f32 * m.width, m.height));
            // Over the margin before the first column.
            if seg.col_start == 0 {
                bar.min.x = screen.min.x;
            }
            if seg.row + 1 == rows {
                // Keep the bar one row tall at the window's bottom edge; the
                // leftover pixels above it get the editor (theme) background.
                let gap = screen.max.y - bar.max.y;
                if gap > 0.0 {
                    let strip = Rect::from_min_max(Pos2::new(bar.min.x, bar.min.y), Pos2::new(screen.max.x, bar.min.y + gap));
                    painter.rect_filled(strip, 0.0, rgb(self.editor.default_colors.bg));
                    // Under each window ending just above the bar, continue
                    // its own background (e.g. the darker terminal panel),
                    // over half the separator column on either side (as
                    // `paint_separators` splits it), and the separator's line
                    // between two of them.
                    let mid = |sep: i64| cell_pos(seg.row as i64, sep).x + m.width - (m.width / 2.0).round();
                    let ending: Vec<&grid::Window> = self
                        .editor
                        .windows
                        .values()
                        .filter(|w| !w.hidden && w.kind == WindowKind::Normal && w.row + w.height as i64 == seg.row as i64)
                        .collect();
                    for w in &ending {
                        let Some(bg) = self
                            .editor
                            .grids
                            .get(&w.grid)
                            .and_then(|g| g.cell(g.height().checked_sub(1)?, g.width().checked_sub(1)?))
                            .map(|c| rgb(self.editor.hl_attr(c.hl_id).colors(&self.editor.default_colors).bg))
                        else {
                            continue;
                        };
                        let x0 = if w.col > 0 { mid(w.col - 1) } else { screen.min.x };
                        let x1 = if w.col as usize + w.width >= cols { screen.max.x } else { mid(w.col + w.width as i64) };
                        painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, strip.y_range()), 0.0, bg);
                    }
                    let base = self.editor.grids.get(&DEFAULT_GRID);
                    for l in &ending {
                        let sep = l.col + l.width as i64;
                        if !ending.iter().any(|r| r.col == sep + 1) {
                            continue;
                        }
                        let cell = base.and_then(|g| g.cell(seg.row.checked_sub(1)?, sep as usize));
                        if let Some(cell) = cell {
                            let line = rgb(self.editor.hl_attr(cell.hl_id).colors(&self.editor.default_colors).fg);
                            painter.vline(mid(sep) + 0.5, strip.y_range(), egui::Stroke::new(style::HAIRLINE, line));
                        }
                    }
                    bar = bar.translate(Vec2::new(0.0, gap));
                }
            }
            if seg.col_end == cols {
                bar.max.x = screen.max.x;
            }
            let sidebar = seg.win.filter(|w| self.sidebar_wins.contains(w));
            if let (Some(win), true) = (sidebar, self.sidebar_hides_statusline) {
                // Continue the panel's background over its statusline.
                let bg = self
                    .editor
                    .windows
                    .values()
                    .find(|w| w.win == win)
                    .and_then(|w| self.editor.grids.get(&w.grid))
                    .and_then(|g| g.cell(g.height().checked_sub(1)?, g.width().checked_sub(1)?))
                    .map(|c| rgb(self.editor.hl_attr(c.hl_id).colors(&self.editor.default_colors).bg))
                    .unwrap_or(rgb(self.editor.default_colors.bg));
                painter.rect_filled(bar, 0.0, bg);
            } else if native {
                // The app's own bar: what Neovim drew on this row isn't read.
                self.paint_native_bar(painter, bar);
            } else if self.statusline_gui {
                let active = seg.win.is_none() || seg.win == cursor_win;
                let colors = self.status_bar_colors();
                let uniform = colors.map(|c| if active { c.active } else { c.inactive });
                let mode_color = (active && self.highlight_mode).then(|| mode_color(&self.editor.mode));
                let top_border = colors.map(|c| c.border);
                // On a single-color bar, the statusline's file-type glyph is
                // replaced by the same icon the explorer and tabs use.
                let filetype = (active && uniform.is_some()).then(|| self.active_filetype.clone());
                let look = StatusLook { uniform, font_size: self.status_font_size, mode_color, top_border, filetype };
                let cols = (seg.col_start, seg.col_end);
                let info = self.renderer.paint_bar(painter, &self.editor, DEFAULT_GRID, seg.row, cols, bar, look);
                let slot = info.and_then(|i| i.icon_slot);
                if let (Some(slot), Some(_), Some(file)) = (slot, uniform, self.active_file.clone()) {
                    // Leave a little room on both sides within the slot.
                    let size = (self.status_font_size * 1.2).min(slot.width() - 6.0).max(8.0).round();
                    let icon_rect = Rect::from_center_size(slot.center(), Vec2::splat(size));
                    self.paint_file_icon(painter, &file, icon_rect);
                }
                if let (Some(info), true) = (info, active) {
                    let fg = uniform.map_or(rgb(self.editor.default_colors.fg), |(_, fg)| fg);
                    let at = Pos2::new(info.left_end + 18.0, bar.center().y);
                    let problems = self.paint_problems(painter, at, fg);
                    self.problems_rect = Some(problems);
                    self.paint_progress(painter, Pos2::new(problems.max.x + 18.0, bar.center().y), bar.max.x, fg);
                }
            }
        }
    }

    /// Work in progress after the problems indicator: a small bar (filled to
    /// the percentage, or a moving segment when there is none) and, for
    /// LSP work, "server: title message". From LSP progress, else an OSC
    /// 9;4 progress bar.
    pub(super) fn paint_progress(&self, painter: &egui::Painter, left_center: Pos2, max_x: f32, fg: egui::Color32) {
        let (percent, text) = match (&self.lsp_progress, self.tty_progress) {
            (Some(p), _) => {
                let words = [p.title.as_str(), p.message.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
                (p.percent, if words.is_empty() { p.source.clone() } else { format!("{}: {words}", p.source) })
            }
            (None, Some(percent)) => (percent, String::new()),
            (None, None) => return,
        };
        let chrome = Chrome::from_editor(&self.editor);
        let accent = chrome.accent;
        let (w, h) = (self.status_font_size * 3.5, 4.0);
        let track = Rect::from_min_size(Pos2::new(left_center.x, left_center.y - h / 2.0), Vec2::new(w, h));
        painter.rect_filled(track, style::round(h), chrome.shade(chrome.panel, style::ON));
        let fill = match percent {
            Some(p) => Rect::from_min_size(track.min, Vec2::new(w * p as f32 / 100.0, h)),
            None => {
                // A third of the track sliding across, over and over.
                let t = (painter.ctx().input(|i| i.time) * 0.8).fract() as f32;
                let x = track.min.x + (w * 4.0 / 3.0) * t - w / 3.0;
                Rect::from_x_y_ranges(x.max(track.min.x)..=(x + w / 3.0).min(track.max.x), track.y_range())
            }
        };
        if fill.width() > 0.0 {
            painter.rect_filled(fill, style::round(h), accent);
        }
        if percent.is_none() {
            painter.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
        if !text.is_empty() {
            let mut font = self.grid_fonts.sidebar_font_id(false);
            font.size = self.status_font_size;
            let x = track.max.x + 8.0;
            let room = ((max_x - x - 200.0) / (font.size * 0.55)).max(8.0) as usize;
            let text = match percent {
                Some(p) => format!("{} {p}%", truncate_chars(&text, room)),
                None => truncate_chars(&text, room),
            };
            painter.text(Pos2::new(x, left_center.y), egui::Align2::LEFT_CENTER, text, font, chrome.dim(fg));
        }
    }

    /// VS Code-style problems indicator: the error and warning icons (VS
    /// Code's) with counts, the icons colored when non-zero (the Problems
    /// list's colors). Returns its rect (clickable).
    pub(super) fn paint_problems(&mut self, painter: &egui::Painter, left_center: Pos2, fg: egui::Color32) -> Rect {
        let totals = self.decos.totals();
        let colors = self.problem_colors();
        let mut font = self.grid_fonts.sidebar_font_id(false);
        font.size = self.status_font_size;
        status::paint_problem_counts(painter, &mut self.icons, left_center, fg, &font, (totals.errors, totals.warnings), [colors[0], colors[1]])
    }

    /// Status bar colors for the current theme; `None` keeps the
    /// statusline's own highlights.
    pub(super) fn status_bar_colors(&self) -> Option<StatusColors> {
        match self.status_colors {
            StatusColorMode::Theme => None,
            StatusColorMode::Fixed(c) => Some(c),
            StatusColorMode::Panel => {
                let chrome = Chrome::from_editor(&self.editor);
                Some(StatusColors {
                    active: (chrome.panel, chrome.fg),
                    inactive: (chrome.panel, chrome.dim(chrome.fg)),
                    border: chrome.line(),
                })
            }
        }
    }

    /// Neovim's 'title' if it sets one, else VS Code style
    /// "file — project" from the current file and the explorer root.
    pub(super) fn display_title(&self) -> String {
        // A panel has the keyboard: what is in it, "terminal — project".
        if let Some(label) = self.focus_label() {
            let project = self.explorer.root().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
            return match project {
                Some(p) => format!("{label} — {p}"),
                None => label,
            };
        }
        if !self.title.is_empty() {
            return self.title.clone();
        }
        let name = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
        let file = self.active_file.as_deref().and_then(name);
        let project = self.explorer.root().and_then(name);
        match (file, project) {
            (Some(f), Some(p)) => format!("{f} — {p}"),
            (Some(f), None) => f,
            (None, Some(p)) => p,
            (None, None) => "Eshvi".to_owned(),
        }
    }

    /// The title bar is shown (not full screen): with `tabs = "wide"` or `"compact"` it
    /// holds the tab pages as pills, else they have their strip.
    pub(super) fn tabs_in_titlebar(&self, ctx: &egui::Context) -> bool {
        self.titlebar.is_some() && !ctx.input(|i| i.viewport().fullscreen).unwrap_or(false)
    }

    pub(super) fn show_titlebar(&mut self, ui: &mut egui::Ui) {
        let Some(height) = self.titlebar else { return };
        if ui.ctx().input(|i| i.viewport().fullscreen).unwrap_or(false) {
            return;
        }
        let chrome = Chrome::from_editor(&self.editor);
        let panel = chrome.panel;
        let font = {
            let mut f = self.grid_fonts.sidebar_font_id(false);
            f.size = (height * 0.5).clamp(10.0, 14.0);
            f
        };
        // Pills: the tab pages (not with the strip), or with one, the window's title (with
        // the strip: plain text, no pill or "+").
        let strip = self.config.window.tabs == crate::config::TitleTabs::Strip;
        let pills: Vec<TitlePill> = if self.editor.tabs.len() >= 2 && !strip {
            self.editor
                .tabs
                .iter()
                .map(|(tab, name)| TitlePill {
                    tab: Some(*tab),
                    label: std::path::Path::new(name)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "[No Name]".into()),
                    file: std::path::PathBuf::from(name),
                    current: *tab == self.editor.current_tab,
                })
                .collect()
        } else {
            vec![TitlePill {
                tab: None,
                label: self.display_title(),
                file: self.active_file.clone().unwrap_or_default(),
                current: true,
            }]
        };
        let wide = self.config.window.tabs == crate::config::TitleTabs::Wide;
        // Layout toggles at the right, like VS Code's: the explorer, the
        // bottom panel (whichever view it shows) and the agent panel; lit
        // while shown.
        let toggles: Vec<(LayoutToggle, bool)> = [
            (LayoutToggle::Explorer, self.explorer_enabled, self.explorer.visible),
            (LayoutToggle::Panel, self.panel_enabled, self.bottom_shown()),
            (LayoutToggle::Agent, self.config.agent.enabled, self.agent_shown()),
        ]
        .into_iter()
        .filter(|(_, enabled, _)| *enabled)
        .map(|(t, _, on)| (t, on))
        .collect();
        let mut clicked = None;
        let mut tab_command = None;
        let mut icons = Vec::new();
        let mut bar_painter = None;
        egui::Panel::top("eshvi-titlebar")
            .exact_size(height)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(panel))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                let resp = ui.interact(rect, ui.scope_id().with("drag"), egui::Sense::CLICK | egui::Sense::DRAG);
                if resp.drag_started() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
                }
                if resp.double_clicked() {
                    let maximized = ui.ctx().input(|i| i.viewport().maximized).unwrap_or(false);
                    ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }
                let p = ui.painter();
                bar_painter = Some(p.clone());
                // Everything on the traffic lights' middle (moved to the
                // bar's, `center_traffic_lights`, unless AppKit wouldn't).
                let middle = self.lights_middle.map_or(rect.center().y, |m| rect.min.y + m);
                let size = Vec2::new((height * 0.85).round(), style::button_size(height));
                if self.grid_fonts.is_live(ui.ctx()) {
                    // Between the traffic lights and the toggles.
                    let end = rect.max.x - 8.0 - toggles.len() as f32 * (size.x + 2.0) - 12.0;
                    let room = (middle - rect.min.y).min(rect.max.y - middle) * 2.0;
                    let area = Rect::from_min_max(Pos2::new(rect.min.x + 78.0, middle - room / 2.0), Pos2::new(end, middle + room / 2.0));
                    // From the editor's left edge, as the text below them (last frame's;
                    // none yet: from the traffic lights).
                    let start = if self.grid_area.min.x.is_finite() { self.grid_area.min.x } else { area.min.x };
                    if strip {
                        // The tab pages are in their strip: the title as plain text.
                        title_text(ui.painter(), rect.center().x, area, &pills[0].label, &font, chrome.text(false));
                    } else {
                        let (command, pill_icons) = title_tabs(ui, area, start, &pills, wide, font.clone(), &chrome);
                        tab_command = command;
                        icons = pill_icons;
                    }
                }
                let mut x = rect.max.x - 8.0;
                for &(toggle, on) in toggles.iter().rev() {
                    let r = Rect::from_center_size(Pos2::new(x - size.x / 2.0, middle), size);
                    x -= size.x + 2.0;
                    let resp = ui
                        .interact(r, ui.scope_id().with(("toggle", toggle as u8)), egui::Sense::CLICK)
                        .on_hover_text(toggle.tooltip());
                    style::paint_button_fill(ui.painter(), r, &chrome, panel, resp.hovered(), false);
                    let color = chrome.text(on || resp.hovered());
                    paint_layout_toggle(ui.painter(), toggle, r.center(), (height * 0.5).round(), on, color);
                    if resp.clicked() {
                        clicked = Some(toggle);
                    }
                }
            });
        match clicked {
            Some(LayoutToggle::Explorer) => self.explorer_command("toggle"),
            Some(LayoutToggle::Panel) => self.bridge.send(UiCommand::Command("EshviPanel toggle".into())),
            Some(LayoutToggle::Agent) => self.bridge.send(UiCommand::Command("EshviAgent toggle".into())),
            None => {}
        }
        // (The icons need `self`, which the panel's closure borrows.)
        if let Some(painter) = bar_painter {
            for (rect, path, mono) in icons {
                match mono {
                    Some(tint) => self.paint_file_icon_mono(&painter, &path, rect, tint),
                    None => self.paint_file_icon(&painter, &path, rect),
                }
            }
        }
        if let Some(c) = tab_command {
            self.bridge.send(UiCommand::Command(c));
        }
    }
}

/// The window's title as plain text on the bar: centered on `center_x` (the window's
/// middle) where it fits in `area`, else moved into it, and cut with "…" when even
/// `area` is too narrow.
fn title_text(painter: &egui::Painter, center_x: f32, area: Rect, title: &str, font: &egui::FontId, color: egui::Color32) {
    let galley = super::tabs::cut_title(painter, title, font, color, area.width().max(0.0));
    let w = galley.size().x;
    let left = (center_x - w / 2.0).min(area.max.x - w).max(area.min.x);
    let y = area.center().y - galley.size().y / 2.0;
    painter.with_clip_rect(area).galley(Pos2::new(left, y), galley, color);
}

/// The least room the title bar's pills keep, however far right the editor starts.
const MIN_ROOM: f32 = 240.0;

/// A pill in the title bar: a tab page, or (`tab` none) the window's
/// title while there is one.
pub(super) struct TitlePill {
    tab: Option<i64>,
    label: String,
    /// For the icon.
    file: std::path::PathBuf,
    current: bool,
}

/// The title bar's pills in `area`: each a shade lighter than the bar, the
/// current one more, with its icon in color (the others' are dimmed like
/// their names); "×" on the current one and on hover (not on a lone
/// title); "+" after them. From `start_x` (the editor's left edge) as far
/// as `area` allows. `wide`: equal pills sharing the room (up to a
/// width), their icon and name centered while there's room; else as wide as
/// their names. A lone title pill is wide either way. Returns the
/// command a click asked for and where the icons go (painted by the caller,
/// with a tint for a dimmed one).
fn title_tabs(
    ui: &mut egui::Ui,
    area: Rect,
    start_x: f32,
    pills: &[TitlePill],
    wide: bool,
    font: egui::FontId,
    chrome: &Chrome,
) -> (Option<String>, Vec<(Rect, std::path::PathBuf, Option<egui::Color32>)>) {
    let panel = chrome.panel;
    // The pills' room starts at `start_x` (never so late there's no room).
    let area = Rect::from_min_max(Pos2::new(start_x.min(area.max.x - MIN_ROOM).max(area.min.x), area.min.y), area.max);
    let painter = ui.painter().with_clip_rect(area);
    let pill_h = (area.height() - 6.0).max(14.0).round();
    let (pad, icon, close_w, gap) = (style::TAB_PAD, super::tabs::file_icon_size(&font), style::button_size(pill_h), 4.0);
    let radius = crate::popup::settings().corner_radius;
    let lone = pills.len() == 1 && pills[0].tab.is_none();
    let wide = wide || lone;
    let n = pills.len().max(1) as f32;
    // Room for the pills: the "+" goes after them.
    let room = area.width() - pill_h - 2.0 * gap;
    let max_w = if lone { 420.0 } else { 240.0 };
    // Each pill's width before its name, and in wide pills the name's room
    // (the icon and name centered, clear of "×" on both sides).
    let side = close_w + 10.0;
    let wide_w = ((room - (n - 1.0) * gap) / n).min(max_w).floor();
    let name_max = if wide {
        // (Short of room, the name starts at the left, as in compact ones.)
        (wide_w - 2.0 * side - icon - 6.0).max(wide_w - pad - icon - 6.0 - side).max(font.size * 2.0)
    } else {
        ((room - n * (pad + icon + 6.0 + 4.0 + close_w + 6.0 + gap)) / n).clamp(font.size * 2.0, 180.0)
    };
    let laid: Vec<_> = pills
        .iter()
        .map(|p| {
            let color = chrome.text(p.current);
            let mut job = egui::text::LayoutJob::single_section(p.label.clone(), egui::TextFormat::simple(font.clone(), color));
            job.wrap = egui::text::TextWrapping::truncate_at_width(name_max);
            let galley = painter.layout_job(job);
            let w = if wide { wide_w } else { (pad + icon + 6.0 + galley.size().x + 4.0 + close_w + 6.0).round() };
            (galley, color, w)
        })
        .collect();
    let mut x = area.min.x;
    let mut command = None;
    let mut icons = Vec::new();
    for (i, (p, (galley, color, w))) in pills.iter().zip(laid).enumerate() {
        let pill = Rect::from_min_size(Pos2::new(x, area.center().y - pill_h / 2.0), Vec2::new(w, pill_h));
        x = pill.max.x + gap;
        // A lone title isn't a button: dragging it moves the window.
        let resp = p.tab.map(|_| ui.interact(pill, ui.scope_id().with(("tab", i)), egui::Sense::click()));
        let hovered = resp.as_ref().is_some_and(|r| r.hovered());
        // At rest, under the pointer, current: as every fill.
        let level = match (p.current, hovered) {
            (true, _) => style::ON,
            (false, true) => style::HOVER,
            (false, false) => style::SUBTLE,
        };
        let fill = chrome.shade(panel, level);
        painter.rect_filled(pill, radius, fill);
        let content = icon + 6.0 + galley.size().x;
        let left = if wide && content + 2.0 * side <= w { pill.center().x - content / 2.0 } else { pill.min.x + pad };
        let icon_rect = Rect::from_center_size(Pos2::new(left + icon / 2.0, pill.center().y), Vec2::splat(icon));
        // The current one's icon in its colors, the others' in the text's.
        icons.push((icon_rect, p.file.clone(), (!p.current).then_some(color)));
        painter.galley(Pos2::new(icon_rect.max.x + 6.0, pill.center().y - galley.size().y / 2.0), galley, color);
        let (Some(tab), Some(resp)) = (p.tab, resp) else { continue };
        let close = Rect::from_center_size(Pos2::new(pill.max.x - 6.0 - close_w / 2.0, pill.center().y), Vec2::splat(close_w));
        let on_close = resp.hover_pos().is_some_and(|p| close.contains(p));
        if resp.hovered() || p.current {
            style::glyph_button(&painter, Glyph::Cross, close.center(), pill_h, chrome, fill, on_close);
        }
        if resp.middle_clicked() || (resp.clicked() && on_close) {
            command = Some(format!("lua pcall(vim.cmd.tabclose, vim.api.nvim_tabpage_get_number({tab}))"));
        } else if resp.clicked() {
            command = Some(format!("lua pcall(vim.api.nvim_set_current_tabpage, {tab})"));
        }
    }
    // "+": a new tab page.
    let plus = Rect::from_min_size(Pos2::new(x, area.center().y - pill_h / 2.0), Vec2::splat(pill_h));
    let resp = ui.interact(plus, ui.scope_id().with("tab-new"), egui::Sense::click());
    style::glyph_button(&painter, Glyph::Plus, plus.center(), pill_h, chrome, panel, resp.hovered());
    if resp.clicked() {
        command = Some("tabnew".into());
    }
    (command, icons)
}

/// The title bar's layout toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LayoutToggle {
    Explorer,
    Panel,
    Agent,
}

impl LayoutToggle {
    fn tooltip(self) -> &'static str {
        match self {
            Self::Explorer => "Toggle Side Bar (⌘B)",
            Self::Panel => "Toggle Panel (⌘J)",
            Self::Agent => "Toggle Agent (⌥⌘B)",
        }
    }
}

/// A layout toggle's icon, `s` points tall, around `c`: the window with its
/// sidebar, bottom panel or right panel, filled while shown (VS Code's
/// layout icons).
pub(super) fn paint_layout_toggle(p: &egui::Painter, toggle: LayoutToggle, c: Pos2, s: f32, on: bool, color: egui::Color32) {
    let stroke = egui::Stroke::new(style::STROKE, color);
    let frame = Rect::from_center_size(c, Vec2::new(s * 1.15, s)).shrink(0.5);
    p.rect_stroke(frame, style::BAR_RADIUS, stroke, egui::StrokeKind::Inside);
    let part = match toggle {
        LayoutToggle::Explorer => Rect::from_min_max(frame.min, Pos2::new(frame.min.x + frame.width() * 0.36, frame.max.y)),
        LayoutToggle::Panel => Rect::from_min_max(Pos2::new(frame.min.x, frame.min.y + frame.height() * 0.5), frame.max),
        LayoutToggle::Agent => Rect::from_min_max(Pos2::new(frame.max.x - frame.width() * 0.36, frame.min.y), frame.max),
    };
    if on {
        p.rect_filled(part, style::BAR_RADIUS, color);
    } else {
        // The divider ends inside the frame's stroke: drawn to the frame's
        // outer edge, its anti-aliased ends stuck out past the rounded frame.
        let inner = frame.shrink(style::STROKE);
        match toggle {
            LayoutToggle::Explorer => p.vline(part.max.x, inner.y_range(), stroke),
            LayoutToggle::Panel => p.hline(inner.x_range(), part.min.y, stroke),
            LayoutToggle::Agent => p.vline(part.min.x, inner.y_range(), stroke),
        };
    }
}

/// Fixed (theme-independent) color for the statusline mode pill, from the
/// mode name of `mode_change`. Muted to sit on a dark bar.
pub(super) fn mode_color(mode: &str) -> egui::Color32 {
    use egui::Color32;
    if mode.starts_with("insert") {
        Color32::from_rgb(0x2e, 0x7d, 0x4f) // green
    } else if mode.starts_with("visual") || mode.starts_with("select") {
        Color32::from_rgb(0x7a, 0x4f, 0xb0) // purple
    } else if mode.starts_with("replace") {
        Color32::from_rgb(0xa8, 0x44, 0x34) // red
    } else if mode.starts_with("cmdline") {
        Color32::from_rgb(0x9c, 0x78, 0x26) // amber
    } else if mode.starts_with("terminal") {
        Color32::from_rgb(0x4e, 0x5d, 0x6c) // slate
    } else {
        Color32::from_rgb(0x2f, 0x5f, 0x9e) // normal / operator: blue
    }
}

/// Move the traffic-light buttons so their middle is `want` points below
/// the window's top (macOS puts them where its own, 28pt, title bar would
/// have them). AppKit lays them out again on some changes (resizing, full
/// screen), so this runs every frame and moves them only when they're off.
/// Returns where their middle is now (`None` elsewhere or if AppKit
/// doesn't say).
pub(super) fn center_traffic_lights(frame: &eframe::Frame, want: f32) -> Option<f32> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSView, NSWindowButton};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(RawWindowHandle::AppKit(handle)) = frame.window_handle().map(|h| h.as_raw()) else { return None };
        // SAFETY: eframe's content view, alive for this frame, on the main thread.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = view.window()?;
        let height = window.contentView()?.frame().size.height;
        // In the window's coordinates, from its bottom as AppKit counts.
        let middle = |b: &NSView| {
            let r = b.convertRect_toView(b.bounds(), None);
            (height - (r.origin.y + r.size.height / 2.0)) as f32
        };
        let close = window.standardWindowButton(NSWindowButton::CloseButton)?;
        let now = middle(&close);
        if (now - want).abs() < 0.25 {
            return Some(now);
        }
        let down = (want - now) as f64;
        for kind in [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton] {
            let Some(button) = window.standardWindowButton(kind) else { continue };
            let Some(parent) = (unsafe { button.superview() }) else { continue };
            let mut r = button.convertRect_toView(button.bounds(), None);
            r.origin.y -= down;
            let r = parent.convertRect_fromView(r, None);
            button.setFrameOrigin(r.origin);
        }
        Some(middle(&close))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (frame, want);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    fn pill(tab: i64, label: &str, current: bool) -> TitlePill {
        TitlePill { tab: Some(tab), label: label.into(), file: label.into(), current }
    }

    /// The title bar's tab-page pills, compact and wide, with the pointer
    /// on one (its fill and "×") and without.
    #[test]
    fn title_pills() {
        let chrome = Chrome::new(egui::Color32::from_rgb(0x28, 0x2c, 0x34), egui::Color32::from_rgb(0xab, 0xb2, 0xbf), egui::Color32::from_rgb(0x4f, 0xa6, 0xed));
        let mut results = egui_kittest::SnapshotResults::new();
        for (name, wide, hover) in [("compact", false, None), ("wide", true, None), ("hovered", false, Some(Pos2::new(175.0, 16.0)))] {
            let pills = vec![pill(1, "main.rs", false), pill(2, "notes.md", true), pill(3, "Cargo.toml", false)];
            let mut harness = Harness::builder().with_size(Vec2::new(600.0, 32.0)).with_pixels_per_point(2.0).with_render_cursor(false).wgpu().build_ui(move |ui| {
                let rect = ui.max_rect();
                ui.painter().rect_filled(rect, 0.0, chrome.panel);
                let area = Rect::from_min_max(Pos2::new(rect.min.x + 78.0, rect.min.y), Pos2::new(rect.max.x - 80.0, rect.max.y));
                // (From where an editor beside a side bar would start.)
                title_tabs(ui, area, rect.min.x + 160.0, &pills, wide, egui::FontId::proportional(13.0), &chrome);
            });
            if let Some(pos) = hover {
                harness.hover_at(pos);
            }
            harness.run();
            harness.snapshot(format!("title_pills_{name}"));
            results.extend_harness(&mut harness);
        }
        results.unwrap();
    }

    /// With the tab pages in their strip, the title bar's title is plain text: on the
    /// window's middle while it fits, moved clear of the traffic lights when not, cut
    /// with "…" when the bar is too narrow.
    #[test]
    fn title_text() {
        let chrome = Chrome::new(egui::Color32::from_rgb(0x28, 0x2c, 0x34), egui::Color32::from_rgb(0xab, 0xb2, 0xbf), egui::Color32::from_rgb(0x4f, 0xa6, 0xed));
        let mut results = egui_kittest::SnapshotResults::new();
        for (name, width, title) in [("centered", 600.0, "main.rs — eshvi"), ("moved", 300.0, "a_long_file_name.rs — eshvi"), ("cut", 220.0, "a_long_file_name.rs — eshvi")] {
            let mut harness = Harness::builder().with_size(Vec2::new(width, 32.0)).with_pixels_per_point(2.0).with_render_cursor(false).wgpu().build_ui(move |ui| {
                let rect = ui.max_rect();
                ui.painter().rect_filled(rect, 0.0, chrome.panel);
                let area = Rect::from_min_max(Pos2::new(rect.min.x + 78.0, rect.min.y), Pos2::new(rect.max.x - 40.0, rect.max.y));
                super::title_text(ui.painter(), rect.center().x, area, title, &egui::FontId::proportional(13.0), chrome.text(false));
            });
            harness.run();
            harness.snapshot(format!("title_text_{name}"));
            results.extend_harness(&mut harness);
        }
        results.unwrap();
    }
}
