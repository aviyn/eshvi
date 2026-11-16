pub type HlId = u64;

/// Underline style of a highlight. Neovim allows only one at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Underline {
    #[default]
    None,
    Line,
    Curl,
    Double,
    Dotted,
    Dashed,
}

/// A highlight definition from `hl_attr_define` (rgb attributes).
/// Colors are `0xRRGGBB`; `None` means "use the default color".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HlAttr {
    pub foreground: Option<u32>,
    pub background: Option<u32>,
    pub special: Option<u32>,
    pub reverse: bool,
    pub italic: bool,
    pub bold: bool,
    pub strikethrough: bool,
    pub underline: Underline,
    /// 0..=100, used by floats/popupmenu for transparency.
    pub blend: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DefaultColors {
    pub fg: u32,
    pub bg: u32,
    pub sp: u32,
}

impl Default for DefaultColors {
    fn default() -> Self {
        Self { fg: 0xffffff, bg: 0x000000, sp: 0xff0000 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    Horizontal,
    Vertical,
}

/// One entry of `mode_info_set`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModeInfo {
    pub name: String,
    pub short_name: String,
    pub cursor_shape: CursorShape,
    /// Percentage of the cell the cursor occupies (horizontal/vertical shapes).
    pub cell_percentage: Option<u32>,
    /// Highlight of the cursor; 0 means "invert the cell colors".
    pub attr_id: HlId,
    pub blinkwait: u64,
    pub blinkon: u64,
    pub blinkoff: u64,
}

/// Resolved colors for drawing a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedColors {
    pub fg: u32,
    pub bg: u32,
    pub sp: u32,
}

impl HlAttr {
    /// Resolve the effective colors, applying defaults and `reverse`.
    pub fn colors(&self, defaults: &DefaultColors) -> ResolvedColors {
        let fg = self.foreground.unwrap_or(defaults.fg);
        let bg = self.background.unwrap_or(defaults.bg);
        let sp = self.special.unwrap_or(fg);
        if self.reverse {
            ResolvedColors { fg: bg, bg: fg, sp }
        } else {
            ResolvedColors { fg, bg, sp }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_swaps_colors() {
        let d = DefaultColors { fg: 1, bg: 2, sp: 3 };
        let attr = HlAttr { foreground: Some(10), reverse: true, ..Default::default() };
        let c = attr.colors(&d);
        assert_eq!((c.fg, c.bg, c.sp), (2, 10, 10));
    }
}
