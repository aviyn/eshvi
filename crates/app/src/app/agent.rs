//! The agent panel's tab strip: drawn by `paint_strip` (pure, with snapshot tests) from what
//! `agent_panel.rs` holds, and the Changes chip it shows.

use egui::{Color32, FontId};

use super::*;

impl VimApp {
    /// What the agents changed, as the chip at the strip's right end shows it.
    pub(super) fn changes_chip(&self) -> Option<ChangesChip> {
        (!self.changes.files.is_empty()).then(|| {
            let (added, removed) = self.changes.totals();
            let deco = crate::decorations::DecoColors::from_editor(&self.editor);
            ChangesChip { added, removed, unseen: self.changes.unseen, open: self.changes.popup, colors: (deco.added, deco.deleted) }
        })
    }
}

/// A tab of the strip.
pub(super) struct StripTab {
    /// The tab's id, what the buttons name.
    pub id: i64,
    pub title: String,
    pub current: bool,
    pub exited: bool,
    /// Output is arriving: a pulsing dot.
    pub active: bool,
    /// It went quiet while not shown: a solid dot.
    pub unseen: bool,
}

/// What the agents changed, for the chip.
#[derive(Clone)]
pub(super) struct ChangesChip {
    pub added: usize,
    pub removed: usize,
    /// The numbers changed since the popup was last open: a dot.
    pub unseen: bool,
    pub open: bool,
    /// The colors of "+n" and "−n".
    pub colors: (Color32, Color32),
}

/// What the strip draws.
pub(super) struct Strip<'a> {
    pub tabs: &'a [StripTab],
    /// The panel has the keyboard.
    pub focused: bool,
    /// The color of the panel's content, which the current tab joins.
    pub content_bg: Color32,
    /// Show the history button (the default tool is Claude).
    pub resume: bool,
    pub changes: Option<ChangesChip>,
}

/// Where the strip's buttons were drawn, and whether something pulses.
#[derive(Default)]
pub(super) struct StripOut {
    pub buttons: Vec<(Rect, AgentButton)>,
    pub animating: bool,
}

