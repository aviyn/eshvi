//! Font setup for the editor grid and popups, including `guifont`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use egui::epaint::text::VariationCoords;
use egui::{FontData, FontDefinitions, FontFamily, FontId};

/// Font request: family names in priority order and a size in points.
#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub families: Vec<String>,
    pub size: f32,
}

impl FontSpec {
    pub fn default_with_size(size: f32) -> Self {
        Self { families: vec![], size }
    }

    /// Parse a 'guifont' value such as `JetBrains_Mono,Menlo:h14` or
    /// `Fira\ Code:h12:b`. Underscores and `\ ` stand for spaces. Options
    /// other than `:h` are ignored. Returns `None` for an empty value.
    pub fn parse_guifont(value: &str, default_size: f32) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value == "*" {
            return None;
        }
        let mut size = default_size;
        let mut families = Vec::new();
        let mut current = String::new();
        let mut chars = value.chars().peekable();
        let mut in_opts = false;
        let mut opt = String::new();
        let flush_opt = |opt: &mut String, size: &mut f32| {
            if let Some(h) = opt.strip_prefix('h') {
                if let Ok(v) = h.parse::<f32>() {
                    if v > 0.0 {
                        *size = v;
                    }
                }
            }
            opt.clear();
        };
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        if in_opts { opt.push(n) } else { current.push(n) }
                    }
                }
                ':' => {
                    if in_opts {
                        flush_opt(&mut opt, &mut size);
                    }
                    in_opts = true;
                }
                ',' => {
                    if in_opts {
                        flush_opt(&mut opt, &mut size);
                        in_opts = false;
                    }
                    let name = current.trim().replace('_', " ");
                    if !name.is_empty() {
                        families.push(name);
                    }
                    current.clear();
                }
                c if in_opts => opt.push(c),
                c => current.push(c),
            }
        }
        if in_opts {
            flush_opt(&mut opt, &mut size);
        }
        let name = current.trim().replace('_', " ");
        if !name.is_empty() {
            families.push(name);
        }
        Some(Self { families, size })
    }
}

/// The grid font as installed in egui.
#[derive(Clone, Debug)]
pub struct GridFonts {
    pub size: f32,
    /// Size of the proportional sidebar font (follows zoom).
    pub sidebar_size: f32,
    /// Size of popups' prose (egui's text styles; follows zoom). `ui_font`'s
    /// size, else the grid's.
    pub ui_size: f32,
    pub has_bold: bool,
    pub has_italic: bool,
    /// Unique per installation, so a new font set can be detected as live.
    generation: u32,
    /// The family actually used, for display/debugging.
    pub primary: String,
    /// "grid", "agent" for the agent panel's font or "term" for the terminals'.
    prefix: &'static str,
    /// The agent panel's font (`[agent] font`), if it has its own.
    pub agent: Option<Box<GridFonts>>,
    /// The terminals' font (`[terminal] font`), if they have their own: they
    /// are drawn by the GUI, each with its own cell size, so it need not be
    /// the grid's.
    pub term: Option<Box<GridFonts>>,
}

impl GridFonts {
    /// For tests that draw no grid text: no fonts installed.
    #[cfg(test)]
    pub fn uninstalled(size: f32) -> Self {
        Self { size, sidebar_size: size, ui_size: size, has_bold: false, has_italic: false, generation: 0, primary: String::new(), prefix: "grid", agent: None, term: None }
    }

    fn family_name(&self, face: &str) -> String {
        format!("{}-{face}-{}", self.prefix, self.generation)
    }

    /// Font for a style; the bool says whether egui must fake italics.
    pub fn font_id(&self, bold: bool, italic: bool) -> (FontId, bool) {
        let bold = bold && self.has_bold;
        let real_italic = italic && self.has_italic;
        let face = match (bold, real_italic) {
            (false, false) => "regular",
            (true, false) => "bold",
            (false, true) => "italic",
            (true, true) => "bold-italic",
        };
        (FontId::new(self.size, FontFamily::Name(self.family_name(face).into())), italic && !real_italic)
    }

