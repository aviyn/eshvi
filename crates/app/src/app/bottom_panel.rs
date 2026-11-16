//! The bottom panel: Problems,
//! drawn by `tree_view` from the diagnostics Neovim reports, and terminals
//! (`crates/term`, drawn by `terminal_view`), under Neovim's grid and as wide as
//! the editor. Its height and what it shows belong to the GUI; `:EshviPanel`,
//! `:EshviTerminal` and `:EshviProblems` only tell it (`gui_panel`).
//!
//! The strip (tabs, buttons, Problems' controls) is `paint_strip`, which has
//! the data it draws and gives back where its buttons are; the keys and
//! clicks that use them are in `events.rs`, the pieces here are the commands.

use std::collections::HashMap;
use std::time::Duration;

use egui::{Align, Color32, FontId, Layout, Rect};

use super::*;
use crate::focus::{self, Dir, Focus, Region};
use crate::problems::{self, Severity};
use crate::terminal_view::{Look, TerminalView};
use crate::tree_view::{self, TreeView};

/// What the panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum View {
    #[default]
    Problems,
    Terminals,
}

/// A terminal tab, as far as its name and place go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TabState {
    pub id: u64,
    /// "Terminal N": the number it was opened as.
    pub seq: usize,
    pub exited: bool,
    /// What runs in it besides its shell, when something does.
    pub foreground: Option<String>,
}

impl TabState {
    /// The tab's name: what runs in it ("cargo"), else "Terminal N".
    pub fn title(&self) -> String {
        match &self.foreground {
            Some(name) if !self.exited && !name.is_empty() => name.clone(),
            _ => format!("Terminal {}", self.seq),
        }
    }
}

/// The terminal tabs, in the order they were opened; the one shown, and the
/// one beside it (a split).
#[derive(Debug, Default)]
pub(super) struct TabSet {
    pub tabs: Vec<TabState>,
    pub current: Option<u64>,
    pub split: Option<u64>,
    seq: usize,
}

impl TabSet {
    pub fn get(&self, id: u64) -> Option<&TabState> {
        self.tabs.iter().find(|t| t.id == id)
    }

    #[cfg(test)]
    pub fn ids(&self) -> Vec<u64> {
        self.tabs.iter().map(|t| t.id).collect()
    }

    /// A new tab, shown (or, `as_split`, beside the shown one).
    pub fn add(&mut self, id: u64, as_split: bool) {
        self.seq += 1;
        self.tabs.push(TabState { id, seq: self.seq, exited: false, foreground: None });
        if as_split && self.current.is_some() {
            self.split = Some(id);
        } else {
            self.current = Some(id);
        }
    }

    /// Show tab `id`: in the main place, or where it already is.
    pub fn select(&mut self, id: u64) {
        if self.get(id).is_none() || self.split == Some(id) || self.current == Some(id) {
            return;
        }
        self.current = Some(id);
    }

    /// The tab that takes the place of the one closed at `index` (its next
    /// neighbour, else the one before).
    fn neighbour(&self, index: usize) -> Option<u64> {
        self.tabs.get(index.min(self.tabs.len().saturating_sub(1))).map(|t| t.id)
    }

    /// Close tab `id`. A closed split just goes; the shown one is replaced by
    /// the split, else by a neighbour. Returns whether any tab is left.
    pub fn remove(&mut self, id: u64) -> bool {
        let Some(at) = self.tabs.iter().position(|t| t.id == id) else { return !self.tabs.is_empty() };
        self.tabs.remove(at);
        if self.split == Some(id) {
            self.split = None;
        }
        if self.current == Some(id) {
            self.current = self.split.take().or_else(|| self.neighbour(at));
        }
        !self.tabs.is_empty()
    }

    /// Put tab `new` where `old` is (a restart): the same place in the order and the same
    /// part of what is shown.
    pub fn replace(&mut self, old: u64, new: u64) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == old) {
            t.id = new;
        }
        for place in [&mut self.current, &mut self.split] {
            if *place == Some(old) {
                *place = Some(new);
            }
        }
    }

    /// The tab `step` places after `from` (wrapping).
    pub fn cycle(&self, from: Option<u64>, step: isize) -> Option<u64> {
        let n = self.tabs.len() as isize;
        if n == 0 {
            return None;
        }
        let at = from.and_then(|f| self.tabs.iter().position(|t| t.id == f)).map_or(if step > 0 { -1 } else { 0 }, |i| i as isize);
        self.tabs.get((at + step).rem_euclid(n) as usize).map(|t| t.id)
    }
}

/// Everything the bottom panel holds.
pub(super) struct BottomPanel {
    pub shown: bool,
    pub view: View,
    /// The panel's height in points, once the user dragged it (or a session
    /// set it); until then `[terminal] height` rows.
    pub height: Option<f32>,
    pub tabs: TabSet,
    pub views: HashMap<u64, TerminalView>,
    pub problems: problems::Model,
    pub tree: TreeView,
    /// The problem rows as last given to the tree, by id.
    rows: HashMap<String, usize>,
    /// The problems changed: the tree is given them again when drawn.
    dirty: bool,
    /// Give the filter field the keyboard when next drawn.
    pub focus_filter: bool,
    /// The panel as last drawn, and its strip's buttons.
    pub rect: Option<Rect>,
    pub hits: StripHits,
    /// The Terminal tabs' names were looked at last.
    polled: Instant,
    /// A drag of the panel's top edge: the pointer's y and the height then.
    pub resize: Option<(f32, f32)>,
    /// A press began in the panel: its release is the panel's too.
    pub pressed: bool,
}

impl BottomPanel {
    pub fn new() -> Self {
        let mut tree = TreeView::new();
        tree.keep_hints = true;
        Self {
            shown: false,
            view: View::Problems,
            height: None,
            tabs: TabSet::default(),
            views: HashMap::new(),
            problems: problems::Model::new(),
            tree,
            rows: HashMap::new(),
            dirty: true,
            focus_filter: false,
            rect: None,
            hits: StripHits::default(),
            polled: Instant::now(),
            resize: None,
            pressed: false,
        }
    }

    /// The shown terminals' ids, left to right.
    pub fn shown_terminals(&self) -> Vec<u64> {
        if self.view != View::Terminals {
            return Vec::new();
        }
        [self.tabs.current, self.tabs.split].into_iter().flatten().collect()
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Hand the tree the rows when the problems or their filters changed.
    fn refresh_tree(&mut self, colors: [u32; 3]) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let rows = self.problems.rows(colors);
        self.rows = rows.problems;
        self.tree.set(nvim_bridge::gui::TreeSet {
            view: "problems".into(),
            nodes: rows.nodes,
            message: self.problems.empty_note().map(str::to_owned),
            merge: true,
            ..Default::default()
        });
    }
}

/// The height of the panel in points: what was set, else `rows` of text and the strip;
/// never under 4 rows or over what leaves 6 rows to the editor.
pub(super) fn clamp_height(wanted: Option<f32>, rows: u32, row: f32, available: f32) -> f32 {
    let min = row * 4.0;
    let max = (available - row * 6.0).max(min);
    wanted.unwrap_or((rows as f32 + 1.0) * row).clamp(min, max)
}

/// The strip's tabs.
pub(super) struct StripTab {
    pub id: u64,
    pub title: String,
    pub exited: bool,
    pub current: bool,
}

/// What the strip draws.
pub(super) struct Strip<'a> {
    pub tabs: &'a [StripTab],
    /// Problems is the view shown.
    pub problems_current: bool,
    /// The panel has the keyboard.
    pub focused: bool,
    /// The color of the panel's content, which the current tab joins.
    pub content_bg: Color32,
    /// Errors and warnings in the workspace: the Problems tab's badge.
    pub badge: (usize, usize),
    /// Errors, warnings and infos in the scope, and which are listed.
    pub counts: [usize; 3],
    pub shown: [bool; 3],
    pub file_scope: bool,
    /// The current file's errors and warnings, and the workspace's.
    pub showing: (usize, usize),
    /// The severities' colors.
    pub colors: [Color32; 3],
}

/// A button of the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    Problems,
    Tab(u64),
    CloseTab(u64),
    New,
    Split,
    /// Hides the panel.
    Close,
    Scope,
    Severity(Severity),
    /// The filter field (egui handles it).
    Filter,
}

/// Where the strip's buttons were drawn.
#[derive(Clone, Debug, Default)]
pub(super) struct StripHits {
    pub strip: Option<Rect>,
    pub tabs: Vec<(Rect, u64)>,
    pub closes: Vec<(Rect, u64)>,
    pub problems: Option<Rect>,
    pub new: Option<Rect>,
    pub split: Option<Rect>,
    pub close: Option<Rect>,
    pub scope: Option<Rect>,
    pub severity: Vec<(Rect, Severity)>,
    pub filter: Option<Rect>,
}

