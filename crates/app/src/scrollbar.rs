//! Scrollbars over editor windows, the diff view and terminals (`scrollbar` in the config): a thin
//! thumb at a window's right edge showing which part of the buffer is in
//! view (`win_viewport`). "auto" shows it while the window scrolls or the
//! pointer is near it, then fades; "always" keeps it; "never" has none.
//! Dragging the thumb, or clicking the track, scrolls the window.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{Color32, Pos2, Rect, Vec2};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Auto,
    Always,
    Never,
}

/// How long "auto" keeps a bar after the window scrolled, then fades.
const SHOW: Duration = Duration::from_millis(900);
const FADE: Duration = Duration::from_millis(350);
const WIDTH: f32 = 6.0;
const WIDTH_HOVER: f32 = 9.0;
const MIN_THUMB: f32 = 24.0;

/// What a bar is on, and so what its drags scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bar {
    /// A Neovim window (its handle); lines are the buffer's.
    Win(i64),
    /// The diff view; lines are pixels (`DiffView::scrollbar_target`).
    Diff,
    /// A terminal view (`TerminalView::id`); lines are those of its scrollback
    /// and screen, the first being the oldest.
    Terminal(u64),
}

/// A bar to put on something: what, its text area, and viewport.
pub struct Target {
    pub bar: Bar,
    pub area: Rect,
    /// First line in view (0-based), the line after the last, line count.
    pub top: i64,
    pub bottom: i64,
    pub lines: i64,
    /// Text and background the thumb is mixed from: the editor's, or a
    /// popup's.
    pub fg: Color32,
    pub bg: Color32,
}

struct Placed {
    bar: Bar,
    track: Rect,
    thumb: Rect,
    lines: i64,
    visible: i64,
}

struct Dragging {
    bar: Bar,
    /// Where in the thumb it was grabbed (px from its top).
    grab: f32,
    track: Rect,
    thumb_h: f32,
    lines: i64,
    visible: i64,
    sent: Option<i64>,
}

pub struct Scrollbars {
    pub mode: Mode,
    bars: Vec<Placed>,
    /// Each window's first line in view, and when it last changed.
    seen: HashMap<Bar, (i64, Instant)>,
    drag: Option<Dragging>,
}

impl Scrollbars {
    pub fn new(mode: Mode) -> Self {
        Self { mode, bars: Vec::new(), seen: HashMap::new(), drag: None }
    }