    /// Proportional font for sidebar windows (e.g. NvimTree).
    pub fn sidebar_font_id(&self, bold: bool) -> FontId {
        let face = if bold { "sidebar-bold" } else { "sidebar-regular" };
        FontId::new(self.sidebar_size, FontFamily::Name(self.family_name(face).into()))
    }

    /// Grid font at the sidebar size, for icons and indent guides in
    /// sidebar rows (keeps their monospace spacing).
    pub fn sidebar_icon_font_id(&self) -> FontId {
        FontId::new(self.sidebar_size, FontFamily::Name(self.family_name("regular").into()))
    }

    /// The font of terminals: their own, else the grid's.
    pub fn terminal(&self) -> &GridFonts {
        self.term.as_deref().unwrap_or(self)
    }

    /// Zoomed: the agent and terminal fonts change by as much as the grid font.
    pub fn with_size(&self, size: f32) -> Self {
        let delta = size - self.size;
        Self {
            term: self.term.as_ref().map(|t| Box::new(GridFonts { size: (t.size + delta).max(6.0), ..(**t).clone() })),
            size,
            sidebar_size: (self.sidebar_size + delta).max(6.0),
            ui_size: (self.ui_size + delta).max(6.0),
            agent: self.agent.as_ref().map(|a| Box::new(GridFonts { size: (a.size + delta).max(6.0), ..(**a).clone() })),
            ..self.clone()
        }
    }

    /// True once egui has loaded these fonts (they become live one pass
    /// after `set_fonts`).
    pub fn is_live(&self, ctx: &egui::Context) -> bool {
        let regular = FontFamily::Name(self.family_name("regular").into());
        ctx.fonts(|f| f.definitions().families.contains_key(&regular))
    }
}

/// Faces in the order regular, bold, italic, bold italic.
#[derive(Clone)]
struct FaceSet {
    name: String,
    faces: [Option<FontData>; 4],
}

fn load(path: &str) -> Option<Arc<Vec<u8>>> {
    std::fs::read(path).ok().map(Arc::new)
}

fn face(bytes: &Arc<Vec<u8>>, index: u32) -> FontData {
    FontData::from_blob(bytes.clone(), index)
}

fn builtin_monospace() -> Option<FaceSet> {
    // Menlo ships with macOS as a collection: 0 Regular, 1 Bold, 2 Italic, 3 Bold Italic.
    if let Some(b) = load("/System/Library/Fonts/Menlo.ttc") {
        return Some(FaceSet {
            name: "Menlo".into(),
            faces: [Some(face(&b, 0)), Some(face(&b, 1)), Some(face(&b, 2)), Some(face(&b, 3))],
        });
    }
    let linux = [
        ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf"),
        ("/usr/share/fonts/TTF/DejaVuSansMono.ttf", "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf"),
    ];
    for (r, b) in linux {
        if let Some(regular) = load(r) {
            return Some(FaceSet {
                name: "DejaVu Sans Mono".into(),
                faces: [Some(face(&regular, 0)), load(b).map(|b| face(&b, 0)), None, None],
            });
        }
    }
    None
}

fn collection() -> &'static Mutex<fontique::Collection> {
    static COLLECTION: OnceLock<Mutex<fontique::Collection>> = OnceLock::new();
    COLLECTION.get_or_init(|| {
        Mutex::new(fontique::Collection::new(fontique::CollectionOptions { shared: false, system_fonts: true }))
    })
}