impl StripHits {
    pub fn at(&self, pos: Pos2) -> Option<Hit> {
        let on = |r: &Option<Rect>| r.is_some_and(|r| r.contains(pos));
        if on(&self.split) {
            Some(Hit::Split)
        } else if let Some(&(_, id)) = self.closes.iter().find(|(r, _)| r.contains(pos)) {
            Some(Hit::CloseTab(id))
        } else if let Some(&(_, s)) = self.severity.iter().find(|(r, _)| r.contains(pos)) {
            Some(Hit::Severity(s))
        } else if on(&self.filter) {
            Some(Hit::Filter)
        } else if on(&self.scope) {
            Some(Hit::Scope)
        } else if on(&self.close) {
            Some(Hit::Close)
        } else if on(&self.problems) {
            Some(Hit::Problems)
        } else if on(&self.new) {
            Some(Hit::New)
        } else {
            self.tabs.iter().find(|(r, _)| r.contains(pos)).map(|&(_, id)| Hit::Tab(id))
        }
    }
}

/// Draw the strip: Problems and a tab per terminal (a prompt glyph and the
/// title, dimmed once the process exited, "×" while hovered), "+", the split
/// button, "×" at the right end, and, with Problems shown, its controls: the
/// current-file switch, the severity toggles and room for the filter field.
pub(super) fn paint_strip(
    painter: &egui::Painter,
    strip: Rect,
    model: &Strip,
    chrome: &Chrome,
    font: &FontId,
    icons: &mut IconTextures,
) -> StripHits {
    let mut hits = StripHits { strip: Some(strip), ..Default::default() };
    let (fg, panel) = (chrome.fg, chrome.panel);
    let line = egui::Stroke::new(style::HAIRLINE, chrome.line());
    let painter = painter.with_clip_rect(strip.intersect(painter.clip_rect()));
    painter.rect_filled(strip, 0.0, panel);
    painter.hline(strip.x_range(), strip.min.y + 0.5, line);
    painter.hline(strip.x_range(), strip.max.y - 0.5, line);
    let focused = model.focused;
    let glyph_w = (font.size * 0.9).round();
    let pad = style::TAB_PAD;
    let mut galleys: Vec<_> = model
        .tabs
        .iter()
        .map(|t| {
            let mut color = chrome.text(t.current && focused);
            if t.exited {
                color = chrome.dim(color);
            }
            (t.id, t.current, color, painter.layout_no_wrap(t.title.clone(), font.clone(), color))
        })
        .collect();
    // The Problems tab, and a badge with the workspace's errors and warnings.
    let problems_color = chrome.text(model.problems_current && focused);
    let problems_galley = painter.layout_no_wrap("Problems".into(), font.clone(), problems_color);
    let count = model.badge.0 + model.badge.1;
    let badge_font = style::scaled(font, style::BADGE);
    let badge_galley = (count > 0).then(|| painter.layout_no_wrap(count.to_string(), badge_font, fg));
    let badge_w = badge_galley.as_ref().map_or(0.0, |g| (g.size().x + 10.0).max(g.size().y + 4.0) + 6.0);
    let mut widths = vec![problems_galley.size().x + badge_w + 2.0 * pad];
    // (Room for a "×" shown while hovered.)
    let close_w = style::button_size(strip.height());
    let fixed = glyph_w + style::GAP + 4.0 + close_w + 1.5 * pad;
    widths.extend(galleys.iter().map(|(.., g)| fixed + g.size().x));
    // Too many or too long: the titles give up room, ending in "…", so the
    // tabs end before "+", split and "×".
    let room = strip.max.x - 6.0 - strip.height() * 3.0 - 10.0 - strip.min.x;
    if widths.iter().sum::<f32>() > room && !galleys.is_empty() {
        let title_max = ((room - widths[0]) / galleys.len() as f32 - fixed).max(font.size * 2.0);
        for (i, (.., color, g)) in galleys.iter_mut().enumerate() {
            if g.size().x > title_max {
                let mut job = egui::text::LayoutJob::single_section(g.text().to_owned(), egui::TextFormat::simple(font.clone(), *color));
                job.wrap = egui::text::TextWrapping::truncate_at_width(title_max);
                *g = painter.layout_job(job);
                widths[i + 1] = fixed + g.size().x;
            }
        }
    }
    let (mut tabs, plus) = super::tabs::terminal_tab_layout(strip, &widths, strip.height());
    // Still too many (the titles shrink only so far): the tabs are cut at their room, and
    // "+" and split keep theirs before "×" instead of being drawn over it.
    let tabs_end = strip.min.x + room;
    let plus = if plus.min.x > tabs_end + 2.0 { Rect::from_min_size(Pos2::new(tabs_end + 2.0, strip.min.y), plus.size()) } else { plus };
    let tabs_room = Rect::from_x_y_ranges(strip.min.x..=tabs_end.max(strip.min.x), strip.y_range());
    let tab_painter = painter.with_clip_rect(painter.clip_rect().intersect(tabs_room));
    let problems_rect = tabs.remove(0);
    if model.problems_current {
        style::paint_current_tab(&tab_painter, problems_rect, focused, chrome, model.content_bg);
    }
    let text_y = problems_rect.center().y - problems_galley.size().y / 2.0;
    let text_w = problems_galley.size().x;
    tab_painter.galley(Pos2::new(problems_rect.min.x + pad, text_y), problems_galley, problems_color);
    if let Some(badge) = badge_galley {
        let size = Vec2::new((badge.size().x + 10.0).max(badge.size().y + 4.0), badge.size().y + 2.0);
        let pill = Rect::from_min_size(Pos2::new(problems_rect.min.x + pad + text_w + 6.0, problems_rect.center().y - size.y / 2.0), size);
        let tint = if model.badge.0 > 0 { model.colors[0] } else { model.colors[1] };
        tab_painter.rect_filled(pill, style::round(size.y), mix(panel, tint, style::TINT));
        tab_painter.galley(pill.center() - badge.size() / 2.0, badge, fg);
    }
    style::paint_tab_divider(&tab_painter, problems_rect, chrome);
    hits.problems = Some(problems_rect.intersect(tabs_room));
    for ((id, current, color, galley), rect) in galleys.into_iter().zip(tabs) {
        if current {
            style::paint_current_tab(&tab_painter, rect, focused, chrome, model.content_bg);
        }
        style::paint_tab_divider(&tab_painter, rect, chrome);
        let text_y = rect.center().y - galley.size().y / 2.0;
        // Prompt glyph "›_".
        let c = Pos2::new(rect.min.x + pad + glyph_w * 0.3, rect.center().y);
        let s = glyph_w * 0.22;
        let glyph_color = if current { mix(mode_color("terminal"), fg, style::TINT) } else { color };
        let stroke = egui::Stroke::new(style::STROKE, glyph_color);
        tab_painter.line_segment([c + Vec2::new(-s, -s), c + Vec2::new(0.0, 0.0)], stroke);
        tab_painter.line_segment([c + Vec2::new(0.0, 0.0), c + Vec2::new(-s, s)], stroke);
        tab_painter.hline(c.x + s * 0.6..=c.x + s * 2.4, c.y + s, stroke);
        tab_painter.galley(Pos2::new(rect.min.x + pad + glyph_w + style::GAP, text_y), galley, color);
        // "×" while hovered: closes this terminal.
        if style::hovered(&tab_painter, rect) {
            let c = Pos2::new(rect.max.x - pad * 0.5 - close_w / 2.0, rect.center().y);
            let on = style::hovered(&tab_painter, Rect::from_center_size(c, Vec2::splat(close_w)));
            let surface = style::tab_fill(chrome, model.content_bg, current, focused);
            let close = style::glyph_button(&tab_painter, Glyph::Cross, c, strip.height(), chrome, surface, on);
            if tabs_room.contains_rect(close) {
                hits.closes.push((close, id));
            }
        }
        if rect.min.x < tabs_end {
            hits.tabs.push((rect.intersect(tabs_room), id));
        }
    }
    // "+" button.
    let h = strip.height();
    style::glyph_button(&painter, Glyph::Plus, plus.center(), h, chrome, panel, style::hovered(&painter, plus));
    hits.new = Some(plus);
    // Split: a terminal beside the shown one (a box halved).
    let split = plus.translate(Vec2::new(plus.width(), 0.0));
    let split_hovered = style::hovered(&painter, split);
    let button = Rect::from_center_size(split.center(), Vec2::splat(style::button_size(h)));
    style::paint_button_fill(&painter, button, chrome, panel, split_hovered, false);
    super::tabs::paint_winbar_button(&painter, super::tabs::WinbarButton::PreviewSide, split.center(), (h * 0.5).round(), chrome.text(split_hovered));
    hits.split = Some(split);
    // "×" at the right end: hides the panel.
    let close = Rect::from_min_max(Pos2::new(strip.max.x - 6.0 - h, strip.min.y), Pos2::new(strip.max.x - 6.0, strip.max.y));
    style::glyph_button(&painter, Glyph::Cross, close.center(), h, chrome, panel, style::hovered(&painter, close));
    hits.close = Some(close);

    // Problems' controls, right to left from "×": the current-file switch, the
    // severity toggles, the filter field. A group that would reach the tabs
    // is left out, the field shrinks first.
    if model.problems_current {
        let small = style::scaled(font, style::SMALL);
        let tabs_end = split.max.x + 8.0;
        let (top, bottom) = (strip.min.y + 4.0, strip.max.y - 4.0);
        let toggle = Rect::from_center_size(
            Pos2::new(close.min.x - 4.0 - h / 2.0, strip.center().y),
            Vec2::new((h * 1.1).round(), style::button_size(h)),
        );
        let mut x = toggle.min.x - 4.0;
        if x < tabs_end {
            return hits;
        }
        let toggle_hovered = style::hovered(&painter, toggle);
        style::paint_button_fill(&painter, toggle, chrome, panel, toggle_hovered, model.file_scope);
        style::paint_page(&painter, toggle.center(), (h * 0.5).round(), chrome.text(model.file_scope || toggle_hovered));
        hits.scope = Some(toggle);
        if toggle_hovered {
            let tip = if model.file_scope { "Showing the current file's problems" } else { "Show only the current file's problems" };
            super::tabs::show_strip_tooltip(painter.ctx(), toggle, tip);
        }
        // Severity toggles: the severity's icon (VS Code's) and count, in its
        // color; dimmed while hidden.
        x -= 10.0;
        let size = (small.size * 1.05).round();
        let toggles: Vec<_> = [(Severity::Info, "info"), (Severity::Warn, "warning"), (Severity::Error, "error")]
            .into_iter()
            .map(|(kind, icon)| {
                let i = kind.index();
                let on = model.shown[i];
                let color = if on { model.colors[i] } else { chrome.dim(chrome.dim(model.colors[i])) };
                let galley = painter.layout_no_wrap(model.counts[i].to_string(), small.clone(), color);
                let rect = Rect::from_min_max(Pos2::new(x - galley.size().x - size - 5.0 - 18.0, top), Pos2::new(x, bottom));
                x = rect.min.x - 3.0;
                (rect, kind, icon, on, color, galley)
            })
            .collect();
        if x < tabs_end {
            return hits;
        }
        for (rect, kind, icon, on, color, galley) in toggles {
            if on {
                painter.rect_filled(rect, style::RADIUS, chrome.shade(panel, style::SUBTLE));
            }
            // Centered on the digits (not the text's box, which is taller
            // above them than below).
            let text_pos = Pos2::new(rect.min.x + 9.0 + size + 5.0, rect.center().y - galley.size().y / 2.0);
            let middle = text_pos.y + digits_middle(&galley, small.size);
            icons.paint_codicon(&painter, icon, Pos2::new(rect.min.x + 9.0 + size / 2.0, middle), size, color);
            painter.galley(text_pos, galley, color);
            hits.severity.push((rect, kind));
        }
        // The text filter's field (drawn by egui over this spot), as wide as
        // there's room for, up to 170; with none, how much of the workspace
        // the current file is.
        let field_w = 170.0_f32.min(x - 8.0 - tabs_end);
        if field_w > 60.0 {
            hits.filter = Some(Rect::from_min_max(Pos2::new(x - 8.0 - field_w, top), Pos2::new(x - 8.0, bottom)));
        } else if model.file_scope {
            let note = format!("Showing {} of {}", model.showing.0, model.showing.1);
            let galley = painter.layout_no_wrap(note, small, chrome.faint());
            let left = x - 8.0 - galley.size().x;
            if left >= tabs_end {
                painter.galley(Pos2::new(left, strip.center().y - galley.size().y / 2.0), galley, fg);
            }
        }
    }
    hits
}

