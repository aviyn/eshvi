//! The native menu bar (macOS), like MacVim's: Neovim's menus (eshvi's
//! defaults from `menu.lua` and the user's own `:amenu` items, reported as
//! `gui_menu`) plus the standard app and Window menu items. Built with muda.
//!
//! A shortcut is the text after `<Tab>` in a menu name, in macOS notation:
//! `⌘S`, `⇧⌘S`, `⌥⌘S`, `⌃\``, `F12`, `⇧F12`.

use nvim_bridge::gui::MenuNode;

/// What a chosen menu item asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuCommand {
    /// A Neovim menu item, by path; `action` is eshvi's for its own items.
    Neovim { path: Vec<String>, action: Option<String> },
    /// Eshvi › Settings…: open the config file.
    Settings,
    /// Eshvi › Quit: ask Neovim (unsaved changes prompt).
    Quit,
}

/// A parsed shortcut.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shortcut {
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    /// One character ("S", "`", "-") or a key name ("F12").
    pub key: String,
}

/// Parse a macOS-style shortcut: modifier symbols (⌃ ⌥ ⇧ ⌘) then a key.
pub fn parse_shortcut(text: &str) -> Option<Shortcut> {
    let mut s = Shortcut::default();
    let mut rest = text.trim();
    while let Some(c) = rest.chars().next() {
        let flag = match c {
            '⌘' => &mut s.cmd,
            '⇧' => &mut s.shift,
            '⌥' => &mut s.alt,
            '⌃' => &mut s.ctrl,
            _ => break,
        };
        *flag = true;
        rest = &rest[c.len_utf8()..];
    }
    let single = rest.chars().count() == 1;
    let fkey = rest.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=20).contains(&n));
    (single || fkey).then(|| Shortcut { key: rest.to_uppercase(), ..s })
}

/// Keys that run a Neovim menu item in the current mode: `<Cmd>emenu
/// File.Save\ As…<CR>` (names escape spaces and dots; `<` is `<lt>` for
/// nvim_input).
pub fn emenu_keys(path: &[String]) -> String {
    let escaped: Vec<String> = path
        .iter()
        .map(|p| p.replace('\\', "\\\\").replace(' ', "\\ ").replace('.', "\\.").replace('<', "<lt>"))
        .collect();
    format!("<Cmd>emenu {}<CR>", escaped.join("."))
}

/// Actions that edit text: in a GUI text field (the explorer's name field)
/// they go to the field instead of Neovim.
pub fn is_text_edit(action: &str) -> bool {
    matches!(action, "undo" | "redo" | "cut" | "copy" | "paste" | "select_all")
}

/// Items that show or hide a panel: checkmark items, checked while it is
/// shown.
pub fn is_toggle(action: &str) -> bool {
    matches!(action, "explorer" | "explorer_view" | "source_control" | "outline" | "panel" | "problems" | "git_output" | "terminal" | "agent" | "preview")
}

/// What the GUI knows of an item of eshvi's: whether it can be chosen now
/// (Copy needs a selection), and for a panel's item whether it is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemState {
    pub enabled: bool,
    pub checked: Option<bool>,
}

impl ItemState {
    pub fn enabled(enabled: bool) -> Self {
        Self { enabled, checked: None }
    }

    pub fn shown(shown: bool) -> Self {
        Self { enabled: true, checked: Some(shown) }
    }
}

#[cfg(target_os = "macos")]
pub use native::MenuBar;

/// No native menu bar elsewhere (yet).
#[cfg(not(target_os = "macos"))]
pub struct MenuBar;

