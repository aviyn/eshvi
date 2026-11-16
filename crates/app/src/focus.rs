//! Who has the keyboard, and where ⌃h/j/k/l go from there (tmux style): the
//! regions of the window are Neovim's grid (the editor), the side bar on its
//! left, the bottom panel under it and the agent panel, full height on the right.
//! Pure, so the moves are tested as a table.

/// A direction of ⌃h/j/k/l.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

impl Dir {
    /// From `Eshvi.navigate`'s argument ("h", "j", "k", "l").
    pub fn from_letter(s: &str) -> Option<Self> {
        Some(match s {
            "h" => Self::Left,
            "j" => Self::Down,
            "k" => Self::Up,
            "l" => Self::Right,
            _ => return None,
        })
    }

    pub fn letter(self) -> char {
        match self {
            Self::Left => 'h',
            Self::Down => 'j',
            Self::Up => 'k',
            Self::Right => 'l',
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Down => Self::Up,
            Self::Up => Self::Down,
        }
    }

    /// ⌃h, ⌃j, ⌃k and ⌃l.
    pub fn from_key(key: egui::Key) -> Option<Self> {
        Some(match key {
            egui::Key::H => Self::Left,
            egui::Key::J => Self::Down,
            egui::Key::K => Self::Up,
            egui::Key::L => Self::Right,
            _ => return None,
        })
    }
}

/// What has the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    /// Neovim.
    #[default]
    Editor,
    /// The side bar: the explorer, Source Control or a tree view.
    Side,
    /// The bottom panel's Problems list.
    Problems,
    /// A terminal of the bottom panel, by its view's id.
    Terminal(u64),
    /// An agent of the agent panel, by its view's id.
    Agent(u64),
    /// The diff view that covers the editor area.
    Diff,
}

impl Focus {
    pub fn region(self) -> Region {
        match self {
            Self::Editor | Self::Diff => Region::Editor,
            Self::Side => Region::Side,
            Self::Problems | Self::Terminal(_) => Region::Panel,
            Self::Agent(_) => Region::Agent,
        }
    }

    /// The bottom panel has it.
    pub fn in_panel(self) -> bool {
        self.region() == Region::Panel
    }

    /// The agent panel has it.
    pub fn in_agent(self) -> bool {
        self.region() == Region::Agent
    }

    /// A terminal (the bottom panel's or an agent) has it: its program gets the keys.
    pub fn in_terminal(self) -> bool {
        matches!(self, Self::Terminal(_) | Self::Agent(_))
    }
}

/// The parts of the window ⌃h/j/k/l move between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Side,
    Editor,
    Panel,
    Agent,
}

/// Which regions are on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Present {
    pub side: bool,
    pub panel: bool,
    pub agent: bool,
}

/// The region next to `from` in direction `dir`, among those on screen.
///
/// ```text
/// ┌──────┬────────┬───────┐
/// │ side │ editor │       │
/// │      ├────────┤ agent │
/// │      │ panel  │       │
/// └──────┴────────┴───────┘
/// ```
///
/// Left from the agent panel goes to the editor (the one beside both the editor and the panel).
pub fn neighbour(from: Region, dir: Dir, present: Present) -> Option<Region> {
    let to = match (from, dir) {
        (Region::Side, Dir::Right) => Region::Editor,
        (Region::Editor, Dir::Left) | (Region::Panel, Dir::Left) => Region::Side,
        (Region::Editor, Dir::Down) => Region::Panel,
        (Region::Panel, Dir::Up) => Region::Editor,
        (Region::Editor, Dir::Right) | (Region::Panel, Dir::Right) => Region::Agent,
        (Region::Agent, Dir::Left) => Region::Editor,
        _ => return None,
    };
    match to {
        Region::Side if !present.side => None,
        Region::Panel if !present.panel => None,
        Region::Agent if !present.agent => None,
        _ => Some(to),
    }
}

/// What Neovim runs to put the cursor in the window at the edge the GUI's
/// ⌃`dir` came in from: going up from the panel enters the bottom-most
/// window, going right from the side bar the left-most.
pub fn enter_editor(dir: Dir) -> String {
    format!("999wincmd {}", dir.opposite().letter())
}

