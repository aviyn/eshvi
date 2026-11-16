use super::*;
use egui::{Align2, Pos2, Rect, Vec2};
use egui_kittest::Harness;

/// The files that draw the chrome: strips, tabs, the title bar, popups,
/// the explorer. (The renderer and the popup theme derive their colors
/// and aren't in it.)
const CHROME: &[(&str, &str)] = &[
    ("app/tabs.rs", include_str!("../app/tabs.rs")),
    ("app/bottom_panel.rs", include_str!("../app/bottom_panel.rs")),
    ("app/chrome.rs", include_str!("../app/chrome.rs")),
    ("app/status.rs", include_str!("../app/status.rs")),
    ("app/agent.rs", include_str!("../app/agent.rs")),
    ("app/actions.rs", include_str!("../app/actions.rs")),
    ("app/layout.rs", include_str!("../app/layout.rs")),
    ("changes_popup.rs", include_str!("../changes_popup.rs")),
    ("diff_view/paint.rs", include_str!("../diff_view/paint.rs")),
    ("app/diff.rs", include_str!("../app/diff.rs")),
    ("messages.rs", include_str!("../messages.rs")),
    ("preview.rs", include_str!("../preview.rs")),
    ("lsp_popups.rs", include_str!("../lsp_popups.rs")),
    ("popupmenu.rs", include_str!("../popupmenu.rs")),
    ("picker.rs", include_str!("../picker.rs")),
    ("picker/preview.rs", include_str!("../picker/preview.rs")),
    ("cmdline.rs", include_str!("../cmdline.rs")),
    ("explorer_view.rs", include_str!("../explorer_view.rs")),
    ("scm_view.rs", include_str!("../scm_view.rs")),
    ("side_bar.rs", include_str!("../side_bar.rs")),
    ("tree_view.rs", include_str!("../tree_view.rs")),
    ("terminal_view.rs", include_str!("../terminal_view.rs")),
];

/// The arguments of the call whose "(" is at `open`, split at top-level
/// commas.
fn args(text: &str, open: usize) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0, open + 1, Vec::new());
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    out.push(text[start..open + i].trim());
                    return out;
                }
            }
            ',' if depth == 1 => {
                out.push(text[start..open + i].trim());
                start = open + i + 1;
            }
            _ => {}
        }
    }
    out
}

fn is_number(arg: &str) -> bool {
    arg.trim_end_matches("_f32").parse::<f32>().is_ok()
}

