//! Per-file decorations shown in the explorer and on tabs: LSP problem
//! counts and git status, with folders summarizing their contents.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::Color32;
use grid::Editor;

use crate::renderer::rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitStatus {
    // Ordered by importance: a folder shows its most important child.
    Untracked,
    Added,
    Renamed,
    Modified,
    Deleted,
    Conflict,
}

impl GitStatus {
    /// Letter shown next to the name, like VS Code.
    pub fn letter(self) -> &'static str {
        match self {
            GitStatus::Untracked => "U",
            GitStatus::Added => "A",
            GitStatus::Renamed => "R",
            GitStatus::Modified => "M",
            GitStatus::Deleted => "D",
            GitStatus::Conflict => "!",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Problems {
    pub errors: u32,
    pub warnings: u32,
}

/// What to draw for one explorer row or tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Deco {
    pub problems: Problems,
    pub git: Option<GitStatus>,
}

#[derive(Default)]
pub struct Decorations {
    problems: HashMap<PathBuf, Problems>,
    git: HashMap<PathBuf, GitStatus>,
    dir_problems: HashMap<PathBuf, Problems>,
    dir_git: HashMap<PathBuf, GitStatus>,
}

impl Decorations {
    pub fn set_problems(&mut self, files: impl IntoIterator<Item = (PathBuf, Problems)>) {
        self.problems = files.into_iter().filter(|(_, p)| p.errors + p.warnings > 0).collect();
        self.dir_problems.clear();
        for (path, p) in &self.problems {
            for dir in path.ancestors().skip(1) {
                let d = self.dir_problems.entry(dir.to_path_buf()).or_default();
                d.errors += p.errors;
                d.warnings += p.warnings;
            }
        }
    }

    pub fn set_git(&mut self, files: HashMap<PathBuf, GitStatus>) {
        self.git = files;
        self.dir_git.clear();
        for (path, status) in &self.git {
            for dir in path.ancestors().skip(1) {
                let d = self.dir_git.entry(dir.to_path_buf()).or_insert(*status);
                *d = (*d).max(*status);
            }
        }
    }

    pub fn get(&self, path: &Path, is_dir: bool) -> Deco {
        if is_dir {
            Deco {
                problems: self.dir_problems.get(path).copied().unwrap_or_default(),
                git: self.dir_git.get(path).copied(),
            }
        } else {
            Deco { problems: self.problems.get(path).copied().unwrap_or_default(), git: self.git.get(path).copied() }
        }
    }

    pub fn totals(&self) -> Problems {
        self.problems.values().fold(Problems::default(), |a, p| Problems {
            errors: a.errors + p.errors,
            warnings: a.warnings + p.warnings,
        })
    }
}

/// Decoration colors from the theme's highlight groups, with VS Code's
/// colors as fallback.
#[derive(Clone, Copy, Debug)]
pub struct DecoColors {
    pub error: Color32,
    pub warning: Color32,
    pub info: Color32,
    pub added: Color32,
    pub modified: Color32,
    pub deleted: Color32,
    /// The text color: what a name's git color is softened toward.
    pub text: Color32,
}

impl DecoColors {
    /// The theme's colors, each kept readable on the panel (a light theme's
    /// pale green or yellow would vanish): moved toward the text color
    /// until it has `MIN_CONTRAST`, as plugins' floats are.
    pub fn from_editor(editor: &Editor) -> Self {
        let text = rgb(editor.default_colors.fg);
        let panel = crate::style::panel_bg(rgb(editor.default_colors.bg));
        // The reported theme color first, then the group lookup (as before),
        // then VS Code's color.
        let fg = |role: &str, groups: &[&str], fallback: Color32| {
            let color = editor
                .deco_theme
                .get(role)
                .copied()
                .or_else(|| groups.iter().find_map(|g| editor.hl_groups.get(*g).and_then(|id| editor.hl_attr(*id).foreground)))
                .map(rgb)
                .unwrap_or(fallback);
            crate::popup::with_contrast(color, panel, text, crate::popup::MIN_CONTRAST)
        };
        Self {
            error: fg("error", &["DiagnosticError"], Color32::from_rgb(0xf1, 0x4c, 0x4c)),
            warning: fg("warn", &["DiagnosticWarn"], Color32::from_rgb(0xcc, 0xa7, 0x00)),
            info: fg("info", &["DiagnosticInfo"], Color32::from_rgb(0x37, 0x94, 0xff)),
            added: fg("added", &["Added", "DiffAdd"], Color32::from_rgb(0x73, 0xc9, 0x91)),
            modified: fg("modified", &["Changed", "DiffChange"], Color32::from_rgb(0xe2, 0xc0, 0x8d)),
            deleted: fg("deleted", &["Removed", "DiffDelete"], Color32::from_rgb(0xc7, 0x4e, 0x39)),
            text,
        }
    }

