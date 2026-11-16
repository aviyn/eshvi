//! Native message UI for `ext_messages`: toasts in the bottom-right corner, a dialog for prompts and long output, and small
//! pills for 'showcmd', recording and the ruler.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontFamily, FontId, Id, Order, Pos2, Rect, RichText, ScrollArea, Sense};
use grid::{Editor, Message, StyledChunk};
use nvim_bridge::gui::Severity;

use crate::fonts::GridFonts;
use crate::popup::PopupTheme;
use crate::renderer::{rgb, CellMetrics};

/// Message kinds that are not worth a toast.
const IGNORED_KINDS: &[&str] = &["search_cmd", "completion", "empty"];

#[derive(Default)]
pub struct MessageView {
    /// When each Neovim message (by `seq`) was first shown.
    shown_at: HashMap<u64, Instant>,
    /// 'showcmd' text and since when it has been the same.
    showcmd: Option<(String, Instant)>,
}

/// 'showcmd' text is shown once it stays this long: a command waiting for
/// more keys ("d2", "\"), not each key of one that ends at once ("gj" for a
/// mapped j), which would flash with every key while scrolling.
const SHOWCMD_DELAY: Duration = Duration::from_millis(400);

/// How long a toast stays: longer for longer text, longest for errors.
pub fn toast_duration(text: &str, severity: Option<Severity>, kind: &str) -> Duration {
    if kind == "search_count" {
        return Duration::from_millis(1500);
    }
    let base = 3000 + 40 * text.chars().count() as u64;
    let min = if severity == Some(Severity::Error) { 8000 } else { 4000 };
    Duration::from_millis(base.clamp(min, 12000))
}

/// Messages that wait for the user instead of timing out.
pub fn is_sticky(m: &Message) -> bool {
    m.is_prompt() || m.kind == "list_cmd" || m.text().lines().count() > 6
}

/// Severity from the message kind, or from its highlight: `vim.notify`
/// warnings arrive as plain messages highlighted with `WarningMsg`.
fn severity_of(editor: &Editor, m: &Message) -> Option<Severity> {
    // By color: Neovim gives a message's `WarningMsg` another id than the one
    // it reports for the group.
    let uses_group = |name: &str| {
        let color = editor.hl_groups.get(name).and_then(|id| editor.hl_attr(*id).foreground);
        color.is_some_and(|color| m.content.iter().any(|c| editor.chunk_attr(&c.style).foreground == Some(color)))
    };
    if m.is_error() || uses_group("ErrorMsg") {
        Some(Severity::Error)
    } else if m.is_warning() || uses_group("WarningMsg") {
        Some(Severity::Warning)
    } else {
        None
    }
}

fn severity_color(editor: &Editor, sev: Option<Severity>, theme: &PopupTheme) -> Color32 {
    let (group, fallback) = match sev {
        Some(Severity::Error) => ("DiagnosticError", Color32::from_rgb(0xf1, 0x4c, 0x4c)),
        Some(Severity::Warning) => ("DiagnosticWarn", Color32::from_rgb(0xcc, 0xa7, 0x00)),
        Some(Severity::Hint) => ("DiagnosticHint", Color32::from_rgb(0x9a, 0xa0, 0xa6)),
        Some(Severity::Info) | None => ("DiagnosticInfo", theme.accent),
    };
    editor.hl_groups.get(group).and_then(|id| editor.hl_attr(*id).foreground).map(rgb).unwrap_or(fallback)
}

/// A sentence, for the interface's font: errors, warnings, prompts and
/// whatever fits on a line. Other output keeps the grid's, whose columns line
/// up (`:ls`, `:map`, a table printed from Lua).
fn is_prose(editor: &Editor, m: &Message) -> bool {
    severity_of(editor, m).is_some() || m.is_prompt() || (m.kind != "list_cmd" && m.text().lines().count() <= 1)
}