/// What a frame of the panel asks the app to do.
#[derive(Default)]
pub(super) struct PanelOut {
    pub pressed: Option<u64>,
    pub links: Vec<(crate::links::Target, Option<std::path::PathBuf>)>,
    pub actions: Vec<tree_view::Action>,
}

impl VimApp {
    /// The panel is on screen.
    pub(super) fn bottom_shown(&self) -> bool {
        self.panel_enabled && self.bottom.shown
    }

    /// Whether `pos` is over the panel.
    pub(super) fn over_bottom_panel(&self, pos: Pos2) -> bool {
        self.panel_enabled && self.bottom.shown && self.bottom.rect.is_some_and(|r| r.contains(pos))
    }

    /// The band along the strip's top edge that resizes the panel.
    pub(super) fn bottom_resize_zone(&self) -> Option<Rect> {
        let strip = self.bottom.hits.strip.filter(|_| self.panel_enabled && self.bottom.shown)?;
        Some(Rect::from_x_y_ranges(strip.x_range(), strip.min.y - 3.0..=strip.min.y + 4.0))
    }

    /// The panel's height in points now.
    pub(super) fn bottom_height(&self, available: f32) -> f32 {
        let row = self.renderer.metrics().height;
        clamp_height(self.bottom.height, self.config.terminal.height.clamp(3, 100), row, available)
    }

    // Focus.

    /// Give the keyboard to `focus`.
    pub(super) fn set_focus(&mut self, focus: Focus) {
        // The editor's place is the diff view while that covers it.
        let focus = if focus == Focus::Editor && self.diff.is_some() { Focus::Diff } else { focus };
        self.focus = focus;
        self.explorer.focused = focus == Focus::Side;
        if focus != Focus::Editor {
            // (The terminal window of `:EshviTermDebug` is one more place the keys can be.)
            if let Some(d) = &mut self.term_debug {
                d.focused = false;
            }
        }
    }

    /// Keep `focus` and the side bar's own flag (which its views set when
    /// clicked into) the same thing.
    pub(super) fn sync_focus(&mut self) {
        if self.explorer.focused && self.focus != Focus::Side {
            self.focus = Focus::Side;
        } else if !self.explorer.focused && self.focus == Focus::Side {
            self.focus = Focus::Editor;
        }
        // The view is closed: the keyboard is Neovim's.
        if self.focus == Focus::Diff && self.diff.is_none() {
            self.focus = Focus::Editor;
        }
        // The panel is gone: the keyboard is Neovim's.
        let gone = match self.focus {
            Focus::Problems | Focus::Terminal(_) => !self.bottom.shown || !self.panel_enabled,
            _ => false,
        };
        if gone {
            self.set_focus(Focus::Editor);
        }
        if let Focus::Terminal(id) = self.focus {
            if !self.bottom.views.contains_key(&id) || !self.bottom.shown_terminals().contains(&id) {
                self.set_focus(Focus::Editor);
            }
        }
        if self.focus == Focus::Problems && self.bottom.view != View::Problems {
            self.set_focus(Focus::Editor);
        }
        if let Focus::Agent(id) = self.focus {
            if !self.config.agent.enabled || !self.agents.shown || self.agents.tabs.current != Some(id) || !self.agents.views.contains_key(&id) {
                self.set_focus(Focus::Editor);
            }
        }
    }

    /// The focus the panel's current view has.
    pub(super) fn panel_focus(&self) -> Focus {
        match (self.bottom.view, self.bottom.tabs.current) {
            (View::Terminals, Some(id)) => Focus::Terminal(id),
            _ => Focus::Problems,
        }
    }

    /// Which regions are on screen.
    fn present(&self) -> focus::Present {
        focus::Present {
            side: self.explorer_enabled && self.explorer.visible,
            panel: self.panel_enabled && self.bottom.shown,
            agent: self.agent_on_screen(),
        }
    }

    /// ⌃h/j/k/l from Neovim's edge (`gui_navigate`).
    pub(super) fn navigate_from_editor(&mut self, dir: Dir) {
        self.navigate_region(Region::Editor, dir);
    }

