//! The native status bar: drawn from the app's own data (the mode and file
//! `status.lua` reports, the repository, the problem counts, the cursor's
//! place) instead of from what Neovim drew on the statusline row. Used with
//! the own config and `[statusline] gui = true`; otherwise `paint_bar` restyles
//! the statusline as before.
//!
//! Left: mode pill, branch, file, problem counts. Right: file type, progress
//! (Top, Bot, n%), line:col. `fit` decides what room each gets.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2};
use nvim_bridge::gui::Status;

use super::*;
use crate::config::StatusColors;
use crate::git::Repo;
use crate::icons::{IconTextures, IconTheme};

/// Space before the first and after the last item.
const EDGE: f32 = style::TAB_PAD;
/// Space between items.
const GAP: f32 = style::GAP * 2.0;
/// Space inside the mode pill, each side.
const PILL_PAD: f32 = style::GAP;
/// How far a hovered item's fill reaches beyond its content, each side.
const HOVER_PAD: f32 = style::GAP - 1.0;
/// Characters a shrunk name keeps room for ("…" and a couple of letters).
const MIN_CHARS: f32 = 2.5;

/// Whether the bar is drawn by the app: with the own config (whose
/// `gui_status` the bar needs) and the GUI statusline on.
pub(super) fn use_native(own_config: bool, gui: bool) -> bool {
    own_config && gui
}

/// The mode's name on the pill, from `nvim_get_mode().mode`.
pub(super) fn mode_label(mode: &str) -> &'static str {
    let mut chars = mode.chars();
    match chars.next() {
        Some('n') if mode.starts_with("no") => "O-PENDING",
        Some('v') => "VISUAL",
        Some('V') => "V-LINE",
        Some('\x16') => "V-BLOCK",
        Some('s') => "SELECT",
        Some('S') => "S-LINE",
        Some('\x13') => "S-BLOCK",
        Some('i') => "INSERT",
        Some('R') if mode.starts_with("Rv") => "V-REPLACE",
        Some('R') => "REPLACE",
        Some('c') => "COMMAND",
        Some('t') => "TERMINAL",
        _ => "NORMAL",
    }
}

/// The name `mode_color` knows the mode by (the UI's `mode_change` names).
pub(super) fn mode_kind(mode: &str) -> &'static str {
    match mode_label(mode) {
        "INSERT" => "insert",
        "VISUAL" | "V-LINE" | "V-BLOCK" => "visual",
        "SELECT" | "S-LINE" | "S-BLOCK" => "select",
        "REPLACE" | "V-REPLACE" => "replace",
        "COMMAND" => "cmdline",
        "TERMINAL" => "terminal",
        _ => "normal",
    }
}

/// Where the cursor is in the file, as lualine's progress shows it: Top on
/// the first line (so in a one-line file), Bot on the last, else the percent.
/// `line:col`, the column in characters as Neovim reports it (`viewport_col`, in
/// bytes, is only for a terminal, where the cursor moves without telling).
pub(super) fn position_text(line: i64, viewport_col: i64, status: &nvim_bridge::gui::Status) -> String {
    let col = if status.col > 0 && status.buftype != "terminal" { status.col } else { viewport_col };
    format!("{line}:{col}")
}

pub(super) fn progress_text(line: i64, total: i64) -> String {
    if line <= 1 {
        "Top".into()
    } else if line >= total {
        "Bot".into()
    } else {
        format!("{}%", line * 100 / total)
    }
}

/// What the bar calls a repository's HEAD: the branch, else the short hash
/// when detached; `None` with neither.
pub(super) fn branch_label(repo: &Repo) -> Option<String> {
    repo.branch.clone().or_else(|| repo.head.as_ref().map(|h| h.chars().take(7).collect()))
}

