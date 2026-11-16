//! Shared look of popups, derived from the user's Neovim highlight groups:
//! the GUI's own (completion menu, hover, signature help, command line,
//! messages) and, mapped onto it by color role, plugins' floats
//! (`FloatPalette`).

use std::sync::OnceLock;

use egui::{Color32, CornerRadius, Margin, Pos2, Rect, Shadow, Stroke, Vec2};
use grid::Editor;

use crate::config::ColorOverrides;
use crate::renderer::rgb;

/// Popup look from the config file; set once at startup.
#[derive(Clone, Copy, Debug)]
pub struct ThemeSettings {
    pub colors: ColorOverrides,
    pub corner_radius: f32,
    pub shadow: bool,
    /// Draw plugins' floats in the popup colors (`FloatPalette`).
    pub restyle_floats: bool,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self { colors: ColorOverrides::default(), corner_radius: 6.0, shadow: true, restyle_floats: true }
    }
}

static SETTINGS: OnceLock<ThemeSettings> = OnceLock::new();

pub fn set_settings(settings: ThemeSettings) {
    let _ = SETTINGS.set(settings);
}

pub fn settings() -> ThemeSettings {
    SETTINGS.get().copied().unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupTheme {
    pub bg: Color32,
    pub fg: Color32,
    pub dim_fg: Color32,
    pub sel_bg: Color32,
    pub sel_fg: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub corner_radius: f32,
    pub shadow: bool,
}

/// Mix two colors; `t` = 0 gives `a`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// The theme's float background (`NormalFloat`, else `Normal`'s).
pub fn normal_float_bg(editor: &Editor) -> Color32 {
    let d = &editor.default_colors;
    editor.hl_groups.get("NormalFloat").map_or(rgb(d.bg), |id| rgb(editor.hl_attr(*id).colors(d).bg))
}

/// WCAG contrast ratio between two colors (1 to 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let luminance = |c: Color32| {
        let lin = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The least contrast text has on its background in the GUI's own drawing
/// and in plugins' floats (`with_contrast`).
pub const MIN_CONTRAST: f32 = 3.0;

/// `fg`, moved toward `toward` just enough to reach `min` contrast on `bg`
/// (or all the way, if even that is short of it).
pub fn with_contrast(fg: Color32, bg: Color32, toward: Color32, min: f32) -> Color32 {
    if contrast(fg, bg) >= min {
        return fg;
    }
    (1..=20).map(|i| mix(fg, toward, i as f32 / 20.0)).find(|&c| contrast(c, bg) >= min).unwrap_or(toward)
}

/// The fill of a plugin's input line drawn as a field (snacks' picker
/// prompt): a little darker than the float around it, like an inset.
pub fn input_field_bg(float_bg: Color32) -> Color32 {
    mix(float_bg, Color32::BLACK, 0.22)
}

impl PopupTheme {
    pub fn from_editor(editor: &Editor) -> Self {
        let d = &editor.default_colors;
        let group = |name: &str| {
            editor.hl_groups.get(name).map(|id| {
                let c = editor.hl_attr(*id).colors(d);
                (rgb(c.fg), rgb(c.bg))
            })
        };
        let (normal_fg, normal_bg) = (rgb(d.fg), rgb(d.bg));
        let (fg, bg) = group("Pmenu").or_else(|| group("NormalFloat")).unwrap_or((normal_fg, normal_bg));
        // A popup that is indistinguishable from the editor gets lifted a bit.
        let bg = if bg == normal_bg { mix(normal_bg, normal_fg, 0.06) } else { bg };
        let (sel_fg, sel_bg) = group("PmenuSel").unwrap_or((fg, mix(bg, fg, 0.18)));
        let sel_bg = if sel_bg == bg { mix(bg, fg, 0.18) } else { sel_bg };
        let st = settings();
        let o = st.colors;
        let (bg, fg) = (o.popup_bg.unwrap_or(bg), o.popup_fg.unwrap_or(fg));
        Self {
            bg,
            fg,
            dim_fg: mix(fg, bg, crate::style::DIM),
            sel_bg: o.popup_selection.unwrap_or(sel_bg),
            sel_fg,
            border: o.popup_border.unwrap_or(mix(bg, fg, 0.15)),
            accent: o.accent.unwrap_or(Color32::from_rgb(0x4f, 0xa6, 0xed)),
            corner_radius: st.corner_radius,
            shadow: st.shadow,
        }
    }

    pub fn frame(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.bg)
            .stroke(Stroke::new(crate::style::HAIRLINE, self.border))
            .corner_radius(self.radius())
            .inner_margin(Margin::same(4))
            .shadow(self.shadow())
    }

    /// The popups' corners (`[popup] corner_radius`).
    pub fn radius(&self) -> CornerRadius {
        CornerRadius::same(self.corner_radius.clamp(0.0, 30.0) as u8)
    }

    /// The popups' shadow, unless turned off (`[popup] shadow`).
    pub fn shadow(&self) -> Shadow {
        if self.shadow { crate::style::shadow() } else { Shadow::NONE }
    }
}