    /// ⌃h/j/k/l in the side bar, Problems or a terminal.
    pub(super) fn navigate(&mut self, dir: Dir) {
        // Between split terminals first.
        if let Focus::Terminal(id) = self.focus {
            let (cur, split) = (self.bottom.tabs.current, self.bottom.tabs.split);
            match (dir, split) {
                (Dir::Right, Some(s)) if cur == Some(id) => return self.set_focus(Focus::Terminal(s)),
                (Dir::Left, Some(_)) if split == Some(id) => return self.set_focus(Focus::Terminal(cur.unwrap_or(id))),
                _ => {}
            }
        }
        self.navigate_region(self.focus.region(), dir);
    }

    fn navigate_region(&mut self, from: Region, dir: Dir) {
        let Some(to) = focus::neighbour(from, dir, self.present()) else { return };
        match to {
            Region::Side => self.set_focus(Focus::Side),
            Region::Panel => {
                // Coming in from the left: the left terminal; from the right, none yet.
                let mut f = self.panel_focus();
                if let (Focus::Terminal(_), Dir::Left, Some(s)) = (f, dir, self.bottom.tabs.split) {
                    f = Focus::Terminal(s);
                }
                self.set_focus(f);
            }
            Region::Agent => {
                if let Some(id) = self.agents.tabs.current {
                    self.set_focus(Focus::Agent(id));
                }
            }
            Region::Editor => {
                self.set_focus(Focus::Editor);
                self.bridge.send(UiCommand::Command(focus::enter_editor(dir)));
            }
        }
    }

    /// ⌃h/j/k/l pressed while `focus` has the keyboard: whether the GUI takes it
    /// as a move (a terminal whose program keeps those keys is left alone).
    pub(super) fn takes_navigation(&self, key: egui::Key, modifiers: egui::Modifiers) -> Option<Dir> {
        let plain_ctrl = modifiers.ctrl && !modifiers.shift && !modifiers.alt && !modifiers.mac_cmd;
        let dir = Dir::from_key(key).filter(|_| plain_ctrl)?;
        match self.focus {
            Focus::Side | Focus::Problems | Focus::Diff => Some(dir),
            Focus::Terminal(id) => {
                let foreground = self.bottom.views.get(&id).and_then(|v| v.foreground_process());
                (!focus::keeps_ctrl_hjkl(foreground.as_deref(), &self.config.terminal.keep_ctrl_hjkl)).then_some(dir)
            }
            Focus::Agent(id) => {
                let foreground = self.agents.views.get(&id).and_then(|v| v.foreground_process());
                (!focus::keeps_ctrl_hjkl(foreground.as_deref(), &self.config.terminal.keep_ctrl_hjkl)).then_some(dir)
            }
            Focus::Editor => None,
        }
    }

    // Commands.

    /// `:EshviPanel`, `:EshviTerminal` and `:EshviProblems`.
    pub(super) fn panel_command(&mut self, ctx: &egui::Context, kind: &str, args: &[String]) {
        if kind == "agent" {
            return self.agent_command(ctx, args);
        }
        if !self.panel_enabled {
            return;
        }
        let arg = |n: usize| args.get(n).map(String::as_str);
        match kind {
            "panel" => match arg(0).unwrap_or("toggle") {
                "toggle" if self.bottom.shown => self.hide_panel(),
                "toggle" | "show" => self.show_panel(ctx),
                "hide" => self.hide_panel(),
                other => self.editor.push_message("emsg", &format!("EshviPanel: unknown action {other}")),
            },
            "terminal" => self.terminal_command(ctx, arg(0).unwrap_or("toggle"), arg(1)),
            "problems" => self.problems_command(args),
            _ => {}
        }
    }

    /// Show the panel as it was last (Problems the first time: it starts no shell), focused.
    fn show_panel(&mut self, ctx: &egui::Context) {
        if self.bottom.view == View::Terminals && !self.bottom.tabs.tabs.is_empty() {
            self.show_terminals(ctx);
        } else {
            self.show_problems();
        }
    }

    pub(super) fn hide_panel(&mut self) {
        self.bottom.shown = false;
        if self.focus.in_panel() {
            self.set_focus(Focus::Editor);
        }
    }

    /// Show the terminals (a new one when there is none), focused.
    fn show_terminals(&mut self, ctx: &egui::Context) {
        if self.bottom.tabs.tabs.is_empty() && self.new_terminal(ctx, false).is_none() {
            return;
        }
        self.bottom.shown = true;
        self.bottom.view = View::Terminals;
        let id = self.bottom.tabs.current.or_else(|| self.bottom.tabs.tabs.first().map(|t| t.id));
        self.bottom.tabs.current = id;
        if let Some(id) = id {
            self.set_focus(Focus::Terminal(id));
        }
    }

    fn show_problems(&mut self) {
        self.bottom.shown = true;
        self.bottom.view = View::Problems;
        self.set_focus(Focus::Problems);
    }

    fn terminal_command(&mut self, ctx: &egui::Context, action: &str, arg: Option<&str>) {
        let id_arg = arg.and_then(|a| a.parse::<u64>().ok());
        match action {
            // As the toggle of a keyboard shortcut does: shown and not focused, it focuses; focused, it hides.
            "toggle" => {
                if self.bottom.shown && self.bottom.view == View::Terminals {
                    if self.focus.in_panel() {
                        self.hide_panel();
                    } else {
                        let f = self.panel_focus();
                        self.set_focus(f);
                    }
                } else {
                    self.show_terminals(ctx);
                }
            }
            "show" => self.show_terminals(ctx),
            "hide" => {
                if self.bottom.view == View::Terminals {
                    self.hide_panel();
                }
            }
            "new" => {
                self.new_terminal(ctx, false);
            }
            "next" | "prev" => {
                let from = self.bottom.tabs.current;
                if let Some(id) = self.bottom.tabs.cycle(from, if action == "next" { 1 } else { -1 }) {
                    self.select_terminal(id);
                }
            }
            "close" => {
                let id = id_arg.or(match self.focus {
                    Focus::Terminal(id) => Some(id),
                    _ => self.bottom.tabs.current,
                });
                if let Some(id) = id {
                    self.close_terminal(id);
                }
            }
            "select" => {
                if let Some(id) = id_arg {
                    self.select_terminal(id);
                }
            }
            "split" => self.split_terminal(ctx),
            "height" => {
                if let Some(rows) = arg.and_then(|a| a.parse::<f32>().ok()) {
                    self.bottom.height = Some((rows.max(3.0) + 1.0) * self.renderer.metrics().height);
                }
            }
            other => self.editor.push_message("emsg", &format!("EshviTerminal: unknown action {other}")),
        }
    }

    fn problems_command(&mut self, args: &[String]) {
        use problems::Scope;
        let (first, rest) = (args.first().map(String::as_str).unwrap_or("toggle"), args.get(1));
        match first {
            "toggle" => {
                if let Some(scope) = rest.and_then(|s| scope_named(s)) {
                    self.set_scope(scope);
                    self.show_problems();
                } else if self.bottom.shown && self.bottom.view == View::Problems {
                    if self.focus == Focus::Problems {
                        self.hide_panel();
                    } else {
                        self.set_focus(Focus::Problems);
                    }
                } else {
                    self.show_problems();
                }
            }
            "show" | "file" | "workspace" => {
                let scope = if first == "show" { rest.and_then(|s| scope_named(s)) } else { scope_named(first) };
                if let Some(scope) = scope {
                    self.set_scope(scope);
                }
                self.show_problems();
            }
            "hide" => {
                if self.bottom.view == View::Problems {
                    self.hide_panel();
                }
            }
            "severity" => {
                if let Some(s) = rest.and_then(|k| k.chars().next()).and_then(Severity::from_letter) {
                    self.bottom.problems.toggle(s);
                    self.bottom.mark_dirty();
                }
            }
            "filter" => {
                self.bottom.problems.filter = args[1..].join(" ");
                self.bottom.mark_dirty();
            }
            other => self.editor.push_message("emsg", &format!("EshviProblems: unknown action {other}")),
        }
        let _ = Scope::Workspace;
    }

    pub(super) fn set_scope(&mut self, scope: problems::Scope) {
        self.bottom.problems.scope = scope;
        self.bottom.mark_dirty();
    }

    // Terminals.

    /// Start a terminal: the login shell in the explorer's folder. Shown (beside the
    /// shown one with `as_split`), and focused.
    fn new_terminal(&mut self, ctx: &egui::Context, as_split: bool) -> Option<u64> {
        let size = term::Size { cols: 80, rows: 24, cell_w: 8, cell_h: 16 };
        let cwd = self.explorer.root().map(std::path::Path::to_path_buf);
        match self.spawn_terminal(ctx, term::shell_argv(self.config.terminal.shell.as_deref(), &self.config.terminal.shell_args), cwd, Vec::new(), size) {
            Ok(t) => {
                let view = TerminalView::new(t);
                let id = view.id();
                self.bottom.views.insert(id, view);
                self.bottom.tabs.add(id, as_split);
                self.bottom.shown = true;
                self.bottom.view = View::Terminals;
                self.set_focus(Focus::Terminal(id));
                Some(id)
            }
            Err(e) => {
                log::warn!("terminal: {e}");
                self.editor.push_message("emsg", &format!("Could not start a terminal: {e}"));
                None
            }
        }
    }