/// The file's name with its flags: ` [+]` modified, ` [-]` readonly or not
/// modifiable. `[No Name]` for none.
pub(super) fn file_label(name: &str, modified: bool, readonly: bool) -> String {
    let mut out = if name.is_empty() { "[No Name]".to_owned() } else { name.to_owned() };
    if modified {
        out.push_str(" [+]");
    }
    if readonly {
        out.push_str(" [-]");
    }
    out
}

/// The bar's items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    Mode,
    Branch,
    File,
    Counts,
    Filetype,
    Progress,
    Position,
}

/// An item and the room it'd like; it can give up room down to `min`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Want {
    pub slot: Slot,
    pub width: f32,
    pub min: f32,
}

/// Where an item goes, and how wide it is allowed to be (less than it
/// wanted: it is cut with "…").
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placed {
    pub slot: Slot,
    pub x: f32,
    pub width: f32,
}

fn run(items: &[Want]) -> f32 {
    items.iter().map(|w| w.width).sum::<f32>() + GAP * items.len().saturating_sub(1) as f32
}

/// Lay the items out in a bar `width` wide, left items from the left edge,
/// right ones from the right, never overlapping. The right items stay as
/// long as they fit (the file type goes first, then the progress); the left
/// ones give room in the order file name (cut with "…", then gone), branch,
/// problem counts, mode.
pub(super) fn fit(left: &[Want], right: &[Want], width: f32) -> Vec<Placed> {
    let mut right = right.to_vec();
    while right.len() > 1 && run(&right) > width - 2.0 * EDGE {
        right.remove(0);
    }
    let right_start = width - EDGE - run(&right);
    let room = if right.is_empty() { width - 2.0 * EDGE } else { right_start - GAP - EDGE };
    let mut left = left.to_vec();
    while run(&left) > room {
        let over = run(&left) - room;
        let Some(i) = [Slot::File, Slot::Branch, Slot::Counts, Slot::Mode].iter().find_map(|s| left.iter().position(|w| w.slot == *s))
        else {
            break;
        };
        if left[i].width - over >= left[i].min {
            left[i].width -= over;
        } else {
            left.remove(i);
        }
    }
    let mut out = Vec::new();
    for (items, start) in [(&left, EDGE), (&right, right_start)] {
        let mut x = start;
        for w in items {
            out.push(Placed { slot: w.slot, x, width: w.width });
            x += w.width + GAP;
        }
    }
    out
}

/// What the bar shows.
pub(super) struct Bar {
    pub mode: &'static str,
    /// The pill's color; `None`: the mode in plain text.
    pub mode_color: Option<Color32>,
    pub branch: Option<String>,
    pub file: String,
    pub problems: (u32, u32),
    pub problem_colors: [Color32; 2],
    /// The file type's name and its icon.
    pub filetype: Option<(String, String)>,
    pub progress: String,
    pub position: String,
}

/// The bar's colors.
pub(super) struct Palette {
    pub bg: Color32,
    pub fg: Color32,
    pub border: Color32,
    pub chrome: Chrome,
}

impl Palette {
    pub fn new(colors: Option<StatusColors>, chrome: Chrome) -> Self {
        match colors {
            Some(c) => Self { bg: c.active.0, fg: c.active.1, border: c.border, chrome },
            None => Self { bg: chrome.panel, fg: chrome.fg, border: chrome.line(), chrome },
        }
    }
}

/// What the painted bar offers the app.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Hits {
    /// Clickable: the branch (shows Source Control), the problem counts.
    pub branch: Option<Rect>,
    pub counts: Option<Rect>,
    /// Where the left items end and the right ones begin (the room between
    /// them is free).
    pub left_end: f32,
    pub right_start: f32,
}

/// The error and warning counts' size: an icon, the number, and the next.
fn counts_icon_size(font: &FontId) -> f32 {
    (font.size * 1.05).round()
}