/// Draw the agent panel's tab strip into `strip`: a tab per agent (a pulsing dot while it
/// works, a dot when it went quiet unseen, dimmed once it exited, "×" on hover), the
/// history button when asked, "+" for a new agent, the Changes chip and "×" at the end to
/// hide the panel.
pub(super) fn paint_strip(painter: &egui::Painter, strip: Rect, model: &Strip, chrome: &Chrome, font: &FontId, icons: &mut IconTextures) -> StripOut {
    let (fg, panel, accent) = (chrome.fg, chrome.panel, chrome.accent);
    let focused = model.focused;
    let content_bg = model.content_bg;
    let ctx = painter.ctx().clone();
    let painter = painter.with_clip_rect(strip.intersect(painter.clip_rect()));
    let mut out = StripOut::default();
    painter.rect_filled(strip, 0.0, panel);
    painter.hline(strip.x_range(), strip.max.y - 0.5, egui::Stroke::new(style::HAIRLINE, chrome.line()));

    let hover = ctx.input(|i| i.pointer.hover_pos()).filter(|p| strip.contains(*p));
    let (pad, icon_w, dot_w) = (style::TAB_PAD, (font.size * 0.8).round(), style::button_size(strip.height()));
    let close = Rect::from_min_max(Pos2::new(strip.max.x - 4.0 - strip.height(), strip.min.y), Pos2::new(strip.max.x - 4.0, strip.max.y));
    // What the agents changed, left of "×": "+8 −3" (a dot when that
    // changed unseen); opens the Changes popup. Nothing changed: none.
    // Its room is kept first: long titles (Claude Code names its
    // terminal after the task) or several agents mustn't push it out.
    let chip = model.changes.as_ref().map(|c| {
        let small = style::scaled(font, style::SMALL);
        let plus_g = painter.layout_no_wrap(format!("+{}", c.added), small.clone(), c.colors.0);
        let minus_g = painter.layout_no_wrap(format!("−{}", c.removed), small, c.colors.1);
        let dot_w = if c.unseen { 10.0 } else { 0.0 };
        let w = 10.0 + dot_w + plus_g.size().x + 6.0 + minus_g.size().x + 10.0;
        let rect = Rect::from_min_max(Pos2::new(close.min.x - 4.0 - w, strip.min.y + 4.0), Pos2::new(close.min.x - 4.0, strip.max.y - 4.0));
        (rect, plus_g, minus_g, dot_w, c.unseen, c.open, c.colors)
    });
    // The tabs share what's left (less "+"); titles too long for their
    // share are cut with "…".
    let buttons = if model.resume { 2.0 } else { 1.0 };
    let tabs_end = chip.as_ref().map_or(close.min.x, |c| c.0.min.x) - 4.0 - buttons * (strip.height() + 2.0);
    let fixed = (pad + icon_w + style::GAP + 4.0 + dot_w + pad * 0.5).round();
    let share = (tabs_end - strip.min.x) / model.tabs.len().max(1) as f32;
    let title_max = (share - fixed).max(font.size * 1.5);
    let time = ctx.input(|i| i.time);
    let mut x = strip.min.x;
    for tab in model.tabs {
        let mut color = chrome.text(tab.current && focused);
        if tab.exited {
            color = chrome.dim(color);
        }
        let mut title = painter.layout_no_wrap(tab.title.clone(), font.clone(), color);
        let mut keep = tab.title.chars().count();
        while title.size().x > title_max && keep > 1 {
            keep -= 1;
            title = painter.layout_no_wrap(truncate_chars(&tab.title, keep), font.clone(), color);
        }
        let rect = Rect::from_min_size(
            Pos2::new(x, strip.min.y),
            Vec2::new((pad + icon_w + style::GAP + title.size().x + 4.0 + dot_w + pad * 0.5).round(), strip.height()),
        );
        x = rect.max.x;
        // Like the editor's file tabs: the current one in the panel's
        // color, joined to it, with an accent on top while focused.
        if tab.current {
            style::paint_current_tab(&painter, rect, focused, chrome, content_bg);
        }
        style::paint_tab_divider(&painter, rect, chrome);
        // An asterisk, the agent's mark.
        let c = Pos2::new(rect.min.x + pad + icon_w * 0.4, rect.center().y);
        let r = icon_w * 0.32;
        let icon_color = if tab.current && !tab.exited { mix(accent, fg, style::TINT) } else { color };
        for k in 0..3 {
            let a = std::f32::consts::PI * (k as f32 / 3.0 + 0.5);
            let d = Vec2::new(a.cos(), a.sin()) * r;
            painter.line_segment([c - d, c + d], egui::Stroke::new(style::STROKE, icon_color));
        }
        let text_x = rect.min.x + pad + icon_w + style::GAP;
        painter.galley(Pos2::new(text_x, rect.center().y - title.size().y / 2.0), title.clone(), color);
        // Status, or "×" while hovered.
        let slot = Rect::from_center_size(Pos2::new(text_x + title.size().x + 4.0 + dot_w / 2.0, rect.center().y), Vec2::splat(dot_w));
        if hover.is_some_and(|p| rect.contains(p)) {
            let on = hover.is_some_and(|p| slot.contains(p));
            let surface = style::tab_fill(chrome, content_bg, tab.current, focused);
            style::glyph_button(&painter, Glyph::Cross, slot.center(), strip.height(), chrome, surface, on);
            out.buttons.push((slot, AgentButton::CloseTab(tab.id)));
        } else if tab.active {
            let pulse = 0.55 + 0.45 * (time as f32 * 4.0).sin();
            painter.circle_filled(slot.center(), 3.0, accent.gamma_multiply(pulse));
            out.animating = true;
        } else if tab.unseen {
            painter.circle_filled(slot.center(), 3.0, accent);
        }
        out.buttons.push((rect, AgentButton::Tab(tab.id)));
    }
    // "+" and, at the right end, "×" (hide the panel).
    let h = strip.height();
    // The history icon (Claude's past conversations) comes before "+".
    if model.resume {
        let resume = Rect::from_min_size(Pos2::new(x + 2.0, strip.min.y), Vec2::splat(h));
        let on = hover.is_some_and(|p| resume.contains(p));
        style::paint_button_fill(&painter, resume, chrome, panel, on, false);
        icons.paint_codicon(&painter, "history", resume.center(), style::icon_size(h), chrome.text(on));
        out.buttons.push((resume, AgentButton::Resume));
        if on {
            super::tabs::show_strip_tooltip(&ctx, resume, "Resume Conversation");
        }
        x = resume.max.x;
    }
    let plus = Rect::from_min_size(Pos2::new(x + 2.0, strip.min.y), Vec2::splat(h));
    style::glyph_button(&painter, Glyph::Plus, plus.center(), h, chrome, panel, hover.is_some_and(|p| plus.contains(p)));
    out.buttons.push((plus, AgentButton::New));
    if close.min.x > plus.max.x {
        style::glyph_button(&painter, Glyph::Cross, close.center(), h, chrome, panel, hover.is_some_and(|p| close.contains(p)));
        out.buttons.push((close, AgentButton::Hide));
    }
    if let Some((chip, plus_g, minus_g, dot_w, unseen, open, colors)) = chip {
        if chip.min.x > plus.max.x + 4.0 {
            let hovered = hover.is_some_and(|p| chip.contains(p));
            let level = if open || hovered { style::ON } else { style::SUBTLE };
            painter.rect_filled(chip, style::round(chip.height()), chrome.shade(panel, level));
            let mut cx = chip.min.x + 10.0;
            if unseen {
                painter.circle_filled(Pos2::new(cx + 2.5, chip.center().y), 3.0, accent);
                cx += dot_w;
            }
            let (pw, ph) = (plus_g.size().x, plus_g.size().y);
            painter.galley(Pos2::new(cx, chip.center().y - ph / 2.0), plus_g, colors.0);
            let mh = minus_g.size().y;
            painter.galley(Pos2::new(cx + pw + 6.0, chip.center().y - mh / 2.0), minus_g, colors.1);
            out.buttons.push((chip, AgentButton::Changes));
        }
    }
    out
}