    pub fn git(&self, status: GitStatus) -> Color32 {
        match status {
            GitStatus::Untracked | GitStatus::Added | GitStatus::Renamed => self.added,
            GitStatus::Modified => self.modified,
            GitStatus::Deleted | GitStatus::Conflict => self.deleted,
        }
    }

    /// A file name's color for its git status: softened, as the letter beside
    /// it has the full color.
    pub fn git_name(&self, status: GitStatus) -> Color32 {
        crate::popup::mix(self.git(status), self.text, crate::style::HALF)
    }

    /// Name color: git status only. Problems show in their count (a folder's
    /// dot), so a file with errors keeps a name you can read.
    pub fn name(&self, deco: &Deco) -> Option<Color32> {
        deco.git.map(|g| self.git_name(g))
    }

    /// A folder's dot: problems win over git status.
    pub fn marker(&self, deco: &Deco) -> Option<Color32> {
        if deco.problems.errors > 0 {
            Some(self.error)
        } else if deco.problems.warnings > 0 {
            Some(self.warning)
        } else {
            deco.git.map(|g| self.git(g))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_summarize_children() {
        let mut d = Decorations::default();
        d.set_problems([
            (PathBuf::from("/p/src/a.rs"), Problems { errors: 2, warnings: 1 }),
            (PathBuf::from("/p/src/x/b.rs"), Problems { errors: 0, warnings: 3 }),
            (PathBuf::from("/p/ok.rs"), Problems::default()),
        ]);
        assert_eq!(d.get(Path::new("/p/src"), true).problems, Problems { errors: 2, warnings: 4 });
        assert_eq!(d.get(Path::new("/p/ok.rs"), false).problems, Problems::default());
        assert_eq!(d.totals(), Problems { errors: 2, warnings: 4 });

        d.set_git(HashMap::from([
            (PathBuf::from("/p/src/a.rs"), GitStatus::Untracked),
            (PathBuf::from("/p/src/x/b.rs"), GitStatus::Modified),
        ]));
        assert_eq!(d.get(Path::new("/p/src"), true).git, Some(GitStatus::Modified));
        assert_eq!(d.get(Path::new("/p/src/x"), true).git, Some(GitStatus::Modified));
        assert_eq!(d.get(Path::new("/p/src/a.rs"), false).git, Some(GitStatus::Untracked));
    }

    fn editor(fg: u32, bg: u32, groups: &[(&str, u32)]) -> Editor {
        let mut editor = Editor::new();
        editor.default_colors = grid::DefaultColors { fg, bg, sp: 0 };
        for (i, (name, color)) in groups.iter().enumerate() {
            editor.hl.insert(i as u64 + 1, grid::HlAttr { foreground: Some(*color), ..Default::default() });
            editor.hl_groups.insert((*name).to_owned(), i as u64 + 1);
        }
        editor
    }

    #[test]
    fn problems_color_the_marker_and_not_the_name() {
        let c = DecoColors::from_editor(&editor(0xcdcecf, 0x192330, &[]));
        let deco = |errors, warnings, git| Deco { problems: Problems { errors, warnings }, git };
        assert_eq!(c.name(&deco(2, 1, None)), None);
        assert_eq!(c.marker(&deco(2, 1, None)), Some(c.error));
        assert_eq!(c.name(&deco(0, 1, None)), None);
        assert_eq!(c.marker(&deco(0, 1, None)), Some(c.warning));
        // Git status still colors the name, and only a clean folder's dot.
        let modified = Some(GitStatus::Modified);
        assert_eq!(c.name(&deco(2, 0, modified)), Some(c.git_name(GitStatus::Modified)));
        assert_eq!(c.marker(&deco(2, 0, modified)), Some(c.error));
        assert_eq!(c.marker(&deco(0, 0, modified)), Some(c.modified));
        assert_eq!(c.marker(&deco(0, 0, None)), None);
    }

    #[test]
    fn a_names_git_color_is_softer_than_its_letters() {
        let c = DecoColors::from_editor(&editor(0xcdcecf, 0x192330, &[("Changed", 0xdbc074)]));
        let soft = c.git_name(GitStatus::Modified);
        assert_eq!(soft, crate::popup::mix(rgb(0xdbc074), rgb(0xcdcecf), crate::style::HALF));
        assert_ne!(soft, c.modified);
        assert_ne!(soft, c.text);
        assert_eq!(c.git(GitStatus::Modified), c.modified);
    }

    #[test]
    fn reported_theme_colors_come_first_then_groups_then_vscode() {
        let mut e = editor(0xcdcecf, 0x192330, &[("Added", 0x81b29a), ("DiagnosticError", 0xc94f6d)]);
        e.deco_theme.insert("added".into(), 0x63cdcf);
        e.deco_theme.insert("modified".into(), 0xdbc074);
        let c = DecoColors::from_editor(&e);
        // Reported beats the group in `hl_groups`.
        assert_eq!(c.added, rgb(0x63cdcf));
        assert_eq!(c.modified, rgb(0xdbc074));
        // Not reported: the group lookup, then VS Code's.
        assert_eq!(c.error, rgb(0xc94f6d));
        assert_eq!(c.deleted, Color32::from_rgb(0xc7, 0x4e, 0x39));
        assert_eq!(c.warning, Color32::from_rgb(0xcc, 0xa7, 0x00));
        // The softening starts from the reported color.
        assert_eq!(c.git_name(GitStatus::Added), crate::popup::mix(rgb(0x63cdcf), rgb(0xcdcecf), crate::style::HALF));
        assert_eq!(c.git(GitStatus::Added), c.added);
    }

    #[test]
    fn reported_colors_still_get_the_contrast_step() {
        let mut e = editor(0x575279, 0xfaf4ed, &[]);
        e.deco_theme.insert("added".into(), 0xb3d9a5);
        let c = DecoColors::from_editor(&e);
        assert_ne!(c.added, rgb(0xb3d9a5));
        let panel = crate::style::panel_bg(rgb(0xfaf4ed));
        assert!(crate::popup::contrast(c.added, panel) >= crate::popup::MIN_CONTRAST - 0.05);
    }

    #[test]
    fn colors_stay_readable_on_the_panel() {
        let panel = |e: &Editor| crate::style::panel_bg(rgb(e.default_colors.bg));
        // A light theme's pale green and yellow (dawnfox-like) are moved
        // toward the text until they can be read, and no further.
        let light = editor(0x575279, 0xfaf4ed, &[("Added", 0xb3d9a5), ("Changed", 0xf0e0a0), ("DiagnosticError", 0x7a2a3a)]);
        let colors = DecoColors::from_editor(&light);
        for c in [colors.added, colors.modified, colors.error] {
            assert!(crate::popup::contrast(c, panel(&light)) >= crate::popup::MIN_CONTRAST - 0.05, "{c:?}");
        }
        assert_ne!(colors.added, rgb(0xb3d9a5));
        // Already readable: as the theme has it. So are VS Code's fallbacks on a dark one.
        assert_eq!(colors.error, rgb(0x7a2a3a));
        let dark = editor(0xcdcecf, 0x192330, &[("Added", 0x81b29a)]);
        let colors = DecoColors::from_editor(&dark);
        assert_eq!(colors.added, rgb(0x81b29a));
        assert_eq!(colors.modified, Color32::from_rgb(0xe2, 0xc0, 0x8d));
        assert_eq!(colors.deleted, Color32::from_rgb(0xc7, 0x4e, 0x39));
    }
}