/// Look up an installed font family by name.
fn system_family(name: &str) -> Option<FaceSet> {
    use fontique::{FontStyle, FontWeight, FontWidth};
    let mut coll = collection().lock().ok()?;
    let family = coll.family_by_name(name)?;
    let pick = |bold: bool, italic: bool| -> Option<FontData> {
        let weight = if bold { FontWeight::BOLD } else { FontWeight::NORMAL };
        let style = if italic { FontStyle::Italic } else { FontStyle::Normal };
        let info = family.match_font(FontWidth::NORMAL, style, weight, false)?;
        let real_weight = info.weight().value() >= 600.0 || info.has_weight_axis();
        let real_italic = info.style() != FontStyle::Normal || info.has_italic_axis() || info.has_slant_axis();
        if (bold && !real_weight) || (italic && !real_italic) {
            return None;
        }
        let blob = info.load(None)?;
        let mut data = FontData::from_blob(Arc::new(blob), info.index());
        if info.has_weight_axis() {
            data.tweak.coords = VariationCoords::new([("wght", if bold { 700.0 } else { 400.0 })]);
        }
        Some(data)
    };
    Some(FaceSet {
        name: family.name().to_owned(),
        faces: [pick(false, false), pick(true, false), pick(false, true), pick(true, true)],
    })
}

static GENERATION: AtomicU32 = AtomicU32::new(0);

/// The macOS UI font (SF, variable) at a given weight, tuned for small text.
fn system_ui_face(weight: f32) -> Option<FontData> {
    let bytes = load("/System/Library/Fonts/SFNS.ttf")?;
    let mut data = face(&bytes, 0);
    // The default optical size is the display cut; 17 is the text cut.
    data.tweak.coords = VariationCoords::new([("wght", weight), ("opsz", 17.0)]);
    Some(data)
}

/// The four grid faces (regular, bold, italic, bold italic) for `spec`, as
/// families `{prefix}-{face}-{generation}`. Unknown families fall back to
/// the platform monospace font, then `fallback` (font data keys), then
/// egui's bundled Hack.
fn install_grid_faces(
    defs: &mut FontDefinitions,
    spec: &FontSpec,
    prefix: &'static str,
    generation: u32,
    fallback: &[String],
) -> GridFonts {
    let hack_chain: Vec<String> = FontDefinitions::default().families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut sets: Vec<FaceSet> = spec.families.iter().filter_map(|n| system_family(n)).collect();
    if sets.is_empty() && !spec.families.is_empty() {
        log::warn!("font: none of {:?} found, using the default", spec.families);
    }
    sets.extend(builtin_monospace());

    let primary = sets.first().map(|s| s.name.clone()).unwrap_or_else(|| "Hack".into());
    let mut has = [false; 4];
    for (i, face_name) in ["regular", "bold", "italic", "bold-italic"].iter().enumerate() {
        let family = format!("{prefix}-{face_name}-{generation}");
        let mut chain = Vec::new();
        for (si, set) in sets.iter().enumerate() {
            // Each family contributes the matching face, else its regular face
            // (as a glyph fallback only).
            let data = set.faces[i].clone().or_else(|| if si > 0 { set.faces[0].clone() } else { None });
            if let Some(data) = data {
                if si == 0 && set.faces[i].is_some() {
                    has[i] = true;
                }
                let key = format!("{family}-{si}");
                defs.font_data.insert(key.clone(), Arc::new(data));
                chain.push(key);
            }
        }
        if i > 0 && !has[i] {
            // No real face: fall back to the regular chain (egui fakes italics).
            chain.clear();
        }
        chain.extend(fallback.iter().filter(|k| !chain.contains(k)).cloned().collect::<Vec<_>>());
        chain.extend(hack_chain.iter().cloned());
        defs.families.insert(FontFamily::Name(family.into()), chain);
    }
    GridFonts {
        size: spec.size,
        sidebar_size: spec.size,
        ui_size: spec.size,
        has_bold: has[1],
        has_italic: has[2],
        generation,
        primary,
        prefix,
        agent: None,
        term: None,
    }
}