/// Scroll bars in a popup like the editor's: a thin thumb in the popup's
/// colors, no track, shown while the pointer is over it (wider while over
/// the bar itself).
pub fn thin_scrollbars(ui: &mut egui::Ui, theme: &PopupTheme) {
    let s = &mut ui.spacing_mut().scroll;
    *s = egui::style::ScrollStyle::floating();
    s.bar_width = 6.0;
    s.floating_width = 3.0;
    s.floating_allocated_width = 0.0;
    s.bar_inner_margin = 2.0;
    s.bar_outer_margin = 2.0;
    s.foreground_color = false;
    s.dormant_background_opacity = 0.0;
    s.active_background_opacity = 0.0;
    s.interact_background_opacity = 0.0;
    s.dormant_handle_opacity = 0.0;
    let v = ui.visuals_mut();
    v.extreme_bg_color = theme.bg;
    v.widgets.inactive.bg_fill = mix(theme.bg, theme.fg, 0.3);
    v.widgets.hovered.bg_fill = mix(theme.bg, theme.fg, 0.42);
    v.widgets.active.bg_fill = mix(theme.bg, theme.fg, 0.55);
}

/// Where to put a popup of `size` anchored at a cursor cell: below the
/// anchor row if it fits, otherwise above. Kept inside `screen`.
pub fn place(screen: Rect, anchor_top: Pos2, cell_height: f32, size: Vec2, prefer_above: bool) -> Pos2 {
    let below = anchor_top.y + cell_height;
    let above = anchor_top.y - size.y;
    let fits_below = below + size.y <= screen.max.y;
    let fits_above = above >= screen.min.y;
    let y = match (prefer_above, fits_above, fits_below) {
        (true, true, _) | (false, true, false) => above,
        (_, _, true) => below,
        // Fits neither: use the side with more room.
        _ => {
            if anchor_top.y - screen.min.y > screen.max.y - below {
                screen.min.y.max(above)
            } else {
                below
            }
        }
    };
    let x = anchor_top.x.min(screen.max.x - size.x).max(screen.min.x);
    Pos2::new(x, y)
}

/// A plugin's float drawn as a popup, by color role rather than per plugin:
/// the theme's float background (`NormalFloat`) becomes the popup's, the
/// border the popup border, and a background that would then vanish into the
/// popup (a cursor line the colorscheme gives `Pmenu`'s color) the
/// selection's. Other colors (titles, text, a plugin's own highlights) stay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatPalette {
    /// The float background being replaced.
    pub float_bg: Color32,
    pub bg: Color32,
    pub border: Color32,
    pub sel_bg: Color32,
}

impl FloatPalette {
    /// `None` when turned off, or when floats already look like popups.
    pub fn from_editor(editor: &Editor) -> Option<Self> {
        if !settings().restyle_floats {
            return None;
        }
        let float_bg = normal_float_bg(editor);
        let theme = PopupTheme::from_editor(editor);
        (float_bg != theme.bg).then_some(Self { float_bg, bg: theme.bg, border: theme.border, sel_bg: theme.sel_bg })
    }

    /// Whether a float with background `base` is restyled: one in the
    /// float color (a plugin's popup), not one with a look of its own (a
    /// terminal, a `Pmenu`-colored completion menu).
    pub fn applies_to(&self, base: Color32) -> bool {
        base == self.float_bg
    }

    pub fn map_bg(&self, color: Color32) -> Color32 {
        if color == self.float_bg {
            self.bg
        } else if color == self.bg {
            self.sel_bg
        } else {
            color
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_flips_above_near_bottom() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let size = Vec2::new(200.0, 150.0);
        assert_eq!(place(screen, Pos2::new(10.0, 100.0), 20.0, size, false), Pos2::new(10.0, 120.0));
        assert_eq!(place(screen, Pos2::new(10.0, 500.0), 20.0, size, false), Pos2::new(10.0, 350.0));
        assert_eq!(place(screen, Pos2::new(10.0, 500.0), 20.0, size, true), Pos2::new(10.0, 350.0));
        assert_eq!(place(screen, Pos2::new(10.0, 100.0), 20.0, size, true), Pos2::new(10.0, 120.0));
        // Clamped horizontally.
        assert_eq!(place(screen, Pos2::new(700.0, 100.0), 20.0, size, false).x, 600.0);
    }
    #[test]
    fn contrast_floor_lifts_dim_text_only_as_far_as_needed() {
        let c = |v: u8| Color32::from_rgb(v, v, v);
        assert!((contrast(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.01);
        let (bg, dim, fg) = (Color32::from_rgb(0x3e, 0x44, 0x52), Color32::from_rgb(0x5a, 0x65, 0x7d), c(0xcd));
        assert!(contrast(dim, bg) < 2.0);
        let lifted = with_contrast(dim, bg, fg, 3.0);
        assert!(contrast(lifted, bg) >= 3.0 && contrast(lifted, bg) < 3.6, "{lifted:?}");
        // Still dimmer than normal text; readable text is left alone.
        assert!(contrast(lifted, bg) < contrast(fg, bg));
        assert_eq!(with_contrast(fg, bg, Color32::WHITE, 3.0), fg);
        assert!(input_field_bg(bg).r() < bg.r());
    }

    #[test]
    fn float_palette_maps_float_background_and_lifts_collisions() {
        let c = |v: u8| Color32::from_rgb(v, v, v);
        let p = FloatPalette { float_bg: c(0x23), bg: c(0x3e), border: c(0x53), sel_bg: c(0x4f) };
        assert!(p.applies_to(c(0x23)) && !p.applies_to(c(0x2b)));
        // The float color becomes the popup's.
        assert_eq!(p.map_bg(c(0x23)), c(0x3e));
        // A cursor line in the popup's color would vanish: the selection.
        assert_eq!(p.map_bg(c(0x3e)), c(0x4f));
        // Anything else stays.
        assert_eq!(p.map_bg(c(0x80)), c(0x80));
    }
}
