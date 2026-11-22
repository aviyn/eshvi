//! The left side bar shows one view at a time. Its header is one row, the
//! same for both: a button for each view, the current view's title, and
//! room at the right for the view's own buttons.

use egui::{Align2, FontId, Id, Pos2, Rect, Sense, Vec2};

use nvim_bridge::gui::{TreeButton, TreeFilter};

use crate::explorer_view::ExplorerColors;
use crate::icons::IconTextures;
use crate::style::{self, Icon};

/// What the side bar shows: the explorer, Source Control, or a tree view that
/// Neovim gave a place of its own (`Eshvi.tree.register` with `side`; the number
/// is `SideInfo::id`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SideView {
    #[default]
    Explorer,
    Git,
    Tree(u32),
}

/// A tree view's place in the side bar: its button, title and header buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct SideInfo {
    pub id: u32,
    /// The tree view's name in Neovim.
    pub view: String,
    pub title: String,
    /// A codicon's name.
    pub icon: String,
    pub tooltip: String,
    pub buttons: Vec<TreeButton>,
    /// The toggles of its Filter menu; none, no Filter button.
    pub filters: Vec<TreeFilter>,
}

/// The tree views that have a place of their own in the side bar, in the order
/// they registered.
#[derive(Default)]
pub struct SideViews {
    list: Vec<SideInfo>,
    last: u32,
}

impl SideViews {
    /// `view` registered with `side`; registering again changes its look and keeps its place.
    pub fn register(&mut self, view: &str, side: nvim_bridge::gui::TreeSide) -> u32 {
        let id = self.by_view(view).map(|i| i.id).unwrap_or_else(|| {
            self.last += 1;
            self.last
        });
        let info = SideInfo { id, view: view.to_owned(), title: side.title, icon: side.icon, tooltip: side.tooltip, buttons: side.buttons, filters: side.filters };
        match self.list.iter_mut().find(|i| i.id == id) {
            Some(slot) => *slot = info,
            None => self.list.push(info),
        }
        id
    }

    /// `view` was unregistered: its button goes. Of `current`, what the side bar shows
    /// now: the Explorer if it was this.
    pub fn unregister(&mut self, view: &str, current: SideView) -> SideView {
        let Some(at) = self.list.iter().position(|i| i.view == view) else { return current };
        let gone = self.list.remove(at);
        if current == SideView::Tree(gone.id) {
            SideView::Explorer
        } else {
            current
        }
    }

    pub fn by_view(&self, view: &str) -> Option<&SideInfo> {
        self.list.iter().find(|i| i.view == view)
    }

    pub fn by_id(&self, id: u32) -> Option<&SideInfo> {
        self.list.iter().find(|i| i.id == id)
    }

    pub fn as_slice(&self) -> &[SideInfo] {
        &self.list
    }
}

impl SideInfo {
    /// How many buttons of its own the header draws before Collapse All.
    pub fn header_buttons(&self) -> usize {
        self.buttons.len() + usize::from(!self.filters.is_empty())
    }
}

/// How a view's button is drawn.
enum Glyph<'a> {
    Drawn(Icon),
    Codicon(&'a str),
}

impl SideView {
    fn built_in_title(self) -> &'static str {
        match self {
            Self::Explorer => "EXPLORER",
            Self::Git => "SOURCE CONTROL",
            Self::Tree(_) => "",
        }
    }

    fn built_in_icon(self) -> Icon {
        match self {
            Self::Explorer | Self::Tree(_) => Icon::Files,
            Self::Git => Icon::Branch,
        }
    }

    fn built_in_tooltip(self) -> &'static str {
        match self {
            Self::Explorer => "Explorer (⇧⌘E)",
            Self::Git => "Source Control (⌃⇧G)",
            Self::Tree(_) => "",
        }
    }
}

/// The views the header has buttons for, in order: the two built in, then the
/// trees' in the order they registered.
pub fn views(extra: &[SideInfo]) -> impl Iterator<Item = SideView> + '_ {
    [SideView::Explorer, SideView::Git].into_iter().chain(extra.iter().map(|i| SideView::Tree(i.id)))
}

/// A row of the side bar's views (and the file lists in them) at the given
/// font size.
pub fn row_height(font_size: f32) -> f32 {
    (font_size * 1.75).round()
}

/// The count in the Source Control button's badge: none at 0.
fn badge_text(count: usize) -> Option<String> {
    match count {
        0 => None,
        1..=99 => Some(count.to_string()),
        _ => Some("99+".to_owned()),
    }
}