/// Lay out highlighted chunks in the interface's font or the grid's, wrapped
/// at `width`.
fn chunks_job(editor: &Editor, chunks: &[StyledChunk], fonts: &GridFonts, prose: bool, fg: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    for chunk in chunks {
        let attr = editor.chunk_attr(&chunk.style);
        let (font_id, fake_italic) =
            if prose { (fonts.sidebar_font_id(attr.bold), attr.italic) } else { fonts.font_id(attr.bold, attr.italic) };
        let color = attr.foreground.map(rgb).unwrap_or(fg);
        job.append(&chunk.text, 0.0, TextFormat { font_id, color, italics: fake_italic, ..Default::default() });
    }
    job
}

/// The tallest the dialog's text gets; longer text scrolls.
fn dialog_text_height(screen: Rect) -> f32 {
    (screen.height() * 0.4).min(300.0)
}

/// The widest a toast gets.
const TOAST_MAX_WIDTH: f32 = 480.0;

/// Room on a message's left for its severity bar.
const BAR_ROOM: i8 = 12;

/// A toast's or the dialog's frame, with the room for the bar.
fn barred_frame(theme: &PopupTheme) -> egui::Frame {
    theme.frame().inner_margin(egui::Margin { left: BAR_ROOM, ..crate::style::POPUP_PAD })
}

/// The severity bar, in the room `barred_frame` leaves at `frame_left`.
fn paint_severity_bar(painter: &egui::Painter, frame_left: f32, top: f32, bottom: f32, color: Color32) {
    let bar = Rect::from_min_max(Pos2::new(frame_left + 3.0, top), Pos2::new(frame_left + 6.0, bottom));
    painter.rect_filled(bar, crate::style::BAR_RADIUS, color);
}

/// Neovim's mode text ("-- INSERT --") is redundant with a statusline;
/// keep everything else ("recording @q").
fn showmode_text(editor: &Editor) -> String {
    let text: String = editor.showmode.iter().map(|c| c.text.as_str()).collect();
    let text = text.trim();
    if text.starts_with("--") && text.ends_with("--") && !text.contains("recording") {
        return String::new();
    }
    text.to_owned()
}

/// ⌘C, as the event or as the key (both come with one press). While the
/// dialog is open this copies from it instead of being a key that closes it.
pub fn is_copy(ev: &egui::Event) -> bool {
    match ev {
        egui::Event::Copy => true,
        egui::Event::Key { key: egui::Key::C, modifiers, .. } => modifiers.command && !modifiers.shift && !modifiers.alt,
        _ => false,
    }
}

impl MessageView {
    /// Whether the dialog (prompts, long output, `:messages`) is open.
    pub fn dialog_open(editor: &Editor) -> bool {
        editor.message_history.is_some() || editor.messages.iter().any(is_sticky)
    }

    /// Keys went to Neovim: sticky messages (prompts, lists) are answered.
    pub fn on_input(&mut self, editor: &mut Editor) {
        editor.messages.retain(|m| !is_sticky(m));
        editor.message_history = None;
    }

    /// Escape also dismisses everything that isn't an error.
    pub fn dismiss_all(&mut self, editor: &mut Editor) {
        self.on_input(editor);
        editor.messages.retain(Message::is_error);
    }