/// The width of what `paint_problem_counts` draws.
pub(super) fn problem_counts_width(p: &Painter, font: &FontId, counts: (u32, u32)) -> f32 {
    let size = counts_icon_size(font);
    let widths: Vec<f32> =
        [counts.0, counts.1].iter().map(|n| size + style::GAP / 1.5 + p.layout_no_wrap(n.to_string(), font.clone(), Color32::WHITE).size().x).collect();
    widths.iter().sum::<f32>() + style::TAB_PAD * (widths.len() - 1) as f32
}

/// The error and warning icons with their counts, the icons and numbers
/// colored when above zero (the Problems list's colors), from `left_center`.
/// Returns the rect that takes the click.
pub(super) fn paint_problem_counts(
    p: &Painter,
    icons: &mut IconTextures,
    left_center: Pos2,
    fg: Color32,
    font: &FontId,
    counts: (u32, u32),
    colors: [Color32; 2],
) -> Rect {
    let size = counts_icon_size(font);
    let mut x = left_center.x;
    let y = left_center.y;
    let mut right = x;
    for (icon, count, color) in [("error", counts.0, colors[0]), ("warning", counts.1, colors[1])] {
        let color = if count > 0 { color } else { fg };
        let galley = p.layout_no_wrap(count.to_string(), font.clone(), color);
        let text_pos = Pos2::new(x + size + style::GAP / 1.5, y - galley.size().y / 2.0);
        let middle = text_pos.y + digits_middle(&galley, font.size);
        icons.paint_codicon(p, icon, Pos2::new(x + size / 2.0, middle), size, color);
        right = text_pos.x + galley.size().x;
        p.galley(text_pos, galley, color);
        x = right + style::TAB_PAD;
    }
    let pad = style::GAP / 1.5;
    Rect::from_min_max(Pos2::new(left_center.x - pad, y - size / 2.0 - pad), Pos2::new(right + pad, y + size / 2.0 + pad))
}