#[cfg(not(target_os = "macos"))]
impl MenuBar {
    pub fn new(_ctx: &egui::Context) -> Self {
        Self
    }
    pub fn set_menus(&mut self, _nodes: &[MenuNode]) {}
    pub fn update(&mut self, _state: &dyn Fn(&str) -> Option<ItemState>) {}
    pub fn poll(&mut self) -> Vec<MenuCommand> {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::collections::HashMap;
    use std::sync::mpsc;

    use muda::accelerator::{Accelerator, Code, Modifiers};
    use muda::{AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

    use super::{is_toggle, parse_shortcut, ItemState, MenuCommand, MenuNode, Shortcut};

    pub struct MenuBar {
        /// Kept alive while it is the app's menu.
        _menu: Menu,
        commands: HashMap<String, MenuCommand>,
        events: mpsc::Receiver<MenuEvent>,
        /// Neovim's menus as last built (a report of the same menus
        /// doesn't rebuild them).
        nodes: Vec<MenuNode>,
        /// eshvi's items, whose state the GUI keeps current.
        live: Vec<Live>,
        /// Chosen items read while a right-click menu was open.
        later: Vec<MenuEvent>,
    }

    /// One of eshvi's items, with the state last set.
    struct Live {
        action: String,
        item: Item,
        /// `:menu disable` from Neovim: stays greyed out.
        disabled: bool,
        state: Option<ItemState>,
    }

    #[derive(Clone)]
    enum Item {
        Plain(MenuItem),
        Check(CheckMenuItem),
    }

    impl MenuBar {
        /// Install the menu bar (app and Window menus until Neovim reports
        /// its menus). Wakes the UI when an item is chosen.
        pub fn new(ctx: &egui::Context) -> Self {
            let (tx, events) = mpsc::channel();
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e| {
                let _ = tx.send(e);
                ctx.request_repaint();
            }));
            let b = build(&[]);
            b.menu.init_for_nsapp();
            Self { _menu: b.menu, commands: b.commands, events, nodes: Vec::new(), live: b.live, later: Vec::new() }
        }

        /// Replace the menus with Neovim's.
        pub fn set_menus(&mut self, nodes: &[MenuNode]) {
            if nodes == self.nodes {
                return;
            }
            let b = build(nodes);
            b.menu.init_for_nsapp();
            self._menu = b.menu;
            self.commands = b.commands;
            self.live = b.live;
            self.nodes = nodes.to_vec();
        }

        /// Grey out or check eshvi's items as the GUI's state says (only
        /// what changed is set).
        pub fn update(&mut self, state: &dyn Fn(&str) -> Option<ItemState>) {
            for live in &mut self.live {
                let Some(new) = state(&live.action) else { continue };
                if live.state == Some(new) {
                    continue;
                }
                live.state = Some(new);
                let enabled = new.enabled && !live.disabled;
                match &live.item {
                    Item::Plain(item) => item.set_enabled(enabled),
                    Item::Check(item) => {
                        item.set_enabled(enabled);
                        if let Some(checked) = new.checked {
                            item.set_checked(checked);
                        }
                    }
                }
            }
        }

        /// Items chosen since the last call.
        pub fn poll(&mut self) -> Vec<MenuCommand> {
            let events: Vec<MenuEvent> = self.later.drain(..).chain(self.events.try_iter()).collect();
            events.into_iter().filter_map(|e| self.commands.get(&e.id.0).cloned()).collect()
        }
    }

    fn accelerator(s: &Shortcut) -> Option<Accelerator> {
        let mut mods = Modifiers::empty();
        for (on, m) in [(s.cmd, Modifiers::META), (s.shift, Modifiers::SHIFT), (s.alt, Modifiers::ALT), (s.ctrl, Modifiers::CONTROL)] {
            if on {
                mods |= m;
            }
        }
        let code = match s.key.as_str() {
            k if k.len() > 1 => match k {
                "F1" => Code::F1,
                "F2" => Code::F2,
                "F3" => Code::F3,
                "F4" => Code::F4,
                "F5" => Code::F5,
                "F6" => Code::F6,
                "F7" => Code::F7,
                "F8" => Code::F8,
                "F9" => Code::F9,
                "F10" => Code::F10,
                "F11" => Code::F11,
                "F12" => Code::F12,
                _ => return None,
            },
            k => match k.chars().next()? {
                c @ 'A'..='Z' => format!("Key{c}").parse().ok()?,
                c @ '0'..='9' => format!("Digit{c}").parse().ok()?,
                '`' => Code::Backquote,
                '-' => Code::Minus,
                '=' => Code::Equal,
                ',' => Code::Comma,
                '.' => Code::Period,
                '/' => Code::Slash,
                '\\' => Code::Backslash,
                ';' => Code::Semicolon,
                '\'' => Code::Quote,
                '[' => Code::BracketLeft,
                ']' => Code::BracketRight,
                _ => return None,
            },
        };
        Some(Accelerator::new(mods, code))
    }

    struct Builder {
        menu: Menu,
        commands: HashMap<String, MenuCommand>,
        live: Vec<Live>,
    }

    impl Builder {
        fn item(&mut self, text: &str, key: &str, command: MenuCommand) -> MenuItem {
            let id = self.add(command);
            let accel = parse_shortcut(key).and_then(|s| accelerator(&s));
            // `&` marks a mnemonic in muda.
            MenuItem::with_id(id, text.replace('&', "&&"), true, accel)
        }

        fn add(&mut self, command: MenuCommand) -> String {
            let id = format!("eshvi-{}", self.commands.len());
            self.commands.insert(id.clone(), command);
            id
        }

        /// A Neovim menu item: a checkmark item for a panel's item or one
        /// Neovim checks (Buffers), else a plain one.
        fn neovim_item(&mut self, menu: &Submenu, node: &MenuNode, path: Vec<String>) {
            let id = self.add(MenuCommand::Neovim { path, action: node.action.clone() });
            let accel = parse_shortcut(&node.key).and_then(|s| accelerator(&s));
            let text = node.name.replace('&', "&&");
            let enabled = !node.disabled;
            let item = if node.checked.is_some() || node.action.as_deref().is_some_and(is_toggle) {
                let item = CheckMenuItem::with_id(id, text, enabled, node.checked.unwrap_or(false), accel);
                let _ = menu.append(&item);
                Item::Check(item)
            } else {
                let item = MenuItem::with_id(id, text, enabled, accel);
                let _ = menu.append(&item);
                Item::Plain(item)
            };
            if let Some(action) = &node.action {
                self.live.push(Live { action: action.clone(), item, disabled: node.disabled, state: None });
            }
        }

