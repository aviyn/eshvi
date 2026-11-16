//! Terminal colors: the 16 ANSI colors and the default foreground and
//! background, and how a cell's color (named, indexed or true color) turns
//! into RGB. The UI sets them from the colorscheme; unset ones keep
//! Alacritty's.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

const fn rgb(c: u32) -> Rgb {
    Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 }
}

/// Alacritty's default 16 colors (its default theme).
const DEFAULT_ANSI: [Rgb; 16] = [
    rgb(0x181818),
    rgb(0xac4242),
    rgb(0x90a959),
    rgb(0xf4bf75),
    rgb(0x6a9fb5),
    rgb(0xaa759f),
    rgb(0x75b5aa),
    rgb(0xd8d8d8),
    rgb(0x6b6b6b),
    rgb(0xc55555),
    rgb(0xaac474),
    rgb(0xfeca88),
    rgb(0x82b8c8),
    rgb(0xc28cb8),
    rgb(0x93d3c3),
    rgb(0xf8f8f8),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub fg: Rgb,
    pub bg: Rgb,
}

impl Default for Palette {
    fn default() -> Self {
        Self { ansi: DEFAULT_ANSI, fg: rgb(0xd8d8d8), bg: rgb(0x181818) }
    }
}

/// How much `DIM` text is dimmed toward the background.
const DIM: f32 = 0.66;

impl Palette {
    /// The default colors with the colorscheme's: Normal's foreground and
    /// background, and `g:terminal_color_0..15` where set (`None`: Alacritty's).
    pub fn from_theme(fg: u32, bg: u32, ansi: &[Option<u32>; 16]) -> Self {
        let mut palette = Self { fg: rgb(fg), bg: rgb(bg), ..Self::default() };
        for (i, c) in ansi.iter().enumerate() {
            if let Some(c) = c {
                palette.ansi[i] = rgb(*c);
            }
        }
        palette
    }

    /// Color `index` of the 256-color table (also what OSC 4 asks about) or one
    /// of the special ones (256 foreground, 257 background, 258 cursor).
    pub fn indexed(&self, index: usize) -> Rgb {
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let i = index - 16;
                let level = |n: usize| if n == 0 { 0 } else { (55 + 40 * n) as u8 };
                Rgb { r: level(i / 36), g: level(i / 6 % 6), b: level(i % 6) }
            }
            232..=255 => {
                let v = (8 + 10 * (index - 232)) as u8;
                Rgb { r: v, g: v, b: v }
            }
            257 => self.bg,
            _ => self.fg,
        }
    }

    /// A cell's color. `overrides` are the colors the program set itself (OSC
    /// 4, 10 and 11); `dim` is for `DIM` text (a foreground only).
    pub fn resolve(&self, color: Color, overrides: &Colors, dim: bool) -> Rgb {
        let (rgb, can_dim) = match color {
            Color::Spec(rgb) => (rgb, true),
            Color::Indexed(i) => (overrides[i as usize].unwrap_or_else(|| self.indexed(i as usize)), true),
            Color::Named(named) => {
                let index = named as usize;
                let own = overrides[index];
                match named {
                    NamedColor::Foreground | NamedColor::BrightForeground => (own.unwrap_or(self.fg), true),
                    NamedColor::DimForeground => (own.unwrap_or_else(|| self.dimmed(self.fg)), false),
                    NamedColor::Background => (own.unwrap_or(self.bg), false),
                    NamedColor::Cursor => (own.unwrap_or(self.fg), false),
                    // The dim colors are 259..=266, the dimmed 0..=7.
                    _ if index >= NamedColor::DimBlack as usize => {
                        let base = self.ansi[index - NamedColor::DimBlack as usize];
                        (own.unwrap_or_else(|| self.dimmed(base)), false)
                    }
                    _ => (own.unwrap_or(self.ansi[index]), true),
                }
            }
        };
        if dim && can_dim { self.dimmed(rgb) } else { rgb }
    }

    fn dimmed(&self, c: Rgb) -> Rgb {
        let toward = |c: u8, bg: u8| (c as f32 * DIM + bg as f32 * (1.0 - DIM)).round() as u8;
        Rgb { r: toward(c.r, self.bg.r), g: toward(c.g, self.bg.g), b: toward(c.b, self.bg.b) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_colors_replace_only_the_ones_set() {
        let mut ansi = [None; 16];
        ansi[1] = Some(0xff0000);
        let p = Palette::from_theme(0xeeeeee, 0x101010, &ansi);
        assert_eq!((p.fg, p.bg, p.ansi[1]), (rgb(0xeeeeee), rgb(0x101010), rgb(0xff0000)));
        assert_eq!(p.ansi[2], DEFAULT_ANSI[2], "unset colors keep Alacritty's");
    }

    #[test]
    fn the_256_table() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), rgb(0x000000));
        assert_eq!(p.indexed(196), rgb(0xff0000));
        assert_eq!(p.indexed(21), rgb(0x0000ff));
        assert_eq!(p.indexed(231), rgb(0xffffff));
        assert_eq!(p.indexed(232), rgb(0x080808));
        assert_eq!(p.indexed(255), rgb(0xeeeeee));
    }

    #[test]
    fn cells_resolve_named_indexed_and_true_colors() {
        let p = Palette::default();
        let none = Colors::default();
        assert_eq!(p.resolve(Color::Named(NamedColor::Red), &none, false), DEFAULT_ANSI[1]);
        assert_eq!(p.resolve(Color::Named(NamedColor::BrightBlue), &none, false), DEFAULT_ANSI[12]);
        assert_eq!(p.resolve(Color::Named(NamedColor::Foreground), &none, false), p.fg);
        assert_eq!(p.resolve(Color::Named(NamedColor::Background), &none, false), p.bg);
        assert_eq!(p.resolve(Color::Indexed(9), &none, false), DEFAULT_ANSI[9]);
        assert_eq!(p.resolve(Color::Spec(rgb(0x123456)), &none, false), rgb(0x123456));
        // The program's own OSC 4 color wins.
        let mut set = Colors::default();
        set[1] = Some(rgb(0x00ff00));
        assert_eq!(p.resolve(Color::Named(NamedColor::Red), &set, false), rgb(0x00ff00));
        // Dim text moves toward the background; a background doesn't dim.
        let dim = p.resolve(Color::Named(NamedColor::Foreground), &none, true);
        assert!(dim.r < p.fg.r && dim.r > p.bg.r);
        assert_eq!(p.resolve(Color::Named(NamedColor::Background), &none, true), p.bg);
    }
}