    /// Draw the bars for this frame's windows.
    pub fn paint(&mut self, ctx: &egui::Context, painter: &egui::Painter, targets: Vec<Target>) {
        self.bars.clear();
        if self.mode == Mode::Never {
            return;
        }
        let now = Instant::now();
        let hover = ctx.input(|i| i.pointer.hover_pos());
        let mut fading = false;
        self.seen.retain(|bar, _| targets.iter().any(|t| t.bar == *bar));
        for t in targets {
            let (fg, bg) = (t.fg, t.bg);
            let visible = (t.bottom - t.top).max(1);
            if t.lines <= visible || t.area.height() < MIN_THUMB * 1.5 {
                continue;
            }
            let changed = match self.seen.get(&t.bar) {
                Some(&(top, at)) if top == t.top => at,
                _ => {
                    self.seen.insert(t.bar, (t.top, now));
                    now
                }
            };
            let track = Rect::from_x_y_ranges(t.area.max.x - WIDTH_HOVER - 3.0..=t.area.max.x - 1.0, t.area.y_range());
            let dragging = self.drag.as_ref().is_some_and(|d| d.bar == t.bar);
            let near = hover.is_some_and(|p| track.expand2(Vec2::new(10.0, 0.0)).contains(p));
            let alpha = match self.mode {
                Mode::Always => 1.0,
                _ if dragging || near => 1.0,
                _ => {
                    let age = now.duration_since(changed);
                    if age < SHOW {
                        fading = true;
                        1.0
                    } else if age < SHOW + FADE {
                        fading = true;
                        1.0 - (age - SHOW).as_secs_f32() / FADE.as_secs_f32()
                    } else {
                        0.0
                    }
                }
            };
            let h = (track.height() * visible as f32 / t.lines as f32).max(MIN_THUMB).min(track.height());
            let y = track.min.y + (track.height() - h) * (t.top as f32 / (t.lines - visible).max(1) as f32).clamp(0.0, 1.0);
            let w = if near || dragging { WIDTH_HOVER } else { WIDTH };
            let thumb = Rect::from_min_max(Pos2::new(track.max.x - w, y), Pos2::new(track.max.x, y + h));
            if alpha > 0.0 {
                if near || dragging {
                    painter.rect_filled(track, 3.0, mix(bg, fg, 0.06).gamma_multiply(alpha));
                }
                let strength = if dragging { 0.55 } else if near { 0.42 } else { 0.3 };
                painter.rect_filled(thumb, w / 2.0, mix(bg, fg, strength).gamma_multiply(alpha));
            }
            // Clickable while shown (or near: "auto" shows it on approach).
            if alpha > 0.0 || near {
                self.bars.push(Placed { bar: t.bar, track, thumb, lines: t.lines, visible });
            }
        }
        if fading {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    /// A press: on a bar, grab its thumb (on the track: centered under the
    /// pointer). Returns true when the bar took it, and the first line to
    /// show if it moved.
    pub fn press(&mut self, pos: Pos2) -> (bool, Option<(Bar, i64)>) {
        let Some(bar) = self.bars.iter().find(|b| b.track.contains(pos)) else { return (false, None) };
        let grab = if bar.thumb.y_range().contains(pos.y) { pos.y - bar.thumb.min.y } else { bar.thumb.height() / 2.0 };
        self.drag = Some(Dragging {
            bar: bar.bar,
            grab,
            track: bar.track,
            thumb_h: bar.thumb.height(),
            lines: bar.lines,
            visible: bar.visible,
            sent: None,
        });
        (true, self.drag_to(pos))
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The pointer moved while dragging: the bar's and its new first line.
    pub fn drag_to(&mut self, pos: Pos2) -> Option<(Bar, i64)> {
        let d = self.drag.as_mut()?;
        let room = (d.track.height() - d.thumb_h).max(1.0);
        let frac = ((pos.y - d.grab - d.track.min.y) / room).clamp(0.0, 1.0);
        let top = (frac * (d.lines - d.visible).max(0) as f32).round() as i64;
        (d.sent != Some(top)).then(|| {
            d.sent = Some(top);
            (d.bar, top)
        })
    }

    pub fn release(&mut self) {
        self.drag = None;
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    crate::popup::mix(a, b, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_the_thumb_maps_to_lines() {
        let mut s = Scrollbars::new(Mode::Always);
        let track = Rect::from_min_max(Pos2::new(90.0, 0.0), Pos2::new(100.0, 100.0));
        // 1000 lines, 100 in view: a thumb of 10% at the top.
        let thumb = Rect::from_min_max(Pos2::new(94.0, 0.0), Pos2::new(100.0, 24.0));
        s.bars.push(Placed { bar: Bar::Win(7), track, thumb, lines: 1000, visible: 100 });
        assert_eq!(s.press(Pos2::new(95.0, 5.0)), (true, Some((Bar::Win(7), 0))));
        // Dragged to the bottom: the last screenful.
        // Half way down the room the thumb has: half way.
        assert_eq!(s.drag_to(Pos2::new(95.0, 5.0 + 38.0)), Some((Bar::Win(7), 450)));
        assert_eq!(s.drag_to(Pos2::new(95.0, 200.0)), Some((Bar::Win(7), 900)));
        assert_eq!(s.drag_to(Pos2::new(95.0, 300.0)), None, "only changes are sent");
        s.release();
        assert!(!s.press(Pos2::new(10.0, 10.0)).0, "not on a bar");
    }
}
