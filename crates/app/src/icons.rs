//! File and folder icons from the bundled Material Icon Theme (MIT, see
//! `assets/material-icons.LICENSE`), looked up like VS Code does and
//! rasterized to egui textures on demand.

use std::collections::HashMap;
use std::sync::OnceLock;

use egui::{ColorImage, TextureHandle, TextureOptions};
use serde::Deserialize;

static THEME_JSON: &str = include_str!("../../../assets/material-icons.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IconTheme {
    file_extensions: HashMap<String, String>,
    file_names: HashMap<String, String>,
    folder_names: HashMap<String, String>,
    folder_names_expanded: HashMap<String, String>,
    file: String,
    folder: String,
    folder_expanded: String,
    /// SVG source by icon name.
    icons: HashMap<String, String>,
}

impl IconTheme {
    /// The bundled theme, parsed on first use.
    pub fn get() -> &'static IconTheme {
        static THEME: OnceLock<IconTheme> = OnceLock::new();
        THEME.get_or_init(|| serde_json::from_str(THEME_JSON).expect("bundled icon theme"))
    }

    /// Icon for a file: exact file name first, then extensions from the
    /// longest (`d.ts` before `ts`), then the generic file icon.
    pub fn file_icon(&self, name: &str) -> &str {
        let lower = name.to_lowercase();
        if let Some(icon) = self.file_names.get(&lower) {
            return icon;
        }
        lower
            .char_indices()
            .filter(|&(_, c)| c == '.')
            .find_map(|(i, _)| self.file_extensions.get(&lower[i + 1..]))
            .unwrap_or(&self.file)
    }

    pub fn folder_icon(&self, name: &str, open: bool) -> &str {
        let lower = name.to_lowercase();
        let (names, default) =
            if open { (&self.folder_names_expanded, &self.folder_expanded) } else { (&self.folder_names, &self.folder) };
        names.get(&lower).unwrap_or(default)
    }

    pub fn svg(&self, icon: &str) -> Option<&str> {
        self.icons.get(icon).map(String::as_str)
    }
}

/// What an unknown codicon name is drawn as.
const GENERIC_CODICON: &str = "info";

/// VS Code's Codicons (CC BY 4.0, see `assets/codicons/`), for the problem
/// severities and the title bars of the Graph and the Outline: one color (`currentColor`), rasterized
/// white so they're drawn tinted.
pub fn codicon(name: &str) -> Option<&'static str> {
    match name {
        "error" => Some(include_str!("../../../assets/codicons/error.svg")),
        "warning" => Some(include_str!("../../../assets/codicons/warning.svg")),
        "info" => Some(include_str!("../../../assets/codicons/info.svg")),
        "git-fetch" => Some(include_str!("../../../assets/codicons/git-fetch.svg")),
        "repo-pull" => Some(include_str!("../../../assets/codicons/repo-pull.svg")),
        "repo-push" => Some(include_str!("../../../assets/codicons/repo-push.svg")),
        "collapse-all" => Some(include_str!("../../../assets/codicons/collapse-all.svg")),
        "type-hierarchy" => Some(include_str!("../../../assets/codicons/type-hierarchy.svg")),
        "refresh" => Some(include_str!("../../../assets/codicons/refresh.svg")),
        "references" => Some(include_str!("../../../assets/codicons/references.svg")),
        "arrow-swap" => Some(include_str!("../../../assets/codicons/arrow-swap.svg")),
        "copy" => Some(include_str!("../../../assets/codicons/copy.svg")),
        "check" => Some(include_str!("../../../assets/codicons/check.svg")),
        "history" => Some(include_str!("../../../assets/codicons/history.svg")),
        "filter" => Some(include_str!("../../../assets/codicons/filter.svg")),
        "filter-filled" => Some(include_str!("../../../assets/codicons/filter-filled.svg")),
        _ => None,
    }
}

/// Rasterize an SVG into a `px` × `px` image, keeping its aspect ratio.
pub fn rasterize(svg: &str, px: u32) -> Option<ColorImage> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
    let size = tree.size();
    let scale = px as f32 / size.width().max(size.height());
    let dx = (px as f32 - size.width() * scale) / 2.0;
    let dy = (px as f32 - size.height() * scale) / 2.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(ColorImage::from_rgba_premultiplied([px as usize, px as usize], pixmap.data()))
}

/// `image` in shades of white by luminance, scaled so its brightest part
/// is white (premultiplied, as `rasterize` gives it).
fn grayscale(mut image: ColorImage) -> ColorImage {
    let luma = |p: &egui::Color32| 0.2126 * p.r() as f32 + 0.7152 * p.g() as f32 + 0.0722 * p.b() as f32;
    let max = image.pixels.iter().filter(|p| p.a() > 0).map(|p| luma(p) / p.a() as f32).fold(0.0, f32::max);
    for p in &mut image.pixels {
        let a = p.a();
        let l = if max > 0.0 { (luma(p) / max).min(a as f32) as u8 } else { a };
        *p = egui::Color32::from_rgba_premultiplied(l, l, l, a);
    }
    image
}