    /// Start `argv` on a PTY in `cwd`, with the terminals' settings (`[terminal] scrollback`,
    /// `option_as_meta`) and a wake-up for the window: what a terminal of either panel is.
    pub(super) fn spawn_terminal(
        &self,
        ctx: &egui::Context,
        argv: Vec<String>,
        cwd: Option<std::path::PathBuf>,
        env: Vec<(String, String)>,
        size: term::Size,
    ) -> std::io::Result<term::Terminal> {
        let wake = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint()) as term::Wake
        };
        let mut opts = term::SpawnOptions::new(argv, size);
        opts.cwd = cwd;
        opts.env = env;
        opts.scrollback = self.config.terminal.scrollback;
        opts.keys = term::keys::KeyOptions { option_as_meta: self.config.terminal.option_as_meta };
        opts.wake = Some(wake);
        term::Terminal::spawn(opts)
    }

    fn select_terminal(&mut self, id: u64) {
        if self.bottom.tabs.get(id).is_none() {
            return;
        }
        self.bottom.tabs.select(id);
        self.bottom.shown = true;
        self.bottom.view = View::Terminals;
        self.set_focus(Focus::Terminal(id));
    }

    /// A terminal beside the shown one; with one already, it gets the keyboard.
    fn split_terminal(&mut self, ctx: &egui::Context) {
        if !(self.bottom.shown && self.bottom.view == View::Terminals) {
            self.show_terminals(ctx);
        }
        if let Some(s) = self.bottom.tabs.split {
            return self.set_focus(Focus::Terminal(s));
        }
        if self.bottom.tabs.current.is_some() {
            self.new_terminal(ctx, true);
        }
    }

    /// End terminal `id` (hanging up its process). With none left the panel goes.
    pub(super) fn close_terminal(&mut self, id: u64) {
        let was_focused = self.focus == Focus::Terminal(id);
        let left = self.bottom.tabs.remove(id);
        self.bottom.views.remove(&id);
        if !left {
            if self.bottom.view == View::Terminals {
                self.hide_panel();
            }
        } else if was_focused {
            let f = self.panel_focus();
            self.set_focus(f);
        }
    }

    /// Terminals that ended: dimmed in the strip, and closed when the shell
    /// said it ended well (code 0). Names follow what runs in them.
    pub(super) fn tick_terminals(&mut self, ctx: &egui::Context) {
        if self.bottom.views.is_empty() {
            return;
        }
        let poll = self.bottom.polled.elapsed() >= Duration::from_secs(1);
        if poll {
            self.bottom.polled = Instant::now();
        }
        let mut done = Vec::new();
        for tab in &mut self.bottom.tabs.tabs {
            let Some(view) = self.bottom.views.get(&tab.id) else { continue };
            let term = view.terminal();
            if let Some((code, _)) = term.exit_status() {
                tab.exited = true;
                tab.foreground = None;
                if code == Some(0) {
                    done.push(tab.id);
                }
            } else if poll {
                tab.foreground = if term.at_shell_prompt() { None } else { term.foreground_process() };
            }
        }
        for id in done {
            self.close_terminal(id);
        }
        if self.bottom.shown && !self.bottom.views.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    // Problems.

    /// What Neovim reported.
    pub(super) fn set_problems(&mut self, items: &[nvim_bridge::gui::Problem]) {
        self.bottom.problems.set(items);
        self.bottom.mark_dirty();
    }

    /// The file being edited and the root, for the file scope and the folders shown.
    fn sync_problem_context(&mut self) {
        let m = &mut self.bottom.problems;
        let root = self.explorer.root().map(std::path::Path::to_path_buf);
        if m.current != self.active_file || m.root != root {
            m.current = self.active_file.clone();
            m.root = root;
            self.bottom.mark_dirty();
        }
    }

    /// Open problem `id` of the list: the file at that place, in the editor window.
    fn open_problem(&mut self, id: &str) {
        let Some(&i) = self.bottom.rows_index().get(id) else { return };
        let Some(item) = self.bottom.problems.items().get(i) else { return };
        let (file, line, col) = (item.file.clone(), item.lnum, item.col);
        self.open_file_at(&file, line, col);
        self.set_focus(Focus::Editor);
    }

    /// Open `path` in the editor window (Neovim's `Eshvi.editor_window`, as the explorer
    /// opens files), at a line and column (from 1; line 0: where the file has the cursor).
    pub(super) fn open_file_at(&self, path: &std::path::Path, line: usize, col: usize) {
        let code = "local path, line, col = ...
            local win = Eshvi.editor_window()
            if win then vim.api.nvim_set_current_win(win) end
            vim.cmd('drop ' .. vim.fn.fnameescape(path))
            if line > 0 then
                pcall(vim.api.nvim_win_set_cursor, 0, { line, math.max(col - 1, 0) })
                vim.cmd('normal! zz')
            end";
        self.bridge.send(UiCommand::Lua {
            code: code.into(),
            args: vec![RpcValue::from(path.to_string_lossy().as_ref()), RpcValue::from(line as i64), RpcValue::from(col as i64)],
        });
    }

    /// A key while Problems has the keyboard. Returns whether it was taken (`/` and
    /// ⌘F go to the filter; Esc gives the keyboard to the editor).
    pub(super) fn problems_key(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> bool {
        if (key == egui::Key::Slash && !modifiers.any()) || (key == egui::Key::F && modifiers.mac_cmd && !modifiers.shift) {
            self.bottom.focus_filter = true;
            return true;
        }
        let actions = self.bottom.tree.key(key, modifiers);
        for action in actions {
            self.problems_action(action);
        }
        true
    }

    fn problems_action(&mut self, action: tree_view::Action) {
        use tree_view::Action;
        match action {
            Action::Accept { id, .. } => self.open_problem(&id),
            Action::Focus => self.set_focus(Focus::Problems),
            Action::FocusEditor => self.set_focus(Focus::Editor),
            _ => {}
        }
    }

    // Input.

    /// An input event, before it is Neovim's: while Problems or a terminal has the
    /// keyboard its keys are theirs (⌃h/j/k/l move to the next region), and the
    /// pointer over the panel is its own. Returns whether the event was taken.
    pub(super) fn panel_event(&mut self, ctx: &egui::Context, ev: &Event) -> bool {
        match ev {
            Event::Key { key, pressed, modifiers, .. } if self.focus.in_panel() && !self.filter_has_keyboard(ctx) => {
                if let Some(dir) = self.takes_navigation(*key, *modifiers) {
                    // (The terminal draws after this: it must not see the key either.)
                    let key = *key;
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Key { key: k, modifiers, .. } if *k == key && modifiers.ctrl)));
                    if *pressed {
                        self.navigate(dir);
                    }
                    return true;
                }
                if self.focus == Focus::Problems && *pressed {
                    if *key == egui::Key::Slash && !modifiers.any() {
                        // (Not typed into the field it opens.)
                        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Text(t) if t == "/")));
                    }
                    self.problems_key(*key, *modifiers);
                }
                true
            }
            Event::Text(_) | Event::Ime(_) | Event::Paste(_) | Event::Copy | Event::Cut if self.focus.in_panel() && !self.filter_has_keyboard(ctx) => true,
            Event::PointerButton { pos, pressed: true, button, .. } if self.over_bottom_panel(*pos) && !self.over_popup(ctx, *pos) => {
                self.bottom.pressed = true;
                self.panel_press(ctx, *pos, *button);
                true
            }
            Event::PointerButton { pressed: false, .. } if self.bottom.resize.is_some() => {
                self.bottom.resize = None;
                self.bottom.pressed = false;
                true
            }
            Event::PointerButton { pressed: false, .. } if std::mem::take(&mut self.bottom.pressed) => true,
            Event::PointerMoved(pos) if self.bottom.resize.is_some() => {
                if let Some((start_y, start_height)) = self.bottom.resize {
                    self.bottom.height = Some(start_height + (start_y - pos.y));
                }
                true
            }
            Event::PointerMoved(_) if self.bottom.pressed => true,
            Event::MouseWheel { .. } if ctx.input(|i| i.pointer.latest_pos()).is_some_and(|p| self.over_bottom_panel(p) && !self.over_popup(ctx, p)) => true,
            _ => false,
        }
    }

    /// The Problems filter field has the keyboard.
    fn filter_has_keyboard(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(problems_filter_id()))
    }

    /// A press in the panel: its top edge resizes it, a button of the strip does its
    /// thing, anywhere else gives the keyboard to what is under the pointer.
    fn panel_press(&mut self, ctx: &egui::Context, pos: Pos2, button: PointerButton) {
        if button == PointerButton::Primary && self.bottom_resize_zone().is_some_and(|z| z.contains(pos)) {
            let height = self.bottom.rect.map_or(0.0, |r| r.height());
            self.bottom.resize = Some((pos.y, height));
            return;
        }
        let primary = button == PointerButton::Primary;
        match self.bottom.hits.at(pos) {
            Some(Hit::Tab(id)) if button == PointerButton::Middle => self.close_terminal(id),
            Some(Hit::Tab(id)) if primary => self.select_terminal(id),
            Some(Hit::CloseTab(id)) if primary => self.close_terminal(id),
            Some(Hit::Problems) if primary => self.show_problems(),
            Some(Hit::New) if primary => {
                self.new_terminal(ctx, false);
            }
            Some(Hit::Split) if primary => self.split_terminal(ctx),
            Some(Hit::Close) if primary => self.hide_panel(),
            Some(Hit::Scope) if primary => {
                let scope = match self.bottom.problems.scope {
                    problems::Scope::File => problems::Scope::Workspace,
                    problems::Scope::Workspace => problems::Scope::File,
                };
                self.set_scope(scope);
            }
            Some(Hit::Severity(s)) if primary => {
                self.bottom.problems.toggle(s);
                self.bottom.mark_dirty();
            }
            // Clicked into: egui focuses the field.
            Some(Hit::Filter) => self.set_focus(Focus::Problems),
            Some(_) => {}
            None => {
                let in_strip = self.bottom.hits.strip.is_some_and(|r| r.contains(pos));
                let focus = match (self.bottom.view, in_strip) {
                    (View::Terminals, false) => self.terminal_at(pos).map_or_else(|| self.panel_focus(), Focus::Terminal),
                    (View::Terminals, true) => self.panel_focus(),
                    (View::Problems, _) => Focus::Problems,
                };
                self.set_focus(focus);
            }
        }
    }

    /// The shown terminal under `pos`.
    fn terminal_at(&self, pos: Pos2) -> Option<u64> {
        let rect = self.bottom.rect?;
        let ids = self.bottom.shown_terminals();
        match (ids.as_slice(), pos.x < rect.center().x) {
            ([a, _], true) => Some(*a),
            ([_, b], false) => Some(*b),
            ([a], _) => Some(*a),
            _ => None,
        }
    }

    // Drawing.

    /// Draw the panel into `rect` (strip, then its view) and take what it asks for.
    pub(super) fn show_bottom_panel(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        self.bottom.rect = Some(rect);
        self.sync_problem_context();
        let m = self.renderer.metrics();
        let chrome = Chrome::from_editor(&self.editor);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, chrome.panel);
        let strip = Rect::from_min_size(rect.min, Vec2::new(rect.width(), m.height));
        let content = Rect::from_min_max(Pos2::new(rect.min.x, strip.max.y), rect.max);
        let colors = self.problem_colors();
        let problems = &self.bottom.problems;
        let tabs: Vec<StripTab> = self
            .bottom
            .tabs
            .tabs
            .iter()
            .map(|t| StripTab {
                id: t.id,
                title: t.title(),
                exited: t.exited,
                current: self.bottom.view == View::Terminals && self.bottom.shown_terminals().contains(&t.id),
            })
            .collect();
        let model = Strip {
            tabs: &tabs,
            problems_current: self.bottom.view == View::Problems,
            focused: self.focus.in_panel(),
            content_bg: chrome.panel,
            badge: problems.badge(),
            counts: problems.counts(),
            shown: problems.shown,
            file_scope: problems.scope == problems::Scope::File,
            showing: problems.showing(),
            colors,
        };
        let mut font = self.grid_fonts.sidebar_font_id(false);
        font.size = self.status_font_size + 1.0;
        self.bottom.hits = paint_strip(&painter, strip, &model, &chrome, &font, &mut self.icons);

        let mut out = PanelOut::default();
        let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
        content_ui.set_clip_rect(content.intersect(ui.clip_rect()));
        match self.bottom.view {
            View::Problems => self.show_problems_list(&mut content_ui, content, &chrome, &mut out),
            View::Terminals => self.show_terminals_view(&mut content_ui, content, &chrome, &mut out),
        }
        // The filter field over its spot in the strip, taking the keyboard when asked.
        if let Some(r) = self.bottom.hits.filter.filter(|_| self.bottom.view == View::Problems) {
            if std::mem::take(&mut self.bottom.focus_filter) {
                ctx.memory_mut(|mem| mem.request_focus(problems_filter_id()));
            }
            let mut text = std::mem::take(&mut self.bottom.problems.filter);
            let (mut changed, escaped) = self.problems_filter_field(&ctx, r, &mut text);
            if escaped {
                text.clear();
                changed = true;
                ctx.memory_mut(|mem| mem.surrender_focus(problems_filter_id()));
            }
            self.bottom.problems.filter = text;
            if changed {
                self.bottom.mark_dirty();
            }
        }
        // The resize cursor over the top edge.
        let hover = ctx.input(|i| i.pointer.hover_pos());
        if self.bottom.resize.is_some() || hover.is_some_and(|p| self.bottom_resize_zone().is_some_and(|z| z.contains(p))) {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        // What the views asked for.
        if let Some(id) = out.pressed {
            self.set_focus(Focus::Terminal(id));
        }
        for (target, dir) in out.links {
            self.open_term_link(&ctx, target, dir);
        }
        for action in out.actions {
            self.problems_action(action);
        }
    }

    fn show_problems_list(&mut self, ui: &mut egui::Ui, content: Rect, chrome: &Chrome, out: &mut PanelOut) {
        let colors = self.problem_colors().map(|c| u32::from_be_bytes([0, c.r(), c.g(), c.b()]));
        self.bottom.refresh_tree(colors);
        let explorer_colors = ExplorerColors::from_chrome(chrome);
        // The list's font: the sidebar's, or the editor's (`[terminal] problems_font`).
        let (font, font_size, row_h) = if self.problems_proportional {
            let mut font = self.grid_fonts.sidebar_font_id(false);
            font.size = self.explorer.font_size;
            (font, self.explorer.font_size, side_bar::row_height(self.explorer.font_size))
        } else {
            let (font, _) = self.grid_fonts.font_id(false, false);
            (font, self.grid_fonts.size, self.renderer.metrics().height)
        };
        let look = tree_view::Look { colors: &explorer_colors, font, font_size, row_h };
        let focused = self.focus == Focus::Problems;
        out.actions = self.bottom.tree.show(ui, &look, focused, content.height());
    }

    fn show_terminals_view(&mut self, ui: &mut egui::Ui, content: Rect, chrome: &Chrome, out: &mut PanelOut) {
        let look = Look {
            fonts: self.grid_fonts.terminal(),
            linespace: self.linespace,
            fg: rgb(self.editor.default_colors.fg),
            bg: chrome.panel,
            accent: chrome.accent,
            ansi: self.editor.term_colors,
            option_as_meta: self.config.terminal.option_as_meta,
            blink: self.blink.enabled,
            copy_on_select: self.config.terminal.copy_on_select,
            middle_click_paste: self.config.terminal.middle_click_paste,
        };
        let ids = self.bottom.shown_terminals();
        let width = if ids.len() == 2 { ((content.width() - style::HAIRLINE) / 2.0).floor() } else { content.width() };
        for (n, id) in ids.iter().enumerate() {
            let x = content.min.x + n as f32 * (width + style::HAIRLINE);
            let r = Rect::from_min_size(Pos2::new(x, content.min.y), Vec2::new(if n == 1 { content.max.x - x } else { width }, content.height()));
            let Some(view) = self.bottom.views.get_mut(id) else { continue };
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            child.set_clip_rect(r.intersect(ui.clip_rect()));
            let o = view.show(&mut child, &look, self.focus == Focus::Terminal(*id));
            if o.pressed {
                out.pressed = Some(*id);
            }
            if let Some(target) = o.link {
                out.links.push((target, view.link_dir()));
            }
            if n == 1 {
                ui.painter().vline(x - style::HAIRLINE / 2.0, r.y_range(), egui::Stroke::new(style::HAIRLINE, chrome.line()));
            }
        }
    }

    // The status bar and the window title follow the focus.

    /// The focused terminal's tab title, or "Problems", while the panel has the keyboard.
    pub(super) fn focus_label(&self) -> Option<String> {
        match self.focus {
            Focus::Terminal(id) => self.bottom.tabs.get(id).map(TabState::title),
            Focus::Agent(id) => self.agents.data.get(&id).map(|t| t.shown_title().to_owned()),
            Focus::Problems => Some("Problems".into()),
            _ => None,
        }
    }

    /// A terminal has the keyboard.
    pub(super) fn terminal_focused(&self) -> bool {
        self.focus.in_terminal()
    }

    /// The focused panel terminal (the bottom panel's or the agent's).
    pub(super) fn focused_terminal(&self) -> Option<&TerminalView> {
        match self.focus {
            Focus::Terminal(id) => self.bottom.views.get(&id),
            Focus::Agent(id) => self.agents.views.get(&id),
            _ => None,
        }
    }

    /// Edit › Find and friends: the focused terminal's find bar, or Problems' filter.
    pub(super) fn panel_find(&mut self, action: &str) {
        match self.focus {
            Focus::Terminal(id) | Focus::Agent(id) => {
                let views = if self.focus.in_agent() { &mut self.agents.views } else { &mut self.bottom.views };
                if let Some(v) = views.get_mut(&id) {
                    match action {
                        "find" => v.open_find(),
                        _ => {
                            v.terminal().search_step(action == "find_next");
                        }
                    }
                }
            }
            Focus::Problems if action == "find" => self.bottom.focus_filter = true,
            _ => {}
        }
    }

    // Sessions.

    /// The panel's part of a session.
    pub(super) fn panel_session(&self) -> Option<nvim_bridge::gui::PanelSession> {
        self.panel_enabled.then(|| nvim_bridge::gui::PanelSession {
            shown: self.bottom.shown,
            height: self.bottom.rect.map_or_else(|| self.bottom.height.unwrap_or(0.0), |r| r.height()).round(),
            view: match self.bottom.view {
                View::Problems => "problems",
                View::Terminals => "terminal",
            }
            .into(),
        })
    }

    /// A loaded session's panel: shown as it was, a terminal view as Problems
    /// (terminals aren't restored).
    pub(super) fn apply_panel_session(&mut self, p: &nvim_bridge::gui::PanelSession) {
        if !self.panel_enabled {
            return;
        }
        self.bottom.view = View::Problems;
        if p.height >= 1.0 {
            self.bottom.height = Some(p.height);
        }
        if p.shown {
            self.bottom.shown = true;
        } else {
            self.hide_panel();
        }
    }
}