    /// Draw messages; returns screen rects (for mouse routing).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        editor: &mut Editor,
        fonts: &GridFonts,
        m: CellMetrics,
        screen: Rect,
        cmdline: Option<Rect>,
    ) -> Vec<Rect> {
        let now = Instant::now();
        let theme = PopupTheme::from_editor(editor);
        let mut rects = Vec::new();
        let mut next_wake: Option<Duration> = None;
        let mut wake = |d: Duration| next_wake = Some(next_wake.map_or(d, |w| w.min(d)));

        // Expire toasts.
        editor.messages.retain(|msg| IGNORED_KINDS.iter().all(|k| *k != msg.kind));
        let seen: Vec<u64> = editor.messages.iter().map(|m| m.seq).collect();
        self.shown_at.retain(|seq, _| seen.contains(seq));
        for msg in &editor.messages {
            self.shown_at.entry(msg.seq).or_insert(now);
        }
        let expired: Vec<u64> = editor
            .messages
            .iter()
            .filter(|msg| !is_sticky(msg))
            .filter(|msg| {
                let left = toast_duration(&msg.text(), severity_of(editor, msg), &msg.kind)
                    .checked_sub(now - self.shown_at[&msg.seq]);
                if let Some(d) = left {
                    wake(d);
                }
                left.is_none()
            })
            .map(|msg| msg.seq)
            .collect();
        editor.messages.retain(|msg| !expired.contains(&msg.seq));

        let right = screen.max.x - 12.0;
        let mut bottom = screen.max.y - m.height - 10.0;
        // Above the command line at the bottom, where the two share columns.
        if let Some(c) = cmdline.filter(|c| c.max.x > right - TOAST_MAX_WIDTH) {
            bottom = bottom.min(c.min.y - 8.0);
        }

        // Pills: showcmd, recording, ruler.
        let mut pill_x = right;
        let mut pills_h: f32 = 0.0;
        let small = crate::style::scaled(&FontId::new(fonts.ui_size, FontFamily::Proportional), crate::style::SMALL);
        let showcmd: String = editor.showcmd.iter().map(|c| c.text.as_str()).collect::<String>().trim().to_owned();
        if self.showcmd.as_ref().is_none_or(|(t, _)| *t != showcmd) {
            self.showcmd = Some((showcmd.clone(), now));
        }
        let showcmd = match &self.showcmd {
            Some((t, since)) if !t.is_empty() && now - *since < SHOWCMD_DELAY => {
                wake(SHOWCMD_DELAY - (now - *since));
                String::new()
            }
            _ => showcmd,
        };
        // (Each kind keeps its Area: a new one is measured unseen first.)
        let pills: Vec<(&str, String)> = [
            ("showcmd", showcmd),
            ("showmode", showmode_text(editor)),
            ("ruler", editor.ruler.iter().map(|c| c.text.as_str()).collect::<String>()),
        ]
        .into_iter()
        .map(|(kind, t)| (kind, t.trim().to_owned()))
        .filter(|(_, t)| !t.is_empty())
        .collect();
        for (kind, text) in &pills {
            // Placed by its width now: a right-aligned Area goes by last
            // frame's size, so a pill whose text changed (the ruler while
            // scrolling) would jump for a frame.
            let text_w = ctx.fonts_mut(|f| f.layout_no_wrap(text.clone(), small.clone(), theme.fg).size().x);
            let resp = egui::Area::new(Id::unique(("eshvi-pill", *kind)))
                .order(Order::Foreground)
                .pivot(Align2::LEFT_BOTTOM)
                .fixed_pos(Pos2::new(pill_x - (text_w + 16.0 + 2.0), bottom))
                .interactable(false)
                .show(ctx, |ui| {
                    crate::style::chip_frame(&theme)
                        .show(ui, |ui| ui.label(RichText::new(text).font(small.clone()).color(theme.fg)));
                })
                .response;
            pill_x = resp.rect.min.x - 6.0;
            pills_h = pills_h.max(resp.rect.height() + 6.0);
        }

        // Dialog: prompts, long output and `:messages`. Toasts go above it.
        let mut toasts_bottom = bottom - pills_h;
        let dialog: Vec<Message> = editor
            .message_history
            .clone()
            .unwrap_or_default()
            .into_iter()
            .chain(editor.messages.iter().filter(|m| is_sticky(m)).cloned())
            .collect();
        if !dialog.is_empty() {
            // (One dialog, one font: `:messages` mixes errors with output.)
            let prose = dialog.iter().all(|m| is_prose(editor, m));
            let width = (screen.width() * 0.7).clamp(320.0_f32.min(screen.width() - 24.0), 900.0);
            let resp = egui::Area::new(Id::unique("eshvi-msg-dialog"))
                .order(Order::Foreground)
                .pivot(Align2::RIGHT_BOTTOM)
                .fixed_pos(Pos2::new(right, bottom - pills_h))
                .show(ctx, |ui| {
                    barred_frame(&theme).show(ui, |ui| {
                        crate::popup::thin_scrollbars(ui, &theme);
                        ui.set_width(width);
                        let frame_left = ui.max_rect().left() - f32::from(BAR_ROOM);
                        // From the top, where an error says what went wrong;
                        // `:messages` and a prompt end with what matters.
                        let from_end = editor.message_history.is_some() || dialog.iter().any(Message::is_prompt);
                        // (As wide as the dialog: the wheel works all over it, the bar is at its edge.)
                        ScrollArea::vertical().auto_shrink([false, true]).max_height(dialog_text_height(screen)).stick_to_bottom(from_end).show(ui, |ui| {
                            let view = ui.clip_rect();
                            for msg in &dialog {
                                let color = severity_color(editor, severity_of(editor, msg), &theme);
                                let row = ui.label(chunks_job(editor, &msg.content, fonts, prose, theme.fg, width)).rect;
                                // (In the frame's margin, so outside the scroll area's clip.)
                                let (top, end) = (row.top().max(view.top()), row.bottom().min(view.bottom()));
                                if top < end {
                                    paint_severity_bar(&ctx.layer_painter(ui.layer_id()), frame_left, top, end, color);
                                }
                            }
                        });
                        ui.add_space(4.0);
                        let hint = if dialog.iter().any(Message::is_prompt) { "Answer with the keyboard" } else { "⌘C to copy, any other key to close" };
                        ui.label(RichText::new(hint).small().color(theme.dim_fg));
                    });
                })
                .response;
            // All of it. A selection's copy comes later in this pass, when
            // the labels' selection ends it, and replaces this one.
            if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
                ctx.copy_text(dialog.iter().map(Message::text).collect::<Vec<_>>().join("\n"));
            }
            toasts_bottom = resp.rect.min.y - 8.0;
            rects.push(resp.rect);
        }

        // Toasts, newest at the bottom.
        let toasts: Vec<&Message> = editor.messages.iter().filter(|m| !is_sticky(m)).collect();

        let width = (screen.width() * 0.4).clamp(260.0_f32.min(screen.width() - 24.0), TOAST_MAX_WIDTH);
        let mut y = toasts_bottom;
        let mut dismissed: Vec<u64> = Vec::new();
        for t in toasts.iter().rev() {
            let accent = severity_color(editor, severity_of(editor, t), &theme);
            let area = egui::Area::new(Id::unique(("eshvi-toast", t.seq)))
                .order(Order::Foreground)
                .pivot(Align2::RIGHT_BOTTOM)
                .fixed_pos(Pos2::new(right, y))
                .sense(Sense::CLICK)
                .show(ctx, |ui| {
                    barred_frame(&theme).show(ui, |ui| {
                        ui.set_width(width);
                        ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                            ui.label(chunks_job(editor, &t.content, fonts, is_prose(editor, t), theme.fg, width));
                        });
                    });
                });
            let rect = area.response.rect;
            paint_severity_bar(&ctx.layer_painter(area.response.layer_id), rect.min.x, rect.min.y + 6.0, rect.max.y - 6.0, accent);
            if area.response.clicked() {
                dismissed.push(t.seq);
            }
            rects.push(rect);
            y = rect.min.y - 8.0;
            if y < screen.min.y + screen.height() * 0.3 {
                break; // keep the stack in the lower part of the window
            }
        }
        editor.messages.retain(|m| !dismissed.contains(&m.seq));

        if let Some(d) = next_wake {
            ctx.request_repaint_after(d);
        }
        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Vec2;
    use grid::ChunkStyle;

    fn msg(kind: &str, text: &str) -> Message {
        Message { seq: 1, id: None, kind: kind.into(), content: vec![StyledChunk { style: ChunkStyle::Id(0), text: text.into() }] }
    }

    type DialogState = (MessageView, Editor, Vec<Rect>);

    fn dialog_harness(editor: Editor) -> egui_kittest::Harness<'static, DialogState> {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let m = CellMetrics { width: 8.0, height: 20.0 };
        let mut fonts = None;
        let mut harness = egui_kittest::Harness::builder().with_size(screen.size()).build_ui_state(
            move |ui, (view, editor, rects): &mut DialogState| {
                // Real fonts: they arrive the frame after they're installed.
                let fonts = fonts.get_or_insert_with(|| crate::fonts::install(ui.ctx(), &crate::fonts::FontSpec::default_with_size(14.0), None, None, None, None));
                if fonts.is_live(ui.ctx()) {
                    *rects = view.show(ui.ctx(), editor, fonts, m, screen, None);
                }
            },
            (MessageView::default(), editor, Vec::new()),
        );
        harness.run();
        harness
    }

    /// The dialog over `editor`'s messages, a frame for each list of events:
    /// what it copied, last copy last, and its rectangles.
    fn dialog_frame(editor: Editor, frames: Vec<Vec<egui::Event>>) -> (Vec<String>, Vec<Rect>) {
        let mut harness = dialog_harness(editor);
        let mut copied = Vec::new();
        for events in frames {
            harness.input_mut().events.extend(events);
            harness.step();
            copied.extend(harness.output().platform_output.commands.iter().filter_map(|c| match c {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            }));
        }
        (copied, harness.state().2.clone())
    }

    fn very_long_error() -> (Editor, String) {
        let text = (0..200).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let mut editor = Editor::new();
        editor.messages.push(msg("emsg", &text));
        (editor, text)
    }

    /// Where the dialog's text starts, as drawn.
    fn text_top(harness: &egui_kittest::Harness<'static, DialogState>, text: &str) -> f32 {
        use egui_kittest::kittest::Queryable;
        harness.get_by_label(text).rect().min.y
    }

    #[test]
    fn long_text_stays_within_the_limit_and_starts_at_its_top() {
        let (editor, text) = very_long_error();
        let harness = dialog_harness(editor);
        let dialog = harness.state().2[0];
        assert!(dialog.height() < 360.0, "{dialog:?}: its text, the hint and the padding");
        assert!(text_top(&harness, &text) >= dialog.min.y, "the first line shows");
    }

    #[test]
    fn the_wheel_scrolls_long_text() {
        let (editor, text) = very_long_error();
        let mut harness = dialog_harness(editor);
        let dialog = harness.state().2[0];
        let before = text_top(&harness, &text);
        harness.input_mut().events.push(egui::Event::PointerMoved(dialog.center()));
        harness.step();
        harness.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -120.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Default::default(),
        });
        harness.run();
        assert!(text_top(&harness, &text) < before - 50.0, "{before} to {}", text_top(&harness, &text));
    }

    #[test]
    fn messages_and_prompts_open_at_their_end() {
        let (mut editor, text) = very_long_error();
        editor.message_history = Some(std::mem::take(&mut editor.messages));
        let harness = dialog_harness(editor);
        assert!(text_top(&harness, &text) < harness.state().2[0].min.y - 100.0, "scrolled to the newest");
    }

    fn long_error() -> Editor {
        let mut editor = Editor::new();
        editor.messages.push(msg("emsg", "first\n2\n3\n4\n5\n6\nlast"));
        editor
    }

    #[test]
    fn command_c_is_the_copy_key_and_only_that() {
        let key = |key, command, shift| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers { command, mac_cmd: command, shift, ..Default::default() },
        };
        assert!(is_copy(&egui::Event::Copy));
        assert!(is_copy(&key(egui::Key::C, true, false)));
        assert!(!is_copy(&key(egui::Key::C, false, false)), "a plain c answers the dialog");
        assert!(!is_copy(&key(egui::Key::C, true, true)));
        assert!(!is_copy(&key(egui::Key::V, true, false)));
    }

    #[test]
    fn the_dialog_is_open_for_long_output_and_messages_only() {
        let mut editor = Editor::new();
        assert!(!MessageView::dialog_open(&editor));
        editor.messages.push(msg("emsg", "short"));
        assert!(!MessageView::dialog_open(&editor), "a toast");
        assert!(MessageView::dialog_open(&long_error()));
        editor.message_history = Some(vec![msg("echo", "old")]);
        assert!(MessageView::dialog_open(&editor));
    }

    #[test]
    fn copy_with_nothing_selected_takes_the_whole_dialog() {
        let (copied, _) = dialog_frame(long_error(), vec![vec![egui::Event::Copy]]);
        assert_eq!(copied.last().map(String::as_str), Some("first\n2\n3\n4\n5\n6\nlast"));
    }

    #[test]
    fn nothing_is_copied_without_a_copy() {
        let (copied, _) = dialog_frame(long_error(), vec![vec![]]);
        assert!(copied.is_empty(), "{copied:?}");
    }

    #[test]
    fn toasts_stand_above_the_dialog() {
        let mut editor = long_error();
        editor.messages.push(msg("wmsg", "short"));
        let (_, rects) = dialog_frame(editor, vec![vec![]]);
        let (dialog, toast) = (rects[0], rects[1]);
        assert!(toast.max.y <= dialog.min.y, "{toast:?} over {dialog:?}");
        assert_eq!(toast.max.x, dialog.max.x, "same right edge");
    }

    #[test]
    fn a_warning_is_known_by_its_highlights_color_under_another_id() {
        use grid::{HlAttr, RedrawEvent};
        let mut editor = Editor::new();
        let yellow = HlAttr { foreground: Some(0xdbc074), ..Default::default() };
        editor.apply(RedrawEvent::HlAttrDefine { id: 7, attr: yellow.clone(), groups: vec![] });
        editor.apply(RedrawEvent::HlGroupSet { name: "WarningMsg".into(), id: 7 });
        editor.apply(RedrawEvent::HlAttrDefine { id: 90, attr: yellow, groups: vec![] });
        let chunk = |id| vec![StyledChunk { style: ChunkStyle::Id(id), text: "careful".into() }];
        let mut warning = msg("echomsg", "");
        warning.content = chunk(90);
        assert_eq!(severity_of(&editor, &warning), Some(Severity::Warning));
        warning.content = chunk(0);
        assert_eq!(severity_of(&editor, &warning), None, "plain text isn't a warning");
    }

    #[test]
    fn durations_and_stickiness() {
        assert_eq!(toast_duration("hi", None, ""), Duration::from_millis(4000));
        assert_eq!(toast_duration("hi", Some(Severity::Error), ""), Duration::from_millis(8000));
        assert_eq!(toast_duration(&"x".repeat(1000), None, ""), Duration::from_millis(12000));
        assert!(is_sticky(&msg("confirm", "Save changes?")));
        assert!(is_sticky(&msg("list_cmd", "  1 %a \"file\"")));
        assert!(is_sticky(&msg("echo", "1\n2\n3\n4\n5\n6\n7")));
        assert!(!is_sticky(&msg("emsg", "E492: Not an editor command")));
    }

    #[test]
    fn sentences_use_the_interfaces_font_and_output_the_grids() {
        let editor = Editor::new();
        for prose in [msg("emsg", "Push failed:\nhook said no"), msg("wmsg", "W10: read-only"), msg("confirm", "Save?\n[Y]es, [N]o"), msg("echo", "written")] {
            assert!(is_prose(&editor, &prose), "{}", prose.kind);
        }
        for output in [msg("list_cmd", "  1 %a \"file\""), msg("lua_print", "{\n  a = 1\n}")] {
            assert!(!is_prose(&editor, &output), "{}", output.kind);
        }
    }
}