/// Draw the header row: a button for each view (the current one lit, the
/// Source Control one with `count` changes in a badge), then the title,
/// cut with "…" before `reserved_right` points at the right, which the
/// view keeps for its own buttons, and dropped when it has no room.
/// The row is `height` tall (every strip's, `[layout] tab_height`).
/// Returns the row and the view whose button was clicked.
#[allow(clippy::too_many_arguments)]
pub fn header(
    ui: &mut egui::Ui,
    view: SideView,
    count: usize,
    reserved_right: f32,
    row_h: f32,
    height: f32,
    font: &FontId,
    colors: &ExplorerColors,
    extra: &[SideInfo],
    icons: &mut IconTextures,
) -> (Rect, Option<SideView>) {
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let chrome = colors.chrome();
    // (Buttons keep inside a low row.)
    let side = row_h.min(height);
    let mut clicked = None;
    let mut x = row.min.x + style::GAP;
    for v in views(extra) {
        let info = extra.iter().find(|i| SideView::Tree(i.id) == v);
        let (tooltip, glyph) = match info {
            Some(i) => (if i.tooltip.is_empty() { i.title.as_str() } else { i.tooltip.as_str() }, Glyph::Codicon(&i.icon)),
            None => (v.built_in_tooltip(), Glyph::Drawn(v.built_in_icon())),
        };
        let rect = Rect::from_min_size(Pos2::new(x, row.center().y - side / 2.0), Vec2::splat(side));
        let resp = ui.interact(rect, Id::unique(("eshvi-side-view", v)), Sense::CLICK).on_hover_text(tooltip);
        let on = v == view;
        style::paint_button_fill(ui.painter(), rect.shrink(2.0), &chrome, colors.bg, resp.hovered(), on);
        let (size, color) = (style::icon_size(side), chrome.text(on || resp.hovered()));
        match glyph {
            Glyph::Drawn(icon) => style::paint_icon(ui.painter(), icon, rect.center(), size, color),
            Glyph::Codicon(name) => icons.paint_codicon(ui.painter(), name, rect.center(), size, color),
        }
        if v == SideView::Git {
            if let Some(text) = badge_text(count) {
                paint_badge(ui, rect, &text, font, colors);
            }
        }
        if resp.clicked() {
            clicked = Some(v);
        }
        // (A gap between the two, so their lit fills don't touch.)
        x = rect.max.x + style::GAP / 3.0;
    }
    // The title, in what is left.
    let title_font = style::scaled(font, style::BADGE);
    let left = x + style::GAP;
    let room = row.max.x - reserved_right - left;
    let title = match view {
        SideView::Tree(id) => extra.iter().find(|i| i.id == id).map_or("", |i| i.title.as_str()),
        built_in => built_in.built_in_title(),
    };
    let mut job = egui::text::LayoutJob::single_section(
        title.to_owned(),
        egui::TextFormat::simple(title_font.clone(), colors.dim),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(room.max(0.0));
    let galley = ui.painter().layout_job(job);
    // (Nothing but "…" is no title.)
    if room >= title_font.size * 3.0 && galley.size().x > 0.0 {
        let top = row.center().y - galley.size().y / 2.0;
        ui.painter().galley(Pos2::new(left, top), galley, colors.dim);
    }
    (row, clicked)
}

/// The change count at the lower right of a button: white on the accent,
/// fully round, small and out in the corner so the icon stays whole.
fn paint_badge(ui: &egui::Ui, button: Rect, text: &str, font: &FontId, colors: &ExplorerColors) {
    let p = ui.painter();
    let font = style::scaled(font, style::TINY);
    let galley = p.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE);
    let h = galley.size().y + 2.0;
    let size = Vec2::new((galley.size().x + 4.0).max(h), h);
    // Its right edge a little past the button's, which the title's gap
    // leaves room for; a wide one grows to the left.
    let corner = button.max + Vec2::new(style::GAP / 2.0, -1.0);
    let badge = Rect::from_min_size(corner - size, size);
    p.rect_filled(badge, style::round(h), colors.accent);
    p.text(badge.center(), Align2::CENTER_CENTER, text, font, egui::Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_counts_up_to_99() {
        assert_eq!(badge_text(0), None);
        assert_eq!(badge_text(7).as_deref(), Some("7"));
        assert_eq!(badge_text(99).as_deref(), Some("99"));
        assert_eq!(badge_text(100).as_deref(), Some("99+"));
    }
}