impl BottomPanel {
    /// The problem rows given to the tree, by id.
    fn rows_index(&self) -> &HashMap<String, usize> {
        &self.rows
    }
}

fn scope_named(name: &str) -> Option<problems::Scope> {
    match name {
        "file" => Some(problems::Scope::File),
        "workspace" => Some(problems::Scope::Workspace),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(n: u64) -> TabSet {
        let mut t = TabSet::default();
        for id in 1..=n {
            t.add(id * 10, false);
        }
        t
    }

    #[test]
    fn a_tab_is_named_after_what_runs_in_it_else_by_its_number() {
        let mut t = TabState { id: 1, seq: 3, exited: false, foreground: None };
        assert_eq!(t.title(), "Terminal 3");
        t.foreground = Some("cargo".into());
        assert_eq!(t.title(), "cargo");
        t.exited = true;
        assert_eq!(t.title(), "Terminal 3");
        t.exited = false;
        t.foreground = Some(String::new());
        assert_eq!(t.title(), "Terminal 3");
    }

    #[test]
    fn tabs_are_numbered_as_they_are_opened_and_the_newest_is_shown() {
        let mut t = tabs(3);
        assert_eq!(t.current, Some(30));
        assert_eq!(t.tabs.iter().map(|t| t.seq).collect::<Vec<_>>(), [1, 2, 3]);
        // Closing one doesn't renumber; the next is still one more.
        t.remove(20);
        t.add(40, false);
        assert_eq!(t.get(40).unwrap().seq, 4);
        assert_eq!(t.ids(), [10, 30, 40]);
    }

    #[test]
    fn closing_the_shown_tab_shows_its_neighbour() {
        let mut t = tabs(4);
        t.select(20);
        assert!(t.remove(20));
        assert_eq!(t.current, Some(30));
        // The last one: the one before.
        t.select(40);
        t.remove(40);
        assert_eq!(t.current, Some(30));
        // A tab that isn't shown goes without changing what is.
        t.remove(10);
        assert_eq!(t.current, Some(30));
        assert!(!t.remove(30));
        assert_eq!(t.current, None);
        // (An unknown tab changes nothing.)
        assert!(!t.remove(99));
    }

    #[test]
    fn a_split_is_beside_the_shown_tab_and_closing_either_keeps_the_other() {
        let mut t = tabs(2);
        t.add(30, true);
        assert_eq!((t.current, t.split), (Some(20), Some(30)));
        assert!(t.remove(30));
        assert_eq!((t.current, t.split), (Some(20), None));
        t.add(40, true);
        // The shown one closes: the split takes its place.
        t.remove(20);
        assert_eq!((t.current, t.split), (Some(40), None));
        // With nothing shown, a "split" is just the shown one.
        let mut empty = TabSet::default();
        empty.add(1, true);
        assert_eq!((empty.current, empty.split), (Some(1), None));
    }

    #[test]
    fn selecting_a_tab_in_the_split_leaves_both_where_they_are() {
        let mut t = tabs(2);
        t.add(30, true);
        t.select(30);
        assert_eq!((t.current, t.split), (Some(20), Some(30)));
        t.select(10);
        assert_eq!((t.current, t.split), (Some(10), Some(30)));
        t.select(99);
        assert_eq!(t.current, Some(10));
    }

    #[test]
    fn a_restarted_tab_takes_the_place_of_the_old_one() {
        let mut t = tabs(3);
        t.select(20);
        t.replace(20, 99);
        assert_eq!(t.ids(), [10, 99, 30]);
        assert_eq!((t.current, t.split), (Some(99), None));
        // A split too; an unknown tab changes nothing.
        t.add(40, true);
        t.replace(40, 41);
        t.replace(7, 8);
        assert_eq!((t.current, t.split), (Some(99), Some(41)));
        assert_eq!(t.ids(), [10, 99, 30, 41]);
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let t = tabs(3);
        assert_eq!(t.cycle(Some(30), 1), Some(10));
        assert_eq!(t.cycle(Some(10), -1), Some(30));
        assert_eq!(t.cycle(Some(10), 1), Some(20));
        // From nothing (Problems is shown): the first, or the last.
        assert_eq!(t.cycle(None, 1), Some(10));
        assert_eq!(t.cycle(None, -1), Some(30));
        assert_eq!(TabSet::default().cycle(None, 1), None);
    }

    #[test]
    fn the_height_is_rows_until_dragged_and_stays_between_its_limits() {
        // 12 rows of text and the strip, 20 points each.
        assert_eq!(clamp_height(None, 12, 20.0, 800.0), 260.0);
        assert_eq!(clamp_height(Some(300.0), 12, 20.0, 800.0), 300.0);
        // Not under 4 rows, nor so tall that the editor has under 6.
        assert_eq!(clamp_height(Some(10.0), 12, 20.0, 800.0), 80.0);
        assert_eq!(clamp_height(Some(790.0), 12, 20.0, 800.0), 680.0);
        // A window too short for both: the minimum.
        assert_eq!(clamp_height(None, 12, 20.0, 100.0), 80.0);
    }

    #[test]
    fn strip_buttons_are_found_where_they_were_drawn() {
        let r = |x: f32, w: f32| Rect::from_min_size(Pos2::new(x, 0.0), Vec2::new(w, 20.0));
        let hits = StripHits {
            tabs: vec![(r(60.0, 80.0), 7)],
            closes: vec![(r(120.0, 18.0), 7)],
            problems: Some(r(0.0, 60.0)),
            new: Some(r(140.0, 20.0)),
            split: Some(r(160.0, 20.0)),
            close: Some(r(400.0, 20.0)),
            scope: Some(r(300.0, 20.0)),
            severity: vec![(r(200.0, 40.0), Severity::Error)],
            filter: Some(r(250.0, 40.0)),
            ..Default::default()
        };
        let at = |x: f32| hits.at(Pos2::new(x, 10.0));
        assert_eq!(at(10.0), Some(Hit::Problems));
        assert_eq!(at(70.0), Some(Hit::Tab(7)));
        // The "×" over the tab wins.
        assert_eq!(at(125.0), Some(Hit::CloseTab(7)));
        assert_eq!(at(150.0), Some(Hit::New));
        assert_eq!(at(170.0), Some(Hit::Split));
        assert_eq!(at(210.0), Some(Hit::Severity(Severity::Error)));
        assert_eq!(at(260.0), Some(Hit::Filter));
        assert_eq!(at(310.0), Some(Hit::Scope));
        assert_eq!(at(410.0), Some(Hit::Close));
        assert_eq!(at(350.0), None);
    }

    fn model<'a>(tabs: &'a [StripTab], problems_current: bool, file_scope: bool) -> Strip<'a> {
        Strip {
            tabs,
            problems_current,
            focused: true,
            content_bg: Color32::TRANSPARENT,
            badge: (2, 1),
            counts: [2, 1, 4],
            shown: [true, true, false],
            file_scope,
            showing: (1, 3),
            colors: [Color32::from_rgb(0xf1, 0x4c, 0x4c), Color32::from_rgb(0xcc, 0xa7, 0x00), Color32::from_rgb(0x37, 0x94, 0xff)],
        }
    }

    fn some_tabs() -> Vec<StripTab> {
        vec![
            StripTab { id: 1, title: "Terminal 1".into(), exited: false, current: false },
            StripTab { id: 2, title: "cargo".into(), exited: false, current: true },
            StripTab { id: 3, title: "Terminal 3".into(), exited: true, current: false },
        ]
    }

    /// The strip with Problems shown and with a terminal; wide and narrow; dark and light.
    #[test]
    fn the_strip() {
        use egui_kittest::Harness;
        let mut results = egui_kittest::SnapshotResults::new();
        for (theme, chrome, _) in crate::style::tests::themes() {
            for (name, width, problems_current, file_scope) in
                [("problems", 760.0, true, false), ("problems_file", 760.0, true, true), ("terminals", 760.0, false, false), ("narrow", 420.0, true, false)]
            {
                let mut harness = Harness::builder().with_size(Vec2::new(width, 28.0)).with_pixels_per_point(2.0).with_render_cursor(false).wgpu().build_ui(move |ui| {
                    let screen = ui.ctx().content_rect();
                    ui.painter().rect_filled(screen, 0.0, chrome.bg);
                    let tabs = some_tabs();
                    let mut icons = IconTextures::default();
                    let m = model(&tabs, problems_current, file_scope);
                    paint_strip(ui.painter(), screen, &m, &chrome, &FontId::proportional(12.0), &mut icons);
                });
                harness.run();
                harness.snapshot(format!("bottom_strip_{name}_{theme}"));
                results.extend_harness(&mut harness);
            }
        }
        results.unwrap();
    }

    #[test]
    fn the_strip_reports_its_buttons_and_leaves_room_for_the_filter() {
        use egui_kittest::Harness;
        let chrome = crate::style::tests::themes()[0].1;
        let hits_at = |width: f32, problems_current: bool| {
            let out = std::rc::Rc::new(std::cell::RefCell::new(StripHits::default()));
            let sink = out.clone();
            let mut harness = Harness::builder().with_size(Vec2::new(width, 28.0)).build_ui(move |ui| {
                let tabs = some_tabs();
                let mut icons = IconTextures::default();
                let m = model(&tabs, problems_current, false);
                *sink.borrow_mut() = paint_strip(ui.painter(), ui.ctx().content_rect(), &m, &chrome, &FontId::proportional(12.0), &mut icons);
            });
            harness.run();
            let hits = out.borrow().clone();
            hits
        };
        let wide = hits_at(760.0, true);
        assert_eq!(wide.tabs.len(), 3);
        assert!(wide.problems.is_some() && wide.new.is_some() && wide.split.is_some() && wide.close.is_some());
        assert_eq!(wide.severity.len(), 3);
        assert!(wide.scope.is_some() && wide.filter.is_some());
        // The controls don't overlap the tabs, "+" or split.
        let tabs_end = wide.split.unwrap().max.x;
        assert!(wide.filter.unwrap().min.x > tabs_end && wide.severity.iter().all(|(r, _)| r.min.x > tabs_end));
        // With a terminal shown, Problems' controls aren't there.
        let term = hits_at(760.0, false);
        assert!(term.scope.is_none() && term.filter.is_none() && term.severity.is_empty());
        // A narrow strip drops the field, then the toggles.
        let narrow = hits_at(430.0, true);
        assert!(narrow.filter.is_none());
        let tiny = hits_at(300.0, true);
        assert!(tiny.severity.is_empty() && tiny.new.is_some());
        // Too narrow even for shortened titles (seen in the VM): the tabs are cut, and "+",
        // split and "×" keep their places, apart and inside the strip.
        for width in [300.0, 220.0] {
            let h = hits_at(width, false);
            let (new, split, close) = (h.new.unwrap(), h.split.unwrap(), h.close.unwrap());
            assert!(new.max.x <= split.min.x + 0.5 && split.max.x < close.min.x, "{width}: {new:?} {split:?} {close:?}");
            assert!(close.max.x <= width);
            assert!(h.tabs.iter().all(|(r, _)| r.max.x <= new.min.x), "{width}: tabs end before +");
        }
    }

    fn problem(file: &str, lnum: usize, severity: u8, message: &str, source: &str, code: &str) -> nvim_bridge::gui::Problem {
        nvim_bridge::gui::Problem {
            file: file.into(),
            lnum,
            col: 5,
            end_lnum: lnum,
            end_col: 9,
            severity,
            message: message.into(),
            source: source.into(),
            code: code.into(),
        }
    }

    /// The Problems list in the panel: rows, a selected row lit while focused, a long
    /// message that keeps its place visible, and the empty note.
    #[test]
    fn the_problems_list() {
        use egui_kittest::Harness;
        let mut results = egui_kittest::SnapshotResults::new();
        for (theme, chrome, _) in crate::style::tests::themes() {
            for (name, width, focused, empty) in [("wide", 640.0, true, false), ("unfocused", 640.0, false, false), ("narrow", 300.0, true, false), ("empty", 300.0, false, true)] {
                let mut model = problems::Model::new();
                model.root = Some("/p".into());
                if !empty {
                    model.set(&[
                        problem("/p/src/main.rs", 12, 1, "mismatched types: expected `u8`, found `String` in this very long message", "rust-analyzer", "E0308"),
                        problem("/p/src/main.rs", 30, 2, "unused variable: `x`", "rustc", ""),
                        problem("/p/lib.lua", 3, 3, "global `vim`", "", ""),
                    ]);
                }
                let mut tree = TreeView::new();
                tree.keep_hints = true;
                let rows = model.rows([0xf14c4c, 0xcca700, 0x3794ff]);
                tree.set(nvim_bridge::gui::TreeSet { nodes: rows.nodes, message: model.empty_note().map(str::to_owned), merge: true, ..Default::default() });
                if !empty {
                    tree.key(egui::Key::ArrowDown, egui::Modifiers::NONE);
                    tree.key(egui::Key::ArrowDown, egui::Modifiers::NONE);
                }
                let tabs = some_tabs();
                let mut harness = Harness::builder()
                    .with_size(Vec2::new(width, 150.0))
                    .with_pixels_per_point(2.0)
                    .with_accessibility_check(false)
                    .wgpu()
                    .with_context_setup(crate::scm_view::tests::system_font)
                    .build_ui_state(
                        move |ui, tree: &mut TreeView| {
                            let colors = ExplorerColors::from_chrome(&chrome);
                            let screen = ui.ctx().content_rect();
                            ui.painter().rect_filled(screen, 0.0, chrome.panel);
                            let strip = Rect::from_min_size(screen.min, Vec2::new(screen.width(), 24.0));
                            let m = model_for(&tabs);
                            paint_strip(ui.painter(), strip, &m, &chrome, &FontId::proportional(12.0), &mut IconTextures::default());
                            let content = Rect::from_min_max(Pos2::new(screen.min.x, strip.max.y), screen.max);
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content));
                            let look = tree_view::Look { colors: &colors, font: FontId::proportional(13.0), font_size: 13.0, row_h: side_bar::row_height(13.0) };
                            tree.show(&mut child, &look, focused, content.height());
                        },
                        tree,
                    );
                harness.run();
                harness.snapshot(format!("bottom_problems_{name}_{theme}"));
                results.extend_harness(&mut harness);
            }
        }
        results.unwrap();
    }

    fn model_for(tabs: &[StripTab]) -> Strip<'_> {
        model(tabs, true, false)
    }
}