/// Draw the bar in `rect` (one row, at the window's bottom edge).
pub(super) fn paint(p: &Painter, rect: Rect, bar: &Bar, pal: &Palette, font: &FontId, icons: &mut IconTextures) -> Hits {
    p.rect_filled(rect, 0.0, pal.bg);
    p.hline(rect.x_range(), rect.min.y + 0.5, Stroke::new(style::HAIRLINE, pal.border));
    let p = &p.with_clip_rect(rect.intersect(p.clip_rect()));
    let cy = rect.center().y;
    let icon = counts_icon_size(font);
    let icon_gap = style::GAP / 1.5;
    let text = |s: &str, color: Color32| p.layout_no_wrap(s.to_owned(), font.clone(), color);
    let min_text = font.size * MIN_CHARS;

    let mode_text_color = if bar.mode_color.is_some() { Color32::WHITE } else { pal.fg };
    let mode = text(bar.mode, mode_text_color);
    // (Items that can't be cut have `min` = `width`.)
    let whole = |slot, width| Want { slot, width, min: width };
    let mut left = vec![whole(Slot::Mode, mode.size().x + 2.0 * PILL_PAD)];
    if let Some(branch) = &bar.branch {
        let w = icon + icon_gap + text(branch, pal.fg).size().x;
        left.push(Want { slot: Slot::Branch, width: w, min: (icon + icon_gap + min_text).min(w) });
    }
    let file_w = text(&bar.file, pal.fg).size().x;
    left.push(Want { slot: Slot::File, width: file_w, min: min_text.min(file_w) });
    left.push(whole(Slot::Counts, problem_counts_width(p, font, bar.problems)));
    let mut right = Vec::new();
    if let Some((name, _)) = &bar.filetype {
        right.push(whole(Slot::Filetype, icon + icon_gap + text(name, pal.fg).size().x));
    }
    for (slot, s) in [(Slot::Progress, &bar.progress), (Slot::Position, &bar.position)] {
        if !s.is_empty() {
            right.push(whole(slot, text(s, pal.fg).size().x));
        }
    }

    let placed = fit(&left, &right, rect.width());
    let at = |slot: Slot| placed.iter().find(|pl| pl.slot == slot).map(|pl| (rect.min.x + pl.x, pl.width));
    let fill_rect = |x: f32, w: f32| {
        Rect::from_x_y_ranges((x - HOVER_PAD)..=(x + w + HOVER_PAD), (rect.min.y + style::HAIRLINE + style::GAP / 2.0)..=(rect.max.y - style::GAP / 2.0))
    };
    let mut hits = Hits { left_end: rect.min.x + EDGE, right_start: rect.max.x - EDGE, ..Hits::default() };
    // `text` cut to `width` with "…".
    let cut = |s: &str, width: f32, color: Color32| {
        let mut job = egui::text::LayoutJob::single_section(s.to_owned(), egui::TextFormat::simple(font.clone(), color));
        job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
        p.layout_job(job)
    };
    let centered = |x: f32, g: &std::sync::Arc<egui::Galley>| Pos2::new(x, cy - g.size().y / 2.0);

    if let Some((x, w)) = at(Slot::Mode) {
        let h = (font.size * 1.45).min(rect.height() - 2.0);
        let pill = Rect::from_center_size(Pos2::new(x + w / 2.0, cy), Vec2::new(w, h));
        if let Some(color) = bar.mode_color {
            p.rect_filled(pill, style::RADIUS, color);
        }
        // All capitals: centered on their height, not on the line's.
        let pos = Pos2::new(x + PILL_PAD, cy - digits_middle(&mode, font.size));
        p.galley(pos, mode, mode_text_color);
        hits.left_end = x + w;
    }
    if let (Some(branch), Some((x, w))) = (&bar.branch, at(Slot::Branch)) {
        let rect_hit = fill_rect(x, w);
        style::paint_button_fill(p, rect_hit, &pal.chrome, pal.bg, style::hovered(p, rect_hit), false);
        style::paint_icon(p, style::Icon::Branch, Pos2::new(x + icon / 2.0, cy), icon, pal.fg);
        let g = cut(branch, w - icon - icon_gap, pal.fg);
        p.galley(centered(x + icon + icon_gap, &g), g, pal.fg);
        hits.branch = Some(rect_hit);
        hits.left_end = x + w;
    }
    if let Some((x, w)) = at(Slot::File) {
        let g = cut(&bar.file, w, pal.fg);
        p.galley(centered(x, &g), g, pal.fg);
        hits.left_end = x + w;
    }
    if let Some((x, w)) = at(Slot::Counts) {
        let rect_hit = fill_rect(x, w);
        style::paint_button_fill(p, rect_hit, &pal.chrome, pal.bg, style::hovered(p, rect_hit), false);
        paint_problem_counts(p, icons, Pos2::new(x, cy), pal.fg, font, bar.problems, bar.problem_colors);
        hits.counts = Some(rect_hit);
        hits.left_end = x + w;
    }
    if let (Some((name, file_icon)), Some((x, _))) = (&bar.filetype, at(Slot::Filetype)) {
        let px = (icon * p.ctx().pixels_per_point()).round() as u32;
        if let Some(tex) = icons.get(p.ctx(), file_icon, px) {
            let r = Rect::from_center_size(Pos2::new(x + icon / 2.0, cy), Vec2::splat(icon));
            p.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        let g = text(name, pal.fg);
        p.galley(centered(x + icon + icon_gap, &g), g, pal.fg);
        hits.right_start = hits.right_start.min(x);
    }
    for (slot, s) in [(Slot::Progress, &bar.progress), (Slot::Position, &bar.position)] {
        if let Some((x, _)) = at(slot) {
            p.text(Pos2::new(x, cy), Align2::LEFT_CENTER, s, font.clone(), pal.fg);
            hits.right_start = hits.right_start.min(x);
        }
    }
    hits
}

impl VimApp {
    /// Whether the status bar is drawn from the app's own data.
    pub(super) fn native_status(&self) -> bool {
        use_native(self.own_config, self.statusline_gui)
    }

    /// What `status.lua` last reported, applied.
    pub(super) fn set_status(&mut self, status: Status) {
        self.status = status;
    }

    /// The view of the window the cursor is in, or the last such window's
    /// while it is in a float or the command line.
    fn status_viewport(&mut self) -> Option<grid::Viewport> {
        let current = self.editor.windows.get(&self.editor.cursor.grid).filter(|w| w.kind == WindowKind::Normal);
        if let Some(view) = current.and_then(|w| w.viewport) {
            self.status_view = Some(view);
        }
        self.status_view
    }

    /// The file's name as the bar shows it: a panel terminal has its tab's title (`focus_label`);
    /// a `:terminal` buffer and the quickfix list have their kind.
    fn status_file(&self) -> String {
        if let Some(label) = self.focus_label() {
            return label;
        }
        let s = &self.status;
        let name = match s.buftype.as_str() {
            "terminal" => "Terminal".into(),
            "quickfix" => "Quickfix List".into(),
            _ => s.name.clone(),
        };
        if matches!(s.buftype.as_str(), "terminal" | "quickfix") {
            return name;
        }
        file_label(&name, s.modified, s.readonly)
    }

    /// The branch, when the file is in the repository (or has no file).
    fn status_branch(&self) -> Option<String> {
        let repo = self.scm.repo()?;
        let inside = self.status.path.is_empty() || std::path::Path::new(&self.status.path).starts_with(&repo.top);
        inside.then(|| branch_label(repo)).flatten()
    }

    pub(super) fn paint_native_bar(&mut self, painter: &Painter, bar_rect: Rect) {
        let chrome = Chrome::from_editor(&self.editor);
        let pal = Palette::new(self.status_bar_colors(), chrome);
        let mut font = self.grid_fonts.sidebar_font_id(false);
        font.size = self.status_font_size;
        let view = self.status_viewport();
        let totals = self.decos.totals();
        let colors = self.problem_colors();
        let filetype = {
            let s = &self.status;
            let shown = !s.filetype.is_empty() && !matches!(s.buftype.as_str(), "terminal" | "quickfix") && !self.focus.in_panel() && !self.focus.in_agent();
            shown.then(|| {
                let file = std::path::Path::new(&s.path).file_name().map(|n| n.to_string_lossy().into_owned());
                let icon = IconTheme::get().file_icon(&file.unwrap_or_else(|| format!("x.{}", s.filetype))).to_owned();
                (s.filetype.clone(), icon)
            })
        };
        // A panel terminal with the keyboard is a terminal mode, whatever Neovim is in.
        let terminal = self.terminal_focused();
        let bar = Bar {
            mode: if terminal { "TERMINAL" } else { mode_label(&self.status.mode) },
            mode_color: self.highlight_mode.then(|| mode_color(if terminal { "terminal" } else { mode_kind(&self.status.mode) })),
            branch: self.status_branch(),
            file: self.status_file(),
            problems: (totals.errors, totals.warnings),
            problem_colors: [colors[0], colors[1]],
            filetype,
            progress: view.map_or_else(String::new, |v| progress_text(v.curline + 1, v.line_count)),
            position: view.map_or_else(String::new, |v| position_text(v.curline + 1, v.curcol + 1, &self.status)),
        };
        let hits = paint(painter, bar_rect, &bar, &pal, &font, &mut self.icons);
        self.native_bar_rect = Some(bar_rect);
        self.branch_rect = hits.branch;
        self.problems_rect = hits.counts;
        if hits.right_start - hits.left_end > 100.0 {
            let at = Pos2::new(hits.left_end + 18.0, bar_rect.center().y);
            self.paint_progress(painter, at, hits.right_start - 18.0, pal.fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_has_its_name() {
        for (mode, label) in [
            ("n", "NORMAL"),
            ("nt", "NORMAL"),
            ("niI", "NORMAL"),
            ("no", "O-PENDING"),
            ("nov", "O-PENDING"),
            ("noV", "O-PENDING"),
            ("no\x16", "O-PENDING"),
            ("v", "VISUAL"),
            ("V", "V-LINE"),
            ("\x16", "V-BLOCK"),
            ("s", "SELECT"),
            ("S", "S-LINE"),
            ("\x13", "S-BLOCK"),
            ("i", "INSERT"),
            ("ic", "INSERT"),
            ("ix", "INSERT"),
            ("R", "REPLACE"),
            ("Rc", "REPLACE"),
            ("Rx", "REPLACE"),
            ("Rv", "V-REPLACE"),
            ("Rvc", "V-REPLACE"),
            ("c", "COMMAND"),
            ("cv", "COMMAND"),
            ("t", "TERMINAL"),
            ("", "NORMAL"),
        ] {
            assert_eq!(mode_label(mode), label, "mode {mode:?}");
        }
    }

    #[test]
    fn pill_colors_follow_the_mode() {
        for (mode, kind) in [("n", "normal"), ("no", "normal"), ("i", "insert"), ("V", "visual"), ("\x13", "select"), ("Rv", "replace"), ("c", "cmdline"), ("t", "terminal")] {
            assert_eq!(mode_kind(mode), kind);
        }
        assert_ne!(mode_color(mode_kind("i")), mode_color(mode_kind("n")));
    }

    #[test]
    fn the_column_is_in_characters_except_in_a_terminal() {
        let status = |col, buftype: &str| nvim_bridge::gui::Status { col, buftype: buftype.into(), ..Default::default() };
        // "ééé x": x is the fifth character, at byte 8.
        assert_eq!(position_text(3, 8, &status(5, "")), "3:5");
        assert_eq!(position_text(3, 8, &status(0, "")), "3:8", "not known yet");
        assert_eq!(position_text(1, 9, &status(1, "terminal")), "1:9");
    }

    #[test]
    fn progress_is_top_percent_or_bottom() {
        assert_eq!(progress_text(1, 1), "Top");
        assert_eq!(progress_text(1, 100), "Top");
        assert_eq!(progress_text(100, 100), "Bot");
        assert_eq!(progress_text(50, 100), "50%");
        assert_eq!(progress_text(2, 3), "66%");
        assert_eq!(progress_text(2, 200), "1%");
    }

    #[test]
    fn the_bar_is_the_apps_own_only_with_the_own_config_and_gui() {
        assert!(use_native(true, true));
        assert!(!use_native(true, false));
        assert!(!use_native(false, true));
        assert!(!use_native(false, false));
    }

    #[test]
    fn file_names_carry_their_flags() {
        assert_eq!(file_label("main.rs", false, false), "main.rs");
        assert_eq!(file_label("main.rs", true, false), "main.rs [+]");
        assert_eq!(file_label("help.txt", false, true), "help.txt [-]");
        assert_eq!(file_label("a", true, true), "a [+] [-]");
        assert_eq!(file_label("", false, false), "[No Name]");
        assert_eq!(file_label("", true, false), "[No Name] [+]");
    }

    #[test]
    fn a_detached_head_shows_its_short_hash() {
        let repo = |branch: Option<&str>, head: Option<&str>| Repo {
            branch: branch.map(str::to_owned),
            head: head.map(str::to_owned),
            ..crate::git::parse_status(std::path::Path::new("/r"), b"")
        };
        assert_eq!(branch_label(&repo(Some("main"), Some("abcdef0123"))).as_deref(), Some("main"));
        assert_eq!(branch_label(&repo(None, Some("abcdef0123"))).as_deref(), Some("abcdef0"));
        assert_eq!(branch_label(&repo(None, None)), None);
    }

    fn want(slot: Slot, width: f32, min: f32) -> Want {
        Want { slot, width, min }
    }

    fn items() -> (Vec<Want>, Vec<Want>) {
        (
            vec![want(Slot::Mode, 70.0, 70.0), want(Slot::Branch, 80.0, 40.0), want(Slot::File, 120.0, 30.0), want(Slot::Counts, 50.0, 50.0)],
            vec![want(Slot::Filetype, 60.0, 60.0), want(Slot::Progress, 30.0, 30.0), want(Slot::Position, 40.0, 40.0)],
        )
    }

    /// No item runs under another or past the bar, and the left ones end
    /// before the right ones begin.
    fn assert_clear(placed: &[Placed], width: f32) {
        let mut spans: Vec<(f32, f32)> = placed.iter().map(|p| (p.x, p.x + p.width)).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        for pair in spans.windows(2) {
            assert!(pair[0].1 + GAP <= pair[1].0 + 0.01, "overlap in {spans:?} at width {width}");
        }
        if let Some(first) = spans.first() {
            assert!(first.0 >= EDGE - 0.01, "{spans:?}");
        }
        if let Some(last) = spans.last() {
            assert!(last.1 <= width - EDGE + 0.01, "{spans:?} at {width}");
        }
    }

    #[test]
    fn a_wide_bar_keeps_everything_whole() {
        let (l, r) = items();
        let placed = fit(&l, &r, 1000.0);
        assert_eq!(placed.len(), 7);
        assert!(placed.iter().all(|p| p.width == l.iter().chain(&r).find(|w| w.slot == p.slot).unwrap().width));
        assert_clear(&placed, 1000.0);
        // The right items end at the right edge.
        let position = placed.iter().find(|p| p.slot == Slot::Position).unwrap();
        assert_eq!(position.x + position.width, 1000.0 - EDGE);
        assert_eq!(placed[0].x, EDGE);
    }

    #[test]
    fn a_narrower_bar_cuts_the_file_name_first() {
        let (l, r) = items();
        let full = 2.0 * EDGE + run(&l) + GAP + run(&r);
        let placed = fit(&l, &r, full - 40.0);
        let by = |s| placed.iter().find(|p| p.slot == s).copied();
        assert_eq!(by(Slot::File).unwrap().width, 80.0);
        assert_eq!(by(Slot::Branch).unwrap().width, 80.0);
        assert_eq!(placed.len(), 7);
        assert_clear(&placed, full - 40.0);
    }

    #[test]
    fn then_the_branch_and_the_right_items_stay() {
        let (l, r) = items();
        let full = 2.0 * EDGE + run(&l) + GAP + run(&r);
        // The name can't be cut enough: it goes, and the branch is cut.
        let placed = fit(&l, &r, full - 100.0);
        assert!(placed.iter().all(|p| p.slot != Slot::File));
        assert!(placed.iter().any(|p| p.slot == Slot::Branch));
        assert_clear(&placed, full - 100.0);
        // Less: the branch goes too; the counts and the mode stay for now.
        let placed = fit(&l, &r, full - 180.0);
        assert!(placed.iter().all(|p| !matches!(p.slot, Slot::File | Slot::Branch)));
        assert!(placed.iter().any(|p| p.slot == Slot::Counts) && placed.iter().any(|p| p.slot == Slot::Mode));
        assert_clear(&placed, full - 180.0);
        for slot in [Slot::Filetype, Slot::Progress, Slot::Position] {
            assert!(placed.iter().any(|p| p.slot == slot), "{slot:?}");
        }
    }

    #[test]
    fn at_every_width_nothing_overlaps() {
        let (l, r) = items();
        // (Below that not even the position fits: the bar clips it.)
        for width in (60..700).step_by(7) {
            let placed = fit(&l, &r, width as f32);
            assert_clear(&placed, width as f32);
            // The position is the last to go.
            {
                assert!(placed.iter().any(|p| p.slot == Slot::Position), "width {width}");
            }
        }
    }

    #[test]
    fn a_bar_too_narrow_for_the_right_items_drops_the_file_type_first() {
        let (l, r) = items();
        let placed = fit(&l, &r, 2.0 * EDGE + 40.0 + GAP + 30.0 + 1.0);
        assert!(placed.iter().all(|p| p.slot != Slot::Filetype));
        assert!(placed.iter().any(|p| p.slot == Slot::Progress) && placed.iter().any(|p| p.slot == Slot::Position));
    }

    fn sample() -> Bar {
        Bar {
            mode: "NORMAL",
            mode_color: Some(mode_color("normal")),
            branch: Some("feature/status-bar".into()),
            file: file_label("renderer.rs", true, false),
            problems: (2, 1),
            problem_colors: [Color32::from_rgb(0xf1, 0x4c, 0x4c), Color32::from_rgb(0xcc, 0xa7, 0x00)],
            filetype: Some(("rust".into(), IconTheme::get().file_icon("a.rs").to_owned())),
            progress: progress_text(40, 100),
            position: "40:13".into(),
        }
    }

    /// The bar in dark and light, wide and narrow, with the pointer on the
    /// branch.
    #[test]
    fn the_bar() {
        use egui_kittest::Harness;
        let mut results = egui_kittest::SnapshotResults::new();
        for (theme, chrome, _) in crate::style::tests::themes() {
            let cases = [("wide", 760.0, None), ("medium", 450.0, None), ("narrow", 330.0, None), ("tiny", 190.0, None), ("branch_hovered", 760.0, Some(Pos2::new(120.0, 12.0)))];
            for (size, width, hover) in cases {
                let mut icons = IconTextures::default();
                let mut harness = Harness::builder().with_size(Vec2::new(width, 24.0)).with_pixels_per_point(2.0).with_render_cursor(false).wgpu().build_ui(move |ui| {
                    let pal = Palette::new(None, chrome);
                    let font = FontId::proportional(12.0);
                    let screen = ui.ctx().content_rect();
                    ui.painter().rect_filled(screen, 0.0, chrome.bg);
                    paint(ui.painter(), screen, &sample(), &pal, &font, &mut icons);
                });
                if let Some(pos) = hover {
                    harness.hover_at(pos);
                }
                harness.run();
                harness.snapshot(format!("status_bar_{theme}_{size}"));
                results.extend_harness(&mut harness);
            }
        }
        results.unwrap();
    }

    /// The painted bar reports where the branch and counts are, and they
    /// stay clear of the right items.
    #[test]
    fn the_bar_reports_its_click_targets() {
        use egui_kittest::Harness;
        let chrome = crate::style::tests::themes()[0].1;
        let hits_at = |width: f32, bar: Bar| {
            let hits = std::rc::Rc::new(std::cell::Cell::new(Hits::default()));
            let out = hits.clone();
            let mut icons = IconTextures::default();
            let mut harness = Harness::builder().with_size(Vec2::new(width + 10.0, 124.0)).build_ui(move |ui| {
                let rect = Rect::from_min_size(Pos2::new(5.0, 100.0), Vec2::new(width, 24.0));
                let pal = Palette::new(None, chrome);
                out.set(paint(ui.painter(), rect, &bar, &pal, &FontId::proportional(12.0), &mut icons));
            });
            harness.run();
            hits.get()
        };
        for width in [760.0, 330.0, 190.0] {
            let hits = hits_at(width, sample());
            assert!(hits.left_end <= hits.right_start, "width {width}: {hits:?}");
            for r in [hits.branch, hits.counts].into_iter().flatten() {
                assert!(r.min.x >= 5.0 && r.max.x <= hits.right_start, "width {width}: {r:?} {hits:?}");
            }
        }
        // Wide: both are there; with no branch, no target for it.
        let hits = hits_at(760.0, sample());
        assert!(hits.branch.is_some() && hits.counts.is_some());
        let hits = hits_at(760.0, Bar { branch: None, ..sample() });
        assert!(hits.branch.is_none() && hits.counts.is_some());
        // Too narrow for the branch: its target is gone with it.
        assert!(hits_at(190.0, sample()).branch.is_none());
    }
}