/// Install fonts for `spec` into egui. `ui` is `ui_font`, the font of all UI
/// text (default: the system UI font, the grid's size); `sidebar` overrides
/// it for sidebar windows (default: `ui`, else the system UI font one point
/// smaller than the grid); `agent` picks the agent panel's and `terminal` the
/// terminals' (default: the grid font). The returned fonts become usable once
/// `GridFonts::is_live` is true (next pass).
pub fn install(
    ctx: &egui::Context,
    spec: &FontSpec,
    ui: Option<&FontSpec>,
    sidebar: Option<&FontSpec>,
    agent: Option<&FontSpec>,
    terminal: Option<&FontSpec>,
) -> GridFonts {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let mut defs = FontDefinitions::default();
    let hack_chain: Vec<String> = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut fonts = install_grid_faces(&mut defs, spec, "grid", generation, &[]);
    // The agent font falls back to the editor font (e.g. a Nerd Font's
    // icons, or symbols like ⏺ that Claude Code prints).
    let editor_chain: Vec<String> = defs.families[&FontFamily::Name(fonts.family_name("regular").into())]
        .iter()
        .filter(|k| !hack_chain.contains(k))
        .cloned()
        .collect();
    fonts.agent = agent.map(|a| Box::new(install_grid_faces(&mut defs, a, "agent", generation, &editor_chain)));
    fonts.term = terminal.map(|t| Box::new(install_grid_faces(&mut defs, t, "term", generation, &editor_chain)));
    fonts.sidebar_size = sidebar.or(ui).map(|s| s.size).unwrap_or(spec.size - 1.0).max(6.0);
    fonts.ui_size = ui.map(|s| s.size).unwrap_or(spec.size).max(6.0);
    let ui_set = ui.and_then(|s| s.families.iter().find_map(|n| system_family(n)));
    if let Some(u) = ui {
        if ui_set.is_none() && !u.families.is_empty() {
            log::warn!("ui_font: none of {:?} found, using the system UI font", u.families);
        }
    }

    // Sidebar: a proportional face first, then the grid chain so icon glyphs
    // (Nerd Font) still resolve.
    let grid_chain = defs.families[&FontFamily::Name(fonts.family_name("regular").into())].clone();
    let custom = sidebar.and_then(|s| s.families.iter().find_map(|n| system_family(n))).or_else(|| ui_set.clone());
    for (bold, face_name) in [(false, "sidebar-regular"), (true, "sidebar-bold")] {
        let data = match &custom {
            Some(set) => set.faces[bold as usize].clone().or_else(|| set.faces[0].clone()),
            None => system_ui_face(if bold { 600.0 } else { 400.0 }),
        };
        let family = fonts.family_name(face_name);
        let mut chain = Vec::new();
        if let Some(data) = data {
            defs.font_data.insert(family.clone(), Arc::new(data));
            chain.push(family.clone());
        }
        chain.extend(grid_chain.iter().cloned());
        defs.families.insert(FontFamily::Name(family.into()), chain);
    }

    // Popups: code in the grid font, prose in the system UI font if found.
    let regular_chain = defs.families[&FontFamily::Name(fonts.family_name("regular").into())].clone();
    let mono = defs.families.entry(FontFamily::Monospace).or_default();
    for (i, k) in regular_chain.iter().filter(|k| !hack_chain.contains(k)).enumerate() {
        mono.insert(i, k.clone());
    }
    let prose = match &ui_set {
        Some(set) => set.faces[0].clone(),
        None => system_ui_face(400.0),
    };
    if let Some(ui_font) = prose {
        defs.font_data.insert("system-ui".to_owned(), Arc::new(ui_font));
        defs.families.entry(FontFamily::Proportional).or_default().insert(0, "system-ui".to_owned());
    }

    ctx.set_fonts(defs);
    fonts
}

/// Status bar font size: `[statusline] font_size`, else a point below
/// `ui_font`'s (tabs, a point larger than the bar, then match it), else 12.
pub fn status_base_size(config: Option<f32>, ui: Option<&FontSpec>) -> f32 {
    config.or(ui.map(|u| u.size - 1.0)).unwrap_or(12.0).clamp(6.0, 40.0)
}