/// Literal shades, radii, strokes and margins in `text`: (line, call).
fn literal_styles(text: &str) -> Vec<(usize, String)> {
    // Comments out (a "//" in a string would cut it short; none here).
    let text: String = text.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    // (call, which argument, whether 0 is fine: a square fill)
    let checks: &[(&str, usize, bool)] = &[
        ("mix(", 2, false),
        ("rect_filled(", 1, true),
        ("rect_stroke(", 1, true),
        (".rect(", 1, true),
        ("Stroke::new(", 0, false),
        ("CornerRadius::same(", 0, true),
        (".corner_radius(", 0, true),
        ("Margin::same(", 0, false),
        ("Margin::symmetric(", 0, false),
        (".inner_margin(", 0, false),
    ];
    let mut found = Vec::new();
    for &(call, n, zero_ok) in checks {
        for (at, _) in text.match_indices(call) {
            // `fn mix(` and the like are definitions, not uses.
            if text[..at].ends_with("fn ") {
                continue;
            }
            let args = args(&text, at + call.len() - 1);
            let Some(arg) = args.get(n) else { continue };
            let zero = arg.trim_end_matches("_f32").parse::<f32>().is_ok_and(|v| v == 0.0);
            if is_number(arg) && !(zero && zero_ok) {
                let line = text[..at].matches('\n').count() + 1;
                found.push((line, format!("{call}… {arg}")));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn chrome_uses_style_tokens() {
    let mut bad = Vec::new();
    for (file, text) in CHROME {
        for (line, call) in literal_styles(text) {
            bad.push(format!("{file}:{line}: {call}"));
        }
    }
    assert!(bad.is_empty(), "draw these with style.rs's tokens:\n{}", bad.join("\n"));
}

#[test]
fn the_check_finds_literals() {
    let text = "let a = mix(x, y, 0.12);\nlet b = mix(x, y, style::HOVER);\np.rect_filled(r, 4.0, c);\np.rect_filled(r, 0.0, c);\n// mix(x, y, 0.3)\nStroke::new(1.4, c)";
    let found: Vec<usize> = literal_styles(text).into_iter().map(|(l, _)| l).collect();
    assert_eq!(found, vec![1, 3, 6]);
}

/// Dark and light themes as the tests draw them.
pub(crate) fn themes() -> [(&'static str, Chrome, PopupTheme); 2] {
    let accent = Color32::from_rgb(0x4f, 0xa6, 0xed);
    let popup = |bg: Color32, fg: Color32| PopupTheme {
        bg,
        fg,
        dim_fg: mix(fg, bg, DIM),
        sel_bg: mix(bg, fg, 0.18),
        sel_fg: fg,
        border: mix(bg, fg, 0.15),
        accent,
        corner_radius: 6.0,
        shadow: true,
    };
    let dark = (Color32::from_rgb(0x28, 0x2c, 0x34), Color32::from_rgb(0xab, 0xb2, 0xbf));
    let light = (Color32::from_rgb(0xfa, 0xfa, 0xfa), Color32::from_rgb(0x38, 0x3a, 0x42));
    [
        ("dark", Chrome::new(dark.0, dark.1, accent), popup(mix(dark.0, dark.1, 0.06), dark.1)),
        ("light", Chrome::new(light.0, light.1, accent), popup(mix(light.0, Color32::BLACK, 0.04), light.1)),
    ]
}

/// One of each piece the chrome is made of, in every state: a strip with a
/// focused current tab, an unfocused one and one at rest; "×" and "+" at
/// rest and hovered; a lit button; a chip and badges; a popup with text
/// buttons.
fn gallery(ui: &mut egui::Ui, c: &Chrome, theme: &PopupTheme) {
    let p = ui.painter().clone();
    let full = ui.max_rect();
    p.rect_filled(full, 0.0, c.bg);
    let font = egui::FontId::proportional(13.0);
    let row_h = 26.0;

    // A strip of tabs.
    let strip = Rect::from_min_size(full.min, Vec2::new(full.width(), row_h));
    p.rect_filled(strip, 0.0, c.panel);
    p.hline(strip.x_range(), strip.max.y - 0.5, Stroke::new(HAIRLINE, c.line()));
    let mut x = strip.min.x;
    for (label, current, focused) in [("main.rs", true, true), ("lib.rs", false, false), ("notes.md", true, false)] {
        let g = p.layout_no_wrap(label.into(), font.clone(), c.text(current && focused));
        let w = TAB_PAD + g.size().x + GAP + button_size(row_h) + TAB_PAD * 0.5;
        let tab = Rect::from_min_size(Pos2::new(x, strip.min.y), Vec2::new(w.round(), row_h));
        x = tab.max.x;
        if current {
            paint_current_tab(&p, tab, focused, c, c.bg);
        }
        paint_tab_divider(&p, tab, c);
        p.galley(Pos2::new(tab.min.x + TAB_PAD, tab.center().y - g.size().y / 2.0), g, c.fg);
        if current {
            let close = Pos2::new(tab.max.x - TAB_PAD * 0.5 - button_size(row_h) / 2.0, tab.center().y);
            glyph_button(&p, Glyph::Cross, close, row_h, c, tab_fill(c, c.bg, current, focused), !focused);
        }
    }
    glyph_button(&p, Glyph::Plus, Pos2::new(x + row_h / 2.0 + 2.0, strip.center().y), row_h, c, c.panel, false);

    // Buttons on the panel: at rest, hovered, lit; a chip; badges.
    let row = Rect::from_min_size(Pos2::new(full.min.x, strip.max.y), Vec2::new(full.width(), row_h + 12.0));
    p.rect_filled(row, 0.0, c.panel);
    let cy = row.center().y;
    let mut x = row.min.x + 16.0;
    for (glyph, hovered) in [(Glyph::Plus, false), (Glyph::Plus, true), (Glyph::Cross, false), (Glyph::Cross, true)] {
        glyph_button(&p, glyph, Pos2::new(x, cy), row_h, c, c.panel, hovered);
        x += button_size(row_h) + 6.0;
    }
    let lit = Rect::from_center_size(Pos2::new(x + 10.0, cy), Vec2::new(button_size(row_h) * 1.4, button_size(row_h)));
    paint_button_fill(&p, lit, c, c.panel, false, true);
    paint_glyph(&p, Glyph::Plus, lit.center(), row_h, c.text(true));
    x = lit.max.x + 12.0;
    let small = scaled(&font, SMALL);
    for level in [SUBTLE, ON] {
        let g = p.layout_no_wrap("+8 −3".into(), small.clone(), c.fg);
        let chip = Rect::from_min_size(Pos2::new(x, cy - 9.0), Vec2::new(g.size().x + 20.0, 18.0));
        p.rect_filled(chip, round(chip.height()), c.shade(c.panel, level));
        p.galley(chip.center() - g.size() / 2.0, g, c.fg);
        x = chip.max.x + 8.0;
    }
    let red = Color32::from_rgb(0xe0, 0x6c, 0x75);
    let badge_font = scaled(&font, BADGE);
    let g = p.layout_no_wrap("12".into(), badge_font.clone(), c.fg);
    let badge = Rect::from_center_size(Pos2::new(x + 12.0, cy), Vec2::new(g.size().x + 10.0, g.size().y + 2.0));
    p.rect_filled(badge, round(badge.height()), mix(c.panel, red, TINT));
    p.galley(badge.center() - g.size() / 2.0, g, c.fg);
    let letter = Rect::from_center_size(Pos2::new(badge.max.x + 20.0, cy), Vec2::splat(18.0));
    p.rect_filled(letter, RADIUS, mix(c.panel, red, WASH));
    p.text(letter.center(), Align2::CENTER_CENTER, "M", small.clone(), red);
    p.text(Pos2::new(letter.max.x + 12.0, cy), Align2::LEFT_CENTER, "dimmed", font.clone(), c.text(false));
    p.text(Pos2::new(letter.max.x + 70.0, cy), Align2::LEFT_CENTER, "faint", font.clone(), c.faint());

    // A popup with text buttons, and a chip.
    let popup = Rect::from_min_size(Pos2::new(full.min.x + 16.0, row.max.y + 16.0), Vec2::new(300.0, 70.0));
    let child = egui::UiBuilder::new().max_rect(popup);
    ui.scope_builder(child, |ui| {
        theme.frame().inner_margin(POPUP_PAD).show(ui, |ui| {
            ui.set_width(popup.width() - 22.0);
            ui.label(egui::RichText::new("fn gallery(ui: &mut Ui)").font(font.clone()).color(theme.fg));
            ui.label(egui::RichText::new("One of each piece").font(small.clone()).color(theme.dim_fg));
            let pc = Chrome { fg: theme.fg, ..*c };
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), egui::Sense::hover());
            let mut right = r.max.x;
            for (label, fill, color) in [
                ("Accept all", primary_fill(theme.accent, false), Color32::WHITE),
                ("Accept", text_button_fill(&pc, theme.bg, true), theme.fg),
                ("Reject", text_button_fill(&pc, theme.bg, false), theme.fg),
            ] {
                let g = ui.painter().layout_no_wrap(label.into(), small.clone(), color);
                let b = Rect::from_min_max(Pos2::new(right - g.size().x - 16.0, r.min.y), Pos2::new(right, r.max.y));
                ui.painter().rect_filled(b, RADIUS, fill);
                ui.painter().galley(b.center() - g.size() / 2.0, g, color);
                right = b.min.x - 6.0;
            }
        });
    });
    let chip = Rect::from_min_size(Pos2::new(popup.max.x + 24.0, popup.min.y + 20.0), Vec2::new(120.0, 24.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(chip), |ui| {
        chip_frame(theme).show(ui, |ui| ui.label(egui::RichText::new("recording @q").font(small.clone()).color(theme.fg)));
    });

    // The icons, one hovered and one lit, then "−" beside the "+".
    let icons = Rect::from_min_size(Pos2::new(full.min.x, popup.max.y + 16.0), Vec2::new(full.width(), row_h + 8.0));
    p.rect_filled(icons, 0.0, c.panel);
    let mut x = icons.min.x + 16.0;
    for (i, icon) in [Icon::Files, Icon::Branch, Icon::Check, Icon::Refresh, Icon::Undo, Icon::OpenFile, Icon::Sync, Icon::ChevronDown]
        .into_iter()
        .enumerate()
    {
        let (hovered, on) = (i == 1, i == 0);
        let button = Rect::from_center_size(Pos2::new(x, icons.center().y), Vec2::splat(button_size(row_h)));
        paint_button_fill(&p, button, c, c.panel, hovered, on);
        paint_icon(&p, icon, button.center(), icon_size(row_h), c.text(on || hovered));
        x += button_size(row_h) + 6.0;
    }
    for glyph in [Glyph::Plus, Glyph::Minus] {
        glyph_button(&p, glyph, Pos2::new(x, icons.center().y), row_h, c, c.panel, false);
        x += button_size(row_h) + 6.0;
    }
}

#[test]
fn chrome_gallery() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (name, chrome, theme) in themes() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(540.0, 236.0))
            .with_pixels_per_point(2.0)
            .wgpu()
            .build_ui(move |ui| gallery(ui, &chrome, &theme));
        harness.run();
        harness.snapshot(format!("chrome_gallery_{name}"));
        results.extend_harness(&mut harness);
    }
    results.unwrap();
}

#[test]
fn egui_menus_take_the_popup_look() {
    let (_, _, theme) = themes()[0];
    let ctx = egui::Context::default();
    apply_to_egui(&ctx, &theme);
    let v = ctx.global_style().visuals.clone();
    assert_eq!(v.window_fill, theme.bg);
    assert_eq!(v.window_stroke.color, theme.border);
    assert_eq!(v.widgets.hovered.weak_bg_fill, theme.sel_bg);
    assert_eq!(v.menu_corner_radius, theme.radius());
}

#[test]
fn text_fields_share_one_edge() {
    for (_, chrome, _) in themes() {
        let color = |focused, hovered| field_stroke(&chrome, focused, hovered).color;
        assert_eq!(color(false, false), chrome.shade(chrome.bg, HOVER));
        assert_eq!(color(false, true), chrome.shade(chrome.bg, ON));
        // The keyboard wins over the pointer.
        assert_eq!(color(true, false), chrome.accent);
        assert_eq!(color(true, true), chrome.accent);
        assert_eq!(field_stroke(&chrome, false, false).width, HAIRLINE);
    }
}