/// Icon textures by (icon name, size in physical pixels).
#[derive(Default)]
pub struct IconTextures {
    textures: HashMap<(String, u32), Option<TextureHandle>>,
}

impl IconTextures {
    pub fn get(&mut self, ctx: &egui::Context, icon: &str, px: u32) -> Option<&TextureHandle> {
        self.textures
            .entry((icon.to_owned(), px))
            .or_insert_with(|| {
                let image = rasterize(IconTheme::get().svg(icon)?, px)?;
                Some(ctx.load_texture(format!("icon-{icon}-{px}"), image, TextureOptions::LINEAR))
            })
            .as_ref()
    }

    /// A file or folder icon without its colors: shades of white (its
    /// brightest part white), to draw tinted, e.g. dimmed for a tab that
    /// isn't the current one.
    pub fn mono(&mut self, ctx: &egui::Context, icon: &str, px: u32) -> Option<&TextureHandle> {
        self.textures
            .entry((format!("mono:{icon}"), px))
            .or_insert_with(|| {
                let image = grayscale(rasterize(IconTheme::get().svg(icon)?, px)?);
                Some(ctx.load_texture(format!("icon-mono-{icon}-{px}"), image, TextureOptions::LINEAR))
            })
            .as_ref()
    }

    /// A Codicon (`codicon`), white, to draw tinted.
    pub fn codicon(&mut self, ctx: &egui::Context, name: &str, px: u32) -> Option<&TextureHandle> {
        // (A name that isn't in the bundled set is drawn as the generic one.)
        let name = if codicon(name).is_some() { name } else { GENERIC_CODICON };
        self.textures
            .entry((format!("codicon:{name}"), px))
            .or_insert_with(|| {
                let image = rasterize(&codicon(name)?.replace("currentColor", "#ffffff"), px)?;
                Some(ctx.load_texture(format!("codicon-{name}-{px}"), image, TextureOptions::LINEAR))
            })
            .as_ref()
    }

    /// Draw a Codicon at `center`, `size` points square, on whole pixels so it stays sharp.
    pub fn paint_codicon(&mut self, painter: &egui::Painter, name: &str, center: egui::Pos2, size: f32, color: egui::Color32) {
        let ppp = painter.ctx().pixels_per_point();
        let px = (size * ppp).round();
        // The warning triangle looks high beside the circles (its weight is
        // at the bottom): a pixel lower.
        let nudge = if name == "warning" { 1.0 } else { 0.0 };
        let min = ((center.to_vec2() * ppp - egui::Vec2::splat(px / 2.0)).round() + egui::Vec2::new(0.0, nudge)) / ppp;
        let rect = egui::Rect::from_min_size(min.to_pos2(), egui::Vec2::splat(px / ppp));
        if let Some(tex) = self.codicon(painter.ctx(), name, px as u32) {
            let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0));
            painter.image(tex.id(), rect, uv, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_like_vscode() {
        let t = IconTheme::get();
        assert_eq!(t.file_icon("main.rs"), "rust");
        assert_eq!(t.file_icon("README.md"), "readme");
        assert_eq!(t.file_icon(".gitignore"), "git");
        assert_eq!(t.file_icon("types.d.ts"), t.file_extensions["d.ts"]);
        assert_eq!(t.file_icon("no_extension_here"), "file");
        assert_eq!(t.folder_icon("src", false), "folder-src");
        assert_eq!(t.folder_icon("src", true), "folder-src-open");
        assert_eq!(t.folder_icon("whatever", true), "folder-open");
    }

    #[test]
    fn codicons_rasterize_white() {
        for name in ["error", "warning", "info", "git-fetch", "repo-pull", "repo-push", "collapse-all", "type-hierarchy", "refresh", "references", "arrow-swap", "filter", "filter-filled", "copy", "check", "history"] {
            let img = rasterize(&codicon(name).unwrap().replace("currentColor", "#ffffff"), 32).unwrap();
            let drawn: Vec<_> = img.pixels.iter().filter(|p| p.a() > 200).collect();
            assert!(!drawn.is_empty(), "{name} is blank");
            assert!(drawn.iter().all(|p| p.r() > 200 && p.g() > 200 && p.b() > 200), "{name} isn't white");
        }
        assert!(codicon("nope").is_none());
    }

    #[test]
    fn rasterizes_svg() {
        let t = IconTheme::get();
        let img = rasterize(t.svg("rust").unwrap(), 32).unwrap();
        assert_eq!(img.size, [32, 32]);
        assert!(img.pixels.iter().any(|p| p.a() > 0), "icon is not blank");
        // Without its colors: gray, the same shape, the brightest part white.
        let gray = grayscale(img.clone());
        assert!(gray.pixels.iter().all(|p| p.r() == p.g() && p.g() == p.b() && p.r() <= p.a()));
        assert!(img.pixels.iter().zip(&gray.pixels).all(|(a, b)| a.a() == b.a()));
        assert!(gray.pixels.iter().any(|p| p.a() > 200 && p.r() as u16 + 2 >= p.a() as u16));
    }
}