        fn fill(&mut self, menu: &Submenu, nodes: &[MenuNode], path: &[String]) {
            for node in nodes {
                let mut path = path.to_vec();
                path.push(node.name.clone());
                if node.is_separator() {
                    let _ = menu.append(&PredefinedMenuItem::separator());
                } else if let Some(children) = &node.children {
                    let sub = Submenu::new(node.name.replace('&', "&&"), true);
                    self.fill(&sub, children, &path);
                    let _ = menu.append(&sub);
                } else {
                    self.neovim_item(menu, node, path);
                }
            }
        }
    }

    fn build(nodes: &[MenuNode]) -> Builder {
        let mut b = Builder { menu: Menu::new(), commands: HashMap::new(), live: Vec::new() };
        let menu = b.menu.clone();
        let sep = PredefinedMenuItem::separator;

        let about = AboutMetadata {
            name: Some("Eshvi".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            icon: crate::icon().and_then(|i| muda::Icon::from_rgba(i.rgba, i.width, i.height).ok()),
            ..Default::default()
        };
        let app = Submenu::new("Eshvi", true);
        let settings = b.item("Settings…", "⌘,", MenuCommand::Settings);
        let quit = b.item("Quit Eshvi", "⌘Q", MenuCommand::Quit);
        let _ = app.append_items(&[
            &PredefinedMenuItem::about(Some("About Eshvi"), Some(about)),
            &sep(),
            &settings,
            &sep(),
            &PredefinedMenuItem::services(None),
            &sep(),
            &PredefinedMenuItem::hide(Some("Hide Eshvi")),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &sep(),
            &quit,
        ]);
        let _ = menu.append(&app);

        // Neovim's menus in order; its Window items join the native Window
        // menu, which (like Help) comes last.
        let mut window_items: &[MenuNode] = &[];
        let mut help = None;
        for node in nodes {
            let Some(children) = &node.children else { continue };
            match node.name.as_str() {
                "Window" => window_items = children,
                "Help" => help = Some(node),
                _ => {
                    let sub = Submenu::new(node.name.replace('&', "&&"), true);
                    b.fill(&sub, children, std::slice::from_ref(&node.name));
                    let _ = menu.append(&sub);
                }
            }
        }

        let window = Submenu::new("Window", true);
        let _ = window.append_items(&[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(Some("Zoom")),
            &PredefinedMenuItem::fullscreen(None),
        ]);
        if !window_items.is_empty() {
            let _ = window.append(&sep());
            b.fill(&window, window_items, &["Window".to_owned()]);
        }
        let _ = window.append_items(&[&sep(), &PredefinedMenuItem::bring_all_to_front(None)]);
        let _ = menu.append(&window);
        window.set_as_windows_menu_for_nsapp();

        if let Some(node) = help {
            let sub = Submenu::new("Help", true);
            b.fill(&sub, node.children.as_deref().unwrap_or_default(), &["Help".to_owned()]);
            let _ = menu.append(&sub);
            sub.set_as_help_menu_for_nsapp();
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_macos_shortcuts() {
        let s = |cmd, shift, alt, ctrl, key: &str| Some(Shortcut { cmd, shift, alt, ctrl, key: key.into() });
        assert_eq!(parse_shortcut("⌘S"), s(true, false, false, false, "S"));
        assert_eq!(parse_shortcut("⇧⌘s"), s(true, true, false, false, "S"));
        assert_eq!(parse_shortcut("⌥⌘S"), s(true, false, true, false, "S"));
        assert_eq!(parse_shortcut("⌃`"), s(false, false, false, true, "`"));
        assert_eq!(parse_shortcut("⌃⇧-"), s(false, true, false, true, "-"));
        assert_eq!(parse_shortcut("⇧F12"), s(false, true, false, false, "F12"));
        assert_eq!(parse_shortcut("⌘\\"), s(true, false, false, false, "\\"));
        for bad in ["", "⌘", ":w", "Ctrl-S", "F99"] {
            assert_eq!(parse_shortcut(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn emenu_keys_escape_names() {
        let path = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(emenu_keys(&path(&["File", "Save"])), "<Cmd>emenu File.Save<CR>");
        assert_eq!(emenu_keys(&path(&["File", "Save As…"])), "<Cmd>emenu File.Save\\ As…<CR>");
        assert_eq!(emenu_keys(&path(&["Mine", "v1.2 <x>"])), "<Cmd>emenu Mine.v1\\.2\\ <lt>x><CR>");
    }
}