/// Whether the program in a terminal's foreground keeps ⌃h/j/k/l for itself.
pub fn keeps_ctrl_hjkl(foreground: Option<&str>, keep: &[String]) -> bool {
    let Some(name) = foreground else { return false };
    let name = name.rsplit('/').next().unwrap_or(name);
    keep.iter().any(|k| k == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: Present = Present { side: true, panel: true, agent: false };
    const ALL: Present = Present { side: true, panel: true, agent: true };

    #[test]
    fn the_regions_are_laid_out_as_on_screen() {
        use Dir::*;
        use Region::*;
        let table = [
            (Side, Right, Some(Editor)),
            (Side, Left, None),
            (Side, Down, None),
            (Side, Up, None),
            (Editor, Left, Some(Side)),
            (Editor, Down, Some(Panel)),
            (Editor, Right, None),
            (Editor, Up, None),
            (Panel, Up, Some(Editor)),
            (Panel, Left, Some(Side)),
            (Panel, Down, None),
            (Panel, Right, None),
        ];
        for (from, dir, to) in table {
            assert_eq!(neighbour(from, dir, BOTH), to, "{from:?} {dir:?}");
        }
    }

    #[test]
    fn the_agent_panel_is_the_right_hand_column() {
        use Dir::*;
        use Region::*;
        let table = [
            (Editor, Right, Some(Agent)),
            (Panel, Right, Some(Agent)),
            (Agent, Left, Some(Editor)),
            (Agent, Right, None),
            (Agent, Up, None),
            (Agent, Down, None),
            (Side, Right, Some(Editor)),
            (Editor, Left, Some(Side)),
            (Panel, Left, Some(Side)),
        ];
        for (from, dir, to) in table {
            assert_eq!(neighbour(from, dir, ALL), to, "{from:?} {dir:?}");
        }
        // Not shown: nothing to go to, and the old moves stay as they were.
        assert_eq!(neighbour(Region::Editor, Dir::Right, BOTH), None);
        assert_eq!(neighbour(Region::Panel, Dir::Right, BOTH), None);
        assert_eq!(neighbour(Region::Agent, Dir::Left, ALL), Some(Region::Editor));
        // Back into the editor from the agent panel: Neovim's right-most window.
        assert_eq!(enter_editor(Dir::Left), "999wincmd l");
    }

    #[test]
    fn a_region_that_is_not_shown_is_not_gone_to() {
        let no_panel = Present { side: true, panel: false, agent: false };
        let no_side = Present { side: false, panel: true, agent: false };
        assert_eq!(neighbour(Region::Editor, Dir::Down, no_panel), None);
        assert_eq!(neighbour(Region::Editor, Dir::Left, no_panel), Some(Region::Side));
        assert_eq!(neighbour(Region::Editor, Dir::Left, no_side), None);
        assert_eq!(neighbour(Region::Panel, Dir::Left, no_side), None);
        assert_eq!(neighbour(Region::Panel, Dir::Up, no_side), Some(Region::Editor));
        assert_eq!(neighbour(Region::Side, Dir::Right, no_panel), Some(Region::Editor));
    }

    #[test]
    fn focus_belongs_to_a_region() {
        assert_eq!(Focus::Editor.region(), Region::Editor);
        assert_eq!(Focus::Side.region(), Region::Side);
        assert_eq!(Focus::Problems.region(), Region::Panel);
        assert_eq!(Focus::Terminal(3).region(), Region::Panel);
        assert_eq!(Focus::Agent(4).region(), Region::Agent);
        // The diff view covers the editor: ⌃h/j/k/l go from there as from it.
        assert_eq!(Focus::Diff.region(), Region::Editor);
        assert!(!Focus::Diff.in_panel() && !Focus::Diff.in_agent() && !Focus::Diff.in_terminal());
        assert!(Focus::Terminal(3).in_panel() && !Focus::Editor.in_panel() && !Focus::Agent(4).in_panel());
        assert!(Focus::Agent(4).in_agent() && Focus::Agent(4).in_terminal() && Focus::Terminal(3).in_terminal());
        assert!(!Focus::Problems.in_terminal() && !Focus::Editor.in_agent());
        assert_eq!(Focus::default(), Focus::Editor);
    }

    #[test]
    fn entering_the_editor_picks_the_window_at_the_edge_it_came_from() {
        // Up from the panel: the bottom-most window; right from the side bar: the left-most.
        assert_eq!(enter_editor(Dir::Up), "999wincmd j");
        assert_eq!(enter_editor(Dir::Right), "999wincmd h");
        assert_eq!(enter_editor(Dir::Left), "999wincmd l");
        assert_eq!(enter_editor(Dir::Down), "999wincmd k");
    }

    #[test]
    fn directions_come_from_letters_and_keys() {
        for (letter, dir) in [("h", Dir::Left), ("j", Dir::Down), ("k", Dir::Up), ("l", Dir::Right)] {
            assert_eq!(Dir::from_letter(letter), Some(dir));
            assert_eq!(dir.letter().to_string(), letter);
            assert_eq!(dir.opposite().opposite(), dir);
        }
        assert_eq!(Dir::from_letter("x"), None);
        assert_eq!(Dir::from_key(egui::Key::K), Some(Dir::Up));
        assert_eq!(Dir::from_key(egui::Key::A), None);
    }

    #[test]
    fn programs_that_want_the_keys_keep_them() {
        let keep: Vec<String> = ["nvim", "vim", "fzf", "lazygit"].map(String::from).to_vec();
        assert!(keeps_ctrl_hjkl(Some("vim"), &keep));
        assert!(keeps_ctrl_hjkl(Some("/opt/homebrew/bin/nvim"), &keep));
        assert!(!keeps_ctrl_hjkl(Some("zsh"), &keep));
        assert!(!keeps_ctrl_hjkl(Some("vimtutor"), &keep));
        assert!(!keeps_ctrl_hjkl(None, &keep));
    }
}