/// A click target in the agent panel's tab strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentButton {
    /// A tab, by id: shows that agent.
    Tab(i64),
    /// A tab's "×": ends that agent.
    CloseTab(i64),
    /// What the agents changed: opens the Changes popup.
    Changes,
    New,
    /// Past conversations (Claude only): the list to resume from.
    Resume,
    Hide,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: i64, title: &str, current: bool) -> StripTab {
        StripTab { id, title: title.into(), current, exited: false, active: false, unseen: false }
    }

    /// A strip with an agent at work, the current one, one that went quiet unseen and an exited one.
    fn some_tabs() -> Vec<StripTab> {
        vec![
            StripTab { active: true, ..tab(1, "Fix the bug in the parser", false) },
            tab(2, "Claude 2", true),
            StripTab { unseen: true, ..tab(3, "Codex 1", false) },
            StripTab { exited: true, ..tab(4, "Claude 4", false) },
        ]
    }

    fn chip() -> ChangesChip {
        ChangesChip { added: 8, removed: 3, unseen: false, open: false, colors: (Color32::from_rgb(0x58, 0xa6, 0x4a), Color32::from_rgb(0xe0, 0x5d, 0x5d)) }
    }

    fn model(tabs: &[StripTab], focused: bool, resume: bool, changes: Option<ChangesChip>) -> Strip<'_> {
        Strip { tabs, focused, content_bg: Color32::TRANSPARENT, resume, changes }
    }

    /// The strip, dark and light: tabs with their dots, the history button, "+", the
    /// Changes chip and "×"; focused or not; a narrow panel, which cuts titles with "…".
    #[test]
    fn the_strip() {
        use egui_kittest::Harness;
        let mut results = egui_kittest::SnapshotResults::new();
        for (theme, chrome, _) in crate::style::tests::themes() {
            for (name, width, count, focused, resume, changes) in [
                ("focused", 560.0, 4, true, true, Some(chip())),
                ("unfocused", 560.0, 4, false, true, Some(chip())),
                ("plain", 560.0, 4, true, false, None),
                ("narrow", 330.0, 2, true, true, Some(ChangesChip { unseen: true, ..chip() })),
            ] {
                let mut harness = Harness::builder().with_size(Vec2::new(width, 28.0)).with_pixels_per_point(2.0).with_render_cursor(false).wgpu().build_ui(move |ui| {
                    let screen = ui.ctx().content_rect();
                    ui.painter().rect_filled(screen, 0.0, chrome.bg);
                    let tabs: Vec<StripTab> = some_tabs().into_iter().take(count).collect();
                    let changes = changes.clone();
                    let m = model(&tabs, focused, resume, changes);
                    paint_strip(ui.painter(), screen, &m, &chrome, &FontId::proportional(12.0), &mut IconTextures::default());
                });
                harness.run();
                harness.snapshot(format!("agent_strip_{name}_{theme}"));
                results.extend_harness(&mut harness);
            }
        }
        results.unwrap();
    }

    #[test]
    fn the_strip_reports_its_buttons_in_the_order_a_press_looks_for_them() {
        use egui_kittest::Harness;
        let chrome = crate::style::tests::themes()[0].1;
        let buttons_at = |width: f32, resume: bool, changes: bool| {
            let out = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let sink = out.clone();
            let mut harness = Harness::builder().with_size(Vec2::new(width, 28.0)).build_ui(move |ui| {
                let tabs = some_tabs();
                let m = model(&tabs, true, resume, changes.then(chip));
                *sink.borrow_mut() = paint_strip(ui.painter(), ui.ctx().content_rect(), &m, &chrome, &FontId::proportional(12.0), &mut IconTextures::default()).buttons;
            });
            harness.run();
            let buttons = out.borrow().clone();
            buttons
        };
        let wide = buttons_at(700.0, true, true);
        let kinds: Vec<AgentButton> = wide.iter().map(|&(_, b)| b).collect();
        // A tab per agent, then the history button, "+", "×" and the chip.
        assert_eq!(kinds.iter().filter(|b| matches!(b, AgentButton::Tab(_))).count(), 4);
        for b in [AgentButton::Resume, AgentButton::New, AgentButton::Hide, AgentButton::Changes] {
            assert!(kinds.contains(&b), "{b:?} in {kinds:?}");
        }
        // The buttons don't overlap each other or the tabs.
        let rect = |b: AgentButton| wide.iter().find(|(_, k)| *k == b).unwrap().0;
        let tabs_end = (1..=4).map(|i| rect(AgentButton::Tab(i)).max.x).fold(0.0, f32::max);
        assert!(rect(AgentButton::Resume).min.x >= tabs_end && rect(AgentButton::New).min.x >= rect(AgentButton::Resume).max.x);
        assert!(rect(AgentButton::Changes).min.x > rect(AgentButton::New).max.x && rect(AgentButton::Hide).min.x >= rect(AgentButton::Changes).max.x);
        // Without Claude as the default tool there is no history button; with nothing changed, no chip.
        let plain: Vec<AgentButton> = buttons_at(700.0, false, false).iter().map(|&(_, b)| b).collect();
        assert!(!plain.contains(&AgentButton::Resume) && !plain.contains(&AgentButton::Changes));
        assert!(plain.contains(&AgentButton::New) && plain.contains(&AgentButton::Hide));
    }
}