/// Explorer font size: `[explorer] font_size`, else `ui_font`'s, else 13.
pub fn explorer_base_size(config: Option<f32>, ui: Option<&FontSpec>) -> f32 {
    config.or(ui.map(|u| u.size)).unwrap_or(13.0).clamp(8.0, 30.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_guifont_values() {
        assert_eq!(FontSpec::parse_guifont("", 13.0), None);
        assert_eq!(
            FontSpec::parse_guifont("JetBrains_Mono,Menlo:h14", 13.0),
            Some(FontSpec { families: vec!["JetBrains Mono".into(), "Menlo".into()], size: 14.0 })
        );
        assert_eq!(
            FontSpec::parse_guifont("Fira\\ Code:h12:b", 13.0),
            Some(FontSpec { families: vec!["Fira Code".into()], size: 12.0 })
        );
        assert_eq!(
            FontSpec::parse_guifont("Menlo:h15.5,Hack", 13.0),
            Some(FontSpec { families: vec!["Menlo".into(), "Hack".into()], size: 15.5 })
        );
        assert_eq!(FontSpec::parse_guifont("Menlo", 11.0).unwrap().size, 11.0);
        assert_eq!(FontSpec::parse_guifont("SF Pro:h13", 11.0).unwrap().families, vec!["SF Pro".to_string()]);
    }

    #[test]
    fn status_and_explorer_sizes_follow_ui_font_unless_set() {
        let ui = FontSpec::parse_guifont("SF Pro:h14", 13.0);
        assert_eq!((status_base_size(None, None), explorer_base_size(None, None)), (12.0, 13.0));
        assert_eq!((status_base_size(None, ui.as_ref()), explorer_base_size(None, ui.as_ref())), (13.0, 14.0));
        assert_eq!((status_base_size(Some(11.0), ui.as_ref()), explorer_base_size(Some(15.0), ui.as_ref())), (11.0, 15.0));
    }

    #[test]
    fn terminals_have_their_own_font_or_the_grids() {
        let ctx = egui::Context::default();
        let grid = FontSpec::default_with_size(14.0);
        let own = FontSpec::parse_guifont("Menlo:h11", 14.0).unwrap();
        // No `[terminal] font`: the grid's.
        let f = install(&ctx, &grid, None, None, None, None);
        assert!(f.term.is_none());
        assert_eq!(f.terminal().size, 14.0);
        // Its own: another size and its own families (so its own cell).
        let f = install(&ctx, &grid, None, None, None, Some(&own));
        assert_eq!(f.terminal().size, 11.0);
        assert_ne!(f.terminal().font_id(false, false).0.family, f.font_id(false, false).0.family);
        // Zoom moves it by as much as the grid font.
        assert_eq!(f.with_size(16.0).terminal().size, 13.0);
    }

    #[test]
    fn ui_font_sets_the_ui_sizes_and_sidebar_overrides_its_own() {
        let ctx = egui::Context::default();
        let grid = FontSpec::default_with_size(14.0);
        let ui = FontSpec::parse_guifont("Georgia:h16", 13.0).unwrap();
        let side = FontSpec::parse_guifont("Georgia:h11", 13.0).unwrap();
        // No `ui_font`: today's sizes (the sidebar a point below the grid).
        let f = install(&ctx, &grid, None, None, None, None);
        assert_eq!((f.sidebar_size, f.ui_size), (13.0, 14.0));
        let f = install(&ctx, &grid, Some(&ui), None, None, None);
        assert_eq!((f.sidebar_size, f.ui_size), (16.0, 16.0));
        let f = install(&ctx, &grid, Some(&ui), Some(&side), None, None);
        assert_eq!((f.sidebar_size, f.ui_size), (11.0, 16.0));
        // Zoom moves them by as much as the grid.
        let z = f.with_size(16.0);
        assert_eq!((z.sidebar_size, z.ui_size), (13.0, 18.0));
    }
}
