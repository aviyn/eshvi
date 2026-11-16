//! Integration test against a real `nvim --embed --clean`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use grid::Editor;
use nvim_bridge::{AgentOptions, Bridge, BridgeEvent, SpawnOptions, UiCommand};

fn nvim_available() -> bool {
    std::process::Command::new("nvim").arg("--version").output().is_ok()
}

/// Run panel terminals in `/bin/sh`, so the user's shell setup can't get in
/// the way (zsh without a `~/.zshrc` opens its first-run menu, which eats
/// the keys the tests type). Windows keeps its default, cmd.exe.
fn plain_shell(mut args: Vec<String>) -> Vec<String> {
    if cfg!(unix) {
        args.splice(0..0, ["--cmd".into(), "set shell=/bin/sh".into()]);
    }
    args
}

/// A lock file folder for Claude Code's IDE connection, apart from the
/// user's `~/.claude/ide`.
fn test_ide_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("eshvi-ide-{name}-{}", std::process::id()))
}

/// Apply batches until `pred` holds or the timeout expires.
fn pump(bridge: &Bridge, ed: &mut Editor, pred: impl Fn(&Editor) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Some(ev) = bridge.try_recv() {
            if let BridgeEvent::Redraw(batch) = ev {
                for e in batch {
                    ed.apply(e);
                }
            }
        }
        if pred(ed) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Like `pump`, with a custom timeout.
fn pump_for(bridge: &Bridge, ed: &mut Editor, timeout: Duration, pred: impl Fn(&Editor) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        while let Some(ev) = bridge.try_recv() {
            if let BridgeEvent::Redraw(batch) = ev {
                batch.into_iter().for_each(|e| ed.apply(e));
            }
        }
        if pred(ed) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Text of the first normal window grid (multigrid puts windows on grid >= 2).
fn window_text(ed: &Editor) -> Vec<String> {
    ed.windows
        .values()
        .find(|w| w.kind == grid::WindowKind::Normal)
        .map(|w| ed.grid_text(w.grid))
        .unwrap_or_default()
}

#[test]
fn opens_file_and_scrolls() {
    if !nvim_available() {
        eprintln!("nvim not found, skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("lines.txt");
    let content: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, content).unwrap();

    let bridge = Bridge::spawn(
        SpawnOptions {
            args: vec!["--clean".into(), "-n".into(), file.to_string_lossy().into()],
            width: 40,
            height: 12,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();

    let mut ed = Editor::new();
    assert!(
        pump(&bridge, &mut ed, |ed| window_text(ed).first().is_some_and(|l| l.starts_with("line 1 "))),
        "file never rendered: {:?}",
        window_text(&ed)
    );
    assert!(ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal));

    // <C-e> scrolls one line; nvim uses grid_scroll for this.
    bridge.send(UiCommand::Input("3<C-e>".into()));
    assert!(
        pump(&bridge, &mut ed, |ed| window_text(ed).first().is_some_and(|l| l.starts_with("line 4 "))),
        "scroll not applied: {:?}",
        window_text(&ed)
    );
    let text = window_text(&ed);
    assert!(text[1].starts_with("line 5 "), "{text:?}");

    // The scrollbars' scroll: the view stays where it's put (the cursor,
    // above it, comes along), and goes back up.
    bridge.send(UiCommand::Command("lua Eshvi.scroll_to(vim.api.nvim_get_current_win(), 60)".into()));
    assert!(
        pump(&bridge, &mut ed, |ed| first_line(ed).starts_with("line 60 ")),
        "scrollbar scroll not kept: {:?}",
        window_text(&ed)
    );
    bridge.send(UiCommand::Command("lua Eshvi.scroll_to(vim.api.nvim_get_current_win(), 1)".into()));
    assert!(pump(&bridge, &mut ed, |ed| first_line(ed).starts_with("line 1 ")), "{:?}", window_text(&ed));

    bridge.send(UiCommand::TryResize { width: 30, height: 8 });
    assert!(pump(&bridge, &mut ed, |ed| ed.grids.get(&1).is_some_and(|g| g.width() == 30 && g.height() == 8)));

    bridge.send(UiCommand::Command("qa!".into()));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut exited = false;
    while Instant::now() < deadline && !exited {
        while let Some(ev) = bridge.try_recv() {
            if let BridgeEvent::Exited { error } = ev {
                assert!(error.is_none(), "{error:?}");
                exited = true;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(exited, "nvim did not exit");
    let _ = std::fs::remove_dir_all(&dir);
}

fn spawn(args: &[&str], width: usize, height: usize) -> Bridge {
    Bridge::spawn(
        SpawnOptions { args: args.iter().map(|s| s.to_string()).collect(), width, height, ..Default::default() },
        Arc::new(|| {}),
    )
    .unwrap()
}

fn first_line(ed: &Editor) -> String {
    window_text(ed).first().cloned().unwrap_or_default()
}

#[test]
fn modes_ctrl_alt_mouse_and_paste() {
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 40, 10);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    // Insert mode with escaped `<`, <C-w> deletes a word, <Esc> to normal.
    bridge.send(UiCommand::Input("ifoo <lt>bar baz<C-w>qux<Esc>".into()));
    assert!(pump(&bridge, &mut ed, |ed| first_line(ed).starts_with("foo <bar qux ")), "{:?}", first_line(&ed));
    assert!(pump(&bridge, &mut ed, |ed| ed.mode == "normal"));

    // Visual mode: select a word and change its case.
    bridge.send(UiCommand::Input("0veU".into()));
    assert!(pump(&bridge, &mut ed, |ed| first_line(ed).starts_with("FOO <bar")), "{:?}", first_line(&ed));

    // <M-x> mapped in insert mode behaves like a terminal's Meta key.
    // Sent as input (not Command) so it is ordered with the keys below.
    bridge.send(UiCommand::Input(":inoremap <lt>M-x> META<CR>".into()));
    bridge.send(UiCommand::Input("A <M-x><Esc>".into()));
    assert!(pump(&bridge, &mut ed, |ed| first_line(ed).contains("qux META")), "{:?}", first_line(&ed));

    // Mouse click on the window grid moves the cursor.
    let win_grid = ed.windows.values().find(|w| w.kind == grid::WindowKind::Normal).unwrap().grid;
    bridge.send(UiCommand::InputMouse {
        button: "left".into(),
        action: "press".into(),
        modifier: "".into(),
        grid: win_grid,
        row: 0,
        col: 4,
    });
    bridge.send(UiCommand::InputMouse {
        button: "left".into(),
        action: "release".into(),
        modifier: "".into(),
        grid: win_grid,
        row: 0,
        col: 4,
    });
    assert!(pump(&bridge, &mut ed, |ed| ed.cursor.grid == win_grid && ed.cursor.col == 4), "{:?}", ed.cursor);

    // nvim_paste inserts text in normal mode after the cursor line.
    bridge.send(UiCommand::Input("o<Esc>".into()));
    bridge.send(UiCommand::Paste("pasted".into()));
    assert!(pump(&bridge, &mut ed, |ed| window_text(ed).iter().any(|l| l.starts_with("pasted"))), "{:?}", window_text(&ed));

    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn clipboard_register_round_trip() {
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 40, 10);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let token = format!("eshvi-clip-{}", std::process::id());
    bridge.send(UiCommand::Input(format!("i{token}<Esc>\"+yy\"+p")));
    assert!(
        pump(&bridge, &mut ed, |ed| window_text(ed).iter().filter(|l| l.starts_with(&token)).count() == 2),
        "{:?}",
        window_text(&ed)
    );
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn splits_and_floats_get_their_own_grids() {
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 80, 20);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    bridge.send(UiCommand::Input(":vsplit<CR>".into()));
    let normals = |ed: &Editor| -> Vec<(i64, i64, usize)> {
        let mut v: Vec<_> = ed
            .windows
            .values()
            .filter(|w| w.kind == grid::WindowKind::Normal && !w.hidden)
            .map(|w| (w.row, w.col, w.width))
            .collect();
        v.sort();
        v
    };
    assert!(pump(&bridge, &mut ed, |ed| normals(ed).len() == 2), "{:?}", normals(&ed));
    let wins = normals(&ed);
    assert_eq!(wins[0].1, 0);
    assert!(wins[1].1 > 30, "right window should start mid-screen: {wins:?}");

    let lua = "local b = vim.api.nvim_create_buf(false, true) \
               vim.api.nvim_open_win(b, false, {relative='editor', row=3, col=10, width=20, height=4, border='rounded'})";
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    assert!(pump(&bridge, &mut ed, |ed| ed
        .windows
        .values()
        .any(|w| matches!(w.kind, grid::WindowKind::Float { .. }) && w.row == 3 && w.col == 10)));
    let float = ed.windows.values().find(|w| matches!(w.kind, grid::WindowKind::Float { .. })).unwrap().grid;
    // Topmost apart from the message grid (zindex 200).
    let top = ed
        .draw_order()
        .into_iter()
        .rfind(|g| !matches!(ed.windows.get(g).map(|w| &w.kind), Some(grid::WindowKind::Message { .. })));
    assert_eq!(top, Some(float));
    // The rounded border is drawn into the float's own grid.
    assert!(ed.grid_text(float)[0].starts_with('╭'), "{:?}", ed.grid_text(float));

    bridge.send(UiCommand::Command("fclose!".into()));
    assert!(pump(&bridge, &mut ed, |ed| !ed.draw_order().contains(&float)));
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn popupmenu_events_and_selection() {
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 60, 12);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    bridge.send(UiCommand::Input("iapple apricot avocado<CR>a<C-n>".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.popupmenu.as_ref().is_some_and(|p| p.items.len() == 3)), "{:?}", ed.popupmenu);
    let pm = ed.popupmenu.clone().unwrap();
    assert_eq!(pm.selected, 0);
    assert_eq!((pm.row, pm.col), (1, 0));
    assert!(pm.grid > 1, "anchored on the window grid with multigrid");

    // Keyboard selection is mirrored by popupmenu_select.
    bridge.send(UiCommand::Input("<C-n>".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.popupmenu.as_ref().is_some_and(|p| p.selected == 1)));

    // A click selects + finishes: the menu closes and the word is inserted.
    bridge.send(UiCommand::SelectPopupmenuItem { item: 2, insert: true, finish: true });
    assert!(pump(&bridge, &mut ed, |ed| ed.popupmenu.is_none() && window_text(ed)[1].starts_with("avocado")), "{:?}", window_text(&ed));
    bridge.send(UiCommand::Command("qa!".into()));
}

/// Wait for a `gui_*` event matching `pred`, applying redraws meanwhile.
fn wait_gui(
    bridge: &Bridge,
    ed: &mut Editor,
    pred: impl Fn(&nvim_bridge::gui::GuiEvent) -> bool,
) -> Option<nvim_bridge::gui::GuiEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Some(ev) = bridge.try_recv() {
            match ev {
                BridgeEvent::Redraw(batch) => batch.into_iter().for_each(|e| ed.apply(e)),
                BridgeEvent::Gui(g) if pred(&g) => return Some(g),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn gui_bridge_plugin_sends_hover_signature_and_close() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_lsp.lua");
    let rtp = format!("set rtp^={repo}");
    let bridge = spawn(
        &["--clean", "-n", "--cmd", &rtp, "-c", "lua require('gui_bridge').setup()", "-c", &format!("luafile {fake}")],
        80,
        20,
    );
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    bridge.send(UiCommand::Command("lua require('gui_bridge').hover()".into()));
    let hover = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Hover { .. })).expect("no gui_hover");
    let GuiEvent::Hover { contents, kind } = hover else { unreachable!() };
    assert_eq!(kind, "markdown");
    assert!(contents.contains("```rust") && contents.contains("fake docs"), "{contents}");

    // Moving the cursor closes popups.
    bridge.send(UiCommand::Input("ifoo<Esc>".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PopupClose(None))).is_some());

    // `(` is a trigger character: signature help with the first parameter active.
    bridge.send(UiCommand::Input("A(".into()));
    let Some(GuiEvent::Signature(help)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Signature(_))) else {
        panic!("no gui_signature")
    };
    let (sig, active) = help.active().unwrap();
    assert_eq!(active, Some(0));
    let p = &sig.parameters[0];
    assert_eq!(&sig.label[p.start..p.end], "a: i32");
    assert_eq!(p.documentation, "first arg");

    // Typing on tracks the active parameter; UTF-16 label offsets are converted to bytes.
    bridge.send(UiCommand::Input("1, ".into()));
    let Some(GuiEvent::Signature(help)) = wait_gui(&bridge, &mut ed, |g| {
        matches!(g, GuiEvent::Signature(h) if h.active_parameter == Some(1))
    }) else {
        panic!("active parameter not updated")
    };
    let (sig, active) = help.active().unwrap();
    let p = &sig.parameters[active.unwrap()];
    assert_eq!(&sig.label[p.start..p.end], "b: u8");

    bridge.send(UiCommand::Input("<Esc>".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PopupClose(None))).is_some());
    bridge.send(UiCommand::Command("qa!".into()));
}

/// With the own config the app ships `gui_bridge` itself: no checkout of the repo on the
/// runtimepath, and K still opens the native hover. A config's own K mapping is kept, and
/// `vim.g.eshvi_lsp_popups = false` leaves it out.
#[test]
fn lsp_popups_are_shipped_with_the_app() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_lsp.lua");
    let start = |extra: &[&str]| {
        let mut args = vec!["--clean".to_string(), "-n".to_string()];
        args.extend(extra.iter().map(|s| s.to_string()));
        args.extend(["-c".to_string(), format!("luafile {fake}")]);
        let bridge = Bridge::spawn(
            SpawnOptions { args: plain_shell(args), width: 80, height: 20, lsp_popups: true, ..Default::default() },
            Arc::new(|| {}),
        )
        .unwrap();
        let mut ed = Editor::new();
        assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
        (bridge, ed)
    };

    let (bridge, mut ed) = start(&[]);
    wait_for(&bridge, &mut ed, "vim.fn.maparg('K', 'n', false, true).desc", "LSP hover");
    // Not from a folder: the source the app passed.
    assert_eq!(
        ask(&bridge, &mut ed, "debug.getinfo(require('gui_bridge').hover, 'S').source"),
        "@eshvi/lua/gui_bridge/init.lua"
    );
    bridge.send(UiCommand::Input("K".into()));
    let Some(GuiEvent::Hover { contents, .. }) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Hover { .. }))
    else {
        panic!("no gui_hover from K")
    };
    assert!(contents.contains("fake docs"), "{contents}");
    bridge.send(UiCommand::Command("qa!".into()));

    let (bridge, mut ed) = start(&["--cmd", "nnoremap K <Cmd>let g:mine = 1<CR>"]);
    wait_for(&bridge, &mut ed, "package.loaded.gui_bridge ~= nil", "true");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.maparg('K', 'n')"), "<Cmd>let g:mine = 1<CR>");
    bridge.send(UiCommand::Command("qa!".into()));

    let (bridge, mut ed) = start(&["--cmd", "let g:eshvi_lsp_popups = v:false"]);
    wait_for(&bridge, &mut ed, "vim.g.gui_bridge_chan ~= nil", "true");
    // Give the app's startup chunks time to have run.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ask(&bridge, &mut ed, "tostring(package.loaded.gui_bridge)"), "nil");
    // Neovim's own K, from the server's attach, stays.
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.maparg('K', 'n', false, true).desc"), "vim.lsp.buf.hover()");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// The diagnostics popup: shown where the cursor rests in Normal mode,
/// closed on typing, and once dismissed (Esc) not shown there again (a
/// later CursorHold, the server checking again) until the cursor moves.
#[test]
fn gui_bridge_diagnostics_stay_dismissed_until_the_cursor_moves() {
    use nvim_bridge::gui::{GuiEvent, PopupKind};
    if !nvim_available() {
        return;
    }
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let rtp = format!("set rtp^={repo}");
    let bridge = spawn(&["--clean", "-n", "--cmd", &rtp, "-c", "lua require('gui_bridge').setup()"], 80, 20);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    // (Commands run on tasks of their own: keys and calls go through
    // Neovim's input queue here, in order.)
    let call = |lua: &str| bridge.send(UiCommand::Input(format!("<Cmd>lua {lua}<CR>")));
    let set = "vim.diagnostic.set(vim.api.nvim_create_namespace('t'), 0, { { lnum = 0, col = 0, end_col = 5, message = 'bad', severity = 1 } })";
    let hold = "vim.api.nvim_exec_autocmds('CursorHold', {})";
    bridge.send(UiCommand::Input("ihello world<Esc>0".into()));
    call(set);
    let shown = |g: &GuiEvent| matches!(g, GuiEvent::Diagnostic(d) if !d.is_empty());
    // Nothing matching for a while.
    let quiet = |bridge: &Bridge, ed: &mut Editor| {
        let until = Instant::now() + Duration::from_millis(700);
        while Instant::now() < until {
            while let Some(ev) = bridge.try_recv() {
                match ev {
                    BridgeEvent::Redraw(batch) => batch.into_iter().for_each(|e| ed.apply(e)),
                    BridgeEvent::Gui(g) if shown(&g) => return false,
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    };
    call(hold);
    assert!(wait_gui(&bridge, &mut ed, shown).is_some(), "not shown on CursorHold");

    // Dismissed: not again here, on a rest or when the server reports again.
    call("require('gui_bridge').dismiss_diagnostics()");
    call(hold);
    call(set);
    assert!(quiet(&bridge, &mut ed), "shown again where it was dismissed");

    // Moved on (and back): shown again.
    bridge.send(UiCommand::Input("l".into()));
    call(hold);
    assert!(wait_gui(&bridge, &mut ed, shown).is_some(), "not shown after moving");

    // Typing closes it, and it stays closed in Insert mode.
    bridge.send(UiCommand::Input("i".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PopupClose(Some(PopupKind::Diagnostic)))).is_some());
    call(set);
    call("require('gui_bridge').show_diagnostics()");
    assert!(quiet(&bridge, &mut ed), "shown in Insert mode");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// blink.cmp's documentation goes to the GUI: a stand-in for blink's modules
/// (its documentation module's functions are replaced), then its keys'
/// calls: shown (detail and Markdown), scrolled, closed.
#[test]
fn blink_documentation_goes_to_the_gui() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 80, 20);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let fake = r#"
        local promise = function(v) return { map = function(self, f) f(v); return self end, catch = function(self) return self end } end
        package.loaded['blink.cmp'] = {}
        package.preload['blink.cmp.completion.windows.menu'] = function()
          return { win = { is_open = function() return true end, get_win = function() return 1234 end } }
        end
        package.preload['blink.cmp.sources.lib'] = function()
          return { resolve = function(_, item) return promise(item) end }
        end
        package.preload['blink.cmp.completion.windows.documentation'] = function()
          return { show_item = function() error('not replaced') end, win = { is_open = function() return false end } }
        end
        vim.api.nvim_exec_autocmds('InsertEnter', {})
        _G.docs = require('blink.cmp.completion.windows.documentation')
    "#;
    let lua = |code: &str| {
        let line = code.split_whitespace().collect::<Vec<_>>().join(" ");
        bridge.send(UiCommand::Input(format!("<Cmd>lua {line}<CR>")));
    };
    lua(fake);
    lua("docs.show_item({}, { detail = 'fn f()', documentation = { kind = 'markdown', value = 'Does *f*.' } })");
    let Some(GuiEvent::Docs { contents, win }) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Docs { .. })) else {
        panic!("no gui_docs: {:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>())
    };
    assert_eq!(win, 1234);
    assert!(contents.contains("fn f()") && contents.contains("---") && contents.contains("Does *f*."), "{contents}");
    // blink's keys see it open, and scroll it.
    lua("assert(docs.win:is_open()); docs.scroll_down(4)");
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::DocsScroll(4))).is_some());
    lua("docs.close(); assert(not docs.win:is_open())");
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Docs { contents, .. } if contents.is_empty())).is_some());
    assert!(!ed.messages.iter().any(|m| m.text().contains("E5108")), "{:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>());
    bridge.send(UiCommand::Command("qa!".into()));
}

/// The same config in a plain (non-GUI) Neovim must not error.
#[test]
fn gui_bridge_plugin_is_harmless_without_gui() {
    if !nvim_available() {
        return;
    }
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_lsp.lua");
    let script = r#"
        local gui = require('gui_bridge')
        gui.setup()
        vim.keymap.set('n', 'K', gui.hover)
        vim.cmd('normal! ifoo')
        gui.hover()
        gui.show_diagnostics()
        gui.signature_help()
        gui.close()
        vim.api.nvim_exec_autocmds('CursorMoved', {})
        vim.api.nvim_exec_autocmds('CursorHold', {})
        vim.api.nvim_exec_autocmds('InsertLeave', {})
        vim.wait(300)
        assert(vim.g.gui_bridge_chan == nil)
        assert(vim.v.errmsg == '', vim.v.errmsg)
        io.stdout:write('OK\n')
    "#;
    let out = std::process::Command::new("nvim")
        .args(["--clean", "--headless", "-n", "--cmd", &format!("set rtp^={repo}"), "-c", &format!("luafile {fake}")])
        .args(["-c", &format!("lua {}", script.replace('\n', " ")), "-c", "qa!"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("OK"), "stdout: {stdout}\nstderr: {stderr}");
    assert!(!stderr.contains("Error") && !stderr.contains("E5"), "stderr: {stderr}");
}

#[test]
fn sidebar_windows_are_reported_by_filetype() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: vec!["--clean".into(), "-n".into()],
            width: 80,
            height: 20,
            sidebar_filetypes: vec!["NvimTree".into()],
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    // A left split showing a buffer with the sidebar filetype.
    bridge.send(UiCommand::Input(":topleft 30vnew | setlocal filetype=NvimTree<CR>".into()));
    let Some(GuiEvent::Sidebar(wins)) =
        wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Sidebar(w) if !w.is_empty()))
    else {
        panic!("no gui_sidebar")
    };
    assert_eq!(wins.len(), 1);
    // The handle matches the window's win_pos, so the GUI can find its grid.
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.win == wins[0] && w.col == 0 && w.width == 30)));

    // Closing it reports an empty list.
    bridge.send(UiCommand::Input(":close<CR>".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Sidebar(w) if w.is_empty())).is_some());
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn messages_are_external_and_leave_no_cmdline_row() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: vec![
                "--clean".into(),
                "-n".into(),
                "-c".into(),
                "lua vim.notify = function() error('plugin notify') end".into(),
            ],
            width: 60,
            height: 12,
            restore_notify: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    let opts = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Options { .. }));
    assert_eq!(opts, Some(GuiEvent::Options { laststatus: 2 }));
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    // No row is reserved for the cmdline: the window plus its statusline
    // fill the whole grid.
    let win = ed.windows.values().find(|w| w.kind == grid::WindowKind::Normal).unwrap().clone();
    assert_eq!(win.row as usize + win.height + 1, 12, "{win:?}");
    assert_eq!(ed.status_segments(false).first().map(|s| s.row), Some(11));

    bridge.send(UiCommand::Input(":echo 'hello there'<CR>".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.messages.iter().any(|m| m.kind == "echo" && m.text() == "hello there")));

    bridge.send(UiCommand::Input(":NoSuchCommand<CR>".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.messages.iter().any(|m| m.is_error() && m.text().contains("E492"))));

    bridge.send(UiCommand::Input(":ls<CR>".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.messages.iter().any(|m| m.kind == "list_cmd")));

    // A plugin replaced vim.notify; the GUI restored the built-in one, so
    // notifications arrive as ordinary messages.
    bridge.send(UiCommand::Command("lua vim.notify('from lua', vim.log.levels.WARN)".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.messages.iter().any(|m| m.text() == "from lua")));
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn layout_global_statusline_and_winbar() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = Bridge::spawn(
        SpawnOptions {
            // The "plugin" replaces vim.notify while the config loads.
            args: vec!["--clean".into(), "-n".into(), "-c".into(), "lua vim.notify = function() end".into()],
            width: 80,
            height: 20,
            restore_notify: true,
            global_statusline: true,
            winbar: true,
            sidebar_filetypes: vec!["NvimTree".into()],
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Options { laststatus: 3 })).is_some());

    bridge.send(UiCommand::Input(":vsplit<CR>:topleft 20vnew | setlocal filetype=NvimTree<CR>".into()));
    let normals = |ed: &Editor| -> Vec<grid::Window> {
        ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal && !w.hidden).cloned().collect()
    };
    assert!(pump(&bridge, &mut ed, |ed| normals(ed).len() == 3));
    // File windows get a winbar (top margin 1); the tree doesn't.
    assert!(
        pump(&bridge, &mut ed, |ed| normals(ed).iter().filter(|w| w.margins.top == 1).count() == 2
            && normals(ed).iter().any(|w| w.col == 0 && w.margins.top == 0)),
        "{:?}",
        normals(&ed).iter().map(|w| (w.col, w.margins)).collect::<Vec<_>>()
    );
    // One global statusline spanning the width.
    let segs = ed.status_segments(true);
    assert_eq!(segs.len(), 1);
    assert_eq!((segs[0].col_start, segs[0].col_end), (0, 80));

    // Stacked: the lower window has no winbar (the GUI draws its tab in the
    // separator row above it); alone again, it has one.
    bridge.send(UiCommand::Input("<Cmd>wincmd l | split<CR>".into()));
    let top_margins = |ed: &Editor| -> Vec<(i64, usize)> {
        let mut v: Vec<_> = normals(ed).iter().filter(|w| w.col > 0).map(|w| (w.row, w.margins.top)).collect();
        v.sort();
        v
    };
    assert!(
        pump(&bridge, &mut ed, |ed| {
            let m = top_margins(ed);
            m.len() == 3 && m.iter().filter(|(_, t)| *t == 0).count() == 1 && m.iter().all(|&(row, t)| (t == 0) == (row > 0))
        }),
        "{:?}",
        top_margins(&ed)
    );
    bridge.send(UiCommand::Input("<Cmd>close<CR>".into()));
    assert!(
        pump(&bridge, &mut ed, |ed| top_margins(ed).len() == 2 && top_margins(ed).iter().all(|(_, t)| *t == 1)),
        "{:?}",
        top_margins(&ed)
    );

    // vim.notify was reassigned during startup, but stays a message.
    bridge.send(UiCommand::Command("lua vim.notify('still a message')".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.messages.iter().any(|m| m.text() == "still a message")));
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn explorer_cwd_buffer_command_and_open_file() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.txt");
    std::fs::write(&file, "hello from disk\n").unwrap();
    let bridge = spawn(&["--clean", "-n"], 60, 12);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    // An explicit :cd is reported (autochdir-style window changes are not).
    bridge.send(UiCommand::Input(format!(":cd {}<CR>", dir.display())));

    let cwd = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Cwd(c) if c.ends_with(&format!("eshvi-open-{}", std::process::id()))));
    assert!(cwd.is_some(), "no gui_cwd for the cd");

    // The current window is a special (nofile) buffer, like a file tree;
    // OpenFile must use a normal window instead.
    bridge.send(UiCommand::Input(":vnew | setlocal buftype=nofile<CR>".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal).count() == 2));
    bridge.send(UiCommand::OpenFile { path: file.clone(), split: false });
    let opened = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Buffer { path, .. } if path.ends_with("hello.txt")));
    assert!(opened.is_some(), "no gui_buffer for the opened file");
    assert!(pump(&bridge, &mut ed, |ed| ed
        .windows
        .values()
        .any(|w| ed.grid_text(w.grid).iter().any(|l| l.starts_with("hello from disk")))));
    // Both windows still exist: the special one wasn't replaced.
    assert_eq!(ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal).count(), 2);

    // A file loaded in the background (its FileType runs in an autocmd
    // window) isn't reported as the current one.
    let other = dir.join("other.rs");
    std::fs::write(&other, "fn main() {}\n").unwrap();
    bridge.send(UiCommand::Input(format!(":call bufload(bufadd('{}'))<CR>", other.display())));
    bridge.send(UiCommand::Input(":EshviExplorer focus<CR>".into()));
    let reported_other = std::cell::Cell::new(false);
    let focus = wait_gui(&bridge, &mut ed, |g| {
        if matches!(g, GuiEvent::Buffer { path, .. } if path.ends_with("other.rs")) {
            reported_other.set(true);
        }
        matches!(g, GuiEvent::Explorer(_))
    });
    assert_eq!(focus, Some(GuiEvent::Explorer("focus".into())));
    assert!(!reported_other.get(), "the background file was reported as current");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// Opening a file with a stale swap file (its Neovim died) shows the
/// "ATTENTION" prompt. When the file is opened from the explorer, input must
/// still reach Neovim so the prompt can be answered.
#[test]
fn swap_file_prompt_when_opening_from_explorer_can_be_answered() {
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-swap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.txt");
    std::fs::write(&file, "swap test content\n").unwrap();
    let swapdir = format!("set directory={}//", dir.display());

    // Leave a stale swap file: a Neovim editing the file is killed.
    let mut holder = std::process::Command::new("nvim")
        .args(["--headless", "--clean", "--cmd", &swapdir])
        .arg(&file)
        // Unsaved changes in the swap file (unmodified ones are deleted silently).
        .args(["-c", "normal! Ochanged", "-c", "preserve"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let swap_exists = || std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).any(|e| e.file_name().to_string_lossy().ends_with(".swp"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !swap_exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300)); // let :preserve finish
    assert!(swap_exists(), "no swap file was created");
    holder.kill().unwrap();
    holder.wait().unwrap();

    // Open it the way the explorer does (`OpenFile`), after startup.
    let bridge = spawn(&["--clean", "--cmd", &swapdir], 60, 10);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    bridge.send(UiCommand::OpenFile { path: file.clone(), split: false });
    assert!(
        pump(&bridge, &mut ed, |ed| ed.messages.iter().any(|m| m.text().contains("swap file"))),
        "no swap prompt: messages {:?}, grids {}, cmdline {:?}, mode {:?}, grid1 {:?}",
        ed.messages,
        ed.grids.len(),
        ed.cmdline,
        ed.mode,
        ed.grid_text(1)
    );
    // "E" = edit anyway.
    bridge.send(UiCommand::Input("e".into()));
    assert!(
        pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| ed.grid_text(w.grid).iter().any(|l| l.starts_with("swap test")))),
        "input never reached Neovim"
    );
    bridge.send(UiCommand::Command("qa!".into()));
    std::thread::sleep(Duration::from_millis(200));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn diagnostics_summary_and_window_files_are_reported() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-diag-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let bridge = spawn(&["--clean", "-n", &file.to_string_lossy()], 60, 10);
    let mut ed = Editor::new();

    let wins = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Windows(w) if !w.is_empty()));
    let Some(GuiEvent::Windows(wins)) = wins else { panic!("no gui_windows") };
    assert!(wins[0].1.ends_with("lib.rs"), "{wins:?}");

    let lua = "local ns = vim.api.nvim_create_namespace('t') \
               vim.diagnostic.set(ns, 0, { \
                 { lnum = 0, col = 0, severity = vim.diagnostic.severity.ERROR, message = 'e1' }, \
                 { lnum = 0, col = 1, severity = vim.diagnostic.severity.ERROR, message = 'e2' }, \
                 { lnum = 0, col = 2, severity = vim.diagnostic.severity.WARN, message = 'w1' } })";
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    let Some(GuiEvent::Diagnostics(files)) =
        wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Diagnostics(f) if !f.is_empty()))
    else {
        panic!("no gui_diagnostics")
    };
    assert_eq!(files.len(), 1);
    assert!(files[0].path.ends_with("lib.rs"));
    assert_eq!((files[0].errors, files[0].warnings), (2, 1));

    // Clearing them reports an empty list.
    bridge.send(UiCommand::Command("lua vim.diagnostic.reset()".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Diagnostics(f) if f.is_empty())).is_some());

    // A save is reported with the file's full path (the Changes count
    // refreshes on it).
    bridge.send(UiCommand::Command("w".into()));
    let Some(GuiEvent::Saved(path)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Saved(_))) else {
        panic!("no gui_saved")
    };
    assert!(path.starts_with('/') && path.ends_with("lib.rs"), "{path}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn file_operations_keep_buffers_in_sync() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-fileops-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old.txt");
    std::fs::write(&old, "content\n").unwrap();
    let bridge = spawn(&["--clean", "-n", &old.to_string_lossy()], 60, 10);
    let mut ed = Editor::new();
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Buffer { path, .. } if path.ends_with("old.txt"))).is_some());

    // The GUI renamed the file on disk: the buffer follows.
    let new = dir.join("new.txt");
    std::fs::rename(&old, &new).unwrap();
    // File operations run off the input path, so poll with a check command.
    let eventually = |bridge: &Bridge, ed: &mut Editor, check: &str, want: &str| -> bool {
        for _ in 0..25 {
            ed.messages.clear();
            bridge.send(UiCommand::Input(format!(":echo {check}<CR>")));
            if pump_for(bridge, ed, Duration::from_millis(200), |ed| ed.messages.iter().any(|m| m.text() == want)) {
                return true;
            }
        }
        false
    };
    bridge.send(UiCommand::Renamed { from: old.clone(), to: new.clone() });
    assert!(eventually(&bridge, &mut ed, "expand('%:t') . ' ' . &modified", "new.txt 0"), "{:?}", ed.messages);

    // The GUI moved it to the Trash: the unmodified buffer is wiped.
    std::fs::remove_file(&new).unwrap();
    bridge.send(UiCommand::Deleted(new.clone()));
    let check = format!("bufexists('{}')", new.display());
    assert!(eventually(&bridge, &mut ed, &check, "0"), "{:?}", ed.messages);

    // Changing the root from the GUI changes Neovim's directory.
    bridge.send(UiCommand::Cd(dir.clone()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Cwd(c) if c.contains("eshvi-fileops"))).is_some());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// The wheel's path over a file: rows down and up from the top line, a
/// closed fold counting as one, the cursor kept in view.
#[test]
fn scroll_by_moves_the_top_line() {
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 80, 20);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    // (Commands run on tasks of their own: in one go where the order counts,
    // and asked again until the answer comes.)
    let run = |cmd: &str| bridge.send(UiCommand::Command(cmd.into()));
    run("lua vim.fn.setline(1, vim.tbl_map(tostring, vim.fn.range(1, 100))); vim.o.scrolloff = 0; vim.o.foldmethod = 'manual'; vim.fn.cursor(1, 1)");
    let check = |ed: &mut Editor, want: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            ed.messages.clear();
            bridge.send(UiCommand::Command("echo line('w0') . ' ' . line('.')".into()));
            if pump_for(&bridge, ed, Duration::from_millis(300), |ed| ed.messages.iter().any(|m| m.text() == want)) {
                return;
            }
            assert!(Instant::now() < deadline, "wanted {want}: {:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>());
        }
    };
    check(&mut ed, "1 1");
    run("lua Eshvi.scroll_by(vim.api.nvim_get_current_win(), 5)");
    check(&mut ed, "6 6");
    run("lua Eshvi.scroll_by(vim.api.nvim_get_current_win(), -2)");
    check(&mut ed, "4 6");
    // A fold of lines 5 to 20 under the top line: one step over it.
    run("lua vim.cmd('5,20fold'); vim.fn.cursor(4, 1); vim.cmd('normal! zt')");
    check(&mut ed, "4 4");
    run("lua Eshvi.scroll_by(vim.api.nvim_get_current_win(), 2)");
    check(&mut ed, "21 21");
    run("lua Eshvi.scroll_by(vim.api.nvim_get_current_win(), -1)");
    check(&mut ed, "5 21");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn menus_are_reported_and_run_in_the_current_mode() {
    use nvim_bridge::gui::{GuiEvent, MenuNode};
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-menu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.txt");
    std::fs::write(&file, "old\n").unwrap();
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(vec!["--clean".into(), "-n".into(), file.to_string_lossy().into()]),
            width: 80,
            height: 24,
            terminal_enabled: true,
            menu: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    let menus = |bridge: &Bridge, ed: &mut Editor, pred: &dyn Fn(&[MenuNode]) -> bool| -> Vec<MenuNode> {
        match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::Menu(m) if pred(m))) {
            Some(GuiEvent::Menu(m)) => m,
            _ => panic!("no matching gui_menu"),
        }
    };
    let m = menus(&bridge, &mut ed, &|m| !m.is_empty());
    let names: Vec<&str> = m.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["File", "Edit", "View", "Go", "Buffers", "Terminal", "Window", "Help"]);
    let file_menu = m[0].children.as_ref().unwrap();
    let save = file_menu.iter().find(|n| n.name == "Save").unwrap();
    assert_eq!((save.key.as_str(), save.action.as_deref()), ("⌘S", Some("save")));
    assert!(file_menu.iter().any(MenuNode::is_separator));
    // File › Open Recent ends with its Clear item.
    let recent = file_menu.iter().find(|n| n.name == "Open Recent").unwrap().children.as_ref().unwrap();
    assert_eq!(recent.last().unwrap().name, "Clear Recently Opened");
    // Buffers lists the file, checked as the current one.
    let buffers = m[4].children.as_ref().unwrap();
    assert!(buffers.iter().any(|b| b.name.ends_with("doc.txt") && b.checked == Some(true)), "{buffers:?}");

    // A second buffer joins Buffers and becomes the checked one.
    bridge.send(UiCommand::Command("edit other.txt".into()));
    let m = menus(&bridge, &mut ed, &|m| m.iter().any(|n| n.name == "Buffers" && n.children.as_ref().is_some_and(|c| c.len() == 2)));
    let buffers = m[4].children.as_ref().unwrap();
    let checked: Vec<_> = buffers.iter().filter(|b| b.checked == Some(true)).map(|b| b.name.as_str()).collect();
    assert_eq!(checked, ["other.txt"], "{buffers:?}");
    // The file left is now recent.
    let recent = m[0].children.as_ref().unwrap().iter().find(|n| n.name == "Open Recent").unwrap();
    assert!(recent.children.as_ref().unwrap()[0].name.ends_with("doc.txt"), "{recent:?}");
    bridge.send(UiCommand::Command("buffer 1".into()));

    // The user's own menus are shown too (after :EshviMenu, or at startup).
    // One command: separate `Command`s run concurrently, in any order.
    bridge.send(UiCommand::Command(r#"lua vim.cmd([[amenu 70.10 Mine.Say\ Hi<Tab>⌘K :echo "hi"<CR>]]) vim.cmd.EshviMenu()"#.into()));
    let m = menus(&bridge, &mut ed, &|m| m.iter().any(|n| n.name == "Mine"));
    let mine = m.iter().find(|n| n.name == "Mine").unwrap();
    assert_eq!(mine.children.as_ref().unwrap()[0], MenuNode { name: "Say Hi".into(), key: "⌘K".into(), ..Default::default() });

    // `:menu disable` greys an item out; a shortcut mapped in Neovim is
    // left to Neovim (the item loses it).
    bridge.send(UiCommand::Command(r#"lua vim.cmd([[amenu disable Mine.Say\ Hi]]) vim.keymap.set('n', '<D-S>', '<Nop>') vim.cmd.EshviMenu()"#.into()));
    let m = menus(&bridge, &mut ed, &|m| m.iter().any(|n| n.name == "Mine" && n.children.as_ref().is_some_and(|c| c[0].disabled)));
    let save_as = m[0].children.as_ref().unwrap().iter().find(|n| n.name == "Save As…").unwrap();
    assert_eq!(save_as.key, "");
    assert_eq!(m[0].children.as_ref().unwrap().iter().find(|n| n.name == "Save").unwrap().key, "⌘S");

    // A right click: Neovim reports its PopUp menu's items (as many as the
    // rows of the menu it draws).
    let win = ed.windows.values().find(|w| w.kind == grid::WindowKind::Normal).unwrap().grid;
    let mouse = |action: &str| UiCommand::InputMouse { button: "right".into(), action: action.into(), modifier: String::new(), grid: win, row: 0, col: 0 };
    bridge.send(mouse("press"));
    let Some(GuiEvent::PopupMenu(items)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PopupMenu(_))) else { panic!("no gui_popup_menu") };
    assert!(items.iter().any(|i| i.name == "Select All") && items.iter().any(MenuNode::is_separator), "{items:?}");
    let float = |ed: &Editor| {
        ed.windows.values().find(|w| matches!(w.kind, grid::WindowKind::Float { mouse_enabled: false, .. })).map(|w| ed.grids[&w.grid].height())
    };
    assert!(pump(&bridge, &mut ed, |ed| float(ed) == Some(items.len())), "{:?} rows for {} items", float(&ed), items.len());
    bridge.send(mouse("release"));
    bridge.send(UiCommand::Input("<Esc>".into()));
    assert!(pump(&bridge, &mut ed, |ed| float(ed).is_none()));

    // Chosen from the menu bar (as the GUI does it), in insert mode: Save
    // writes the file.
    bridge.send(UiCommand::Input("ccnew<Cmd>emenu File.Save<CR><Esc>".into()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read_to_string(&file).unwrap() != "new\n" {
        assert!(Instant::now() < deadline, "not saved: {:?}", std::fs::read_to_string(&file));
        pump_for(&bridge, &mut ed, Duration::from_millis(50), |_| false);
    }
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn which_key_title_shows_the_keys_pressed() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    // A stand-in for which-key's view module, loaded after setup.
    let dir = std::env::temp_dir().join(format!("eshvi-wk-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lua/which-key")).unwrap();
    std::fs::write(
        dir.join("lua/which-key/view.lua"),
        "local M = {} function M.trail(node, o) return { { 'crumbs', 'X' } } end return M",
    )
    .unwrap();
    let rtp = format!("set rtp^={}", dir.display());
    let bridge = spawn(&["--clean", "-n", "--cmd", &rtp], 60, 10);
    let mut ed = Editor::new();
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Windows(_))).is_some());

    let lua = "local v = require('which-key.view') \
               local t = function(path, o) return v.trail({ path = path }, o)[1][1] end \
               vim.api.nvim_buf_set_lines(0, 0, -1, false, { \
                 t({ '<Space>', 'f' }, { title = true }) .. '|' .. t({ '<C-w>' }, { title = true }) \
                 .. '|' .. t({}, { title = true }) .. '|' .. t({ 'g' }, {}) })";
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    // Titles become the keys; the root and other uses keep which-key's own.
    let want = " Space f | Ctrl+W |crumbs|crumbs";
    assert!(pump(&bridge, &mut ed, |ed| first_line(ed).trim_end() == want), "{:?}", first_line(&ed));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn snacks_image_buffers_show_again_when_they_come_back() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    // A stand-in for snacks.image's placement: hidden once its buffer left
    // every window, and (the bug) nothing unhides it.
    let dir = std::env::temp_dir().join(format!("eshvi-snacks-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lua/snacks/image")).unwrap();
    std::fs::write(
        dir.join("lua/snacks/image/placement.lua"),
        "local M = {} M.__index = M \
         function M.new(buf, opts) return setmetatable({ buf = buf, opts = opts or {}, eids = {}, hidden = true }, M) end \
         function M:ready() return true end \
         function M:wins() return vim.tbl_filter(function(w) return vim.api.nvim_win_get_buf(w) == self.buf end, \
           vim.api.nvim_tabpage_list_wins(0)) end \
         function M:update() self.eids = { vim.api.nvim_buf_set_extmark(self.buf, \
           vim.api.nvim_create_namespace('snacks.image'), 0, 0, {}) } end \
         return M",
    )
    .unwrap();
    let rtp = format!("set rtp^={}", dir.display());
    let bridge = spawn(&["--clean", "-n", "--cmd", &rtp], 60, 10);
    let mut ed = Editor::new();
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Windows(_))).is_some());

    let lua = "local P = require('snacks.image.placement') \
               local ns = vim.api.nvim_create_namespace('snacks.image') \
               local cur = vim.api.nvim_get_current_buf() \
               vim.api.nvim_buf_set_extmark(cur, ns, 0, 0, { virt_text = { { 'spinner' } } }) \
               local shown, inline, away = P.new(cur), P.new(cur, { inline = true }), P.new(vim.api.nvim_create_buf(true, false)) \
               shown:update() inline:update() away:update() \
               vim.api.nvim_buf_set_lines(0, 0, -1, false, { table.concat({ tostring(shown.hidden), \
                 tostring(inline.hidden), tostring(away.hidden), #vim.api.nvim_buf_get_extmarks(cur, ns, 0, -1, {}) }, '|') })";
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    // Shown again in its window, the stale spinner gone; inline images and
    // buffers in no window stay as they were.
    let want = "false|true|true|2";
    assert!(pump(&bridge, &mut ed, |ed| first_line(ed).trim_end() == want), "{:?}", first_line(&ed));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ui_send_data_and_lsp_progress_reach_the_gui() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 40, 10);
    let mut ed = Editor::new();
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Windows(_))).is_some());

    // The startup background query was answered in time: no "terminal did
    // not respond" warning.
    let warned = |ed: &Editor| ed.messages.iter().any(|m| m.text().contains("E1568"));
    assert!(!warned(&ed), "{:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>());

    // Attached with `stdout_tty`: data for the terminal comes as `ui_send`.
    bridge.send(UiCommand::Command(r#"lua vim.api.nvim_ui_send("\27_Ga=p,U=1,i=3\27\\")"#.into()));
    let want = b"\x1b_Ga=p,U=1,i=3\x1b\\";
    assert!(
        pump(&bridge, &mut ed, |ed| ed.tty_out.windows(want.len()).any(|w| w == want)),
        "{:?}",
        String::from_utf8_lossy(&ed.tty_out)
    );
    // snacks.image is told the placeholders work.
    bridge.send(UiCommand::Command("lua vim.api.nvim_buf_set_lines(0, 0, -1, false, { vim.env.SNACKS_KITTY })".into()));
    assert!(pump(&bridge, &mut ed, |ed| first_line(ed).trim_end() == "1"));

    // LSP progress: the latest running one, then none.
    let progress = |value: &str| {
        format!("lua vim.api.nvim_exec_autocmds('LspProgress', {{ data = {{ client_id = 9, params = {{ token = 't', value = {value} }} }} }})")
    };
    bridge.send(UiCommand::Command(progress("{ kind = 'begin', title = 'Indexing', percentage = 40 }")));
    let Some(GuiEvent::Progress(Some(p))) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Progress(Some(_)))) else {
        panic!("no gui_progress")
    };
    assert_eq!((p.source.as_str(), p.title.as_str(), p.percent), ("LSP", "Indexing", Some(40)));
    bridge.send(UiCommand::Command(progress("{ kind = 'end' }")));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Progress(None))).is_some());
    // A server detaching mid-way takes its progress with it.
    bridge.send(UiCommand::Command(progress("{ kind = 'begin', title = 'Loading' }")));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Progress(Some(_)))).is_some());
    bridge.send(UiCommand::Command("lua vim.api.nvim_exec_autocmds('LspDetach', { data = { client_id = 9 } })".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Progress(None))).is_some());
    bridge.send(UiCommand::Command("qa!".into()));
}

/// Connects to eshvi's IDE server the way Claude Code 2.1 does: the lock
/// file's token in a header, the `mcp` subprotocol, `initialize`,
/// `ide_connected`; then reads what the editor sends and calls a tool.
#[test]
fn claude_code_connects_and_gets_selection_mentions_and_diagnostics() {
    use futures::{SinkExt, StreamExt};
    use serde_json::{json, Value as Json};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let dir = test_ide_dir("connect");
    let _ = std::fs::remove_dir_all(&dir);
    let work = std::env::temp_dir().join(format!("eshvi-ide-work-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let file = work.join("lib.rs");
    std::fs::write(&file, "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(vec!["--clean".into(), "-n".into(), file.to_string_lossy().into()]),
            width: 100,
            height: 24,
            agent: Some(AgentOptions {
                command: vec!["sh".into(), "-c".into(), "read x".into()],
                ide_dir: Some(dir.clone()),
                ..Default::default()
            }),
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    // The lock file Claude Code looks for.
    let find_lock = || {
        std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "lock"))
    };
    let lock = (0..100)
        .find_map(|_| {
            let found = find_lock();
            if found.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            }
            found
        })
        .expect("a lock file");
    let port: u16 = lock.file_stem().unwrap().to_str().unwrap().parse().unwrap();
    let info: Json = serde_json::from_str(&std::fs::read_to_string(&lock).unwrap()).unwrap();
    assert_eq!((info["ideName"].as_str(), info["transport"].as_str()), (Some("Eshvi"), Some("ws")));
    assert_eq!(info["pid"].as_u64(), Some(std::process::id() as u64));
    assert!(info["workspaceFolders"].as_array().is_some_and(|f| !f.is_empty()));
    let token = info["authToken"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 32);
    // Only the user may read the token.
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&lock).unwrap().permissions()) & 0o777, 0o600);

    // (Agents are told the port by the GUI: `gui_agent_env`, in `eshvi_agent_only_notifies_the_gui`.)

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let request = |token: &str| {
            let mut req = format!("ws://127.0.0.1:{port}").into_client_request().unwrap();
            req.headers_mut().insert("X-Claude-Code-Ide-Authorization", token.parse().unwrap());
            req.headers_mut().insert("Sec-WebSocket-Protocol", "mcp".parse().unwrap());
            req
        };
        assert!(tokio_tungstenite::connect_async(request("0123")).await.is_err(), "a wrong token is refused");
        let (mut ws, resp) = tokio_tungstenite::connect_async(request(&token)).await.unwrap();
        assert_eq!(resp.headers()["sec-websocket-protocol"], "mcp");

        let send = |v: Json| Message::text(v.to_string());
        // The next message matching `pred` (others are skipped).
        async fn next_matching(
            ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
            pred: impl Fn(&Json) -> bool,
        ) -> Json {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let msg = tokio::time::timeout_at(deadline, ws.next()).await.expect("timed out").unwrap().unwrap();
                if let Message::Text(t) = msg {
                    let v: Json = serde_json::from_str(&t).unwrap();
                    if pred(&v) {
                        return v;
                    }
                }
            }
        }

        ws.send(send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "2.1"}}})))
            .await
            .unwrap();
        let init = next_matching(&mut ws, |v| v["id"] == 1).await;
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        ws.send(send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))).await.unwrap();
        ws.send(send(json!({"jsonrpc": "2.0", "method": "ide_connected", "params": {"pid": 1}}))).await.unwrap();
        ws.send(send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))).await.unwrap();
        let tools = next_matching(&mut ws, |v| v["id"] == 2).await;
        let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        for name in ["openDiff", "close_tab", "closeAllDiffTabs", "getDiagnostics"] {
            assert!(names.contains(&name), "{names:?}");
        }

        // Moving in the file: the selection follows (the cursor as an
        // empty selection; a visual selection with its text).
        bridge.send(UiCommand::Input("<C-\\><C-n><C-w>hgg".into()));
        bridge.send(UiCommand::Input("jVj".into()));
        let sel = next_matching(&mut ws, |v| v["method"] == "selection_changed" && v["params"]["selection"]["isEmpty"] == false).await;
        let p = &sel["params"];
        assert!(p["filePath"].as_str().unwrap().ends_with("lib.rs"), "{p}");
        assert_eq!(p["text"], "fn b() {}\nfn c() {}");
        assert_eq!((p["selection"]["start"]["line"].as_i64(), p["selection"]["end"]["line"].as_i64()), (Some(1), Some(2)));

        // Lines sent as context, as an @-mention.
        bridge.send(UiCommand::Input(":EshviAgentSend<CR>".into()));
        let m = next_matching(&mut ws, |v| v["method"] == "at_mentioned").await;
        assert!(m["params"]["filePath"].as_str().unwrap().ends_with("lib.rs"));
        assert_eq!((m["params"]["lineStart"].as_i64(), m["params"]["lineEnd"].as_i64()), (Some(1), Some(2)));

        // Diagnostics, as Claude asks for them around its edits.
        let lua = "vim.diagnostic.set(vim.api.nvim_create_namespace('t'), vim.fn.bufnr('lib.rs'), \
                   {{ lnum = 0, col = 3, severity = vim.diagnostic.severity.ERROR, message = 'boom', source = 'test' }})";
        bridge.send(UiCommand::Command(format!("lua {lua}")));
        tokio::time::sleep(Duration::from_millis(200)).await;
        ws.send(send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "getDiagnostics", "arguments": {}}})))
            .await
            .unwrap();
        let d = next_matching(&mut ws, |v| v["id"] == 3).await;
        let list: Json = serde_json::from_str(d["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let file_diags = &list.as_array().unwrap()[0];
        assert!(file_diags["uri"].as_str().unwrap().starts_with("file://"));
        assert_eq!(file_diags["diagnostics"][0]["message"], "boom");
        assert_eq!(file_diags["diagnostics"][0]["severity"], "Error");
        assert_eq!(file_diags["diagnostics"][0]["range"]["start"]["character"], 3);

        // The open files, and whether one has unsaved changes.
        let call = |id: i64, name: &str, args: Json| {
            send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}}))
        };
        ws.send(call(4, "getOpenEditors", json!({}))).await.unwrap();
        let e = next_matching(&mut ws, |v| v["id"] == 4).await;
        let editors: Json = serde_json::from_str(e["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(editors["tabs"][0]["label"], "lib.rs", "{editors}");
        let path = file.canonicalize().unwrap().to_string_lossy().into_owned();
        ws.send(call(5, "checkDocumentDirty", json!({ "filePath": path }))).await.unwrap();
        let d = next_matching(&mut ws, |v| v["id"] == 5).await;
        let dirty: Json = serde_json::from_str(d["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!((dirty["success"].as_bool(), dirty["isDirty"].as_bool()), (Some(true), Some(false)), "{dirty}");

        // A proposed edit, as a diff beside the file. Edited, then `:w`:
        // saved, and Claude gets the final text.
        let diff = |id: i64, tab: &str| {
            call(id, "openDiff", json!({ "old_file_path": path, "new_file_path": path,
                "new_file_contents": "fn a() {}\nfn b2() {}\nfn c() {}\n", "tab_name": tab }))
        };
        ws.send(diff(6, "edit-1")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        bridge.send(UiCommand::Input("<Esc>GA // mine<Esc>:w<CR>".into()));
        let r = next_matching(&mut ws, |v| v["id"] == 6).await;
        let content = &r["result"]["content"];
        assert_eq!(content[0]["text"], "FILE_SAVED", "{r}");
        assert_eq!(content[1]["text"], "fn a() {}\nfn b2() {}\nfn c() {} // mine\n");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn a() {}\nfn b2() {}\nfn c() {} // mine\n");

        // `:q` on the proposal rejects it; the file stays.
        ws.send(diff(7, "edit-2")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        bridge.send(UiCommand::Input(":q<CR>".into()));
        let r = next_matching(&mut ws, |v| v["id"] == 7).await;
        assert_eq!(r["result"]["content"][0]["text"], "DIFF_REJECTED", "{r}");
        assert!(std::fs::read_to_string(&file).unwrap().contains("// mine"));

        // Answered in Claude's own prompt: it closes the view.
        ws.send(diff(8, "edit-3")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        ws.send(call(9, "close_tab", json!({ "tab_name": "edit-3" }))).await.unwrap();
        let r = next_matching(&mut ws, |v| v["id"] == 8).await;
        assert_eq!(r["result"]["content"][0]["text"], "TAB_CLOSED", "{r}");
        let r = next_matching(&mut ws, |v| v["id"] == 9).await;
        assert_eq!(r["result"]["content"][0]["text"], "TAB_CLOSED", "{r}");

        // The same tab name again while its view is up: the first call is
        // answered "rejected" (its view closes), the second one waits for
        // its own view, and `:w` there is its answer.
        ws.send(diff(10, "again")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        ws.send(diff(11, "again")).await.unwrap();
        let r = next_matching(&mut ws, |v| v["id"] == 10 || v["id"] == 11).await;
        assert_eq!((r["id"].as_i64(), r["result"]["content"][0]["text"].as_str()), (Some(10), Some("DIFF_REJECTED")), "{r}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        bridge.send(UiCommand::Input(":w<CR>".into()));
        let r = next_matching(&mut ws, |v| v["id"] == 11).await;
        assert_eq!(r["result"]["content"][0]["text"], "FILE_SAVED", "{r}");
    });
    bridge.send(UiCommand::Command("qa!".into()));
    // The lock file goes with eshvi.
    assert!((0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        !lock.exists()
    }));
    let _ = std::fs::remove_dir_all(work);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn changes_diffs_open_against_the_snapshot() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let repo = std::env::temp_dir().join(format!("eshvi-changes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&["add", "."]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();

    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(vec!["--clean".into(), "-n".into(), "-c".into(), format!("cd {}", repo.display())]),
            width: 100,
            height: 24,
            agent: Some(AgentOptions {
                command: vec!["sh".into(), "-c".into(), "read x".into()],
                ide_dir: Some(test_ide_dir("changes")),
                ..Default::default()
            }),
            winbar: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    // A file's diff, `[agent] diff = "tab"`: a tab page with the snapshot's
    // version left of the file (which has the cursor).
    let top = repo.canonicalize().unwrap();
    let diff = |how: &str| format!("lua Eshvi.changes_diff([==[{}]==], 'HEAD', 'a.txt', false, '{how}')", top.display());
    let a = repo.join("a.txt");
    bridge.send(UiCommand::Command(diff("tab")));
    let tab_open = |ed: &Editor| ed.messages.iter().any(|m| m.text().starts_with("a.txt: changes"));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), tab_open), "{:?}", ed.messages);
    // (No frame of floats: the float diff is gone.)
    assert!(!ed.windows.values().any(|w| matches!(w.kind, grid::WindowKind::Float { .. })));
    let texts = |ed: &Editor| -> Vec<Vec<String>> { ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal).map(|w| ed.grid_text(w.grid)).collect()};
    assert!(
        pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| {
            let t = texts(ed);
            t.iter().any(|t| t.iter().any(|l| l.contains("two"))) && t.iter().any(|t| t.iter().any(|l| l.trim() == "one") && !t.iter().any(|l| l.contains("two")))
        }),
        "{:?}",
        texts(&ed)
    );
    ed.messages.clear();
    bridge.send(UiCommand::Input(":echo &diff . ' ' . expand('%:t') . ' ' . win_gettype() . tabpagenr('$')<CR>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text() == "1 a.txt 2")), "{:?}", ed.messages);
    // One change undone in the file (`do` takes the snapshot's version of
    // it), then saved.
    bridge.send(UiCommand::Input("Gdo:w<CR>".into()));
    assert!(
        (0..100).any(|_| {
            pump_for(&bridge, &mut ed, Duration::from_millis(50), |_| false);
            std::fs::read_to_string(&a).unwrap() == "one\n"
        }),
        "{:?}",
        std::fs::read_to_string(&a)
    );
    // q on the original closes the tab.
    bridge.send(UiCommand::Input("<C-w>hq".into()));
    ed.messages.clear();
    bridge.send(UiCommand::Input(":echo tabpagenr('$')<CR>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text() == "1")), "{:?}", ed.messages);

    // Again, with the file changed: both sides have headers, so their rows
    // line up; q on the original closes it.
    std::fs::write(&a, "one\nthree\n").unwrap();
    bridge.send(UiCommand::Command("checktime".into()));
    bridge.send(UiCommand::Command(diff("tab")));
    ed.messages.clear();
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), tab_open), "{:?}", ed.messages);
    ed.messages.clear();
    bridge.send(UiCommand::Input(":echo tabpagenr('$') . &diff . expand('%:t') . '|' . getwinvar(winnr('h'), '&winbar')<CR>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text() == "21a.txt|%t")), "{:?}", ed.messages);
    ed.messages.clear();
    bridge.send(UiCommand::Input("<C-w>hq".into()));
    bridge.send(UiCommand::Input(":echo tabpagenr('$')<CR>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text() == "1")), "{:?}", ed.messages);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(repo);
}

/// `[git] diff = "buffer"` and `[agent] diff = "buffer"`, and a whole commit:
/// what `git diff` and `git show` print, in a buffer of filetype `diff` in the
/// file's window; q goes back to the file.
#[test]
fn diffs_open_in_a_buffer() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let repo = std::env::temp_dir().join(format!("eshvi-diff-buffer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    std::fs::write(repo.join("b.txt"), "x\n").unwrap();
    git(&["add", "."]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    let oid = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(repo.join("new.txt"), "fresh\n").unwrap();

    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(vec!["--clean".into(), "-n".into(), "-c".into(), format!("cd {}", repo.display()), "b.txt".into()]),
            width: 100,
            height: 24,
            agent: Some(AgentOptions {
                command: vec!["sh".into(), "-c".into(), "read x".into()],
                ide_dir: Some(test_ide_dir("diff-buffer")),
                ..Default::default()
            }),
            winbar: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let top = repo.canonicalize().unwrap();
    let top = top.display();
    // (Asked again until it holds: git answers in the background.)
    let ask = |bridge: &Bridge, ed: &mut Editor, expr: &str, want: &str| {
        let holds = (0..50).any(|_| {
            ed.messages.clear();
            bridge.send(UiCommand::Input(format!(":echo {expr}<CR>")));
            pump_for(bridge, ed, Duration::from_millis(100), |ed| ed.messages.iter().any(|m| m.text() == want))
        });
        assert!(holds, "{expr}: {:?}", ed.messages);
    };
    let lines = "join(getline(1, '$'), '|')";

    // A file against the index, as git prints it.
    bridge.send(UiCommand::Command(format!(
        "lua Eshvi.changes_diff([==[{top}]==], '', 'a.txt', false, 'buffer', {{ left_label = 'Index', right_label = 'Working Tree' }})"
    )));
    ask(&bridge, &mut ed, "&filetype", "diff");
    ask(
        &bridge,
        &mut ed,
        lines,
        "diff --git a/a.txt b/a.txt|--- a/a.txt|+++ b/a.txt|@@ -1 +1,2 @@| one|+two",
    );
    ask(&bridge, &mut ed, "&modifiable . win_gettype() . expand('%:t') . '|' . &winbar", "0a.txt (Index → Working Tree)|%f %m");
    // Another of the same name replaces it (the name free again); then one of
    // another file; q goes back to the file from before them.
    bridge.send(UiCommand::Command("let g:first = bufnr()".into()));
    bridge.send(UiCommand::Command(format!("lua Eshvi.changes_diff([==[{top}]==], '', 'a.txt', false, 'buffer', {{ left_label = 'Index', right_label = 'Working Tree' }})")));
    ask(&bridge, &mut ed, "(bufnr() != g:first) . bufexists(g:first) . expand('%:t')", "10a.txt (Index → Working Tree)");
    bridge.send(UiCommand::Command(format!("lua Eshvi.changes_diff([==[{top}]==], 'HEAD', 'new.txt', false, 'buffer')")));
    ask(
        &bridge,
        &mut ed,
        lines,
        "diff --git a/new.txt b/new.txt|--- /dev/null|+++ b/new.txt|@@ -0,0 +1 @@|+fresh",
    );
    bridge.send(UiCommand::Input("q".into()));
    ask(&bridge, &mut ed, "expand('%:t') . bufnr('new.txt (changes)')", "b.txt-1");
    // A deleted file; no changes opens nothing.
    bridge.send(UiCommand::Command(format!("lua Eshvi.changes_diff([==[{top}]==], 'HEAD', 'a.txt', true, 'buffer', {{ left_label = 'HEAD', right = '' }})")));
    ask(&bridge, &mut ed, lines, "diff --git a/a.txt b/a.txt|--- a/a.txt|+++ /dev/null|@@ -1 +0,0 @@|-one");
    bridge.send(UiCommand::Input("q".into()));
    ask(&bridge, &mut ed, "expand('%:t')", "b.txt");
    ed.messages.clear();
    bridge.send(UiCommand::Command(format!("lua Eshvi.changes_diff([==[{top}]==], 'HEAD', 'b.txt', false, 'buffer')")));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text() == "b.txt: no changes")), "{:?}", ed.messages);

    // A whole commit: `git show`; its tab's "×" goes back as q does.
    bridge.send(UiCommand::Command(format!("lua Eshvi.commit_diff([==[{top}]==], [==[{oid}]==])")));
    ask(&bridge, &mut ed, "&filetype . expand('%:t')", &format!("diff{} (commit)", &oid[..7]));
    ask(&bridge, &mut ed, "getline(1) . '|' . (search('^+++ b/b.txt') > 0)", &format!("commit {oid}|1"));
    bridge.send(UiCommand::Command("lua Eshvi.close_window(vim.api.nvim_get_current_win())".into()));
    ask(&bridge, &mut ed, "expand('%:t')", "b.txt");
    // 'nohidden' and unsaved changes: beside them, which stay.
    bridge.send(UiCommand::Input(":set nohidden<CR>ix<Esc>".into()));
    bridge.send(UiCommand::Command(format!("lua Eshvi.commit_diff([==[{top}]==], [==[{oid}]==])")));
    ask(&bridge, &mut ed, "&filetype . getbufvar('b.txt', '&modified') . (winbufnr(winnr('j')) == bufnr('b.txt'))", "diff11");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(repo);
}

/// `[agent] enabled = false`: the diffs are not part of the agent panel.
#[test]
fn diffs_open_without_the_agent_panel() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let repo = std::env::temp_dir().join(format!("eshvi-diff-no-agent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&["add", "."]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    let oid = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();

    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(vec!["--clean".into(), "-n".into(), "-c".into(), format!("cd {}", repo.display()), "a.txt".into()]),
            width: 100,
            height: 24,
            winbar: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let top = repo.canonicalize().unwrap();
    let top = top.display();
    let ask = |bridge: &Bridge, ed: &mut Editor, expr: &str, want: &str| {
        let holds = (0..50).any(|_| {
            ed.messages.clear();
            bridge.send(UiCommand::Input(format!(":echo {expr}<CR>")));
            pump_for(bridge, ed, Duration::from_millis(100), |ed| ed.messages.iter().any(|m| m.text() == want))
        });
        assert!(holds, "{expr}: {:?}", ed.messages);
    };
    let diff = |how: &str| {
        format!("lua Eshvi.changes_diff([==[{top}]==], '', 'a.txt', false, '{how}', {{ left_label = 'Index', right_label = 'Working Tree' }})")
    };

    // A buffer, and back.
    bridge.send(UiCommand::Command(diff("buffer")));
    ask(&bridge, &mut ed, "&filetype . getline('$')", "diff+two");
    bridge.send(UiCommand::Input("q".into()));
    ask(&bridge, &mut ed, "expand('%:t')", "a.txt");
    // A tab page.
    bridge.send(UiCommand::Command(diff("tab")));
    ask(&bridge, &mut ed, "&diff . tabpagenr('$')", "12");
    bridge.send(UiCommand::Command("tabonly".into()));
    ask(&bridge, &mut ed, "tabpagenr('$')", "1");
    // A whole commit.
    bridge.send(UiCommand::Command(format!("lua Eshvi.commit_diff([==[{top}]==], [==[{oid}]==])")));
    ask(&bridge, &mut ed, "&filetype . getline(1)[:6]", "diffcommit ");
    bridge.send(UiCommand::Input("q".into()));
    ask(&bridge, &mut ed, "expand('%:t')", "a.txt");
    // Nothing but a terminal in the way: a window beside it, an editor's.
    bridge.send(UiCommand::Input(":terminal<CR><C-\\><C-n>:only<CR>".into()));
    ask(&bridge, &mut ed, "&buftype . winnr('$')", "terminal1");
    bridge.send(UiCommand::Command(diff("buffer")));
    ask(&bridge, &mut ed, "&filetype . winnr('$')", "diff2");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(repo);
}

/// The Source Control panel's diffs: what `changes_diff` does with `opts`.
#[test]
fn source_control_diffs_compare_head_index_and_the_file() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let repo = std::env::temp_dir().join(format!("eshvi-scm-diff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&["add", "."]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    // HEAD: one. Index: one, two. The file: one, two, three.
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&["add", "a.txt"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();

    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(vec!["--clean".into(), "-n".into(), "-c".into(), format!("cd {}", repo.display())]),
            width: 100,
            height: 24,
            agent: Some(AgentOptions {
                command: vec!["sh".into(), "-c".into(), "read x".into()],
                ide_dir: Some(test_ide_dir("scm-diff")),
                ..Default::default()
            }),
            winbar: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let top = repo.canonicalize().unwrap();
    let texts = |ed: &Editor| -> Vec<Vec<String>> { ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal).map(|w| ed.grid_text(w.grid)).collect() };
    let has = |texts: &[Vec<String>], word: &str| texts.iter().any(|t| t.iter().any(|l| l.contains(word)));
    let has_line = |texts: &[Vec<String>], line: &str| texts.iter().any(|t| t.iter().any(|l| l.trim() == line));
    // (Asked again until it holds: git answers in the background.)
    let echo = |bridge: &Bridge, ed: &mut Editor, expr: &str, want: &str| {
        let holds = (0..50).any(|_| {
            ed.messages.clear();
            bridge.send(UiCommand::Input(format!(":echo {expr}<CR>")));
            pump_for(bridge, ed, Duration::from_millis(100), |ed| ed.messages.iter().any(|m| m.text() == want))
        });
        assert!(holds, "{expr}: {:?}", ed.messages);
    };

    // Staged, in a tab: HEAD left, the index right, both read-only, named by their labels.
    bridge.send(UiCommand::Command(format!(
        "lua Eshvi.changes_diff([==[{}]==], 'HEAD', 'a.txt', false, 'tab', \
         {{ right = '', left_label = 'HEAD', right_label = 'Index' }})",
        top.display()
    )));
    let staged = |ed: &Editor| {
        let t = texts(ed);
        has(&t, "two") && !has(&t, "three") && has_line(&t, "one")
    };
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), staged), "{:?}", texts(&ed));
    echo(&bridge, &mut ed, "&modifiable . &diff . bufname('%') . tabpagenr('$')", "01a.txt (Index)2");
    // The left side is read-only too.
    echo(&bridge, &mut ed, "getbufvar(winbufnr(winnr('h')), '&modifiable') . bufname(winbufnr(winnr('h')))", "0a.txt (HEAD)");

    // A working-tree diff in another tab: the index left, the file itself right (editable).
    bridge.send(UiCommand::Command("tabclose".into()));
    bridge.send(UiCommand::Command(format!(
        "lua Eshvi.changes_diff([==[{}]==], '', 'a.txt', false, 'tab', \
         {{ left_label = 'Index', right_label = 'Working Tree' }})",
        top.display()
    )));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| has(&texts(ed), "three")), "{:?}", texts(&ed));
    echo(&bridge, &mut ed, "&modifiable . &diff . expand('%:t') . bufexists('a.txt (Index)')", "11a.txt1");

    // A deleted file: the left side alone.
    bridge.send(UiCommand::Command("tabclose".into()));
    bridge.send(UiCommand::Command(format!(
        "lua Eshvi.changes_diff([==[{}]==], 'HEAD', 'a.txt', true, 'tab', {{ right = '', left_label = 'HEAD' }})",
        top.display()
    )));
    echo(&bridge, &mut ed, "tabpagenr('$') . winnr('$') . bufname('%')", "21a.txt (HEAD)");
    // (A `float` is not a mode any more: it opens the tab.)
    bridge.send(UiCommand::Command("tabclose".into()));
    bridge.send(UiCommand::Command(format!(
        "lua Eshvi.changes_diff([==[{}]==], 'HEAD', 'a.txt', false, 'float', {{ right = '', right_label = 'Index' }})",
        top.display()
    )));
    echo(&bridge, &mut ed, "tabpagenr('$') . &diff", "21");
    assert!(!ed.windows.values().any(|w| matches!(w.kind, grid::WindowKind::Float { .. })));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(repo);
}

/// `:EshviGit` tells the GUI, with its argument (toggle when none).
#[test]
fn eshvi_git_command_reaches_the_gui() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 80, 24);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    for (command, want) in [
        ("EshviGit focus", "focus"),
        ("EshviGit", "toggle"),
        ("EshviGit hide", "hide"),
        ("EshviGit show", "show"),
        ("EshviGit graph", "graph"),
    ] {
        bridge.send(UiCommand::Input(format!(":{command}<CR>")));
        let got = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Git(_)));
        assert_eq!(got, Some(GuiEvent::Git(want.into())), "{command}");
    }
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn external_windows_are_reported_as_external() {
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 80, 24);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let lua = "local b = vim.api.nvim_create_buf(false, true) \
               vim.api.nvim_buf_set_lines(b, 0, -1, false, { 'outside' }) \
               vim.api.nvim_open_win(b, false, { external = true, width = 30, height = 5 })";
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    assert!(
        pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::External
            && ed.grids.get(&w.grid).is_some_and(|g| g.width() == 30 && g.height() == 5)
            && ed.grid_text(w.grid)[0].starts_with("outside"))),
        "{:?}",
        ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>()
    );
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn window_tab_close_works_like_quit() {
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 80, 20);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let ask = |bridge: &Bridge, ed: &mut Editor, lua: &str, want: &str| {
        ed.messages.clear();
        bridge.send(UiCommand::Command(format!("lua print({lua})")));
        assert!(
            pump_for(bridge, ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text() == want)),
            "{lua}: {:?}",
            ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>()
        );
    };
    // Two windows: the "×" closes the one it's on.
    bridge.send(UiCommand::Input("<Cmd>vsplit<CR>".into()));
    ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)", "2");
    bridge.send(UiCommand::Input("<Cmd>lua Eshvi.close_window(vim.fn.win_getid(1))<CR>".into()));
    ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)", "1");
    // Modified: the window closes and the buffer stays, hidden, with its
    // changes (:q with 'hidden', Neovim's default).
    bridge.send(UiCommand::Input("<Cmd>vsplit | enew | call setline(1, 'x')<CR>".into()));
    bridge.send(UiCommand::Input("<Cmd>lua Eshvi.close_window(vim.api.nvim_get_current_win())<CR>".into()));
    ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)", "1");
    ask(&bridge, &mut ed, "#vim.tbl_filter(function(b) return vim.bo[b].modified end, vim.api.nvim_list_bufs())", "1");
    // The last window with a file: an empty buffer instead of quitting.
    bridge.send(UiCommand::Input("<Cmd>only<CR>".into()));
    bridge.send(UiCommand::Input("<Cmd>lua Eshvi.close_window(vim.api.nvim_get_current_win())<CR>".into()));
    ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0) .. ' ' .. vim.fn.bufname()", "1 ");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// `:EshviPreview`: a Markdown file previewed in place and beside it; the
/// reports name the windows, the line to show (following the cursor) and
/// send the text once and again when it changes; typing goes back to the
/// text; a side preview goes with its file.
#[test]
fn markdown_previews_are_reported_and_follow_the_file() {
    use nvim_bridge::gui::{GuiEvent, PreviewWin};
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-preview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.md");
    std::fs::write(&file, "# Title\n\ntext\n\n## Two\n\nmore\n").unwrap();
    let bridge = spawn(&["--clean", "-n", file.to_str().unwrap()], 80, 20);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let previews = |bridge: &Bridge, ed: &mut Editor, pred: &dyn Fn(&[PreviewWin]) -> bool| -> Vec<PreviewWin> {
        match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::Preview(w) if pred(w))) {
            Some(GuiEvent::Preview(w)) => w,
            other => panic!("no matching gui_preview: {other:?}"),
        }
    };

    // In place, at the cursor's line; its text comes with it.
    bridge.send(UiCommand::Input("<Cmd>5<CR><Cmd>EshviPreview<CR>".into()));
    let w = previews(&bridge, &mut ed, &|w| w.len() == 1);
    assert!(!w[0].side && w[0].line == 5 && w[0].path.ends_with("doc.md"), "{w:?}");
    // (The text was sent just before the report.)
    bridge.send(UiCommand::Input("<Cmd>call setline(3, 'changed')<CR>".into()));
    let text = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PreviewText { text, .. } if text.contains("changed")));
    assert!(text.is_some(), "the changed text wasn't sent");
    // The cursor moves: so does the line.
    bridge.send(UiCommand::Input("gg".into()));
    previews(&bridge, &mut ed, &|w| w.len() == 1 && w[0].line == 1);
    // Typing goes back to the text.
    bridge.send(UiCommand::Input("i".into()));
    previews(&bridge, &mut ed, &|w| w.is_empty());
    bridge.send(UiCommand::Input("<Esc>".into()));

    // Beside: a window of its own; the file's stays current.
    bridge.send(UiCommand::Input("<Cmd>EshviPreview side<CR>".into()));
    let w = previews(&bridge, &mut ed, &|w| w.len() == 1 && w[0].side);
    let side_win = w[0].win;
    // Again: closes it.
    bridge.send(UiCommand::Input("<Cmd>EshviPreview side<CR>".into()));
    previews(&bridge, &mut ed, &|w| w.is_empty());
    // A side preview goes with its file.
    bridge.send(UiCommand::Input("<Cmd>EshviPreview side<CR>".into()));
    let w = previews(&bridge, &mut ed, &|w| w.len() == 1 && w[0].side);
    assert_ne!(w[0].win, side_win);
    bridge.send(UiCommand::Input("<Cmd>bwipeout!<CR>".into()));
    previews(&bridge, &mut ed, &|w| w.is_empty());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// Files dropped on the window: in the window under the pointer, one after another;
// a window that can't hold a file gives way to the editor's.
#[test]
fn dropped_files_open_in_the_window_they_fell_on() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["left.txt", "right.txt", "x.txt", "y.txt", "z.txt"] {
        std::fs::write(dir.join(name), name).unwrap();
    }
    let bridge = spawn(&["--clean", "-n", "-c", &format!("cd {}", dir.display()), "-c", "edit left.txt | vsplit right.txt"], 100, 30);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal).count() == 2));
    // Each window's file, the current one marked.
    let windows = |ed: &mut Editor, want: &str| {
        pump_for(&bridge, ed, Duration::from_millis(300), |_| false);
        ed.messages.clear();
        bridge.send(UiCommand::Command("lua print('WINS ' .. table.concat(vim.tbl_map(function(w) return vim.fn.fnamemodify(vim.api.nvim_buf_get_name(vim.api.nvim_win_get_buf(w)), ':t') .. (w == vim.api.nvim_get_current_win() and '*' or '') end, vim.api.nvim_tabpage_list_wins(0)), ' '))".into()));
        let want = format!("WINS {want}");
        assert!(
            pump_for(&bridge, ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text() == want)),
            "wanted {want}: {:?}",
            ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>()
        );
    };
    // The window ids: the first is the one `vsplit` left (splitright is off).
    ed.messages.clear();
    bridge.send(UiCommand::Command("lua print('IDS ' .. table.concat(vim.api.nvim_tabpage_list_wins(0), ' '))".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text().starts_with("IDS "))));
    let ids = ed.messages.iter().map(|m| m.text()).find(|t| t.starts_with("IDS ")).unwrap();
    let ids: Vec<i64> = ids[4..].split(' ').map(|i| i.parse().unwrap()).collect();
    windows(&mut ed, "right.txt* left.txt");

    // On the window that doesn't have the focus.
    bridge.send(UiCommand::OpenDropped { paths: vec![dir.join("x.txt")], win: Some(ids[1]) });
    windows(&mut ed, "right.txt x.txt*");
    // Several: all opened, the last one shown.
    bridge.send(UiCommand::OpenDropped { paths: vec![dir.join("y.txt"), dir.join("z.txt")], win: Some(ids[0]) });
    windows(&mut ed, "z.txt* x.txt");
    ed.messages.clear();
    bridge.send(UiCommand::Command("lua print('LISTED ' .. tostring(vim.fn.buflisted('y.txt') == 1))".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| ed.messages.iter().any(|m| m.text() == "LISTED true")));
    // Not over any window: where the focus is.
    bridge.send(UiCommand::OpenDropped { paths: vec![dir.join("left.txt")], win: None });
    windows(&mut ed, "left.txt* x.txt");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Native completion (completion.lua) ----

/// A session with `completion.lua` on. Waits until it has set itself up
/// (on VimEnter).
fn completion_session(extra_args: &[&str]) -> (Bridge, Editor) {
    let mut args = vec!["--clean".to_string(), "-n".to_string()];
    args.extend(extra_args.iter().map(|s| s.to_string()));
    let bridge = Bridge::spawn(
        SpawnOptions { args: plain_shell(args), width: 80, height: 24, native_completion: true, ..Default::default() },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    (bridge, ed)
}

/// Run Lua and wait for what it returns (as a string), through a message.
fn ask(bridge: &Bridge, ed: &mut Editor, expr: &str) -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let tag = format!("ASK{} ", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    bridge.send(UiCommand::Command(format!("lua print('{tag}' .. tostring({expr}))")));
    let found = std::cell::RefCell::new(None);
    pump_for(bridge, ed, Duration::from_secs(5), |ed| {
        *found.borrow_mut() = ed.messages.iter().map(|m| m.text()).find(|t| t.starts_with(&tag));
        found.borrow().is_some()
    });
    let found = found.into_inner();
    found.unwrap_or_else(|| panic!("no answer to {expr}: {:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>()))[tag.len()..]
        .to_string()
}

/// Wait until `expr` is `want`.
fn wait_for(bridge: &Bridge, ed: &mut Editor, expr: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut got = String::new();
    while Instant::now() < deadline {
        got = ask(bridge, ed, expr);
        if got == want {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{expr}: wanted {want:?}, got {got:?}");
}

/// The menu's words, once `pred` holds for them.
fn menu_words(bridge: &Bridge, ed: &mut Editor, what: &str, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
    let words = |ed: &Editor| -> Vec<String> {
        ed.popupmenu.as_ref().map(|pm| pm.items.iter().map(|i| i.word.clone()).collect()).unwrap_or_default()
    };
    assert!(pump(bridge, ed, |ed| pred(&words(ed))), "{what}: menu {:?}", words(ed));
    words(ed)
}

/// Move the selection (`<C-n>`) until `word` is the one selected.
fn select_word(bridge: &Bridge, ed: &mut Editor, word: &str) {
    let selected = |ed: &Editor| {
        ed.popupmenu.as_ref().and_then(|pm| usize::try_from(pm.selected).ok().map(|i| pm.items[i].word.clone()))
    };
    for _ in 0..12 {
        pump_for(bridge, ed, Duration::from_millis(300), |_| false);
        if selected(ed).as_deref() == Some(word) {
            return;
        }
        bridge.send(UiCommand::Input("<C-n>".into()));
    }
    panic!("{word} not selectable: {:?}", ed.popupmenu);
}

/// Start the fake language server in the current buffer, and wait for the
/// completion setup to have attached to it.
fn start_fake_completion_lsp(bridge: &Bridge, ed: &mut Editor) {
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_completion_lsp.lua");
    wait_for(bridge, ed, "vim.go.autocomplete", "true");
    bridge.send(UiCommand::Command(format!("luafile {fake}")));
    wait_for(bridge, ed, "vim.bo.complete:find('o^20', 1, true) ~= nil", "true");
}

#[test]
fn completion_menu_puts_the_language_servers_items_before_buffer_words() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = Bridge::spawn(
        SpawnOptions { args: vec!["--clean".into(), "-n".into()], native_completion: true, ..Default::default() },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    // The GUI is told to show only the documentation asked for.
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PumDoc { selected: -1, .. })).is_some());
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    start_fake_completion_lsp(&bridge, &mut ed);
    // Buffer words that match "for" too, found before the server's answer.
    bridge.send(UiCommand::Input(
        "<Cmd>lua vim.api.nvim_buf_set_lines(0, 0, -1, false, {'formatted_value = 1', '-- forward', ''})<CR>".into(),
    ));
    bridge.send(UiCommand::Input("GAstring.for".into()));
    let words = menu_words(&bridge, &mut ed, "string.for", |w| w.contains(&"formatted_value".to_string()) && w.contains(&"format".to_string()));
    let at = |w: &str| words.iter().position(|x| x == w).unwrap();
    assert!(at("for") < at("format") && at("format") < at("formatted_value"), "server's first, words after: {words:?}");
    assert!(at("format") < at("forward"), "{words:?}");
    assert_eq!(ask(&bridge, &mut ed, "vim.o.completeopt"), "menuone,noinsert,fuzzy,nosort");
    assert_eq!(ask(&bridge, &mut ed, "vim.o.pumheight"), "12");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn enter_accepts_a_selected_item_and_otherwise_does_what_it_did() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    wait_for(&bridge, &mut ed, "vim.go.autocomplete", "true");
    let lines = |bridge: &Bridge, ed: &mut Editor| ask(bridge, ed, "table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '|')");
    let set = |bridge: &Bridge, text: &str| {
        bridge.send(UiCommand::Input(format!("<Cmd>lua vim.api.nvim_buf_set_lines(0, 0, -1, false, {{{text}}})<CR>")));
    };

    // No other mapping: an item selected is accepted, else a new line.
    set(&bridge, "'formatted_value', ''");
    bridge.send(UiCommand::Input("GAformat".into()));
    menu_words(&bridge, &mut ed, "format", |w| w.contains(&"formatted_value".to_string()));
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('$')", "3");
    assert_eq!(lines(&bridge, &mut ed), "formatted_value|format|");
    bridge.send(UiCommand::Input("<Esc>".into()));

    set(&bridge, "'formatted_value', ''");
    bridge.send(UiCommand::Input("GAformat".into()));
    menu_words(&bridge, &mut ed, "format", |w| w.contains(&"formatted_value".to_string()));
    select_word(&bridge, &mut ed, "formatted_value");
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.pumvisible()", "0");
    assert_eq!(lines(&bridge, &mut ed), "formatted_value|formatted_value", "accepted, no new line");
    bridge.send(UiCommand::Input("<Esc>".into()));

    // A mapping of <CR> made later (as lazy.nvim's plugins do), an expression
    // and a function that edits: run when nothing is selected, and with no menu.
    bridge.send(UiCommand::Input(
        "<Cmd>lua vim.keymap.set('i', '<lt>CR>', function() return '<lt>CR>EXPR' end, { expr = true })<CR>".into(),
    ));
    set(&bridge, "'formatted_value', ''");
    bridge.send(UiCommand::Input("GAformat".into()));
    menu_words(&bridge, &mut ed, "format", |w| w.contains(&"formatted_value".to_string()));
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('$')", "3");
    assert_eq!(lines(&bridge, &mut ed), "formatted_value|format|EXPR");
    // (No menu: the line is empty or the word is not completable.)
    bridge.send(UiCommand::Input("<Esc>".into()));
    set(&bridge, "'  '");
    bridge.send(UiCommand::Input("A<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('$')", "2");
    assert!(lines(&bridge, &mut ed).ends_with("EXPR"), "{}", lines(&bridge, &mut ed));
    bridge.send(UiCommand::Input("<Esc>".into()));

    bridge.send(UiCommand::Input(
        "<Cmd>lua vim.keymap.set('i', '<lt>CR>', function() vim.api.nvim_put({ 'FUNC' }, 'c', false, true) end)<CR>".into(),
    ));
    set(&bridge, "'formatted_value', ''");
    bridge.send(UiCommand::Input("GAformat".into()));
    menu_words(&bridge, &mut ed, "format", |w| w.contains(&"formatted_value".to_string()));
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.')", "formatFUNC");
    // With an item selected the mapping stays out of it.
    bridge.send(UiCommand::Input("<Esc>".into()));
    set(&bridge, "'formatted_value', ''");
    bridge.send(UiCommand::Input("GAformat".into()));
    menu_words(&bridge, &mut ed, "format", |w| w.contains(&"formatted_value".to_string()));
    select_word(&bridge, &mut ed, "formatted_value");
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.')", "formatted_value");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn ctrl_space_shows_resolved_docs_that_follow_the_selection() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    start_fake_completion_lsp(&bridge, &mut ed);
    bridge.send(UiCommand::Input("ix.fo".into()));
    menu_words(&bridge, &mut ed, "x.fo", |w| w.iter().any(|x| x == "format"));
    // (The items carry docs of their own: `early docs of format`.)
    select_word(&bridge, &mut ed, "for");
    let current = |ed: &Editor| {
        let pm = ed.popupmenu.as_ref().unwrap();
        (pm.selected, pm.items[pm.selected as usize].word.clone())
    };
    while bridge.try_recv().is_some() {}
    let (first, first_word) = current(&ed);
    bridge.send(UiCommand::Input("<C-Space>".into()));
    let doc = |bridge: &Bridge, ed: &mut Editor, word: Option<&str>| {
        let word = word.map(str::to_owned);
        let ev = wait_gui(bridge, ed, |g| match g {
            GuiEvent::PumDoc { contents, .. } => match &word {
                Some(w) => contents.ends_with(&format!("RESOLVED docs of {w}")),
                None => contents.is_empty(),
            },
            _ => false,
        });
        ev.unwrap_or_else(|| panic!("no doc event for {word:?}"))
    };
    let GuiEvent::PumDoc { selected, .. } = doc(&bridge, &mut ed, Some(&first_word)) else { unreachable!() };
    assert_eq!(selected, first);
    // Moving on: the docs follow.
    bridge.send(UiCommand::Input("<C-n>".into()));
    let (second, second_word) = (1_i64, "format".to_string());
    assert_ne!(first, second);
    let GuiEvent::PumDoc { selected, .. } = doc(&bridge, &mut ed, Some(&second_word)) else { unreachable!() };
    assert_eq!(selected, second);
    // Again: off. Then on, and leaving insert mode hides them.
    bridge.send(UiCommand::Input("<C-Space>".into()));
    doc(&bridge, &mut ed, None);
    bridge.send(UiCommand::Input("<C-Space>".into()));
    doc(&bridge, &mut ed, Some(&second_word));
    bridge.send(UiCommand::Input("<Esc>".into()));
    doc(&bridge, &mut ed, None);
    // Docs are not left on for the next menu.
    bridge.send(UiCommand::Input("Ax.fo<C-n>".into()));
    let mut shown = false;
    pump_for(&bridge, &mut ed, Duration::from_millis(1), |_| false);
    let deadline = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < deadline {
        while let Some(ev) = bridge.try_recv() {
            match ev {
                BridgeEvent::Redraw(batch) => batch.into_iter().for_each(|e| ed.apply(e)),
                BridgeEvent::Gui(GuiEvent::PumDoc { contents, .. }) => shown |= !contents.is_empty(),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ed.popupmenu.is_some(), "a menu to have no docs");
    assert!(!shown, "docs only when asked for");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn enter_only_accepts_the_first_item_when_it_was_typed_for() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    start_fake_completion_lsp(&bridge, &mut ed);
    let lines = |bridge: &Bridge, ed: &mut Editor| ask(bridge, ed, "table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '|')");
    let clear = |bridge: &Bridge| {
        bridge.send(UiCommand::Input("<Esc><Cmd>lua vim.api.nvim_buf_set_lines(0, 0, -1, false, {})<CR>".into()));
    };

    // Nothing typed for it: a new line, not the item that came up by itself.
    bridge.send(UiCommand::Input("ix.".into()));
    menu_words(&bridge, &mut ed, "x.", |w| w.contains(&"byte".to_string()));
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('$')", "2");
    assert_eq!(lines(&bridge, &mut ed), "x.|");
    clear(&bridge);

    // The word typed in full: the line ends too.
    bridge.send(UiCommand::Input("ix.rep".into()));
    menu_words(&bridge, &mut ed, "x.rep", |w| w.first().is_some_and(|f| f == "rep"));
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('$')", "2");
    assert_eq!(lines(&bridge, &mut ed), "x.rep|");
    clear(&bridge);

    // Typed part of it: accepted, no new line.
    bridge.send(UiCommand::Input("ix.re".into()));
    menu_words(&bridge, &mut ed, "x.re", |w| w.first().is_some_and(|f| f == "rep"));
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.pumvisible()", "0");
    assert_eq!(lines(&bridge, &mut ed), "x.rep");
    clear(&bridge);

    // Moved to with nothing typed: accepted, no new line.
    bridge.send(UiCommand::Input("ix.".into()));
    menu_words(&bridge, &mut ed, "x.", |w| w.contains(&"format".to_string()));
    select_word(&bridge, &mut ed, "format");
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.pumvisible()", "0");
    assert_eq!(lines(&bridge, &mut ed), "x.format");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn a_plugin_mapping_enter_only_if_nothing_is_mapped_still_gets_to() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    wait_for(&bridge, &mut ed, "vim.go.autocomplete", "true");
    // As mini.pairs does, loaded by lazy.nvim on the first InsertEnter.
    let lazy_plugin = concat!(
        "vim.api.nvim_create_autocmd('InsertEnter', { once = true, callback = function() ",
        "if vim.fn.maparg('<lt>CR>', 'i') == '' then ",
        "vim.keymap.set('i', '<lt>CR>', function() return '<lt>CR>PLUGIN' end, { expr = true }) ",
        "end end })",
    );
    bridge.send(UiCommand::Input(format!("<Cmd>lua {lazy_plugin}<CR>")));
    wait_for(&bridge, &mut ed, "#vim.api.nvim_get_autocmds({ event = 'InsertEnter' }) > 0", "true");
    bridge.send(UiCommand::Input("i".into()));
    wait_for(&bridge, &mut ed, "vim.fn.maparg('<CR>', 'i') ~= ''", "true");
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.')", "PLUGIN");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn tab_and_the_arrows_move_through_the_menu() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    start_fake_completion_lsp(&bridge, &mut ed);
    bridge.send(UiCommand::Input("ix.".into()));
    menu_words(&bridge, &mut ed, "x.", |w| w.contains(&"format".to_string()));
    bridge.send(UiCommand::Input("<Tab>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.')", "x.for");
    bridge.send(UiCommand::Input("<Tab>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.')", "x.format");
    bridge.send(UiCommand::Input("<S-Tab>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.')", "x.for");
    // The arrows move without inserting; Enter takes what they're on.
    bridge.send(UiCommand::Input("<Down>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.complete_info({ 'selected' }).selected", "2");
    bridge.send(UiCommand::Input("<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.')", "x.format");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.line('$')"), "1", "accepted, no new line");
    // No menu: a plain Tab.
    bridge.send(UiCommand::Input("<Esc>o<Tab>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.') == '\\t'", "true");
    bridge.send(UiCommand::Input("<Esc><Cmd>lua vim.api.nvim_buf_set_lines(0, 0, -1, false, {})<CR>".into()));

    // In a snippet Tab jumps, even with the menu up.
    bridge.send(UiCommand::Input("ifunction".into()));
    let words = menu_words(&bridge, &mut ed, "function", |w| w.iter().any(|x| x.starts_with("function")));
    assert!(!words.is_empty());
    select_word(&bridge, &mut ed, &words[0]);
    bridge.send(UiCommand::Input("<C-y>".into()));
    wait_for(&bridge, &mut ed, "vim.snippet.active()", "true");
    wait_for(&bridge, &mut ed, "vim.fn.line('$')", "3");
    bridge.send(UiCommand::Input("re".into()));
    menu_words(&bridge, &mut ed, "re", |w| w.contains(&"rep".to_string()));
    bridge.send(UiCommand::Input("<Tab>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "2");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn the_command_lines_arrows_move_the_menu_and_keep_history() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    wait_for(&bridge, &mut ed, "vim.go.autocomplete", "true");
    let selected = |ed: &Editor| ed.popupmenu.as_ref().map(|pm| pm.selected);
    let line = |ed: &Editor| ed.cmdline.last().map(|c| c.text()).unwrap_or_default();

    bridge.send(UiCommand::Input(":colorscheme ".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| selected(ed) == Some(-1)), "the menu, nothing selected");
    bridge.send(UiCommand::Input("<Down>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| selected(ed) == Some(0)), "Down enters it");
    bridge.send(UiCommand::Input("<Down>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| selected(ed) == Some(1)));
    bridge.send(UiCommand::Input("<Up>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| selected(ed) == Some(0)), "Up moves back");
    bridge.send(UiCommand::Input("<Esc>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.mode()", "n");

    // Nothing selected: Up is history, and Down steps back through it.
    // (Commands that have completions themselves: the menu opens on each.)
    bridge.send(UiCommand::Input(":colorscheme blue<CR>:colorscheme default<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "default");
    bridge.send(UiCommand::Input(":colorscheme ".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| selected(ed) == Some(-1)));
    bridge.send(UiCommand::Input("<Up>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| line(ed) == "colorscheme default"), "{}", line(&ed));
    bridge.send(UiCommand::Input("<Up>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| line(ed) == "colorscheme blue"), "{}", line(&ed));
    bridge.send(UiCommand::Input("<Down>".into()));
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| line(ed) == "colorscheme default"), "{}", line(&ed));
    bridge.send(UiCommand::Input("<Esc>".into()));
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn ctrl_space_without_a_menu_asks_the_language_server() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    start_fake_completion_lsp(&bridge, &mut ed);
    // Autocompletion off for the moment: a menu only on request.
    bridge.send(UiCommand::Input("<Cmd>lua vim.bo.autocomplete = false<CR>ix.<C-Space>".into()));
    menu_words(&bridge, &mut ed, "after Ctrl-Space", |w| w.iter().any(|x| x == "format"));
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn a_snippet_item_expands_when_accepted() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    start_fake_completion_lsp(&bridge, &mut ed);
    bridge.send(UiCommand::Input("ix.fun".into()));
    menu_words(&bridge, &mut ed, "x.fun", |w| w.iter().any(|x| x == "function"));
    select_word(&bridge, &mut ed, "function");
    bridge.send(UiCommand::Input("<C-y>".into()));
    wait_for(&bridge, &mut ed, "vim.snippet.active()", "true");
    assert_eq!(
        ask(&bridge, &mut ed, "table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '|')"),
        "x.function ()|\t|end",
        "multi-line snippet"
    );
    // Tab goes on to the next stop, inside the body.
    bridge.send(UiCommand::Input("<Tab>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "2");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn the_config_can_opt_out_and_prompts_get_no_autocompletion() {
    if !nvim_available() {
        return;
    }
    // Opting out: Neovim's own settings stay, and no <C-Space> mapping.
    let (bridge, mut ed) = completion_session(&["--cmd", "let g:eshvi_native_completion = v:false"]);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(ask(&bridge, &mut ed, "vim.go.autocomplete"), "false");
    assert_eq!(ask(&bridge, &mut ed, "vim.o.completeopt:find('fuzzy') == nil and vim.o.pumheight == 0"), "true");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.maparg('<C-Space>', 'i')"), "");
    bridge.send(UiCommand::Command("qa!".into()));

    let (bridge, mut ed) = completion_session(&[]);
    wait_for(&bridge, &mut ed, "vim.go.autocomplete", "true");
    wait_for(&bridge, &mut ed, "vim.bo.autocomplete", "true");
    // A scratch buffer (pickers' prompts, panels), in its window.
    bridge.send(UiCommand::Command("enew | setlocal buftype=nofile".into()));
    wait_for(&bridge, &mut ed, "vim.bo.autocomplete", "false");
    bridge.send(UiCommand::Command("new | setlocal buftype=prompt".into()));
    wait_for(&bridge, &mut ed, "vim.bo.autocomplete", "false");
    // And no menu while typing there.
    bridge.send(UiCommand::Input("ifoobar foob".into()));
    pump_for(&bridge, &mut ed, Duration::from_millis(700), |_| false);
    assert!(ed.popupmenu.is_none(), "no menu in a prompt: {:?}", ed.popupmenu);
    assert!(ask(&bridge, &mut ed, "vim.fn.getline('.')").ends_with("foobar foob"), "the typing arrived");
    bridge.send(UiCommand::Input("<Esc>".into()));
    // A normal buffer again: on.
    bridge.send(UiCommand::Command("only | enew".into()));
    wait_for(&bridge, &mut ed, "vim.bo.autocomplete", "true");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn paths_complete_while_typing_from_the_buffers_folder() {
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-paths-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("alpha_dir")).unwrap();
    std::fs::write(dir.join("alpha.txt"), "").unwrap();
    std::fs::write(dir.join("alpha_dir/inner.txt"), "").unwrap();
    std::fs::write(dir.join("notes.txt"), "").unwrap();
    std::fs::write(dir.join("format.txt"), "").unwrap();
    let (bridge, mut ed) = completion_session(&[]);
    wait_for(&bridge, &mut ed, "vim.go.autocomplete", "true");
    bridge.send(UiCommand::Input(format!("<Cmd>edit {}<CR>", dir.join("notes.txt").display())));
    // (The working directory is elsewhere: relative to the file's folder.)
    bridge.send(UiCommand::Input("i./al".into()));
    let words = menu_words(&bridge, &mut ed, "./al", |w| w.iter().any(|x| x == "alpha.txt"));
    assert_eq!(&words[..2], ["alpha_dir/", "alpha.txt"], "folders first: {words:?}");
    let kinds: Vec<_> = ed.popupmenu.as_ref().unwrap().items.iter().take(2).map(|i| i.kind.clone()).collect();
    assert_eq!(kinds, ["Folder", "File"]);
    // After a "/" typed, the folder's content.
    bridge.send(UiCommand::Input("<Esc>cc./alpha_dir/".into()));
    menu_words(&bridge, &mut ed, "./alpha_dir/", |w| w == ["inner.txt"]);
    // Accepted, only the last part is replaced.
    bridge.send(UiCommand::Input("<C-n><C-y>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline('.')", "./alpha_dir/inner.txt");
    // With a language server (its items match "for" too): the paths first.
    start_fake_completion_lsp(&bridge, &mut ed);
    bridge.send(UiCommand::Input("<Esc>cc./for".into()));
    let words = menu_words(&bridge, &mut ed, "./for", |w| w.iter().any(|x| x == "format") && w.iter().any(|x| x == "format.txt"));
    assert_eq!(words[0], "format.txt", "paths before the server's items: {words:?}");
    // Not a path: words only. A comment's slashes aren't one.
    bridge.send(UiCommand::Input("<Esc>cc// alp".into()));
    pump_for(&bridge, &mut ed, Duration::from_millis(700), |_| false);
    assert!(ed.popupmenu.as_ref().is_none_or(|pm| pm.items.iter().all(|i| i.kind != "File" && i.kind != "Folder")));
    // (The menu of "./for" is gone, or only has words.)
    assert!(ed.popupmenu.as_ref().is_none_or(|pm| pm.items.iter().all(|i| i.word != "format.txt")), "{:?}", ed.popupmenu);
    assert!(ed.popupmenu.as_ref().is_none_or(|pm| pm.items.iter().all(|i| i.kind != "File" && i.kind != "Folder")));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_command_line_shows_a_menu_and_up_down_stay_history() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = completion_session(&[]);
    wait_for(&bridge, &mut ed, "vim.o.wildoptions", "pum");
    bridge.send(UiCommand::Input(":set wildm".into()));
    let words = menu_words(&bridge, &mut ed, ":set wildm", |w| w.iter().any(|x| x == "wildmode"));
    assert!(words.len() >= 2, "{words:?}");
    assert!(ed.popupmenu.as_ref().is_some_and(|pm| pm.grid < 0), "the command line's menu");
    // Up with the menu showing: the history, not the menu.
    bridge.send(UiCommand::Input("<Esc>:echo 'one'<CR>:echo 'two'<CR>:ec".into()));
    menu_words(&bridge, &mut ed, ":ec", |w| !w.is_empty());
    bridge.send(UiCommand::Input("<Up>".into()));
    assert!(pump(&bridge, &mut ed, |ed| ed.cmdline.last().is_some_and(|c| c.text() == "echo 'two'")), "{:?}", ed.cmdline.last().map(|c| c.text()));
    // A search offers a menu too (words of the buffer).
    bridge.send(UiCommand::Input("<Esc>".into()));
    bridge.send(UiCommand::Input("<Cmd>lua vim.api.nvim_buf_set_lines(0, 0, -1, false, {'searchable', 'x'})<CR>".into()));
    bridge.send(UiCommand::Input("/sea".into()));
    menu_words(&bridge, &mut ed, "/sea", |w| w.iter().any(|x| x == "searchable"));
    bridge.send(UiCommand::Input("<Esc>".into()));
    bridge.send(UiCommand::Command("qa!".into()));
}

// ---- The native status bar's data (status.lua: gui_status) ----

fn status_session(native: bool, args: &[&str], width: usize, height: usize) -> (Bridge, Editor) {
    let mut all = vec!["--clean".to_string(), "-n".to_string()];
    all.extend(args.iter().map(|s| s.to_string()));
    let bridge = Bridge::spawn(
        SpawnOptions { args: plain_shell(all), width, height, native_status: native, ..Default::default() },
        Arc::new(|| {}),
    )
    .unwrap();
    // (No waiting for a window here: `pump` would drop the first reports.)
    (bridge, Editor::new())
}

/// The next `gui_status` for which `pred` holds.
fn status_where(bridge: &Bridge, ed: &mut Editor, pred: impl Fn(&nvim_bridge::gui::Status) -> bool) -> nvim_bridge::gui::Status {
    use nvim_bridge::gui::GuiEvent;
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::Status(s) if pred(s))) {
        Some(GuiEvent::Status(s)) => s,
        _ => panic!("no matching gui_status"),
    }
}

fn status_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("eshvi-status-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_status_reports_the_mode_and_the_file() {
    if !nvim_available() {
        return;
    }
    let dir = status_dir("modes");
    let file = dir.join("hello.lua");
    std::fs::write(&file, "local a = 1\nlocal b = 2\nlocal c = 3\n").unwrap();
    let path = std::fs::canonicalize(&file).unwrap().to_string_lossy().into_owned();
    let (bridge, mut ed) = status_session(true, &["--cmd", "filetype on", file.to_str().unwrap()], 80, 24);

    let s = status_where(&bridge, &mut ed, |s| s.mode == "n" && s.filetype == "lua");
    assert_eq!((s.path.as_str(), s.name.as_str()), (path.as_str(), "hello.lua"));
    assert!(!s.modified && !s.readonly && s.buftype.is_empty());

    for (keys, mode) in [("V", "V"), ("<Esc><C-v>", "\x16"), ("<Esc>v", "v"), ("<Esc>i", "i"), ("<Esc>R", "R"), ("<Esc>gR", "Rv"), ("<Esc>d", "no"), ("<Esc>:", "c")] {
        bridge.send(UiCommand::Input(keys.into()));
        let s = status_where(&bridge, &mut ed, |s| s.mode == mode);
        assert_eq!(s.name, "hello.lua", "in mode {mode:?}");
    }
    // Back to normal; select mode (<C-g> in visual).
    bridge.send(UiCommand::Input("<Esc><Esc>v<C-g>".into()));
    status_where(&bridge, &mut ed, |s| s.mode == "s");
    bridge.send(UiCommand::Input("<Esc>".into()));
    status_where(&bridge, &mut ed, |s| s.mode == "n");

    // A change marks the buffer; writing clears it.
    bridge.send(UiCommand::Input("ix<Esc>".into()));
    let s = status_where(&bridge, &mut ed, |s| s.modified);
    assert_eq!(s.path, path);
    bridge.send(UiCommand::Command("write".into()));
    status_where(&bridge, &mut ed, |s| !s.modified);
    // 'readonly' and not modifiable both count.
    bridge.send(UiCommand::Command("set readonly".into()));
    status_where(&bridge, &mut ed, |s| s.readonly);
    bridge.send(UiCommand::Command("set noreadonly".into()));
    status_where(&bridge, &mut ed, |s| !s.readonly);
    bridge.send(UiCommand::Command("set nomodifiable".into()));
    status_where(&bridge, &mut ed, |s| s.readonly);
    bridge.send(UiCommand::Command("set modifiable".into()));
    status_where(&bridge, &mut ed, |s| !s.readonly);
    // A new file type is reported.
    bridge.send(UiCommand::Command("set filetype=text".into()));
    status_where(&bridge, &mut ed, |s| s.filetype == "text");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_help_file_is_readonly_and_a_new_buffer_has_no_name() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = status_session(true, &[], 80, 24);
    bridge.send(UiCommand::Command("help".into()));
    let s = status_where(&bridge, &mut ed, |s| s.name == "help.txt" && s.filetype == "help");
    assert_eq!((s.name.as_str(), s.filetype.as_str(), s.readonly, s.modified), ("help.txt", "help", true, false));
    bridge.send(UiCommand::Command("enew".into()));
    let s = status_where(&bridge, &mut ed, |s| s.buftype.is_empty());
    assert_eq!((s.name.as_str(), s.path.as_str(), s.readonly), ("", "", false));
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn a_float_or_a_prompt_leaves_the_file_shown() {
    if !nvim_available() {
        return;
    }
    let dir = status_dir("float");
    let file = dir.join("keep.txt");
    std::fs::write(&file, "text\n").unwrap();
    let (bridge, mut ed) = status_session(true, &[file.to_str().unwrap()], 80, 24);
    status_where(&bridge, &mut ed, |s| s.name == "keep.txt");
    // A picker-like float: a `nofile` buffer with a name of its own, entered.
    bridge.send(UiCommand::Command(
        "lua local b = vim.api.nvim_create_buf(false, true); vim.api.nvim_buf_set_name(b, 'picker-prompt'); \
         vim.api.nvim_open_win(b, true, { relative = 'editor', row = 2, col = 2, width = 20, height = 3 })"
            .into(),
    ));
    bridge.send(UiCommand::Input("i".into()));
    // The mode follows; the file doesn't.
    let s = status_where(&bridge, &mut ed, |s| s.mode == "i");
    assert_eq!((s.name.as_str(), s.buftype.as_str()), ("keep.txt", ""));
    // The same for a prompt buffer in the float.
    bridge.send(UiCommand::Input("<Esc>".into()));
    bridge.send(UiCommand::Command(
        "lua local b = vim.api.nvim_create_buf(false, true); vim.bo[b].buftype = 'prompt'; vim.api.nvim_buf_set_name(b, 'prompt-two'); \
         vim.api.nvim_open_win(b, true, { relative = 'editor', row = 4, col = 4, width = 20, height = 3 })"
            .into(),
    ));
    bridge.send(UiCommand::Input("<Esc>v".into()));
    let s = status_where(&bridge, &mut ed, |s| s.mode == "v");
    assert_eq!(s.name, "keep.txt");
    // Back in the file's window it is reported again, after the float closes.
    bridge.send(UiCommand::Input("<Esc>".into()));
    bridge.send(UiCommand::Command("only".into()));
    let s = status_where(&bridge, &mut ed, |s| s.mode == "n" && s.name == "keep.txt");
    assert_eq!(s.path, std::fs::canonicalize(&file).unwrap().to_string_lossy());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_window_that_is_not_a_float_changes_the_file_shown() {
    if !nvim_available() {
        return;
    }
    let dir = status_dir("split");
    let (a, b) = (dir.join("a.txt"), dir.join("b.md"));
    std::fs::write(&a, "a\n").unwrap();
    std::fs::write(&b, "b\n").unwrap();
    let (bridge, mut ed) = status_session(true, &["--cmd", "filetype on", a.to_str().unwrap()], 80, 24);
    status_where(&bridge, &mut ed, |s| s.name == "a.txt");
    bridge.send(UiCommand::Command(format!("split {}", b.display())));
    let s = status_where(&bridge, &mut ed, |s| s.name == "b.md");
    assert_eq!(s.filetype, "markdown");
    bridge.send(UiCommand::Input("<C-w>j".into()));
    status_where(&bridge, &mut ed, |s| s.name == "a.txt");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_native_bar_forces_one_global_statusline() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = status_session(true, &["--cmd", "set laststatus=2"], 80, 24);
    wait_for(&bridge, &mut ed, "vim.o.laststatus", "3");
    bridge.send(UiCommand::Command("set laststatus=2".into()));
    wait_for(&bridge, &mut ed, "vim.o.laststatus", "3");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn nothing_is_sent_without_the_own_config() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = status_session(false, &[], 80, 24);
    for keys in ["V<Esc>", "ix<Esc>", ":<Esc>"] {
        bridge.send(UiCommand::Input(keys.into()));
        std::thread::sleep(Duration::from_millis(150));
    }
    bridge.send(UiCommand::Command("split".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Status(_))).is_none(), "gui_status without native_status");
    assert_eq!(ask(&bridge, &mut ed, "vim.o.laststatus"), "2");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn the_status_column_counts_characters() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = status_session(true, &[], 80, 24);
    bridge.send(UiCommand::Input("<Cmd>lua vim.api.nvim_buf_set_lines(0, 0, -1, false, {'ééé x שלום y'})<CR>".into()));
    for (keys, col) in [("gg0", 1), ("gg0lll", 4), ("ggfx", 5), ("ggfy", 12)] {
        bridge.send(UiCommand::Input(format!("<Esc>{keys}")));
        let got = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Status(s) if s.col == col));
        assert!(got.is_some(), "after {keys:?}: no gui_status with col {col}");
    }
    // And while typing.
    bridge.send(UiCommand::Input("<Esc>ggA é".into()));
    let got = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Status(s) if s.col == 15));
    assert!(got.is_some(), "no gui_status with col 15");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn the_cursor_column_is_the_bytes_before_it_plus_one() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = status_session(true, &[], 80, 24);
    // "ééé" is three characters and six bytes; a tab is one byte too.
    bridge.send(UiCommand::Input("<Cmd>lua vim.api.nvim_buf_set_lines(0, 0, -1, false, {'ééé x', '\tab'})<CR>".into()));
    let viewport = |ed: &Editor| ed.windows.values().find(|w| w.kind == grid::WindowKind::Normal).and_then(|w| w.viewport);
    for (keys, line, col_bytes) in [("gg0", 0, 0), ("gg0lll", 0, 6), ("gg$", 0, 7), ("ggfx", 0, 7), ("ggj0l", 1, 1), ("ggj$", 1, 2)] {
        bridge.send(UiCommand::Input(format!("<Esc>{keys}")));
        let ok = pump(&bridge, &mut ed, |ed| viewport(ed).is_some_and(|v| (v.curline, v.curcol) == (line, col_bytes)));
        let v = viewport(&ed);
        assert!(ok, "after {keys:?}: {v:?}");
        // The same number `col('.')` gives, less one.
        assert_eq!(ask(&bridge, &mut ed, "vim.fn.col('.')"), (col_bytes + 1).to_string(), "{keys}");
    }
    bridge.send(UiCommand::Command("qa!".into()));
}

// ---- The native picker (picker.lua: gui_picker_open, Eshvi.picker.accept) ----

use nvim_bridge::gui::{PickerChoice, PickerOpen};

fn picker_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("eshvi-picker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // (As Neovim says its cwd: `/var` is `/private/var`.)
    std::fs::canonicalize(&dir).unwrap()
}

fn picker_session(native: bool, args: &[&str]) -> (Bridge, Editor) {
    picker_session_with(native, false, args)
}

/// `picker_session`, with the app's slang support (`SpawnOptions::slang`) if `slang`.
fn picker_session_with(native: bool, slang: bool, args: &[&str]) -> (Bridge, Editor) {
    let mut all = Vec::new();
    // (`--clean` is no `-u`, for a test that brings its own.)
    if !args.contains(&"-u") {
        all.push("--clean".to_string());
    }
    all.extend(["-n".to_string(), "-i".to_string(), "NONE".to_string()]);
    all.extend(args.iter().map(|s| s.to_string()));
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(all),
            width: 100,
            height: 30,
            native_picker: native,
            terminal_enabled: true,
            slang,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    (bridge, Editor::new())
}

/// The next picker Neovim opens.
fn next_picker(bridge: &Bridge, ed: &mut Editor) -> PickerOpen {
    use nvim_bridge::gui::GuiEvent;
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::PickerOpen(_))) {
        Some(GuiEvent::PickerOpen(open)) => {
            // (The app says the box is up; Neovim otherwise waits a moment for it.)
            bridge.send(UiCommand::PickerOpened(open.id));
            open
        }
        _ => panic!("no gui_picker_open"),
    }
}

fn accept(bridge: &Bridge, id: i64, chosen: Vec<PickerChoice>, how: &str) {
    bridge.send(UiCommand::PickerAccept { id, chosen, how: how.into(), state: Default::default() });
}

fn by_idx(idx: usize) -> PickerChoice {
    PickerChoice { idx: Some(idx), ..Default::default() }
}

fn by_path(path: &std::path::Path) -> PickerChoice {
    PickerChoice { path: Some(path.to_string_lossy().into_owned()), ..Default::default() }
}

fn texts(open: &PickerOpen) -> Vec<&str> {
    open.items.iter().map(|i| i.text.as_str()).collect()
}

#[test]
fn every_source_opens_the_picker_with_what_neovim_knows() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("sources");
    let outside = picker_dir("sources-outside");
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(name), "x\n").unwrap();
    }
    std::fs::write(outside.join("o.txt"), "x\n").unwrap();
    let oldfiles = format!("let v:oldfiles = ['{}', '{}', '{}']", dir.join("c.txt").display(), outside.join("o.txt").display(), dir.join("gone.txt").display());
    let (bridge, mut ed) = picker_session(
        true,
        &["--cmd", &format!("cd {}", dir.display()), "--cmd", &oldfiles, dir.join("a.txt").to_str().unwrap(), dir.join("b.txt").to_str().unwrap()],
    );
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    let cwd = dir.to_string_lossy().into_owned();

    bridge.send(UiCommand::Command("lua Eshvi.picker.files()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.source.as_str(), open.title.as_str(), open.cwd.as_str()), ("files", "Find files", cwd.as_str()));
    assert!(open.files && open.multi && open.items.is_empty());

    bridge.send(UiCommand::Command(format!("lua Eshvi.picker.files({{ cwd = '{}', title = 'Config' }})", outside.display())));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.source.as_str(), open.title.as_str(), open.cwd.as_str()), ("files", "Config", outside.to_str().unwrap()));

    bridge.send(UiCommand::Command("lua Eshvi.picker.git_files()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.source.as_str(), open.title.as_str()), ("git_files", "Git files"));
    assert!(open.items.is_empty());

    // With the explorer's root known (`:cd`, not 'autochdir' following the file) it is the folder.
    // (Each pair in one command: two commands in a row run on tasks of their own and
    // can swap, which made this test fail about once in ten runs.)
    bridge.send(UiCommand::Command(format!("lua vim.g.eshvi_root = '{}'; Eshvi.picker.files()", outside.display())));
    assert_eq!(next_picker(&bridge, &mut ed).cwd, outside.to_str().unwrap());

    // Buffers: the listed ones, by path, relative to the folder.
    bridge.send(UiCommand::Command("lua vim.g.eshvi_root = nil; Eshvi.picker.buffers()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.source.as_str(), open.title.as_str(), open.files), ("items", "Buffers", true));
    let mut names = texts(&open);
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt"]);
    assert!(open.items.iter().all(|i| i.group.as_deref() == Some("buffer")));
    assert_eq!(open.items[0].path.as_deref(), Some(dir.join(&open.items[0].text).to_str().unwrap()));

    // Recent: every readable old file, with the one outside the folder shown by its path.
    bridge.send(UiCommand::Command("lua Eshvi.picker.recent()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Recent files");
    assert_eq!(open.items.len(), 2, "{:?}", texts(&open));
    assert_eq!(open.items[0].text, "c.txt");
    assert!(open.items[1].text.ends_with("o.txt") && open.items[1].text.starts_with('~') || open.items[1].text.starts_with('/'), "{:?}", texts(&open));
    assert!(open.items.iter().all(|i| i.group.as_deref() == Some("recent")));

    // Smart: the buffers but the current file, then the recent ones in the folder (not
    // the buffers again, nor the one outside it).
    bridge.send(UiCommand::Command("lua Eshvi.picker.smart()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.source.as_str(), open.title.as_str()), ("smart", "Find files"));
    assert_eq!(texts(&open), ["b.txt", "c.txt"]);
    // (And the walk is told to leave the current one out.)
    assert_eq!(open.exclude.as_deref(), Some(dir.join("a.txt").to_str().unwrap()));
    assert_eq!(open.items[0].group.as_deref(), Some("buffer"));
    assert_eq!(open.items[1].group.as_deref(), Some("recent"));

    // The command does the same.
    bridge.send(UiCommand::Command("EshviPick buffers".into()));
    assert_eq!(next_picker(&bridge, &mut ed).title, "Buffers");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn a_live_pick_asks_for_each_query_and_answers_with_the_item_given() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = picker_session(true, &[]);
    bridge.send(UiCommand::Command(
        "lua Eshvi.picker.pick({ title = 'Search hierarchy', hint = 'Type', \
         ask = function(q, reply) reply({ { text = 'top.' .. q, desc = 'module', node = q .. '-1' }, { text = 'top.' .. q .. '2', node = q .. '-2' } }) end, \
         on_choice = function(item, idx) _G.chose = item and (item.node .. ':' .. idx) or 'nothing' end })"
            .into(),
    ));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.title.as_str(), open.live, open.multi, open.hint.as_str()), ("Search hierarchy", true, false, "Type"));
    assert!(open.items.is_empty());
    // The rows of the latest answer are what is chosen from.
    bridge.send(UiCommand::PickerQuery { id: open.id, gen: 1, text: "cpu".into() });
    let rows = picker_items_for(&bridge, &mut ed, 1);
    assert_eq!(rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["top.cpu", "top.cpu2"]);
    assert_eq!(rows[0].desc, "module");
    bridge.send(UiCommand::PickerQuery { id: open.id, gen: 2, text: "mem".into() });
    picker_items_for(&bridge, &mut ed, 2);
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "_G.chose", "mem-2:2");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn a_select_is_a_picker_and_answers_once() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = picker_session(true, &[]);
    let record = "function(item, idx) table.insert(_G.got, TAG .. tostring(item) .. ':' .. tostring(idx) .. ':' .. tostring(vim.in_fast_event())) end";
    let select = |tag: &str| {
        format!(
            "lua vim.ui.select({{ 'alpha', 'beta', 'gamma' }}, {{ prompt = 'Pick one:', format_item = function(x) return x:upper() end }}, {})",
            record.replace("TAG", &format!("'{tag}='"))
        )
    };
    wait_for(&bridge, &mut ed, "vim.ui.select == Eshvi.picker.select", "true");
    bridge.send(UiCommand::Command("lua _G.got = {}".into()));

    // Accepted: the item and its place, once, on the main loop.
    bridge.send(UiCommand::Command(select("A")));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.title.as_str(), open.source.as_str(), open.files, open.multi), ("Pick one", "items", false, false));
    assert_eq!(texts(&open), ["ALPHA", "BETA", "GAMMA"]);
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "table.concat(_G.got, ' ')", "A=beta:2:false");
    // Answering again changes nothing: not by a late cancel, accept or a second one.
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });
    accept(&bridge, open.id, vec![by_idx(1)], "edit");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ask(&bridge, &mut ed, "table.concat(_G.got, ' ')"), "A=beta:2:false");

    // Cancelled: nothing, once.
    bridge.send(UiCommand::Command(select("B")));
    let open = next_picker(&bridge, &mut ed);
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });
    wait_for(&bridge, &mut ed, "table.concat(_G.got, ' ')", "A=beta:2:false B=nil:nil:false");

    // Replaced by another picker: the first hears nothing, once; the second is still open.
    bridge.send(UiCommand::Command(select("C")));
    let first = next_picker(&bridge, &mut ed);
    bridge.send(UiCommand::Command(select("D")));
    let second = next_picker(&bridge, &mut ed);
    assert!(second.id > first.id);
    wait_for(&bridge, &mut ed, "table.concat(_G.got, ' ')", "A=beta:2:false B=nil:nil:false C=nil:nil:false");
    // (A late answer for the first one is ignored.)
    accept(&bridge, first.id, vec![by_idx(3)], "edit");
    bridge.send(UiCommand::PickerCancel { id: first.id, state: Default::default() });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ask(&bridge, &mut ed, "#_G.got"), "3");
    accept(&bridge, second.id, vec![by_idx(3)], "edit");
    wait_for(&bridge, &mut ed, "_G.got[4]", "D=gamma:3:false");
    // A file picker replaced by a select leaves nothing behind either.
    bridge.send(UiCommand::Command("lua Eshvi.picker.files()".into()));
    let files = next_picker(&bridge, &mut ed);
    bridge.send(UiCommand::Command(select("E")));
    let select_open = next_picker(&bridge, &mut ed);
    assert!(select_open.id > files.id);
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn quitting_cancels_an_open_select_once_and_tells_the_gui() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("quit");
    let marker = dir.join("answered");
    let (bridge, mut ed) = picker_session(true, &[]);
    wait_for(&bridge, &mut ed, "vim.ui.select == Eshvi.picker.select", "true");
    bridge.send(UiCommand::Command(format!(
        "lua vim.ui.select({{ 'a' }}, nil, function(i, x) vim.fn.writefile({{ tostring(i) .. tostring(x) }}, '{}', 'a') end)",
        marker.display()
    )));
    let open = next_picker(&bridge, &mut ed);
    bridge.send(UiCommand::Command("qa!".into()));
    let closed = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PickerClose(id) if *id == open.id));
    assert!(closed.is_some(), "gui_picker_close");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "nilnil\n");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_plugins_select_loses_and_the_opt_out_and_the_shared_config_leave_neovims() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("config");
    // A config (and so any plugin it loads) setting its own `vim.ui.select`: now, when Neovim
    // has started, and when lazy.nvim says VeryLazy.
    let plugin = dir.join("plugin.lua");
    std::fs::write(
        &plugin,
        "local mine = function() end\n\
         vim.g.mine = mine\n\
         vim.ui.select = mine\n\
         vim.api.nvim_create_autocmd('VimEnter', { callback = function() vim.ui.select = mine end })\n\
         vim.api.nvim_create_autocmd('User', { pattern = 'VeryLazy', callback = function() vim.ui.select = mine end })\n",
    )
    .unwrap();
    let (bridge, mut ed) = picker_session(true, &["-u", plugin.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    assert_eq!(ask(&bridge, &mut ed, "vim.ui.select == Eshvi.picker.select"), "true", "after VimEnter");
    assert_eq!(ask(&bridge, &mut ed, "vim.ui.select ~= vim.g.mine"), "true");
    bridge.send(UiCommand::Command("doautocmd User VeryLazy".into()));
    assert_eq!(ask(&bridge, &mut ed, "vim.ui.select == Eshvi.picker.select"), "true", "after VeryLazy");
    bridge.send(UiCommand::Command("qa!".into()));

    // Opted out: Neovim's own stays, and the picker says so instead of opening.
    let optout = dir.join("optout.lua");
    std::fs::write(&optout, "vim.g.eshvi_native_picker = false\nvim.g.theirs = vim.ui.select\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["-u", optout.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    assert_eq!(ask(&bridge, &mut ed, "vim.ui.select == vim.g.theirs"), "true");
    bridge.send(UiCommand::Command("lua Eshvi.picker.files()".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, nvim_bridge::gui::GuiEvent::PickerOpen(_))).is_none(), "an opened picker");
    assert!(ed.messages.iter().any(|m| m.text().contains("native picker is off")), "{:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>());
    bridge.send(UiCommand::Command("qa!".into()));

    // Not asked for (the shared config): nothing of it at all.
    let (bridge, mut ed) = picker_session(false, &[]);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    assert_eq!(ask(&bridge, &mut ed, "tostring(Eshvi.picker)"), "nil");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.exists(':EshviPick')"), "0");
    assert_eq!(ask(&bridge, &mut ed, "debug.getinfo(vim.ui.select, 'S').source:match('picker') == nil"), "true");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.exists(':EshviPick')"), "0");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn accepted_files_open_where_a_file_goes() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("accept");
    let odd = dir.join("we ird #1 %.txt");
    for (name, text) in [("a.txt", "one\ntwo\nthree\n"), ("b.txt", "alpha\nbeta\ngamma\n")] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    std::fs::write(&odd, "odd\n").unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), a.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    let open_one = |bridge: &Bridge, ed: &mut Editor| {
        bridge.send(UiCommand::Command("lua Eshvi.picker.files()".into()));
        next_picker(bridge, ed).id
    };
    let current = "vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ':t')";

    // At a line and column.
    let id = open_one(&bridge, &mut ed);
    accept(&bridge, id, vec![PickerChoice { lnum: Some(2), col: Some(3), ..by_path(&b) }], "edit");
    wait_for(&bridge, &mut ed, current, "b.txt");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 2, 2 }");

    // A path with spaces, `#` and `%` is that file, not a command's argument.
    let id = open_one(&bridge, &mut ed);
    accept(&bridge, id, vec![by_path(&odd)], "edit");
    wait_for(&bridge, &mut ed, current, "we ird #1 %.txt");

    // A file already in a window is focused, not opened again.
    bridge.send(UiCommand::Command(format!("vsplit {}", a.display())));
    wait_for(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)", "2");
    wait_for(&bridge, &mut ed, current, "a.txt");
    bridge.send(UiCommand::Command("wincmd w".into()));
    wait_for(&bridge, &mut ed, current, "we ird #1 %.txt");
    let id = open_one(&bridge, &mut ed);
    accept(&bridge, id, vec![by_path(&a)], "edit");
    wait_for(&bridge, &mut ed, current, "a.txt");
    assert_eq!(ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)"), "2");

    // Beside, and in a tab.
    let id = open_one(&bridge, &mut ed);
    accept(&bridge, id, vec![by_path(&b)], "split");
    wait_for(&bridge, &mut ed, current, "b.txt");
    wait_for(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)", "3");
    let id = open_one(&bridge, &mut ed);
    accept(&bridge, id, vec![by_path(&odd)], "tab");
    wait_for(&bridge, &mut ed, "#vim.api.nvim_list_tabpages()", "2");
    wait_for(&bridge, &mut ed, current, "we ird #1 %.txt");

    // Several: the quickfix list, opened.
    let id = open_one(&bridge, &mut ed);
    accept(&bridge, id, vec![by_path(&a), PickerChoice { lnum: Some(3), ..by_path(&b) }], "quickfix");
    wait_for(&bridge, &mut ed, "#vim.fn.getqflist()", "2");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist()[2].lnum"), "3");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.fnamemodify(vim.fn.bufname(vim.fn.getqflist()[2].bufnr), ':t')"), "b.txt");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist({ title = 1 }).title"), "Find files");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_chosen_buffer_is_shown_even_with_no_file() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("buffers");
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), "x\n").unwrap();
    }
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), dir.join("a.txt").to_str().unwrap(), dir.join("b.txt").to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    // An unnamed buffer with something in it is a row too, by its number.
    bridge.send(UiCommand::Command("enew | call setline(1, 'scratch')".into()));
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%')", "");
    bridge.send(UiCommand::Command("buffer 1".into()));
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    bridge.send(UiCommand::Command("lua Eshvi.picker.buffers()".into()));
    let open = next_picker(&bridge, &mut ed);
    let at = |name: &str| open.items.iter().position(|i| i.text == name).map(|i| i + 1).unwrap_or_else(|| panic!("{:?}", texts(&open)));
    let scratch = at("[No Name]");
    assert_eq!(open.items[scratch - 1].path, None);
    accept(&bridge, open.id, vec![by_idx(scratch)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.getline(1)", "scratch");
    bridge.send(UiCommand::Command("lua Eshvi.picker.buffers()".into()));
    let open = next_picker(&bridge, &mut ed);
    let b = open.items.iter().position(|i| i.text == "b.txt").unwrap() + 1;
    accept(&bridge, open.id, vec![by_idx(b)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ':t')", "b.txt");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_chosen_from_a_terminal_window_opens_in_the_editor_window() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("terminal");
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "one\n").unwrap();
    std::fs::write(&b, "two\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), a.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    let editor = ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()");
    bridge.send(UiCommand::Input(":botright split | terminal<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "terminal");
    std::thread::sleep(Duration::from_millis(500));
    bridge.send(UiCommand::Input("i".into()));
    wait_for(&bridge, &mut ed, "vim.fn.mode()", "t");
    // Opened from the terminal (a mapping in terminal mode runs `<Cmd>`).
    bridge.send(UiCommand::Input("<Cmd>lua Eshvi.picker.files()<CR>".into()));
    let open = next_picker(&bridge, &mut ed);
    // Nothing happened to the terminal meanwhile: cancelling leaves it in terminal mode.
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!((ask(&bridge, &mut ed, "vim.bo.buftype"), ask(&bridge, &mut ed, "vim.fn.mode()")), ("terminal".into(), "t".into()));

    bridge.send(UiCommand::Input("<Cmd>lua Eshvi.picker.files()<CR>".into()));
    let open = next_picker(&bridge, &mut ed);
    accept(&bridge, open.id, vec![by_path(&b)], "edit");
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ':t')"), "b.txt");
    // In the window where the file was, not the terminal's.
    assert_eq!(ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()"), editor);
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.mode()"), "n");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_picker_opened_in_insert_mode_leaves_it_alone_until_something_is_opened() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("insert");
    let b = dir.join("b.txt");
    std::fs::write(&b, "two\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display())]);
    bridge.send(UiCommand::Input("ihello<Cmd>lua Eshvi.picker.files()<CR>".into()));
    let open = next_picker(&bridge, &mut ed);
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.mode()"), "i");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getline(1)"), "hello");
    bridge.send(UiCommand::Input("<Cmd>lua Eshvi.picker.files()<CR>".into()));
    let open = next_picker(&bridge, &mut ed);
    accept(&bridge, open.id, vec![by_path(&b)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ':t')", "b.txt");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.mode()"), "n");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The picker's phase 2: grep, lines and the preview ----

use nvim_bridge::gui::PickerPreview;

/// Ask for a preview and wait for its answer.
fn preview(bridge: &Bridge, ed: &mut Editor, req: i64, path: Option<&std::path::Path>, buf: Option<i64>, lnum: usize) -> PickerPreview {
    use nvim_bridge::gui::GuiEvent;
    bridge.send(UiCommand::PickerPreview { req, path: path.map(|p| p.to_string_lossy().into_owned()), buf, lnum });
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::PickerPreview(p) if p.req == req)) {
        Some(GuiEvent::PickerPreview(p)) => p,
        _ => panic!("no gui_picker_preview for {req}"),
    }
}

#[test]
fn grep_sources_open_the_picker_with_a_preview() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("grep");
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "alpha needle\nsecond\n").unwrap();
    std::fs::write(&b, "beta\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), a.to_str().unwrap(), b.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    let current = "vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ':t')";

    bridge.send(UiCommand::Command(format!("lua Eshvi.picker.grep({{ query = 'abc' }})")));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.source.as_str(), open.preview, open.query.as_str(), open.literal, open.title.as_str()), ("grep", true, "abc", false, "Grep"));
    assert_eq!(open.cwd, dir.to_str().unwrap());
    assert!(open.grep_files.is_none() && open.extensions.is_none() && open.items.is_empty());

    // Only some files: the extensions go to the app, which does the search.
    bridge.send(UiCommand::Command("lua Eshvi.picker.grep({ title = 'Grep Verilog', extensions = { 'sv', 'v' } })".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.source.as_str(), open.title.as_str()), ("grep", "Grep Verilog"));
    assert_eq!(open.extensions, Some(vec!["sv".to_owned(), "v".to_owned()]));

    // The word under the cursor: literal.
    bridge.send(UiCommand::Command("buffer 1 | call setline(1, 'foo.bar baz') | normal! 0".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline(1)", "foo.bar baz");
    bridge.send(UiCommand::Command("lua Eshvi.picker.grep_word()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.source.as_str(), open.query.as_str(), open.literal, open.preview), ("grep", "foo", true, true));
    assert!(open.extensions.is_none());
    bridge.send(UiCommand::Command("lua Eshvi.picker.grep_word({ title = 'Grep Verilog word', extensions = { 'sv' } })".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.title.as_str(), open.query.as_str(), open.literal), ("Grep Verilog word", "foo", true));
    assert_eq!(open.extensions, Some(vec!["sv".to_owned()]));
    // (Cancelled, and given time: in the app keys typed while the box is open are its field's, so
    // keys sent to Neovim now would be taken as typed before the box was up.)
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });
    std::thread::sleep(Duration::from_millis(300));

    // A Visual selection, which it leaves.
    bridge.send(UiCommand::Input("<Esc>0fbvee<Cmd>lua Eshvi.picker.grep_word()<CR>".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.query.as_str(), open.literal), ("bar baz", true));
    wait_for(&bridge, &mut ed, "vim.fn.mode()", "n");
    // Cancel it so the next one isn't a replacement.
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });

    // The listed buffers' files.
    bridge.send(UiCommand::Command("lua Eshvi.picker.grep_buffers()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.source, "grep");
    let mut files = open.grep_files.expect("files");
    files.sort();
    assert_eq!(files, [a.to_str().unwrap(), b.to_str().unwrap()]);

    // `:EshviPick` has them too.
    bridge.send(UiCommand::Command("EshviPick grep".into()));
    assert_eq!(next_picker(&bridge, &mut ed).source, "grep");

    // Accepting a match opens the file at its line and column; the line's text goes to the quickfix list.
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.grep()");
    accept(&bridge, open.id, vec![PickerChoice { lnum: Some(1), col: Some(7), ..by_path(&a) }], "edit");
    wait_for(&bridge, &mut ed, current, "a.txt");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 1, 6 }");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.grep()");
    let hit = |p: &std::path::Path, l, t: &str| PickerChoice { lnum: Some(l), col: Some(1), text: Some(t.into()), ..by_path(p) };
    accept(&bridge, open.id, vec![hit(&a, 1, "alpha needle"), hit(&b, 1, "beta")], "quickfix");
    wait_for(&bridge, &mut ed, "#vim.fn.getqflist()", "2");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist()[1].text"), "alpha needle");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist({ title = 1 }).title"), "Grep");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// Run `command` and return the picker it opens.
fn next_picker_after(bridge: &Bridge, ed: &mut Editor, command: &str) -> PickerOpen {
    bridge.send(UiCommand::Command(command.into()));
    next_picker(bridge, ed)
}

#[test]
fn the_lines_picker_has_the_buffers_text_unsaved_included() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("lines");
    let a = dir.join("a.txt");
    std::fs::write(&a, "one\n\n  two\nthree\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), a.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    // Edited, not saved; the cursor on the third line.
    bridge.send(UiCommand::Command("call setline(4, 'unsaved four') | 3".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "3");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.lines()");
    assert!(open.preview && open.numbered && !open.files);
    assert_eq!(texts(&open), ["one", "two", "unsaved four"], "blank lines are left out, the indent too");
    let rows: Vec<(Option<usize>, Option<usize>)> = open.items.iter().map(|i| (i.lnum, i.col)).collect();
    assert_eq!(rows, [(Some(1), Some(1)), (Some(3), Some(3)), (Some(4), Some(1))]);
    assert!(open.items.iter().all(|i| i.buf.is_some() && i.path.as_deref() == a.to_str()));
    assert_eq!(open.selected, 2, "the row at the cursor starts selected");
    // Accepting puts the cursor on the line.
    accept(&bridge, open.id, vec![PickerChoice { idx: Some(3), ..Default::default() }], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "4");
    // A buffer with no name: the rows have no path, and are still the lines.
    bridge.send(UiCommand::Command("enew | call setline(1, ['x1', 'x2'])".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline(2)", "x2");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.lines()");
    assert_eq!(texts(&open), ["x1", "x2"]);
    assert!(open.items.iter().all(|i| i.path.is_none() && i.buf.is_some()));
    bridge.send(UiCommand::Command("buffer 1".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('$')", "4");
    accept(&bridge, open.id, vec![PickerChoice { idx: Some(2), ..Default::default() }], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.getline(1)", "x1");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.line('.')"), "2");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_preview_reads_the_file_without_opening_it() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("preview-file");
    let lua = dir.join("it's \"q\" file.lua");
    std::fs::write(&lua, "local x = 1\nreturn x\n").unwrap();
    let keep = dir.join("keep.txt");
    std::fs::write(&keep, "keep\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), keep.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    // Count what a buffer being opened would fire.
    bridge.send(UiCommand::Command(
        "lua _G.fired = {}; vim.api.nvim_create_autocmd({ 'BufNew', 'BufAdd', 'BufRead', 'BufReadPre', 'BufReadPost', 'BufEnter', 'BufWinEnter', 'FileType', 'BufNewFile' }, \
         { callback = function(ev) table.insert(_G.fired, ev.event) end })"
            .into(),
    ));
    wait_for(&bridge, &mut ed, "type(_G.fired)", "table");
    let before = ask(&bridge, &mut ed, "#vim.api.nvim_list_bufs() .. ':' .. #vim.fn.getbufinfo()");
    // The highlight of a Lua keyword, which the answer must carry.
    // (`@keyword.return.lua` falls back to `@keyword.return`, then `@keyword`.)
    bridge.send(UiCommand::Command(
        "lua vim.api.nvim_set_hl(0, '@keyword.lua', { fg = 0xff0000, bold = true }); vim.api.nvim_set_hl(0, '@keyword.return', { link = '@keyword.lua' })".into(),
    ));
    wait_for(&bridge, &mut ed, "vim.api.nvim_get_hl(0, { name = '@keyword.lua' }).fg", "16711680");

    let p = preview(&bridge, &mut ed, 1, Some(&lua), None, 2);
    assert_eq!((p.req, p.first_line, p.total, p.lnum), (1, 1, 2, 2));
    assert_eq!(p.lines, ["local x = 1", "return x"]);
    assert!(p.note.is_empty(), "{:?}", p.note);
    // `local` (0..5) and `return` (line 1, 0..6) are keywords, in the color set, bold.
    let kw: Vec<_> = p.spans.iter().filter(|s| s.fg == Some(0xff0000)).map(|s| (s.line, s.start, s.end, s.bold)).collect();
    assert_eq!(kw, [(0, 0, 5, true), (1, 0, 6, true)]);
    assert_eq!(p.path, lua.to_str().unwrap());
    // Nothing was opened, entered or detected: no buffer, no event.
    assert_eq!(ask(&bridge, &mut ed, "#vim.api.nvim_list_bufs() .. ':' .. #vim.fn.getbufinfo()"), before);
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(_G.fired)"), "{}");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.bufnr('it')"), "-1");

    // Colors change with the colorscheme's cache: set another and ask again.
    bridge.send(UiCommand::Command("lua vim.api.nvim_set_hl(0, '@keyword.lua', { fg = 0x00ff00 }); vim.api.nvim_exec_autocmds('ColorScheme', {})".into()));
    std::thread::sleep(Duration::from_millis(100));
    let p = preview(&bridge, &mut ed, 2, Some(&lua), None, 1);
    assert!(p.spans.iter().any(|s| s.fg == Some(0x00ff00) && s.start == 0 && s.end == 5), "{:?}", p.spans);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_preview_of_a_loaded_buffer_shows_its_unsaved_text() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("preview-buf");
    let a = dir.join("a.lua");
    std::fs::write(&a, "local saved = 1\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), a.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    bridge.send(UiCommand::Command("call setline(1, 'local unsaved = 2')".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline(1)", "local unsaved = 2");
    // Asked for by its path, and by its number.
    let by_path = preview(&bridge, &mut ed, 1, Some(&a), None, 1);
    assert_eq!(by_path.lines, ["local unsaved = 2"]);
    let buf: i64 = ask(&bridge, &mut ed, "vim.api.nvim_get_current_buf()").parse().unwrap();
    let by_buf = preview(&bridge, &mut ed, 2, None, Some(buf), 1);
    assert_eq!((by_buf.lines.clone(), by_buf.buf, by_buf.path.as_str()), (vec!["local unsaved = 2".to_string()], Some(buf), a.to_str().unwrap()));
    assert!(!by_buf.spans.is_empty(), "highlighted from the buffer's text too");
    // An unnamed buffer, and one that isn't text.
    bridge.send(UiCommand::Command("enew | call setline(1, 'scratch')".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getline(1)", "scratch");
    let unnamed: i64 = ask(&bridge, &mut ed, "vim.api.nvim_get_current_buf()").parse().unwrap();
    assert_eq!(preview(&bridge, &mut ed, 3, None, Some(unnamed), 1).lines, ["scratch"]);
    bridge.send(UiCommand::Command("help".into()));
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "help");
    let help: i64 = ask(&bridge, &mut ed, "vim.api.nvim_get_current_buf()").parse().unwrap();
    let p = preview(&bridge, &mut ed, 4, None, Some(help), 1);
    assert!(p.lines.is_empty() && !p.note.is_empty(), "{p:?}");
    // A buffer that's gone.
    let p = preview(&bridge, &mut ed, 5, None, Some(9999), 1);
    assert!(p.lines.is_empty() && !p.note.is_empty(), "{p:?}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_preview_with_no_parser_is_plain_and_files_that_cannot_be_shown_say_so() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("preview-plain");
    let plain = dir.join("notes.zzzz");
    std::fs::write(&plain, "local x = 1\r\nline two\n").unwrap();
    let binary = dir.join("blob.bin");
    std::fs::write(&binary, [b'a', 0, b'b']).unwrap();
    let big = dir.join("big.txt");
    std::fs::write(&big, vec![b'x'; 2 * 1024 * 1024 + 1]).unwrap();
    let many = dir.join("many.txt");
    std::fs::write(&many, "\n".repeat(200_001)).unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display())]);
    wait_for(&bridge, &mut ed, "vim.fn.getcwd()", dir.to_str().unwrap());

    let p = preview(&bridge, &mut ed, 1, Some(&plain), None, 1);
    assert_eq!(p.lines, ["local x = 1", "line two"], "a carriage return is not text");
    assert!(p.spans.is_empty() && p.note.is_empty());
    for (req, path, note) in [
        (2, &binary, "Binary file"),
        (3, &big, "Too large to preview"),
        (4, &many, "Too large to preview"),
        (5, &dir.join("gone.txt"), "Can't read this file"),
        (6, &dir, "Can't read this file"),
    ] {
        let p = preview(&bridge, &mut ed, req, Some(path), None, 1);
        assert_eq!((p.note.as_str(), p.lines.len()), (note, 0), "{}", path.display());
    }
    // Still working afterwards.
    assert_eq!(preview(&bridge, &mut ed, 7, Some(&plain), None, 2).lines.len(), 2);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_preview_is_a_window_of_the_file_around_the_line() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("preview-window");
    let big = dir.join("big.lua");
    let text: String = (1..=5000).map(|i| format!("local v{i} = {i}\n")).collect();
    std::fs::write(&big, text).unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display())]);
    wait_for(&bridge, &mut ed, "vim.fn.getcwd()", dir.to_str().unwrap());
    let p = preview(&bridge, &mut ed, 1, Some(&big), None, 3000);
    assert_eq!((p.first_line, p.lines.len(), p.total, p.lnum), (2500, 1000, 5000, 3000));
    assert_eq!(p.lines[0], "local v2500 = 2500");
    assert!(p.spans.iter().all(|s| s.line < 1000));
    // The ends are clamped, still a thousand lines.
    let p = preview(&bridge, &mut ed, 2, Some(&big), None, 1);
    assert_eq!((p.first_line, p.lines.len()), (1, 1000));
    let p = preview(&bridge, &mut ed, 3, Some(&big), None, 5000);
    assert_eq!((p.first_line, p.lines.len()), (4001, 1000));
    // A line past the end is the last.
    let p = preview(&bridge, &mut ed, 4, Some(&big), None, 99999);
    assert_eq!((p.first_line + p.lines.len() - 1, p.lnum), (5000, 5000));
    // Parsing a window of a big file is quick.
    let started = Instant::now();
    preview(&bridge, &mut ed, 5, Some(&big), None, 2000);
    eprintln!("a window of a 5000-line file: {:?}", started.elapsed());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Phase 3: LSP locations, symbols, diagnostics, quickfix and location lists ----

use nvim_bridge::gui::GuiEvent;

/// a.lua (with a Hebrew line, whose UTF-16 and byte columns differ) and b.lua,
/// open in a session whose folder is theirs, with the fake servers loaded.
fn lsp_session(name: &str) -> (Bridge, Editor, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let dir = picker_dir(name);
    let (a, b) = (dir.join("a.lua"), dir.join("b.lua"));
    std::fs::write(&a, "local alpha = 1\nprint(alpha, alpha)\n-- שלום alpha\nreturn alpha\n").unwrap();
    std::fs::write(&b, "local beta = alpha\nreturn beta\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), a.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_picker_lsp.lua");
    bridge.send(UiCommand::Command(format!("luafile {fake}")));
    wait_for(&bridge, &mut ed, "type(fake_lsp)", "function");
    bridge.send(UiCommand::Command("lua fake_watch()".into()));
    wait_for(&bridge, &mut ed, "type(_G.notes)", "table");
    (bridge, ed_of(ed), dir, a, b)
}

/// Change what a fake server answers, and wait until it has (two commands in a
/// row can swap).
fn fake_set(bridge: &Bridge, ed: &mut Editor, lua: String) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    bridge.send(UiCommand::Command(format!("{lua}; _G.fake_set_{n} = true")));
    wait_for(bridge, ed, &format!("_G.fake_set_{n}"), "true");
}

fn ed_of(ed: Editor) -> Editor {
    ed
}

/// Start a fake server for the current buffer and wait until it is attached.
fn start_fake(bridge: &Bridge, ed: &mut Editor, name: &str, opts: &str, attached: usize) {
    bridge.send(UiCommand::Command(format!("lua fake_lsp('{name}', {opts})")));
    // (Attached and initialized: only then does the client say what it supports.)
    wait_for(bridge, ed, "#vim.lsp.get_clients({ bufnr = 0, method = 'textDocument/definition' })", &attached.to_string());
}

/// Put the cursor at `line` (from 1), byte column `col` (from 0).
fn cursor_to(bridge: &Bridge, ed: &mut Editor, line: usize, col: usize) {
    bridge.send(UiCommand::Command(format!("call cursor({line}, {})", col + 1)));
    wait_for(bridge, ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", &format!("{{ {line}, {col} }}"));
}

fn file_and_cursor(bridge: &Bridge, ed: &mut Editor) -> String {
    ask(bridge, ed, "vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ':t') .. vim.inspect(vim.api.nvim_win_get_cursor(0)):gsub('%s+', '')")
}

fn picker_items_for(bridge: &Bridge, ed: &mut Editor, gen: i64) -> Vec<nvim_bridge::gui::PickerItem> {
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::PickerItems { gen: g, .. } if *g == gen)) {
        Some(GuiEvent::PickerItems { items, .. }) => items,
        _ => panic!("no gui_picker_items for {gen}"),
    }
}

#[test]
fn one_location_is_a_jump_with_no_picker_and_ctrl_o_and_ctrl_t_come_back() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, b) = lsp_session("loc-one");
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    cursor_to(&bridge, &mut ed, 2, 6);
    // The definition is on the first line of a.lua.
    let def = format!("lua FAKE.one['textDocument/definition'] = {{ fake_loc('{}', 0, 6, 11) }}", a.display());
    fake_set(&bridge, &mut ed, def);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_definitions()".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 1, 6 }");
    assert_eq!(ask(&bridge, &mut ed, "_G.opens"), "0", "one answer opens no box");
    // Ctrl-O comes back to where it was asked.
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 2, 6 }");
    // And so does Ctrl-T, the tag stack's.
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_definitions()".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 1, 6 }");
    bridge.send(UiCommand::Input("<C-t>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 2, 6 }");
    // In another file: Ctrl-O goes back to this one.
    fake_set(&bridge, &mut ed, format!("lua FAKE.one['textDocument/definition'] = fake_loc('{}', 1, 7, 11)", b.display()));
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_definitions()".into()));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "b.lua");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 2, 7 }");
    assert_eq!(ask(&bridge, &mut ed, "_G.opens"), "0");
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "a.lua");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 2, 6 }");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn several_locations_open_the_picker_merged_sorted_and_without_the_cursors_place() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, b) = lsp_session("loc-many");
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    start_fake(&bridge, &mut ed, "two", "{}", 2);
    cursor_to(&bridge, &mut ed, 2, 6);
    let loc = |p: &std::path::Path, l, c1, c2| format!("fake_loc('{}', {l}, {c1}, {c2})", p.display());
    // One server says b.lua's, the cursor's own place and a.lua's last line; the other
    // repeats two of them and adds a.lua's first.
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['textDocument/references'] = {{ {}, {}, {} }}; FAKE.two['textDocument/references'] = {{ {}, {}, {} }}",
        loc(&b, 0, 13, 18),
        loc(&a, 1, 6, 11),
        loc(&a, 3, 7, 12),
        loc(&a, 3, 7, 12),
        loc(&b, 0, 13, 18),
        loc(&a, 0, 6, 11),
    ));
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_references()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.title.as_str(), open.source.as_str(), open.preview, open.live), ("References", "items", true, false));
    // Each place once, the cursor's own left out, this file first.
    let places: Vec<_> = open.items.iter().map(|i| (i.text.as_str(), i.lnum.unwrap(), i.col.unwrap())).collect();
    assert_eq!(places, [("a.lua", 1, 7), ("a.lua", 4, 8), ("b.lua", 1, 14)]);
    let first = &open.items[0];
    let line = first.line.as_ref().expect("the line");
    assert_eq!((line.text.as_str(), line.ranges.clone(), line.line_ranges.clone()), ("local alpha = 1", vec![(6, 11)], vec![(6, 11)]));
    assert!(first.path.as_deref() == a.to_str() && first.search.as_deref() == Some("a.lua local alpha = 1"), "{first:?}");
    // The place is the line's trimmed text: this one is indented in the file? (it is not) -- the last line is.
    assert_eq!(open.items[1].line.as_ref().unwrap().text, "return alpha");
    // Accept the second: the cursor goes there, and Ctrl-O returns.
    accept(&bridge, open.id, vec![PickerChoice { idx: Some(2), lnum: Some(4), col: Some(8), ..Default::default() }], "edit");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 4, 7 }");
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 2, 6 }");
    bridge.send(UiCommand::Input("<C-o>".into()));
    // Enter with marks makes a quickfix list of the lines.
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_references()".into()));
    let open = next_picker(&bridge, &mut ed);
    let mark = |i: usize, p: &std::path::Path, l, c, t: &str| PickerChoice {
        idx: Some(i),
        lnum: Some(l),
        col: Some(c),
        text: Some(t.into()),
        path: Some(p.to_string_lossy().into_owned()),
    };
    accept(&bridge, open.id, vec![mark(1, &a, 1, 7, "local alpha = 1"), mark(3, &b, 1, 14, "local beta = alpha")], "quickfix");
    wait_for(&bridge, &mut ed, "#vim.fn.getqflist()", "2");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist()[2].text"), "local beta = alpha");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist({ title = 1 }).title"), "References");
    assert_eq!(ask(&bridge, &mut ed, "#vim.api.nvim_list_bufs()"), "3", "a.lua, b.lua (the quickfix entry) and the quickfix window's -- no more");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn no_results_no_server_and_one_server_failing_while_another_answers() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, _b) = lsp_session("loc-none");
    cursor_to(&bridge, &mut ed, 2, 6);
    let notes = |bridge: &Bridge, ed: &mut Editor| ask(bridge, ed, "table.concat(_G.notes, ';')");
    // No server at all.
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_references()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "1");
    assert_eq!(notes(&bridge, &mut ed), "No language server supports references|3", "a warning");
    // A server with nothing to say.
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_references()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "2");
    assert_eq!(notes(&bridge, &mut ed), "No language server supports references|3;No references found|2", "an info message");
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_type_definitions()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "3");
    assert!(notes(&bridge, &mut ed).ends_with("No type definitions found|2"));
    // One fails and the other answers: what came is used (one place: a jump).
    start_fake(&bridge, &mut ed, "two", "{}", 2);
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['textDocument/definition'] = {{ error = 'boom' }}; FAKE.two['textDocument/definition'] = {{ fake_loc('{}', 3, 7, 12) }}",
        a.display()
    ));
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_definitions()".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 4, 7 }");
    // All failing says why.
    fake_set(&bridge, &mut ed, "lua FAKE.two['textDocument/definition'] = { error = 'bust' }".into());
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_definitions()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "4");
    assert!(notes(&bridge, &mut ed).ends_with("boom|3"), "{}", notes(&bridge, &mut ed));
    assert_eq!(ask(&bridge, &mut ed, "_G.opens"), "0");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn location_links_and_columns_in_the_servers_units_land_on_the_right_byte() {
    if !nvim_available() {
        return;
    }
    // "-- שלום alpha": `alpha` is character 8 in UTF-16 (and UTF-32), byte 12.
    for (encoding, character) in [("utf-16", 8), ("utf-32", 8), ("utf-8", 12)] {
        let (bridge, mut ed, dir, a, _b) = lsp_session(&format!("loc-enc-{encoding}"));
        start_fake(&bridge, &mut ed, "one", &format!("{{ encoding = '{encoding}' }}"), 1);
        cursor_to(&bridge, &mut ed, 2, 6);
        assert_eq!(ask(&bridge, &mut ed, "vim.lsp.get_clients({ bufnr = 0 })[1].offset_encoding"), encoding);
        // A LocationLink: the target's selection range is where to go.
        let link = format!(
            "{{ targetUri = vim.uri_from_fname('{}'), originSelectionRange = {{ start = {{ line = 1, character = 6 }}, ['end'] = {{ line = 1, character = 11 }} }}, \
             targetRange = fake_loc('{}', 2, 0, {}).range, targetSelectionRange = fake_loc('{}', 2, {character}, {}).range }}",
            a.display(),
            a.display(),
            character + 5,
            a.display(),
            character + 5,
        );
        fake_set(&bridge, &mut ed, format!("lua FAKE.one['textDocument/definition'] = {{ {link} }}"));
        bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_definitions()".into()));
        wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 3, 12 }");
        // With another place the row says the same byte, and marks the word.
        fake_set(&bridge, &mut ed, format!(
            "lua FAKE.one['textDocument/definition'] = {{ {link}, fake_loc('{}', 0, 6, 11) }}",
            a.display()
        ));
        bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_definitions()".into()));
        let open = next_picker(&bridge, &mut ed);
        assert_eq!(open.title, "Definitions");
        let row = open.items.iter().find(|i| i.lnum == Some(3)).expect("the Hebrew line");
        assert_eq!(row.col, Some(13), "{encoding}");
        let line = row.line.as_ref().unwrap();
        assert_eq!(line.text, "-- שלום alpha");
        assert_eq!(line.ranges, [(12, 17)], "{encoding}");
        assert_eq!(&line.text[12..17], "alpha");
        bridge.send(UiCommand::Command("qa!".into()));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn an_answer_for_a_place_the_cursor_has_left_is_dropped() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, b) = lsp_session("loc-moved");
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    cursor_to(&bridge, &mut ed, 2, 6);
    let slow = |p: &std::path::Path| format!("{{ delay = 400, value = {{ fake_loc('{}', 0, 6, 11), fake_loc('{}', 3, 7, 12) }} }}", p.display(), p.display());
    fake_set(&bridge, &mut ed, format!("lua FAKE.one['textDocument/references'] = {}", slow(&a)));
    // Moved on in the same buffer.
    // (One command: two in a row can swap.)
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_references(); vim.fn.cursor(3, 1)".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 3, 0 }");
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(ask(&bridge, &mut ed, "_G.opens"), "0", "no box for a place that was left");
    assert_eq!(ask(&bridge, &mut ed, "#_G.notes"), "0");
    // Moved to another buffer.
    bridge.send(UiCommand::Command(format!("lua Eshvi.picker.lsp_references(); vim.cmd.edit('{}')", b.display())));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "b.lua");
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(ask(&bridge, &mut ed, "_G.opens"), "0");
    assert_eq!(file_and_cursor(&bridge, &mut ed), "b.lua{1,0}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn symbols_in_both_forms_start_on_the_one_at_the_cursor() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, _b) = lsp_session("symbols");
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    let range = |l: usize, c1: usize, c2: usize| format!("fake_loc('{}', {l}, {c1}, {c2}).range", a.display());
    // Hierarchical: a module with two children, then a variable.
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['textDocument/documentSymbol'] = {{ \
           {{ name = 'M', kind = 2, range = {r0}, selectionRange = {s0}, children = {{ \
             {{ name = 'foo', kind = 12, range = {r1}, selectionRange = {s1} }}, \
             {{ name = 'bar', kind = 6, range = {r2}, selectionRange = {s2} }} }} }}, \
           {{ name = 'tail', kind = 13, range = {r3}, selectionRange = {s3} }} }}",
        r0 = range(0, 0, 14),
        s0 = range(0, 6, 11),
        r1 = range(1, 0, 19),
        s1 = range(1, 0, 5),
        r2 = range(2, 0, 20),
        s2 = range(2, 3, 11),
        r3 = range(3, 0, 12),
        s3 = range(3, 7, 12),
    ));
    cursor_to(&bridge, &mut ed, 3, 14);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_symbols()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.title.as_str(), open.preview, open.live, open.files), ("Symbols", true, false, false));
    assert_eq!(texts(&open), ["M", "foo", "bar", "tail"], "source order");
    let shape: Vec<_> = open.items.iter().map(|i| (i.kind.as_deref().unwrap(), i.depth)).collect();
    assert_eq!(shape, [("Module", 0), ("Function", 1), ("Method", 1), ("Variable", 0)]);
    assert_eq!(open.selected, 3, "the last that starts at or before the cursor");
    assert!(open.items.iter().all(|i| i.buf.is_some() && i.path.as_deref() == a.to_str() && i.desc.is_empty()));
    // The jump is to the selection range (with the Hebrew line's byte column).
    assert_eq!(open.items[2].lnum, Some(3));
    assert_eq!(open.items[2].col, Some(4), "character 3 of '-- שלום alpha' is byte 3, the column 4");
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 2, 0 }");
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 3, 14 }");

    // Flat: SymbolInformation in the wrong order, with the container.
    let info = |name: &str, kind: usize, l: usize, container: &str| {
        format!("{{ name = '{name}', kind = {kind}, containerName = '{container}', location = fake_loc('{}', {l}, 0, 5) }}", a.display())
    };
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['textDocument/documentSymbol'] = {{ {}, {}, {} }}",
        info("late", 12, 3, "Outer"),
        info("early", 13, 0, ""),
        info("middle", 6, 1, "Outer")
    ));
    cursor_to(&bridge, &mut ed, 1, 0);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_symbols()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(texts(&open), ["early", "middle", "late"], "source order");
    let descs: Vec<_> = open.items.iter().map(|i| i.desc.as_str()).collect();
    assert_eq!(descs, ["", "Outer", "Outer"], "the parent, for the flat form");
    assert!(open.items.iter().all(|i| i.depth == 0));
    assert_eq!(open.selected, 1);
    // A buffer whose server has no symbols.
    fake_set(&bridge, &mut ed, "lua FAKE.one['textDocument/documentSymbol'] = {}".into());
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_symbols()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "1");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn workspace_symbols_are_a_live_picker_the_server_answers_for_each_query() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, b) = lsp_session("workspace");
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    let (ap, bp) = (a.display().to_string(), b.display().to_string());
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['workspace/symbol'] = function(params) \
           _G.asked = (_G.asked or '') .. params.query .. ';'; \
           local all = {{ \
             {{ name = 'foo_alpha', kind = 12, location = fake_loc('{ap}', 0, 6, 11) }}, \
             {{ name = 'foo_beta', kind = 6, location = fake_loc('{bp}', 1, 0, 6) }}, \
             {{ name = 'zeta', kind = 13, location = fake_loc('{ap}', 3, 7, 12) }} }}; \
           local out = {{}}; \
           for _, s in ipairs(all) do if s.name:find(params.query, 1, true) then table.insert(out, s) end end; \
           if params.query == 'slow' then return {{ delay = 300, value = out }} end; \
           return out end"
    ));
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_workspace_symbols()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert!(open.live && open.preview && open.items.is_empty());
    assert_eq!((open.title.as_str(), open.hint.as_str()), ("Workspace symbols", "Type to search symbols"));
    assert_eq!(ask(&bridge, &mut ed, "tostring(_G.asked)"), "nil", "nothing is asked until there is a query");
    // A query: the server's answer, with the number it was asked with.
    bridge.send(UiCommand::PickerQuery { id: open.id, gen: 1, text: "foo".into() });
    let rows = picker_items_for(&bridge, &mut ed, 1);
    assert_eq!(rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["foo_alpha", "foo_beta"]);
    assert_eq!(rows.iter().map(|r| r.kind.as_deref().unwrap()).collect::<Vec<_>>(), ["Function", "Method"]);
    assert_eq!((rows[0].path.as_deref(), rows[0].lnum, rows[0].col), (a.to_str(), Some(1), Some(7)));
    assert_eq!(rows[1].desc, "b.lua");
    // A slow answer comes after a quick one for a newer query: both carry their own number.
    bridge.send(UiCommand::PickerQuery { id: open.id, gen: 2, text: "slow".into() });
    bridge.send(UiCommand::PickerQuery { id: open.id, gen: 3, text: "zeta".into() });
    // (Each query runs on a task of its own: they may arrive in either order, each with its number.)
    let mut seen = std::collections::BTreeMap::new();
    while seen.len() < 2 {
        match wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PickerItems { .. })) {
            Some(GuiEvent::PickerItems { gen, items, .. }) => {
                seen.insert(gen, items);
            }
            _ => panic!("answers so far: {:?}", seen.keys()),
        }
    }
    assert_eq!(seen[&3].len(), 1);
    // (The order the queries reach the server in is not promised either.)
    let mut asked: Vec<String> = ask(&bridge, &mut ed, "_G.asked").split(';').filter(|q| !q.is_empty()).map(str::to_owned).collect();
    asked.sort();
    assert_eq!(asked, ["foo", "slow", "zeta"]);
    // An answer for a picker that was replaced is not sent.
    bridge.send(UiCommand::Command("lua Eshvi.picker.buffers()".into()));
    let _ = next_picker(&bridge, &mut ed);
    bridge.send(UiCommand::PickerQuery { id: open.id, gen: 4, text: "foo".into() });
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::PickerItems { .. })).is_none());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_live_row_accepted_by_what_it_holds_opens_the_file() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, b) = lsp_session("workspace-accept");
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_workspace_symbols()".into()));
    let open = next_picker(&bridge, &mut ed);
    let _ = &a;
    accept(&bridge, open.id, vec![PickerChoice { path: Some(b.to_string_lossy().into_owned()), lnum: Some(2), col: Some(8), ..Default::default() }], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "b.lua");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 2, 7 }");
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "a.lua");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// Set diagnostics for a file's buffer: `(line from 0, col, severity, message)`.
fn set_diagnostics(bridge: &Bridge, _ed: &mut Editor, file: &std::path::Path, list: &str) {
    bridge.send(UiCommand::Command(format!(
        "lua do local b = vim.fn.bufadd('{}'); vim.fn.bufload(b); \
         vim.diagnostic.set(vim.api.nvim_create_namespace('t'), b, {list}) end",
        file.display()
    )));
}

#[test]
fn diagnostics_are_sorted_by_severity_and_the_buffers_are_only_the_buffers() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, b) = lsp_session("diag");
    let s = |n: usize| format!("vim.diagnostic.severity.{}", ["ERROR", "WARN", "INFO", "HINT"][n - 1]);
    set_diagnostics(
        &bridge,
        &mut ed,
        &a,
        &format!(
            "{{ {{ lnum = 3, col = 0, end_lnum = 3, end_col = 6, severity = {}, message = 'return here\\nsecond line' }}, \
               {{ lnum = 0, col = 6, end_col = 11, severity = {}, message = 'a hint' }}, \
               {{ lnum = 1, col = 0, end_lnum = 2, end_col = 4, severity = {}, message = 'spans lines' }} }}",
            s(1),
            s(4),
            s(2)
        ),
    );
    set_diagnostics(&bridge, &mut ed, &b, &format!("{{ {{ lnum = 0, col = 0, end_col = 5, severity = {}, message = 'b is wrong' }} }}", s(1)));
    wait_for(&bridge, &mut ed, "#vim.diagnostic.get()", "4");
    // All files: by severity, then path and line.
    bridge.send(UiCommand::Command("lua Eshvi.picker.diagnostics()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.title.as_str(), open.preview, open.files), ("Diagnostics", true, false));
    assert_eq!(texts(&open), ["return here", "b is wrong", "spans lines", "a hint"]);
    let sev: Vec<_> = open.items.iter().map(|i| i.severity.unwrap()).collect();
    assert_eq!(sev, [1, 1, 2, 4]);
    assert_eq!(open.items[0].desc, "a.lua:4");
    assert_eq!(open.items[1].desc, "b.lua:1");
    assert_eq!(open.items[0].search.as_deref(), Some("return here a.lua"), "the message and the path are filtered");
    assert_eq!((open.items[0].lnum, open.items[0].col, open.items[0].marks.clone()), (Some(4), Some(1), vec![(0, 6)]));
    assert!(open.items[2].marks[0].1 >= 1000, "a range over several lines marks to the line's end");
    assert!(open.items.iter().all(|i| i.buf.is_some() && i.path.is_some()));
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "b.lua");
    // The buffer's: only b.lua's, with the line instead of the path.
    bridge.send(UiCommand::Command("lua Eshvi.picker.diagnostics({ buf = true })".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Buffer diagnostics");
    assert_eq!(texts(&open), ["b is wrong"]);
    assert_eq!(open.items[0].desc, "Ln 1");
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });
    // None: a message.
    bridge.send(UiCommand::Command("lua vim.diagnostic.reset(); Eshvi.picker.diagnostics()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "1");
    assert_eq!(ask(&bridge, &mut ed, "_G.notes[1]"), "No diagnostics|2");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn quickfix_and_location_lists_go_to_an_entry_with_cc_and_ll() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, b) = lsp_session("lists");
    // Empty: a message, no box.
    bridge.send(UiCommand::Command("lua Eshvi.picker.qflist()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "1");
    assert_eq!(ask(&bridge, &mut ed, "_G.notes[1]"), "Quickfix list is empty|2");
    bridge.send(UiCommand::Command("lua Eshvi.picker.loclist()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "2");
    assert_eq!(ask(&bridge, &mut ed, "_G.notes[2]"), "Location list is empty|2");
    assert_eq!(ask(&bridge, &mut ed, "_G.opens"), "0");
    // A list with a free-text entry among the files'.
    bridge.send(UiCommand::Command(format!(
        "lua vim.fn.setqflist({{}}, ' ', {{ title = 'Build', items = {{ \
           {{ filename = '{}', lnum = 2, col = 7, text = 'first' }}, \
           {{ text = 'warning: free text from a compiler' }}, \
           {{ filename = '{}', lnum = 1, col = 1, text = 'third' }} }} }})",
        a.display(),
        b.display()
    )));
    wait_for(&bridge, &mut ed, "#vim.fn.getqflist()", "3");
    bridge.send(UiCommand::Command("lua Eshvi.picker.qflist()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.title.as_str(), open.preview), ("Build", true));
    assert_eq!(texts(&open), ["a.lua", "warning: free text from a compiler", "b.lua"]);
    assert!(open.items[1].path.is_none() && open.items[1].line.is_none(), "free text has no file");
    assert_eq!(open.items[0].line.as_ref().unwrap().text, "first");
    assert_eq!((open.items[2].lnum, open.items[2].col), (Some(1), Some(1)));
    // Accept the third: `:cc 3`, so the list's position moved.
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist({ idx = 0 }).idx"), "1");
    accept(&bridge, open.id, vec![by_idx(3)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "b.lua");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist({ idx = 0 }).idx"), "3");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 1, 0 }");
    // The free-text entry does nothing.
    bridge.send(UiCommand::Command("lua Eshvi.picker.qflist()".into()));
    let open = next_picker(&bridge, &mut ed);
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist({ idx = 0 }).idx"), "3");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.expand('%:t')"), "b.lua");
    // The first, in a split.
    bridge.send(UiCommand::Command("lua Eshvi.picker.qflist()".into()));
    let open = next_picker(&bridge, &mut ed);
    accept(&bridge, open.id, vec![by_idx(1)], "split");
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "a.lua");
    assert_eq!(ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)"), "2");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getqflist({ idx = 0 }).idx"), "1");

    // A location list belongs to its window.
    bridge.send(UiCommand::Command(format!(
        "lua vim.fn.setloclist(0, {{}}, ' ', {{ title = 'Here', items = {{ \
           {{ filename = '{}', lnum = 1, col = 1, text = 'one' }}, {{ filename = '{}', lnum = 2, col = 1, text = 'two' }} }} }})",
        b.display(),
        a.display()
    )));
    wait_for(&bridge, &mut ed, "#vim.fn.getloclist(0)", "2");
    bridge.send(UiCommand::Command("lua Eshvi.picker.loclist()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Here");
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.getloclist(0, { idx = 0 }).idx", "2");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.expand('%:t')"), "a.lua");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.line('.')"), "2");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_location_picker_opened_in_a_terminal_window_jumps_in_the_editor_window() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _a, b) = lsp_session("loc-terminal");
    set_diagnostics(&bridge, &mut ed, &b, "{ { lnum = 1, col = 2, end_col = 6, severity = 1, message = 'in b' } }");
    wait_for(&bridge, &mut ed, "#vim.diagnostic.get()", "1");
    let editor = ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()");
    bridge.send(UiCommand::Input(":botright split | terminal<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "terminal");
    std::thread::sleep(Duration::from_millis(500));
    bridge.send(UiCommand::Input("<Cmd>lua Eshvi.picker.diagnostics()<CR>".into()));
    let open = next_picker(&bridge, &mut ed);
    accept(&bridge, open.id, vec![by_idx(1)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "b.lua");
    assert_eq!(ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()"), editor, "in the editor's window, not the terminal's");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 2, 2 }");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Native picker, phase 4: typing at once, the palette, Neovim's lists, resume ----

fn typed_keys(bridge: &Bridge, ed: &mut Editor, id: i64) -> Vec<String> {
    use nvim_bridge::gui::GuiEvent;
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::PickerTyped { id: i, .. } if *i == id)) {
        Some(GuiEvent::PickerTyped { keys, .. }) => keys,
        _ => panic!("no gui_picker_typed for {id}"),
    }
}

/// The next picker Neovim opens, without telling it the box is up.
fn next_picker_unanswered(bridge: &Bridge, ed: &mut Editor) -> PickerOpen {
    use nvim_bridge::gui::GuiEvent;
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::PickerOpen(_))) {
        Some(GuiEvent::PickerOpen(open)) => open,
        _ => panic!("no gui_picker_open"),
    }
}

const TYPED_AT_ONCE: &str = "ddjccxyz<Esc>p<CR><Space><lt>é";
const TYPED_BACK: [&str; 14] = ["d", "d", "j", "c", "c", "x", "y", "z", "<Esc>", "p", "<CR>", "<Space>", "<lt>", "é"];

#[test]
fn keys_typed_before_the_box_is_up_go_to_it_in_order_and_never_run_as_commands() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("typed-at-once");
    let a = dir.join("a.txt");
    std::fs::write(&a, "one\ntwo\nthree\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), a.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    // A mapping that opens the picker, as `<leader>/` does; how long `open` took is kept.
    bridge.send(UiCommand::Command(
        "lua vim.keymap.set('n', '<F5>', function() local t = vim.uv.hrtime(); Eshvi.picker.files(); _G.took = (vim.uv.hrtime() - t) / 1e6 end)".into(),
    ));
    wait_for(&bridge, &mut ed, "vim.fn.maparg('<F5>', 'n') ~= ''", "true");
    // A second's wait instead of 0.1 s: opening the picker took longer than 0.1 s on a busy CI
    // runner, so an answered open looked like one that waited.
    bridge.send(UiCommand::Command("let g:eshvi_picker_wait_ms = 1000".into()));
    wait_for(&bridge, &mut ed, "vim.g.eshvi_picker_wait_ms", "1000");

    // The box never answers: Neovim waits its moment, goes on, and hands over what was typed.
    // The keys are one `nvim_input`, the mapping's and then what is typed at once.
    bridge.send(UiCommand::Input(format!("<F5>{TYPED_AT_ONCE}")));
    let open = next_picker_unanswered(&bridge, &mut ed);
    assert_eq!(typed_keys(&bridge, &mut ed, open.id), TYPED_BACK);
    // None of them ran: the buffer is as it was, and Neovim is in Normal mode, not
    // operator-pending (what a stray `c` or `d` leaves), with nothing pending.
    assert_eq!(ask(&bridge, &mut ed, "table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '|')"), "one|two|three");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.mode(1)"), "n");
    assert_eq!(ask(&bridge, &mut ed, "vim.bo.modified"), "false");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 1, 0 }");
    let timed_out: f64 = ask(&bridge, &mut ed, "_G.took").parse().unwrap();
    assert!(timed_out >= 999.0, "it waited for the box: {timed_out} ms");

    // Answered at once: the wait ends early, and the same keys come.
    bridge.send(UiCommand::Command("lua Eshvi.picker.cancel(1000000)".into()));
    bridge.send(UiCommand::Input(format!("<F5>{TYPED_AT_ONCE}")));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(typed_keys(&bridge, &mut ed, open.id), TYPED_BACK);
    let answered: f64 = ask(&bridge, &mut ed, "_G.took").parse().unwrap();
    assert!(answered < 500.0, "it did not wait the whole time: {answered} ms");
    assert_eq!(ask(&bridge, &mut ed, "table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), '|')"), "one|two|three");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.mode(1)"), "n");

    // Nothing typed: an empty list is handed over, so the box knows nothing more is coming.
    bridge.send(UiCommand::Input("<F5>".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(typed_keys(&bridge, &mut ed, open.id), Vec::<String>::new());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// A session with Eshvi's menus (the palette lists their items) and notes of what `vim.notify` says.
fn palette_session(dir: &std::path::Path, files: &[&std::path::Path]) -> (Bridge, Editor) {
    let mut args = vec!["--clean".to_string(), "-n".into(), "-i".into(), "NONE".into(), "--cmd".into(), format!("cd {}", dir.display())];
    args.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
    let bridge = Bridge::spawn(
        SpawnOptions { args: plain_shell(args), width: 100, height: 30, native_picker: true, menu: true, terminal_enabled: true, ..Default::default() },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    wait_for(&bridge, &mut ed, "type(Eshvi.menu_items)", "function");
    bridge.send(UiCommand::Command("lua _G.notes = {}; vim.notify = function(m) table.insert(_G.notes, m) end".into()));
    wait_for(&bridge, &mut ed, "type(_G.notes)", "table");
    (bridge, ed)
}

fn idx_of(open: &PickerOpen, text: &str) -> usize {
    open.items.iter().position(|i| i.text == text).unwrap_or_else(|| panic!("no row {text:?} in {:?}", texts(open))) + 1
}

#[test]
fn the_palette_lists_the_menus_items_and_the_user_commands_and_runs_them() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("palette");
    let a = dir.join("a.txt");
    std::fs::write(&a, "one\ntwo\n").unwrap();
    let (bridge, mut ed) = palette_session(&dir, &[&a]);
    bridge.send(UiCommand::Command(
        "lua vim.api.nvim_create_user_command('EshviRun', function() _G.ran = (_G.ran or 0) + 1 end, { desc = 'Run the thing' }); \
         vim.api.nvim_create_user_command('EshviArg', function(o) _G.got_arg = o.args end, { nargs = 1, desc = 'Needs one' })"
            .into(),
    ));
    wait_for(&bridge, &mut ed, "vim.fn.exists(':EshviArg')", "2");

    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.palette()");
    assert_eq!((open.title.as_str(), open.source.as_str(), open.files, open.multi, open.preview), ("Command palette", "items", false, false, false));
    let find = |text: &str| open.items.iter().find(|i| i.text == text).unwrap_or_else(|| panic!("no {text}: {:?}", texts(&open)));
    // Menu items "Menu: Name", with the shortcut at the right and the action to ask the app about.
    assert_eq!((find("File: Save").hint.as_str(), find("File: Save").gate.as_deref()), ("⌘S", Some("save")));
    assert_eq!(find("File: Save As…").hint, "⇧⌘S");
    assert_eq!(find("Edit: Copy").gate.as_deref(), Some("copy"));
    assert_eq!(find("View: Side Bar").hint, "⌘B");
    // The palette's own item has the key; the command line has none (the key is the palette's).
    assert_eq!(find("View: Command Palette…").hint, "⇧⌘P");
    assert_eq!(find("View: Command Line").hint, "");
    assert_eq!(find("Go: Go to File…").hint, "⌘P");
    // Panels' toggles are listed (the app greys them by state); what the menu doesn't define
    // here (the agent, with no agent panel) is not.
    assert!(texts(&open).contains(&"View: Terminal"));
    assert!(!texts(&open).iter().any(|t| t.starts_with("View: Agent") || t.starts_with("View: Send to Agent")), "{:?}", texts(&open));
    // Open Recent and Buffers change with the session: they are not in the static list.
    assert!(!texts(&open).iter().any(|t| t.contains("Open Recent")));
    // User commands with their descriptions; built-in commands are not here.
    assert_eq!(find(":EshviRun").desc, "Run the thing");
    assert_eq!(find(":EshviArg").desc, "Needs one");
    assert!(!texts(&open).iter().any(|t| *t == ":write" || *t == ":edit"), "built-in commands are `<leader>sC`'s");
    // Each thing once.
    let mut sorted = texts(&open);
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), open.items.len(), "nothing is listed twice");

    // Enter on a menu item runs it as clicking does: File › New File makes an empty buffer.
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, "File: New File"))], "edit");
    wait_for(&bridge, &mut ed, "vim.api.nvim_buf_get_name(0) == '' and vim.fn.bufnr('$') >= 2", "true");

    // A user command that takes no argument runs.
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.palette()");
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, ":EshviRun"))], "edit");
    wait_for(&bridge, &mut ed, "_G.ran", "1");
    // One that needs an argument goes to the command line to be finished, and does not run.
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.palette()");
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, ":EshviArg"))], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.mode()", "c");
    wait_for(&bridge, &mut ed, "vim.fn.getcmdline()", "EshviArg ");
    assert_eq!(ask(&bridge, &mut ed, "tostring(_G.got_arg)"), "nil");
    bridge.send(UiCommand::Input("hello<CR>".into()));
    wait_for(&bridge, &mut ed, "_G.got_arg", "hello");

    // Recently run first, in the order run (this session only).
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.palette()");
    assert_eq!(&texts(&open)[..3], [":EshviArg", ":EshviRun", "File: New File"], "{:?}", &texts(&open)[..5]);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_shortcut_the_user_mapped_is_not_the_palettes() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("palette-mapped");
    let (bridge, mut ed) = palette_session(&dir, &[]);
    bridge.send(UiCommand::Command("nnoremap <D-s> <Cmd>echo 'mine'<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.maparg('<D-s>', 'n') ~= ''", "true");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.palette()");
    assert_eq!(open.items.iter().find(|i| i.text == "File: Save").unwrap().hint, "", "the menu bar doesn't show it either");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_the_native_picker_the_menu_is_as_it_was() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("palette-shared");
    let mut ed = Editor::new();
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(vec!["--clean".into(), "-n".into(), "-i".into(), "NONE".into()]),
            width: 100,
            height: 30,
            native_picker: false,
            menu: true,
            terminal_enabled: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    wait_for(&bridge, &mut ed, "type(Eshvi.menu_items)", "function");
    // The command line keeps ⇧⌘P; there is no palette item.
    wait_for(&bridge, &mut ed, "vim.fn.menu_get('View.Command Line')[1].actext", "⇧⌘P");
    assert_eq!(
        ask(&bridge, &mut ed, "vim.tbl_contains(vim.tbl_map(function(m) return m.name end, vim.fn.menu_get('View')[1].submenus), 'Command Palette…')"),
        "false"
    );
    assert_eq!(ask(&bridge, &mut ed, "tostring(Eshvi.picker)"), "nil");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// A session on `a.txt` (five lines) and `b.txt` in a folder of its own.
fn lists_session(name: &str) -> (Bridge, Editor, std::path::PathBuf) {
    let dir = picker_dir(name);
    std::fs::write(dir.join("a.txt"), "l1 one\nl2 two\nl3 three\nl4 four\nl5 five\n").unwrap();
    std::fs::write(dir.join("b.txt"), "b1\nb2\nb3 the third\n").unwrap();
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), dir.join("a.txt").to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    bridge.send(UiCommand::Command("lua _G.notes = {}; vim.notify = function(m) table.insert(_G.notes, m) end".into()));
    wait_for(&bridge, &mut ed, "type(_G.notes)", "table");
    (bridge, ed, dir)
}

#[test]
fn command_and_search_history_newest_first_run_and_search() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("history");
    bridge.send(UiCommand::Command("lua vim.fn.histadd(':', 'let g:one = 1'); vim.fn.histadd(':', 'let g:two = 2'); vim.fn.histadd('/', 'three'); vim.fn.histadd('/', 'four')".into()));
    wait_for(&bridge, &mut ed, "vim.fn.histget('/', -1)", "four");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.command_history()");
    assert_eq!((open.title.as_str(), open.preview, open.files), ("Command history", false, false));
    let t = texts(&open);
    assert_eq!(&t[..2], ["let g:two = 2", "let g:one = 1"], "newest first");
    // Enter runs it and puts it in the history again, newest.
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "vim.g.one", "1");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.histget(':', -1)"), "let g:one = 1");
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.g.two)"), "nil");

    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.search_history()");
    assert_eq!((open.title.as_str(), texts(&open)[..2].to_vec()), ("Search history", vec!["four", "three"]));
    // Enter searches (typed: the cursor moves and the pattern is the last search).
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.getreg('/')", "three");
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "3");
    // No history: a message.
    bridge.send(UiCommand::Command("lua vim.fn.histdel('/'); vim.fn.histdel(':')".into()));
    bridge.send(UiCommand::Command("lua Eshvi.picker.search_history()".into()));
    wait_for(&bridge, &mut ed, "_G.notes[#_G.notes]", "No search history");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn commands_lists_the_builtin_ones_and_the_users_with_their_descriptions() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("commands");
    bridge.send(UiCommand::Command(
        "lua vim.api.nvim_create_user_command('EshviRun', function() _G.ran = (_G.ran or 0) + 1 end, { desc = 'Run the thing' })".into(),
    ));
    wait_for(&bridge, &mut ed, "vim.fn.exists(':EshviRun')", "2");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.commands()");
    assert_eq!(open.title, "Commands");
    let write = open.items.iter().find(|i| i.text == ":write").expect("built-in :write");
    assert_eq!(write.desc, "");
    assert_eq!(open.items.iter().find(|i| i.text == ":EshviRun").unwrap().desc, "Run the thing");
    // A user command runs; a built-in one is left on the command line, its arguments unknown.
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, ":EshviRun"))], "edit");
    wait_for(&bridge, &mut ed, "_G.ran", "1");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.commands()");
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, ":write"))], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.getcmdline()", "write ");
    bridge.send(UiCommand::Input("<Esc>".into()));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn keymaps_show_their_mode_and_only_normal_mode_ones_run() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("keymaps");
    bridge.send(UiCommand::Command(
        "lua vim.keymap.set('n', '<Space>q', function() _G.ran_q = true end, { desc = 'Run q' }); \
         vim.keymap.set('i', 'jk', '<Esc>', { desc = 'Leave insert' }); \
         vim.keymap.set('x', '<Space>w', 'y', {}); \
         vim.keymap.set('n', '<Space>b', function() _G.ran_b = true end, { buffer = 0, desc = 'Buffer only' }); \
         vim.keymap.set('n', '<Plug>(hidden)', function() end, {})"
            .into(),
    ));
    wait_for(&bridge, &mut ed, "vim.fn.maparg('<Space>b', 'n') ~= ''", "true");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.keymaps()");
    assert_eq!((open.title.as_str(), open.preview), ("Keymaps", false));
    let row = |lhs: &str, mode: &str| open.items.iter().find(|i| i.text == lhs && i.prefix == mode).unwrap_or_else(|| panic!("{lhs} {mode}: {:?}", texts(&open)));
    assert_eq!(row("<Space>q", "n").desc, "Run q");
    assert_eq!(row("jk", "i").desc, "Leave insert");
    assert_eq!(row("<Space>w", "x").desc, "y", "no desc: the right side");
    assert_eq!(row("<Space>b", "n").desc, "Buffer only", "the buffer's own too");
    assert!(!texts(&open).iter().any(|t| t.contains("<Plug>")));
    // A normal-mode map runs (as typed, remapped); another mode's doesn't, and says why.
    let q = open.items.iter().position(|i| i.text == "<Space>q").unwrap() + 1;
    accept(&bridge, open.id, vec![by_idx(q)], "edit");
    wait_for(&bridge, &mut ed, "_G.ran_q", "true");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.keymaps()");
    let jk = open.items.iter().position(|i| i.text == "jk").unwrap() + 1;
    accept(&bridge, open.id, vec![by_idx(jk)], "edit");
    wait_for(&bridge, &mut ed, "#_G.notes", "1");
    assert!(ask(&bridge, &mut ed, "_G.notes[1]").contains("normal-mode"), "{}", ask(&bridge, &mut ed, "_G.notes[1]"));
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.mode()"), "n");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn help_has_the_tags_and_opens_the_page_where_it_is_told() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("help");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.help()");
    assert_eq!(open.title, "Help");
    assert!(open.items.len() > 1000, "{}", open.items.len());
    assert!(!open.preview && !open.files);
    let tag = idx_of(&open, "quickref");
    accept(&bridge, open.id, vec![by_idx(tag)], "edit");
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "help");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.winlayout()[1]"), "col", "a split above");
    bridge.send(UiCommand::Command("only | edit a.txt".into()));
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.help()");
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, "quickref"))], "vsplit");
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "help");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.winlayout()[1]"), "row", "`:vert help`");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn registers_show_their_text_on_a_line_and_paste_after_the_cursor() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("registers");
    bridge.send(UiCommand::Command("lua vim.fn.setreg('a', 'hello'); vim.fn.setreg('b', 'two\\nlines'); vim.fn.setreg('c', '')".into()));
    wait_for(&bridge, &mut ed, "vim.fn.getreg('a')", "hello");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.registers()");
    assert_eq!(open.title, "Registers");
    let find = |name: &str| open.items.iter().find(|i| i.text == name).unwrap_or_else(|| panic!("{name}: {:?}", texts(&open)));
    assert_eq!(find("\"a").desc, "hello");
    assert!(find("\"a").mono, "the grid font");
    assert_eq!(find("\"b").desc, "two↵lines", "one line");
    assert!(!texts(&open).contains(&"\"c"), "an empty register is not listed");
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, "\"a"))], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.getline(1)", "lhello1 one");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn marks_jump_to_the_place_with_a_preview_and_local_and_global_ones_are_listed() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("marks");
    let b = dir.join("b.txt");
    bridge.send(UiCommand::Command(format!("lua vim.cmd('normal! 2Gma'); vim.cmd('edit {}'); vim.cmd('normal! 3GmB'); vim.cmd('edit a.txt')", b.display())));
    wait_for(&bridge, &mut ed, "vim.fn.getpos(\"'B\")[2]", "3");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.marks()");
    assert!(open.preview && !open.files);
    assert_eq!(open.title, "Marks");
    let local = open.items.iter().find(|i| i.text == "'a").expect("local mark a");
    assert_eq!((local.lnum, local.path.as_deref()), (Some(2), dir.join("a.txt").to_str()));
    assert!(local.desc.contains("a.txt:2") && local.desc.contains("l2 two"), "{}", local.desc);
    let global = open.items.iter().find(|i| i.text == "'B").expect("global mark B");
    assert_eq!((global.lnum, global.path.as_deref()), (Some(3), b.to_str()));
    assert!(global.desc.contains("b.txt:3") && global.desc.contains("b3 the third"), "{}", global.desc);
    // Jumps to a mark in another file; and the place it came from is in the jumplist.
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, "'B"))], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "b.txt");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.line('.')"), "3");
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "a.txt");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn jumps_newest_first_go_there_the_way_ctrl_o_does() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("jumps");
    // Jumps from line 1 to 4, to 2, to 5: the jumplist holds where each was left.
    bridge.send(UiCommand::Command("lua vim.cmd('normal! 4G'); vim.cmd('normal! 2G'); vim.cmd('normal! 5G')".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "5");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.jumps()");
    assert!(open.preview);
    assert_eq!(open.title, "Jumps");
    let lines: Vec<_> = open.items.iter().map(|i| i.lnum.unwrap()).collect();
    assert_eq!(lines, [2, 4, 1], "newest first");
    assert_eq!(open.items[1].text, "a.txt:4");
    assert_eq!(open.items[1].desc, "l4 four");
    // The second row is line 4: two Ctrl-O's from the end.
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "4");
    // The jumplist moved with it: Ctrl-I goes on to line 2, as it would from there.
    bridge.send(UiCommand::Input("<C-i>".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "2");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn undo_history_newest_first_starts_on_the_current_state_and_goes_to_the_one_chosen() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("undo");
    for (n, word) in ["x", "y", "z"].iter().enumerate() {
        bridge.send(UiCommand::Input(format!("A{word}<Esc>")));
        wait_for(&bridge, &mut ed, "vim.fn.line('.') == 1 and vim.fn.undotree().seq_cur", &(n + 1).to_string());
    }
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getline(1)"), "l1 oneyxz".replace("yxz", "xyz"));
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.undo()");
    assert_eq!((open.title.as_str(), open.preview), ("Undo history", false));
    assert_eq!(texts(&open), ["3", "2", "1"], "newest first");
    assert_eq!(open.selected, 1, "the current state");
    assert!(open.items.iter().all(|i| !i.desc.is_empty() && i.hint.starts_with('+')), "{:?}", open.items);
    assert!(open.items[0].desc.contains("ago") || open.items[0].desc == "just now");
    accept(&bridge, open.id, vec![by_idx(3)], "edit");
    wait_for(&bridge, &mut ed, "vim.fn.getline(1)", "l1 onex");
    // No history: a message.
    bridge.send(UiCommand::Command("lua vim.cmd('enew!')".into()));
    bridge.send(UiCommand::Command("lua Eshvi.picker.undo()".into()));
    wait_for(&bridge, &mut ed, "_G.notes[#_G.notes]", "No undo history");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn colorschemes_apply_as_the_selection_moves_come_back_on_escape_and_stay_on_enter() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("colorschemes");
    bridge.send(UiCommand::Command("colorscheme default | set background=dark".into()));
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "default");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.colorschemes()");
    assert_eq!((open.title.as_str(), open.on_select, open.preview), ("Colorschemes", true, false));
    let here = idx_of(&open, "default");
    assert_eq!(open.selected, here, "the one in use starts selected");
    // The app says the selection stayed on `morning` (a light scheme): it is applied, the background with it.
    bridge.send(UiCommand::PickerSelected { id: open.id, idx: idx_of(&open, "morning") });
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "morning");
    assert_eq!(ask(&bridge, &mut ed, "vim.o.background"), "light", "a scheme that sets the background changes it");
    bridge.send(UiCommand::PickerSelected { id: open.id, idx: idx_of(&open, "desert") });
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "desert");
    // Escape: the first one, and its background, are back.
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "default");
    assert_eq!(ask(&bridge, &mut ed, "vim.o.background"), "dark");
    // Enter keeps what is chosen (also when the app didn't say the selection had stayed).
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.colorschemes()");
    bridge.send(UiCommand::PickerSelected { id: open.id, idx: idx_of(&open, "desert") });
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "desert");
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, "desert"))], "edit");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(ask(&bridge, &mut ed, "vim.g.colors_name"), "desert");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.colorschemes()");
    accept(&bridge, open.id, vec![by_idx(idx_of(&open, "blue"))], "edit");
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "blue");
    // A scheme that is replaced by another picker is undone too (nothing was chosen).
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.colorschemes()");
    bridge.send(UiCommand::PickerSelected { id: open.id, idx: idx_of(&open, "desert") });
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "desert");
    bridge.send(UiCommand::Command("lua Eshvi.picker.buffers()".into()));
    wait_for(&bridge, &mut ed, "vim.g.colors_name", "blue");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn resume_reopens_the_last_picker_with_its_query_and_row_and_reads_its_source_again() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir) = lists_session("resume");
    let settle = || std::thread::sleep(Duration::from_millis(300));
    // Nothing to resume yet.
    bridge.send(UiCommand::Command("lua Eshvi.picker.resume()".into()));
    wait_for(&bridge, &mut ed, "_G.notes[#_G.notes]", "No picker to resume");

    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.buffers()");
    assert_eq!(texts(&open), ["a.txt"]);
    // A new buffer appears while it is closed.
    let want = nvim_bridge::gui::PickerWant { text: "a.txt".into(), path: Some(dir.join("a.txt").to_string_lossy().into_owned()), lnum: None };
    bridge.send(UiCommand::PickerCancel { id: open.id, state: nvim_bridge::gui::PickerState { query: "txt".into(), row: Some(want.clone()) } });
    bridge.send(UiCommand::Command(format!("badd {}", dir.join("b.txt").display())));
    wait_for(&bridge, &mut ed, "#vim.fn.getbufinfo({ buflisted = 1 })", "2");
    let again = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.resume()");
    assert_eq!((again.title.as_str(), again.query.as_str(), again.want.as_ref()), ("Buffers", "txt", Some(&want)));
    let mut names = texts(&again);
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt"], "the source is read again");

    // A select is not resumable: the one before it still is.
    bridge.send(UiCommand::PickerCancel { id: again.id, state: nvim_bridge::gui::PickerState { query: "b".into(), row: None } });
    settle(); // (A cancel and the next command run on tasks of their own and can swap.)
    let select = next_picker_after(&bridge, &mut ed, "lua vim.ui.select({ 'x', 'y' }, {}, function() end)");
    bridge.send(UiCommand::PickerCancel { id: select.id, state: nvim_bridge::gui::PickerState { query: "zzz".into(), row: None } });
    settle();
    let third = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.resume()");
    assert_eq!((third.title.as_str(), third.query.as_str()), ("Buffers", "b"));
    // So are the colorschemes not.
    bridge.send(UiCommand::PickerCancel { id: third.id, state: nvim_bridge::gui::PickerState { query: "b".into(), row: None } });
    settle();
    let schemes = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.colorschemes()");
    bridge.send(UiCommand::PickerCancel { id: schemes.id, state: nvim_bridge::gui::PickerState { query: "q".into(), row: None } });
    settle();
    let fourth = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.resume()");
    assert_eq!(fourth.title, "Buffers");

    // A grep comes back as it was, with the query it ended on (the app searches again).
    let grep = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.grep()");
    assert_eq!((grep.source.as_str(), grep.query.as_str()), ("grep", ""));
    bridge.send(UiCommand::PickerAccept {
        id: grep.id,
        chosen: vec![],
        how: "edit".into(),
        state: nvim_bridge::gui::PickerState { query: "three".into(), row: None },
    });
    settle();
    let regrep = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.resume()");
    assert_eq!((regrep.source.as_str(), regrep.title.as_str(), regrep.query.as_str(), regrep.preview), ("grep", "Grep", "three", true));
    // Replaced by another picker without an answer: it is still the last, with no query of its own.
    let other = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.files()");
    assert!(other.id > regrep.id);
    bridge.send(UiCommand::PickerCancel { id: other.id, state: nvim_bridge::gui::PickerState { query: "a".into(), row: None } });
    settle();
    let files_again = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.resume()");
    assert_eq!((files_again.source.as_str(), files_again.query.as_str()), ("files", "a"));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn symbols_start_on_the_innermost_symbol_around_the_cursor_not_a_parameter() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, _b) = lsp_session("symbols-inner");
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    let range = |l: usize, c1: usize, l2: usize, c2: usize| {
        format!("{{ start = {{ line = {l}, character = {c1} }}, ['end'] = {{ line = {l2}, character = {c2} }} }}")
    };
    let _ = &a;
    // outer(a, b) spans lines 0..3 with two parameters on its first line; inner is nested in it.
    // After them, a top-level table that is only a variable.
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['textDocument/documentSymbol'] = {{ \
           {{ name = 'outer', kind = 12, range = {o}, selectionRange = {os}, children = {{ \
             {{ name = 'a', kind = 13, range = {pa}, selectionRange = {pa} }}, \
             {{ name = 'b', kind = 13, range = {pb}, selectionRange = {pb} }}, \
             {{ name = 'inner', kind = 6, range = {i}, selectionRange = {is}, children = {{ \
               {{ name = 'local_x', kind = 13, range = {x}, selectionRange = {x} }} }} }} }} }}, \
           {{ name = 'tbl', kind = 13, range = {t}, selectionRange = {t} }} }}",
        o = range(0, 0, 2, 3),
        os = range(0, 6, 0, 11),
        pa = range(0, 12, 0, 13),
        pb = range(0, 15, 0, 16),
        i = range(1, 0, 1, 8),
        is = range(1, 2, 1, 7),
        x = range(1, 2, 1, 5),
        t = range(3, 0, 3, 10),
    ));
    // The cursor in the body, past the parameters, where a "last symbol starting before the cursor" is `b`.
    cursor_to(&bridge, &mut ed, 3, 1);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_symbols()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(texts(&open), ["outer", "a", "b", "inner", "local_x", "tbl"]);
    assert_eq!(open.items[open.selected - 1].text, "outer", "the function, not its last parameter");
    // On `inner`'s line, inside it and inside `local_x`: the innermost container, `inner`, not the variable.
    cursor_to(&bridge, &mut ed, 2, 3);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_symbols()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.items[open.selected - 1].text, "inner");
    // In a variable only (no function around it): that variable.
    cursor_to(&bridge, &mut ed, 4, 4);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_symbols()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.items[open.selected - 1].text, "tbl");
    // Outside every range (past the end of the last): the last that starts before the cursor, as before.
    bridge.send(UiCommand::Command("lua vim.fn.append(vim.fn.line('$'), { '', '', '' })".into()));
    cursor_to(&bridge, &mut ed, 7, 0);
    bridge.send(UiCommand::Command("lua Eshvi.picker.lsp_symbols()".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.items[open.selected - 1].text, "tbl");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn commands_sent_back_to_back_run_in_the_order_sent() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = picker_session(false, &[]);
    bridge.send(UiCommand::Command("lua _G.order = {}".into()));
    wait_for(&bridge, &mut ed, "type(_G.order)", "table");
    for round in 0..500 {
        bridge.send(UiCommand::Command(format!("lua table.insert(_G.order, {})", round * 2)));
        bridge.send(UiCommand::Command(format!("lua table.insert(_G.order, {})", round * 2 + 1)));
    }
    wait_for(&bridge, &mut ed, "#_G.order", "1000");
    let swapped = ask(&bridge, &mut ed, "(function() local n = 0 for i = 2, #_G.order do if _G.order[i] < _G.order[i-1] then n = n + 1 end end return n end)()");
    // (Each on a task of its own they swapped: 46 to 100 of the 999 neighbours.)
    assert_eq!(swapped, "0", "commands arrived out of order");
    bridge.send(UiCommand::Command("qa!".into()));
}

// ---- Git history of a file and of a line (picker.lua: git_log_file, git_log_line) ----

fn git_in(dir: &std::path::Path, author: &str, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", &format!("user.name={author}"), "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repository with `a.txt` renamed to `b.txt` and edited since: four
/// commits, by two authors; returns the folder and the commits, newest first.
fn history_repo(name: &str) -> (std::path::PathBuf, Vec<String>) {
    let dir = picker_dir(name);
    git_in(&dir, "Ann", &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git_in(&dir, "Ann", &["add", "."]);
    git_in(&dir, "Ann", &["commit", "-qm", "first file"]);
    git_in(&dir, "Bea", &["mv", "a.txt", "b.txt"]);
    git_in(&dir, "Bea", &["commit", "-qm", "rename it"]);
    std::fs::write(dir.join("b.txt"), "one\nTWO\nthree\n").unwrap();
    git_in(&dir, "Ann", &["commit", "-qam", "edit two"]);
    std::fs::write(dir.join("b.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    git_in(&dir, "Bea", &["commit", "-qam", "add four"]);
    let oids = git_in(&dir, "Ann", &["log", "--format=%H"]).lines().map(str::to_owned).collect();
    (dir, oids)
}

fn history_session(dir: &std::path::Path, file: &str) -> (Bridge, Editor) {
    let (bridge, mut ed) = picker_session(true, &["--cmd", &format!("cd {}", dir.display()), dir.join(file).to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    bridge.send(UiCommand::Command("lua _G.notes = {}; vim.notify = function(m) table.insert(_G.notes, m) end".into()));
    wait_for(&bridge, &mut ed, "type(_G.notes)", "table");
    (bridge, ed)
}

/// The next `gui_git_diff`.
fn next_git_diff(bridge: &Bridge, ed: &mut Editor) -> nvim_bridge::gui::GuiEvent {
    use nvim_bridge::gui::GuiEvent;
    wait_gui(bridge, ed, |g| matches!(g, GuiEvent::GitDiff { .. })).expect("no gui_git_diff")
}

/// Nothing opens a picker for a moment (a message was shown instead).
fn no_picker_follows(bridge: &Bridge, ed: &mut Editor) {
    use nvim_bridge::gui::GuiEvent;
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        while let Some(ev) = bridge.try_recv() {
            match ev {
                BridgeEvent::Redraw(batch) => batch.into_iter().for_each(|e| ed.apply(e)),
                BridgeEvent::Gui(GuiEvent::PickerOpen(open)) => panic!("a picker opened: {}", open.title),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_files_history_follows_a_rename_and_enter_asks_for_the_diff_as_the_graph_does() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (dir, oids) = history_repo("gitlog-file");
    let (bridge, mut ed) = history_session(&dir, "b.txt");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.git_log_file()");
    assert_eq!(open.title, "History of b.txt");
    assert_eq!(texts(&open), ["add four", "edit two", "rename it", "first file"], "newest first, across the rename");
    assert!(!open.multi && !open.files && !open.preview, "no marks, no file rows, no preview");
    assert_eq!(open.items[0].prefix, oids[0][..7], "the short hash is the dim prefix");
    assert!(open.items[0].desc.starts_with("Bea, "), "{}", open.items[0].desc);
    assert!(open.items[1].desc.starts_with("Ann, "), "{}", open.items[1].desc);
    assert!(open.items[0].desc.ends_with("ago") || open.items[0].desc.ends_with("now"), "{}", open.items[0].desc);
    let search = open.items[2].search.as_deref().unwrap();
    assert!(search.contains("rename it") && search.contains(&oids[2][..7]) && search.contains("Bea"), "{search}");
    // The rename: the file under its new name, the old one on the left.
    accept(&bridge, open.id, vec![by_idx(3)], "edit");
    assert_eq!(
        next_git_diff(&bridge, &mut ed),
        GuiEvent::GitDiff {
            top: dir.to_string_lossy().into_owned(),
            oid: oids[2].clone(),
            parent: oids[3].clone(),
            path: "b.txt".into(),
            left_path: Some("a.txt".into()),
            deleted: false
        }
    );
    // A commit before the rename: the file as it was named then; the root commit has no parent.
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.git_log_file()");
    accept(&bridge, open.id, vec![by_idx(4)], "edit");
    assert_eq!(
        next_git_diff(&bridge, &mut ed),
        GuiEvent::GitDiff {
            top: dir.to_string_lossy().into_owned(),
            oid: oids[3].clone(),
            parent: String::new(),
            path: "a.txt".into(),
            left_path: None,
            deleted: false
        }
    );
    // A plain edit.
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.git_log_file()");
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    let GuiEvent::GitDiff { oid, parent, path, left_path, .. } = next_git_diff(&bridge, &mut ed) else { unreachable!() };
    assert_eq!((oid, parent, path, left_path), (oids[1].clone(), oids[2].clone(), "b.txt".into(), None));
    // Escape asks for nothing.
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.git_log_file()");
    bridge.send(UiCommand::PickerCancel { id: open.id, state: Default::default() });
    no_git_diff_follows(&bridge, &mut ed);
    // It is in the list of `:EshviPick`.
    let open = next_picker_after(&bridge, &mut ed, "EshviPick git_log_file");
    assert_eq!(open.title, "History of b.txt");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

fn no_git_diff_follows(bridge: &Bridge, ed: &mut Editor) {
    use nvim_bridge::gui::GuiEvent;
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        while let Some(ev) = bridge.try_recv() {
            match ev {
                BridgeEvent::Redraw(batch) => batch.into_iter().for_each(|e| ed.apply(e)),
                BridgeEvent::Gui(g @ GuiEvent::GitDiff { .. }) => panic!("a diff was asked for: {g:?}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_lines_history_has_the_commits_that_changed_it_and_the_files_name_then() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (dir, oids) = history_repo("gitlog-line");
    let (bridge, mut ed) = history_session(&dir, "b.txt");
    bridge.send(UiCommand::Command("lua vim.api.nvim_win_set_cursor(0, { 2, 0 })".into()));
    wait_for(&bridge, &mut ed, "vim.api.nvim_win_get_cursor(0)[1]", "2");
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.git_log_line()");
    assert_eq!(open.title, "History of b.txt:2");
    // (The rename and the line added after changed nothing on it.)
    assert_eq!(texts(&open), ["edit two", "first file"]);
    assert_eq!(open.items[0].prefix, oids[1][..7]);
    assert!(!open.multi && !open.preview);
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    assert_eq!(
        next_git_diff(&bridge, &mut ed),
        GuiEvent::GitDiff {
            top: dir.to_string_lossy().into_owned(),
            oid: oids[3].clone(),
            parent: String::new(),
            path: "a.txt".into(),
            left_path: None,
            deleted: false
        }
    );
    let open = next_picker_after(&bridge, &mut ed, "lua Eshvi.picker.git_log_line()");
    accept(&bridge, open.id, vec![by_idx(1)], "edit");
    let GuiEvent::GitDiff { oid, parent, path, .. } = next_git_diff(&bridge, &mut ed) else { unreachable!() };
    assert_eq!((oid, parent, path), (oids[1].clone(), oids[2].clone(), "b.txt".into()));
    // The line the last commit added.
    bridge.send(UiCommand::Command("lua vim.api.nvim_win_set_cursor(0, { 4, 0 })".into()));
    wait_for(&bridge, &mut ed, "vim.api.nvim_win_get_cursor(0)[1]", "4");
    let open = next_picker_after(&bridge, &mut ed, "EshviPick git_log_line");
    assert_eq!((open.title.as_str(), texts(&open)), ("History of b.txt:4", vec!["add four"]));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn history_says_why_it_has_none_and_opens_no_box() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let notes = |bridge: &Bridge, ed: &mut Editor| ask(bridge, ed, "table.concat(_G.notes, ';')");
    let (dir, _) = history_repo("gitlog-none");
    std::fs::write(dir.join("loose.txt"), "x\n").unwrap();
    std::fs::write(dir.join("staged.txt"), "x\n").unwrap();
    git_in(&dir, "Ann", &["add", "staged.txt"]);
    let (bridge, mut ed) = history_session(&dir, "loose.txt");
    // Untracked.
    bridge.send(UiCommand::Command("lua Eshvi.picker.git_log_file()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "1");
    assert_eq!(notes(&bridge, &mut ed), "Git history: loose.txt is not tracked by git");
    bridge.send(UiCommand::Command("lua Eshvi.picker.git_log_line()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "2");
    // Added, never committed.
    bridge.send(UiCommand::Command("edit staged.txt".into()));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "staged.txt");
    bridge.send(UiCommand::Command("lua Eshvi.picker.git_log_file()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "3");
    assert!(notes(&bridge, &mut ed).ends_with("Git history: no commits for staged.txt"), "{}", notes(&bridge, &mut ed));
    // No name.
    bridge.send(UiCommand::Command("enew".into()));
    bridge.send(UiCommand::Command("lua Eshvi.picker.git_log_file()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "4");
    assert!(notes(&bridge, &mut ed).ends_with("Git history: this buffer is not a file"));
    no_picker_follows(&bridge, &mut ed);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);

    // Outside any repository.
    let plain = picker_dir("gitlog-norepo");
    std::fs::write(plain.join("x.txt"), "x\n").unwrap();
    let (bridge, mut ed) = history_session(&plain, "x.txt");
    bridge.send(UiCommand::Command("lua Eshvi.picker.git_log_file()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "1");
    assert_eq!(notes(&bridge, &mut ed), "Git history: not in a git repository");
    bridge.send(UiCommand::Command("lua Eshvi.picker.git_log_line()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "2");
    no_picker_follows(&bridge, &mut ed);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(plain);

    // A repository with no commit at all.
    let fresh = picker_dir("gitlog-fresh");
    git_in(&fresh, "Ann", &["init", "-q"]);
    std::fs::write(fresh.join("n.txt"), "x\n").unwrap();
    git_in(&fresh, "Ann", &["add", "n.txt"]);
    let (bridge, mut ed) = history_session(&fresh, "n.txt");
    bridge.send(UiCommand::Command("lua Eshvi.picker.git_log_file()".into()));
    wait_for(&bridge, &mut ed, "#_G.notes", "1");
    assert_eq!(notes(&bridge, &mut ed), "Git history: no commits yet");
    no_picker_follows(&bridge, &mut ed);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(fresh);
}

// ---- The repo's own Neovim config (config/nvim): snacks' picker is off, the picker keys are plain mappings ----

fn config_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/nvim").canonicalize().unwrap()
}

fn lua_files(dir: &std::path::Path, into: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            lua_files(&path, into);
        } else if path.extension().is_some_and(|e| e == "lua") {
            into.push(path);
        }
    }
}

#[test]
fn the_config_has_no_snacks_picker_left_and_loads_its_picker_keys_without_plugins() {
    if !nvim_available() {
        return;
    }
    let config = config_dir();
    let mut files = Vec::new();
    lua_files(&config, &mut files);
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for word in ["Snacks.picker", "Snacks.explorer"] {
            assert!(!text.contains(word), "{} still has {word}", file.display());
        }
    }
    let init = std::fs::read_to_string(config.join("init.lua")).unwrap();
    assert!(init.contains("require(\"picker_keys\")"), "init.lua loads the keys");

    // The keys file, on its own (no plugin manager, no plugins): what it maps.
    let (bridge, mut ed) = picker_session(
        true,
        &["--cmd", &format!("set rtp^={}", config.display()), "--cmd", "let mapleader=' '", "-c", "lua _G.keys = require('picker_keys')"],
    );
    wait_for(&bridge, &mut ed, "type(_G.keys)", "table");
    // (A key given twice is the last one's, as in the snacks spec it came from.)
    let expected: [(&str, &str, &str); 40] = [
        ("n", "<Space><Space>", "Smart Find Files"),
        ("n", "<Space>,", "Buffers"),
        ("n", "<Space>/", "Grep"),
        ("n", "<Space>:", "Command History"),
        ("n", "<Space>fb", "Buffers"),
        ("n", "<Space>fc", "Find Config File"),
        ("n", "<Space>ff", "Find Files"),
        ("n", "<Space>fg", "Find Git Files"),
        ("n", "<Space>fr", "Recent"),
        ("n", "<Space>fe", "File Explorer"),
        ("n", "<Space>gl", "Git Log"),
        ("n", "<Space>gL", "Git Log Line"),
        ("n", "<Space>gs", "Git Status"),
        ("n", "<Space>gd", "Git Diff"),
        ("n", "<Space>gf", "Git Log File"),
        ("n", "<Space>sb", "Buffer Lines"),
        ("n", "<Space>sB", "Grep Open Buffers"),
        ("n", "<Space>sg", "Grep"),
        ("n", "<Space>sw", "Visual selection or word"),
        ("x", "<Space>sw", "Visual selection or word"),
        ("n", "<Space>s\"", "Registers"),
        ("n", "<Space>s/", "Search History"),
        ("n", "<Space>sc", "Command History"),
        ("n", "<Space>sC", "Commands"),
        ("n", "<Space>sd", "Buffer Diagnostics"),
        ("n", "<Space>sh", "Help Pages"),
        ("n", "<Space>sj", "Jumps"),
        ("n", "<Space>sk", "Keymaps"),
        ("n", "<Space>sl", "Location List"),
        ("n", "<Space>sm", "Marks"),
        ("n", "<Space>sq", "Quickfix List"),
        ("n", "<Space>sR", "Resume"),
        ("n", "<Space>su", "Undo History"),
        ("n", "<Space>uC", "Colorschemes"),
        ("n", "gd", "Goto Definition"),
        ("n", "gD", "Goto Declaration"),
        ("n", "gr", "References"),
        ("n", "gI", "Goto Implementation"),
        ("n", "gy", "Goto T[y]pe Definition"),
        ("n", "<Space>ss", "LSP Symbols"),
    ];
    for (mode, lhs, desc) in expected {
        let got = ask(&bridge, &mut ed, &format!("vim.fn.maparg(vim.keycode('{}'), '{mode}', false, true).desc", lhs.replace("<Space>", "<lt>Space>").replace("<lt>Space>", " ").replace('\'', "\\'")));
        assert_eq!(got, desc, "{mode} {lhs}");
    }
    // The ones that go through commands, and `gr`'s nowait.
    for (lhs, rhs) in [("<Space>gs", "<Cmd>EshviGit focus<CR>"), ("<Space>gd", "<Cmd>EshviGit focus<CR>"), ("<Space>gl", "<Cmd>EshviGit graph<CR>"), ("<Space>fe", "<Cmd>EshviExplorer focus<CR>")] {
        let lhs = lhs.replace("<Space>", " ");
        assert_eq!(ask(&bridge, &mut ed, &format!("vim.fn.maparg('{lhs}', 'n')")), rhs, "{lhs:?}");
    }
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.maparg('gr', 'n', false, true).nowait"), "1");
    // What was dropped is not mapped here.
    for lhs in [" fp", " sa", " sH", " si", " sM", " sp", " n"] {
        assert_eq!(ask(&bridge, &mut ed, &format!("vim.fn.maparg('{lhs}', 'n')")), "", "{lhs:?}");
    }
    // snacks keeps lazygit and no more: its spec has the one key and the picker off.
    wait_for(
        &bridge,
        &mut ed,
        &format!("(function() local s = dofile('{}/lua/plugins/snacks.lua') return #s.keys .. s.keys[1][1] .. tostring(s.opts.picker.enabled) end)()", config.display()),
        "1<leader>ggfalse",
    );
    bridge.send(UiCommand::Command("qa!".into()));
}

// ---- The popups health check (float_log.lua, health.lua: `:checkhealth eshvi`) ----

fn health_dir(name: &str) -> std::path::PathBuf {
    // Where `health.lua` is written (stdpath('cache')): not the user's own folders, and the
    // same for every session of this run.
    static CACHE: std::sync::Once = std::sync::Once::new();
    CACHE.call_once(|| {
        let cache = std::env::temp_dir().join(format!("eshvi-health-cache-{}", std::process::id()));
        std::env::set_var("XDG_CACHE_HOME", cache);
    });
    let dir = std::env::temp_dir().join(format!("eshvi-health-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(&dir).unwrap()
}

/// A plugin laid out as lazy.nvim does: `<dir>/lazy/<name>/lua/<module>.lua`. Returns its folder.
fn health_plugin(dir: &std::path::Path, name: &str, module: &str, code: &str) -> std::path::PathBuf {
    let root = dir.join("lazy").join(name);
    std::fs::create_dir_all(root.join("lua")).unwrap();
    std::fs::write(root.join("lua").join(format!("{module}.lua")), code).unwrap();
    root
}

/// A plugin that opens a float: `open(enter, focusable)` calls `nvim_open_win` on its line 3.
/// (Not `return nvim_open_win(…)`: a tail call leaves no frame of the plugin's to find.)
const OPENER: &str = "local M = {}\nfunction M.open(enter, focusable)\n  local win = vim.api.nvim_open_win(vim.api.nvim_create_buf(false, true), enter, { relative = 'editor', row = 1, col = 1, width = 10, height = 2, focusable = focusable })\n  return win\nend\nreturn M\n";

fn health_session(plugins: &[&std::path::Path], args: &[&str], health: bool) -> (Bridge, Editor) {
    let mut all = Vec::new();
    if !args.contains(&"-u") {
        all.push("--clean".to_string());
    }
    all.extend(["-n".to_string(), "-i".to_string(), "NONE".to_string()]);
    for p in plugins {
        all.extend(["--cmd".to_string(), format!("set rtp^={}", p.display())]);
    }
    all.extend(args.iter().map(|s| s.to_string()));
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(all),
            width: 100,
            height: 30,
            health_check: health,
            native_picker: health,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    (bridge, Editor::new())
}

fn count_of(owner: &str) -> String {
    format!("(EshviFloatLog.owners['{owner}'] or {{}}).count")
}

/// Run `:checkhealth eshvi` and read the report's buffer back, once it is complete.
fn health_text(bridge: &Bridge, ed: &mut Editor, dir: &std::path::Path) -> String {
    let out = dir.join("report.txt");
    let _ = std::fs::remove_file(&out);
    bridge.send(UiCommand::Command("checkhealth eshvi".into()));
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last = String::new();
    while Instant::now() < deadline {
        pump_for(bridge, ed, Duration::from_millis(200), |_| false);
        bridge.send(UiCommand::Command(format!(
            "lua for _, b in ipairs(vim.api.nvim_list_bufs()) do if vim.bo[b].filetype == 'checkhealth' then vim.fn.writefile(vim.api.nvim_buf_get_lines(b, 0, -1, false), [[{}]]) end end",
            out.display()
        )));
        pump_for(bridge, ed, Duration::from_millis(200), |_| false);
        if let Ok(text) = std::fs::read_to_string(&out) {
            if text.contains("Could open floats, not seen yet") && text.lines().count() > 8 {
                return text;
            }
            last = text;
        }
    }
    panic!("no report: {last:?} {:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>());
}

fn line_with<'a>(text: &'a str, what: &str) -> &'a str {
    text.lines().find(|l| l.contains(what)).unwrap_or_else(|| panic!("no line with {what:?} in:\n{text}"))
}

#[test]
fn a_plugins_float_is_logged_under_its_name_wherever_it_was_opened_from() {
    if !nvim_available() {
        return;
    }
    let dir = health_dir("log");
    let menu = health_plugin(&dir, "fake-menu", "fake_menu", OPENER);
    let cached = health_plugin(
        &dir,
        "fake-cached",
        "fake_cached",
        "local open = vim.api.nvim_open_win\nreturn { open = function()\n  local win = open(vim.api.nvim_create_buf(false, true), false, { relative = 'editor', row = 1, col = 1, width = 5, height = 1 })\n  return win\nend }\n",
    );
    let preview = health_plugin(
        &dir,
        "fake-preview",
        "fake_preview",
        "return { open = function()\n  local buf = vim.lsp.util.open_floating_preview({ 'hello' }, 'markdown', {})\n  return buf\nend }\n",
    );
    let (bridge, mut ed) = health_session(&[&menu, &cached, &preview], &[], true);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");

    bridge.send(UiCommand::Command("lua require('fake_menu').open(false)".into()));
    wait_for(&bridge, &mut ed, &count_of("fake-menu"), "1");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owners['fake-menu'].file:match('fake_menu.lua$')"), "fake_menu.lua");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owners['fake-menu'].line"), "3");
    assert_eq!(ask(&bridge, &mut ed, "next(EshviFloatLog.owners['fake-menu'].sites)"), format!("{}:3", menu.join("lua/fake_menu.lua").display()));

    // `local open = vim.api.nvim_open_win`, kept when the plugin was loaded: after the wrapper was in.
    bridge.send(UiCommand::Command("lua require('fake_cached').open()".into()));
    wait_for(&bridge, &mut ed, &count_of("fake-cached"), "1");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owners['fake-cached'].line"), "3");

    // A runtime function opening it for the plugin: the plugin's float, not Neovim's.
    bridge.send(UiCommand::Command("lua require('fake_preview').open()".into()));
    wait_for(&bridge, &mut ed, &count_of("fake-preview"), "1");
    assert_eq!(ask(&bridge, &mut ed, &count_of("Neovim")), "nil");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owners['fake-preview'].file:match('fake_preview.lua$')"), "fake_preview.lua");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.unknown"), "0");

    // The wrapper changes nothing: the same results, the same errors (with the caller's position, as
    // when the original is called from there).
    let same = "(function()\n\
        local wrapper = vim.api.nvim_open_win\n\
        local orig\n\
        for i = 1, 20 do local n, v = debug.getupvalue(wrapper, i); if n == 'orig' then orig = v end end\n\
        if not orig then return 'no orig upvalue' end\n\
        local cfg = { relative = 'editor', row = 1, col = 1, width = 4, height = 1 }\n\
        local b = vim.api.nvim_create_buf(false, true)\n\
        local w = wrapper(b, false, cfg)\n\
        local o = orig(b, false, cfg)\n\
        if select('#', wrapper(b, false, cfg)) ~= select('#', orig(b, false, cfg)) then return 'result count' end\n\
        if not (type(w) == type(o) and vim.api.nvim_win_is_valid(w) and vim.api.nvim_win_get_config(w).relative == 'editor') then return 'result' end\n\
        local function err(f, ...) local args = { ... } local ok, e = pcall(function() local x = f(unpack(args)) return x end) return ok, e end\n\
        local ok1, e1 = err(wrapper, 99999, false, cfg)\n\
        local ok2, e2 = err(orig, 99999, false, cfg)\n\
        if ok1 or ok2 or e1 ~= e2 or type(e1) ~= 'string' or not e1:find('Invalid buffer id', 1, true) then return 'error: ' .. tostring(e1) .. ' vs ' .. tostring(e2) end\n\
        local _, a1 = err(wrapper, b)\n\
        local _, a2 = err(orig, b)\n\
        if a1 ~= a2 or not a1:find('Expected 3 arguments', 1, true) then return 'arity: ' .. tostring(a1) .. ' vs ' .. tostring(a2) end\n\
        local _, a3 = pcall(wrapper, 99999, false, cfg)\n\
        local _, a4 = pcall(orig, 99999, false, cfg)\n\
        if a3 ~= a4 then return 'direct: ' .. tostring(a3) .. ' vs ' .. tostring(a4) end\n\
        return 'same'\n\
        end)()";
    assert_eq!(ask(&bridge, &mut ed, &same.replace('\n', " ")), "same");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn core_config_and_app_floats_are_told_apart() {
    if !nvim_available() {
        return;
    }
    let dir = health_dir("owners");
    // The user's config: a file under stdpath('config') (XDG_CONFIG_HOME set before the first float).
    let xdg = dir.join("xdg");
    let conf = xdg.join("nvim");
    std::fs::create_dir_all(&conf).unwrap();
    std::fs::write(
        conf.join("init.lua"),
        "vim.api.nvim_open_win(vim.api.nvim_create_buf(false, true), false, { relative = 'editor', row = 1, col = 1, width = 5, height = 1 })\n",
    )
    .unwrap();
    let init = conf.join("init.lua");
    let (bridge, mut ed) = health_session(
        &[],
        &["--cmd", &format!("lua vim.env.XDG_CONFIG_HOME = [[{}]]", xdg.display()), "-u", init.to_str().unwrap()],
        true,
    );
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    assert_eq!(ask(&bridge, &mut ed, &count_of("your config")), "1");
    assert!(ask(&bridge, &mut ed, "EshviFloatLog.owners['your config'].file").ends_with("nvim/init.lua"));

    // A float Neovim opens by itself (`<C-w>d`, bound in the runtime): only runtime frames.
    bridge.send(UiCommand::Command(
        "lua vim.diagnostic.set(vim.api.nvim_create_namespace('t'), 0, { { lnum = 0, col = 0, message = 'boom' } })".into(),
    ));
    wait_for(&bridge, &mut ed, "#vim.diagnostic.get(0)", "1");
    bridge.send(UiCommand::Input("<C-w>d".into()));
    wait_for(&bridge, &mut ed, &count_of("Neovim"), "1");
    assert_eq!(ask(&bridge, &mut ed, "#vim.tbl_keys(EshviFloatLog.owners)"), "2");

    // Typed with `:lua`: the user's.
    bridge.send(UiCommand::Command(
        "lua vim.api.nvim_open_win(vim.api.nvim_create_buf(false, true), false, { relative = 'editor', row = 3, col = 1, width = 5, height = 1 })".into(),
    ));
    wait_for(&bridge, &mut ed, &count_of("your config"), "2");

    // The app's own chunks (`nvim_exec_lua`, source `<nvim>`) and `lua/gui_bridge`: Eshvi, apart.
    // (What `exec_lua` runs has the source `<nvim>`: picker.lua's own functions do, checked below.)
    bridge.send(UiCommand::Command(
        "lua load([[local win = vim.api.nvim_open_win(vim.api.nvim_create_buf(false, true), false, { relative = 'editor', row = 5, col = 1, width = 5, height = 1 })]], '<nvim>')()".into(),
    ));
    wait_for(&bridge, &mut ed, &count_of("Eshvi"), "1");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owner_of(debug.getinfo(Eshvi.picker.select, 'S').source)"), "Eshvi");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owner_of('@/home/me/projects/vimgui/lua/gui_bridge/init.lua')"), "Eshvi");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owner_of('@/x/pack/p/start/some.nvim/lua/a.lua')"), "some.nvim");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owner_of('@/x/pack/p/opt/other/lua/a.lua')"), "other");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owner_of('@/x/somewhere/else/thing.lua')"), "thing.lua");

    // The report doesn't list Eshvi's.
    let text = health_text(&bridge, &mut ed, &dir);
    assert!(!text.contains("Eshvi:"), "{text}");
    assert!(line_with(&text, "Neovim:").contains("1 popup"), "{text}");
    assert!(line_with(&text, "your config:").contains("2 popups"), "{text}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn vimscript_floats_are_unknown_and_wrapped_ones_are_not_counted_twice() {
    if !nvim_available() {
        return;
    }
    let dir = health_dir("unknown");
    let menu = health_plugin(&dir, "fake-menu", "fake_menu", OPENER);
    let (bridge, mut ed) = health_session(&[&menu], &[], true);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    for _ in 0..3 {
        bridge.send(UiCommand::Command("lua require('fake_menu').open(false)".into()));
    }
    wait_for(&bridge, &mut ed, &count_of("fake-menu"), "3");
    // Give the scheduled checks their turns.
    for _ in 0..5 {
        ask(&bridge, &mut ed, "1");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.unknown"), "0", "wrapped floats counted as unknown");

    bridge.send(UiCommand::Command(
        "call nvim_open_win(bufnr('%'), 0, #{relative: 'editor', row: 5, col: 1, width: 5, height: 1})".into(),
    ));
    wait_for(&bridge, &mut ed, "EshviFloatLog.unknown", "1");
    for _ in 0..5 {
        ask(&bridge, &mut ed, "1");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.unknown"), "1");
    assert_eq!(ask(&bridge, &mut ed, &count_of("fake-menu")), "3");

    // A split isn't a float, and closed floats are forgotten.
    bridge.send(UiCommand::Command("vsplit".into()));
    for _ in 0..4 {
        ask(&bridge, &mut ed, "1");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.unknown"), "1");

    let text = health_text(&bridge, &mut ed, &dir);
    assert!(line_with(&text, "1 float was opened from Vimscript").contains("before the check started"), "{text}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn non_focusable_overlays_are_not_menus() {
    if !nvim_available() {
        return;
    }
    let dir = health_dir("overlay");
    let label = health_plugin(&dir, "fake-label", "fake_label", OPENER);
    let menu = health_plugin(&dir, "fake-picker", "fake_picker", OPENER);
    let (bridge, mut ed) = health_session(&[&label, &menu], &[], true);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    bridge.send(UiCommand::Command("lua require('fake_label').open(false, false)".into()));
    bridge.send(UiCommand::Command("lua require('fake_picker').open(true)".into()));
    wait_for(&bridge, &mut ed, &count_of("fake-label"), "1");
    wait_for(&bridge, &mut ed, &count_of("fake-picker"), "1");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owners['fake-label'].menus .. '/' .. EshviFloatLog.owners['fake-label'].overlays"), "0/1");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.owners['fake-picker'].menus .. '/' .. EshviFloatLog.owners['fake-picker'].overlays"), "1/0");
    let text = health_text(&bridge, &mut ed, &dir);
    let overlay = line_with(&text, "fake-label:");
    assert!(overlay.contains("non-focusable overlays") && !overlay.contains("WARNING"), "{text}");
    // Not a known menu plugin, and it can take focus: something to look at.
    let other = text.lines().position(|l| l.contains("fake-picker:")).unwrap();
    assert!(text.lines().nth(other.saturating_sub(0)).unwrap().contains("fake-picker: 1 popup"), "{text}");
    assert!(text.lines().take(other + 1).any(|l| l.contains("WARNING") && l.contains("fake-picker")), "{text}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn checkhealth_eshvi_names_the_plugins_that_open_menus_and_what_overrides_eshvi() {
    if !nvim_available() {
        return;
    }
    let dir = health_dir("report");
    // A menu plugin by name, a deliberate window, a plugin nobody knows, and one that takes
    // vim.ui.input and vim.notify over (after the config starts, as dressing and notify do).
    let which = health_plugin(&dir, "which-key.nvim", "fake_which", OPENER);
    let lazy = health_plugin(&dir, "lazy.nvim", "fake_lazy", OPENER);
    let strange = health_plugin(&dir, "strange.nvim", "fake_strange", OPENER);
    let dressing = health_plugin(
        &dir,
        "dressing.nvim",
        "fake_dressing",
        "vim.ui.input = function() end\nvim.notify = function() end\nreturn {}\n",
    );
    // Code that could open floats, not run: counted by the code search; and one only in its tests.
    let idle = health_plugin(&dir, "idle.nvim", "idle", "local function f()\n  vim.api.nvim_open_win(0, true, {})\n  vim.lsp.util.open_floating_preview({}, 'markdown')\nend\nreturn {}\n");
    std::fs::create_dir_all(idle.join("tests")).unwrap();
    std::fs::write(idle.join("tests").join("t.lua"), "vim.api.nvim_open_win(0, true, {})\n").unwrap();
    let quiet = health_plugin(&dir, "quiet.nvim", "quiet", "return {}\n");
    let cfg = dir.join("init.lua");
    std::fs::write(&cfg, "require('fake_dressing')\nvim.g.eshvi_health_allow = vim.g.allow\npackage.loaded['blink.cmp'] = {}\npackage.loaded['lualine'] = {}\n").unwrap();
    let (bridge, mut ed) = health_session(
        &[&which, &lazy, &strange, &dressing, &idle, &quiet],
        &["-u", cfg.to_str().unwrap()],
        true,
    );
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    for m in ["fake_which", "fake_which", "fake_lazy", "fake_strange"] {
        bridge.send(UiCommand::Command(format!("lua require('{m}').open(false)")));
    }
    wait_for(&bridge, &mut ed, &count_of("which-key.nvim"), "2");
    wait_for(&bridge, &mut ed, &count_of("strange.nvim"), "1");
    wait_for(&bridge, &mut ed, &count_of("lazy.nvim"), "1");

    let text = health_text(&bridge, &mut ed, &dir);
    assert!(!text.contains("ERROR"), "{text}");
    assert!(!text.contains("failed"), "{text}");
    for section in ["Popups seen in this session", "Menus Eshvi replaces", "Native features", "Could open floats, not seen yet"] {
        assert!(text.contains(section), "{section}\n{text}");
    }
    let pos = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s}\n{text}"));
    assert!(pos("Popups seen") < pos("Menus Eshvi replaces") && pos("Menus Eshvi replaces") < pos("Native features") && pos("Native features") < pos("Could open floats"));

    // Menu plugin: a warning with count and the top site, and what to do about it.
    let w = line_with(&text, "which-key.nvim:");
    assert!(w.contains("WARNING") && w.contains("2 popups") && w.contains("most from lua/fake_which.lua:3") && w.contains("menu plugin"), "{w}");
    assert!(text.contains("vim.g.eshvi_health_allow"), "{text}");
    // An unknown plugin: also a warning. A deliberate window: only info.
    assert!(line_with(&text, "strange.nvim:").contains("WARNING"), "{text}");
    let l = line_with(&text, "lazy.nvim:");
    assert!(!l.contains("WARNING") && l.contains("its own window"), "{l}");
    // Overrides, named.
    assert!(line_with(&text, "vim.ui.input:").contains("dressing") && line_with(&text, "vim.ui.input:").contains("WARNING"), "{text}");
    assert!(line_with(&text, "vim.notify:").contains("WARNING") && line_with(&text, "vim.notify:").contains("fake_dressing.lua") == false, "{text}");
    assert!(line_with(&text, "vim.ui.select:").contains("OK") && line_with(&text, "vim.ui.select:").contains("native picker"), "{text}");
    // Native features, and what is in their way.
    assert!(line_with(&text, "Completion: blink.cmp").contains("WARNING"), "{text}");
    assert!(line_with(&text, "Status bar: lualine").contains("WARNING"), "{text}");
    assert!(line_with(&text, "Status bar: drawn by Eshvi").contains("OK"), "{text}");
    assert!(line_with(&text, "Picker: Eshvi's native picker").contains("OK"), "{text}");
    // The code search: what could open floats and wasn't seen; tests skipped; seen ones left out.
    assert!(line_with(&text, "only what the code could do").contains("nvim_open_win"), "{text}");
    assert!(line_with(&text, "idle.nvim:").contains("2 places"), "{text}");
    assert!(!text.contains("quiet.nvim:"), "{text}");
    assert!(!text.lines().any(|l| l.contains("which-key.nvim:") && l.contains("in its code")), "{text}");

    // The allow list moves it to OK.
    bridge.send(UiCommand::Command("lua vim.g.eshvi_health_allow = { 'which-key.nvim' }".into()));
    let text = health_text(&bridge, &mut ed, &dir);
    let w = line_with(&text, "which-key.nvim:");
    assert!(w.contains("OK") && w.contains("allowed") && !w.contains("WARNING"), "{w}");
    assert!(line_with(&text, "strange.nvim:").contains("WARNING"), "{text}");

    // `:EshviHealth` is the same report.
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.exists(':EshviHealth')"), "2");
    // Nothing the health check does is an error message.
    let bad: Vec<_> = ed.messages.iter().map(|m| m.text()).filter(|t| t.contains("E5108") || t.contains("Error")).collect();
    assert!(bad.is_empty(), "{bad:?}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_code_search_uses_lazys_list_of_loaded_plugins_and_none_is_needed() {
    if !nvim_available() {
        return;
    }
    let dir = health_dir("lazylist");
    let seen_p = health_plugin(&dir, "seen.nvim", "seen_p", OPENER);
    let loaded = health_plugin(&dir, "loaded.nvim", "loaded_p", OPENER);
    let waiting = health_plugin(&dir, "waiting.nvim", "waiting_p", OPENER);
    let cfg = dir.join("init.lua");
    // A stand-in for lazy.nvim: its plugin list, and telescope installed but not loaded.
    std::fs::write(
        &cfg,
        format!(
            "package.loaded['lazy'] = {{ plugins = function() return {{\n\
             {{ name = 'seen.nvim', dir = [[{}]], _ = {{ loaded = {{}} }} }},\n\
             {{ name = 'loaded.nvim', dir = [[{}]], _ = {{ loaded = {{}} }} }},\n\
             {{ name = 'waiting.nvim', dir = [[{}]], _ = {{}} }},\n\
             {{ name = 'telescope.nvim', dir = '/nowhere', _ = {{}} }},\n\
             }} end }}\n",
            seen_p.display(),
            loaded.display(),
            waiting.display()
        ),
    )
    .unwrap();
    let (bridge, mut ed) = health_session(&[&seen_p], &["-u", cfg.to_str().unwrap()], true);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    bridge.send(UiCommand::Command("lua require('seen_p').open(false)".into()));
    wait_for(&bridge, &mut ed, &count_of("seen.nvim"), "1");
    let text = health_text(&bridge, &mut ed, &dir);
    assert!(line_with(&text, "loaded.nvim:").contains("1 place"), "{text}");
    assert!(!text.contains("waiting.nvim:"), "an unloaded plugin is not active\n{text}");
    assert!(!text.lines().any(|l| l.contains("seen.nvim:") && l.contains("in its code")), "{text}");
    let t = line_with(&text, "Picker: telescope.nvim");
    assert!(t.contains("WARNING") && t.contains("not loaded yet"), "{t}");
    bridge.send(UiCommand::Command("qa!".into()));

    // Without lazy.nvim: the runtimepath, and still no error.
    let (bridge, mut ed) = health_session(&[&loaded], &[], true);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    let text = health_text(&bridge, &mut ed, &dir);
    assert!(!text.contains("ERROR"), "{text}");
    assert!(line_with(&text, "loaded.nvim:").contains("1 place"), "{text}");
    assert!(line_with(&text, "No plugin has opened a float since Neovim started").contains("run this again"), "{text}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_opt_out_records_nothing_and_a_shared_config_has_none_of_it() {
    if !nvim_available() {
        return;
    }
    let dir = health_dir("optout");
    let menu = health_plugin(&dir, "fake-menu", "fake_menu", OPENER);
    let cfg = dir.join("init.lua");
    std::fs::write(&cfg, "vim.g.eshvi_health = false\n").unwrap();
    let (bridge, mut ed) = health_session(&[&menu], &["-u", cfg.to_str().unwrap()], true);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    bridge.send(UiCommand::Command("lua require('fake_menu').open(false)".into()));
    bridge.send(UiCommand::Command(
        "call nvim_open_win(bufnr('%'), 0, #{relative: 'editor', row: 5, col: 1, width: 5, height: 1})".into(),
    ));
    wait_for(&bridge, &mut ed, "#vim.api.nvim_list_wins()", "3");
    for _ in 0..5 {
        ask(&bridge, &mut ed, "1");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(ask(&bridge, &mut ed, "next(EshviFloatLog.owners)"), "nil");
    assert_eq!(ask(&bridge, &mut ed, "EshviFloatLog.unknown"), "0");
    let text = health_text(&bridge, &mut ed, &dir);
    assert!(line_with(&text, "Not recording").contains("vim.g.eshvi_health = false"), "{text}");
    assert!(!text.contains("fake-menu: 1 popup"), "recorded while off\n{text}");
    assert!(line_with(&text, "fake-menu:").contains("in its code"), "{text}");
    bridge.send(UiCommand::Command("qa!".into()));

    // Not asked for: no wrapper, no log, no command.
    let (bridge, mut ed) = health_session(&[&menu], &[], false);
    wait_for(&bridge, &mut ed, "vim.v.vim_did_enter", "1");
    assert_eq!(ask(&bridge, &mut ed, "tostring(EshviFloatLog)"), "nil");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.exists(':EshviHealth')"), "0");
    assert_eq!(ask(&bridge, &mut ed, "debug.getinfo(vim.api.nvim_open_win, 'S').what"), "C");
    bridge.send(UiCommand::Command("lua require('fake_menu').open(false)".into()));
    wait_for(&bridge, &mut ed, "#vim.api.nvim_list_wins()", "2");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The Outline and the native tree view ----

use nvim_bridge::gui::{TreeNode, TreeSet};

/// Between the names in an outline node's id.
const SEP: char = '\u{1f}';

/// The next root set of `view` for which `pred` holds.
fn tree_set_of(bridge: &Bridge, ed: &mut Editor, view: &str, pred: impl Fn(&TreeSet) -> bool) -> TreeSet {
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::TreeSet(s) if s.view == view && s.parent.is_none() && pred(s))) {
        Some(GuiEvent::TreeSet(s)) => s,
        _ => panic!("no gui_tree_set of {view} that fits"),
    }
}

fn outline_set(bridge: &Bridge, ed: &mut Editor, pred: impl Fn(&TreeSet) -> bool) -> TreeSet {
    tree_set_of(bridge, ed, "outline", pred)
}

/// The next outline with rows.
fn outline_rows(bridge: &Bridge, ed: &mut Editor) -> TreeSet {
    outline_set(bridge, ed, |s| !s.nodes.is_empty())
}

fn outline_message(bridge: &Bridge, ed: &mut Editor, text: &str) -> TreeSet {
    outline_set(bridge, ed, |s| s.message.as_deref() == Some(text))
}

/// Labels in order, two spaces in for each level.
fn shape(nodes: &[TreeNode]) -> Vec<String> {
    fn walk(nodes: &[TreeNode], depth: usize, out: &mut Vec<String>) {
        for n in nodes {
            out.push(format!("{}{}", "  ".repeat(depth), n.label));
            walk(&n.children, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, &mut out);
    out
}

fn flat_nodes(nodes: &[TreeNode]) -> Vec<&TreeNode> {
    nodes.iter().flat_map(|n| std::iter::once(n).chain(flat_nodes(&n.children))).collect()
}

fn ids(nodes: &[TreeNode]) -> Vec<String> {
    flat_nodes(nodes).iter().map(|n| n.id.clone()).collect()
}

/// A Lua range, lines from 0.
fn rng(l1: usize, c1: usize, l2: usize, c2: usize) -> String {
    format!("{{ start = {{ line = {l1}, character = {c1} }}, ['end'] = {{ line = {l2}, character = {c2} }} }}")
}

/// What the gui messages say over the next `ms`, kept for a look.
fn gui_for(bridge: &Bridge, ed: &mut Editor, ms: u64) -> Vec<GuiEvent> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        while let Some(ev) = bridge.try_recv() {
            match ev {
                BridgeEvent::Redraw(batch) => batch.into_iter().for_each(|e| ed.apply(e)),
                BridgeEvent::Gui(g) => out.push(g),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    out
}

fn selects(events: &[GuiEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|g| match g {
            GuiEvent::TreeSelect { view, id, .. } if view == "outline" => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn tree_accept(bridge: &Bridge, view: &str, id: &str, how: &str) {
    bridge.send(UiCommand::TreeAccept { view: view.into(), id: id.into(), how: how.into() });
}

/// A hierarchical answer on a.lua: module M (mod) with foo and bar (the Hebrew line), then tail.
fn module_symbols() -> String {
    format!(
        "{{ {{ name = 'M', kind = 2, detail = 'mod', range = {r0}, selectionRange = {s0}, children = {{ \
            {{ name = 'foo', kind = 12, range = {r1}, selectionRange = {s1} }}, \
            {{ name = 'bar', kind = 6, range = {r2}, selectionRange = {s2} }} }} }}, \
          {{ name = 'tail', kind = 13, range = {r3}, selectionRange = {s3} }} }}",
        r0 = rng(0, 0, 2, 13),
        s0 = rng(0, 6, 0, 11),
        r1 = rng(1, 0, 1, 19),
        s1 = rng(1, 0, 1, 5),
        r2 = rng(2, 0, 2, 13),
        s2 = rng(2, 8, 2, 13),
        r3 = rng(3, 0, 3, 12),
        s3 = rng(3, 7, 3, 12),
    )
}

/// a.lua with a fake server that has said it has nothing yet, ready for `module_symbols`.
fn outline_session(name: &str) -> (Bridge, Editor, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let (bridge, mut ed, dir, a, b) = lsp_session(name);
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    (bridge, ed, dir, a, b)
}

#[test]
fn the_outline_is_the_servers_symbols_as_a_tree_with_the_names_kinds_and_places() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _a, _b) = outline_session("outline-tree");
    // A server with nothing to say: the message.
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    let set = outline_message(&bridge, &mut ed, "No symbols found");
    assert!(set.nodes.is_empty());
    fake_set(&bridge, &mut ed, format!("lua FAKE.one['textDocument/documentSymbol'] = {}", module_symbols()));
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    let set = outline_rows(&bridge, &mut ed);
    assert_eq!(shape(&set.nodes), ["M", "  foo", "  bar", "tail"]);
    assert_eq!((set.message, set.expand), (None, Some(1)));
    let all = flat_nodes(&set.nodes);
    let kinds: Vec<_> = all.iter().map(|n| n.kind.as_str()).collect();
    assert_eq!(kinds, ["Module", "Function", "Method", "Variable"]);
    assert_eq!(all[0].hint, "mod", "the detail is the hint");
    assert_eq!(all.iter().map(|n| n.line).collect::<Vec<_>>(), [Some(1), Some(2), Some(3), Some(4)], "the name's line");
    assert_eq!(all[0].end_line, Some(3), "the range's end");
    assert_eq!(ids(&set.nodes), ["M".to_string(), format!("M{SEP}foo"), format!("M{SEP}bar"), "tail".to_string()]);
    assert!(set.buf.is_some());
    // Choosing the method: the cursor goes to its name, past the Hebrew (the server counts UTF-16).
    tree_accept(&bridge, "outline", &format!("M{SEP}bar"), "open");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 3, 12 }");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flat_symbols_are_nested_by_the_ranges_they_hold_not_by_their_container_names() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, _b) = outline_session("outline-flat");
    let info = |name: &str, kind: usize, l1: usize, l2: usize, c2: usize| {
        format!(
            "{{ name = '{name}', kind = {kind}, containerName = 'Wrong', location = {{ uri = vim.uri_from_fname('{}'), range = {} }} }}",
            a.display(),
            rng(l1, 0, l2, c2)
        )
    };
    // In no order; Outer holds lines 0 to 2, `tail` is after it.
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['textDocument/documentSymbol'] = {{ {}, {}, {}, {} }}",
        info("inner2", 12, 2, 2, 13),
        info("tail", 13, 3, 3, 12),
        info("Outer", 5, 0, 2, 13),
        info("inner1", 6, 1, 1, 19),
    ));
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    let set = outline_rows(&bridge, &mut ed);
    assert_eq!(shape(&set.nodes), ["Outer", "  inner1", "  inner2", "tail"]);
    assert!(flat_nodes(&set.nodes).iter().all(|n| n.hint.is_empty()), "the container name isn't used");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_refresh_after_an_edit_keeps_the_ids_and_overloads_are_numbered() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _a, _b) = outline_session("outline-ids");
    let twice = format!(
        "lua FAKE.one['textDocument/documentSymbol'] = {{ \
           {{ name = 'f', kind = 12, range = {r0}, selectionRange = {r0} }}, \
           {{ name = 'f', kind = 12, range = {r1}, selectionRange = {r1} }}, \
           {{ name = 'f', kind = 13, range = {r2}, selectionRange = {r2} }} }}",
        r0 = rng(0, 0, 0, 5),
        r1 = rng(1, 0, 1, 5),
        r2 = rng(3, 0, 3, 5)
    );
    fake_set(&bridge, &mut ed, twice);
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    let first = outline_rows(&bridge, &mut ed);
    assert_eq!(ids(&first.nodes), ["f", "f#2", "f"], "the n-th of a name and kind");
    // The same symbols after a line was added at the top: the lines moved, the ids not.
    fake_set(&bridge, &mut ed, format!("lua FAKE.one['textDocument/documentSymbol'] = {}", module_symbols()));
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    let before = outline_rows(&bridge, &mut ed);
    fake_set(&bridge, &mut ed, "lua vim.api.nvim_buf_set_lines(0, 0, 0, false, { '-- new top' })".into());
    // (The server answers as before, ranges a line off: it doesn't matter to the id.)
    let after = outline_set(&bridge, &mut ed, |s| !s.nodes.is_empty());
    assert_eq!(ids(&after.nodes), ids(&before.nodes), "typing doesn't change the ids");
    assert!(after.gen > before.gen, "a new set of rows each time");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_answer_for_older_text_than_the_one_on_show_is_dropped() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _a, _b) = outline_session("outline-stale");
    // The first answer is slow, and about text that has since changed.
    fake_set(&bridge, &mut ed, "lua FAKE.one['textDocument/documentSymbol'] = { delay = 900, value = { { name = 'OLD', kind = 12, range = ".to_string()
        + &rng(0, 0, 0, 5) + ", selectionRange = " + &rng(0, 0, 0, 5) + " } } }");
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    std::thread::sleep(Duration::from_millis(100));
    fake_set(&bridge, &mut ed, "lua vim.api.nvim_buf_set_lines(0, 0, 0, false, { '-- new top' })".into());
    fake_set(&bridge, &mut ed, "lua FAKE.one['textDocument/documentSymbol'] = { { name = 'NEW', kind = 12, range = ".to_string()
        + &rng(0, 0, 0, 5) + ", selectionRange = " + &rng(0, 0, 0, 5) + " } }");
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    let set = outline_set(&bridge, &mut ed, |s| shape(&s.nodes) == ["NEW"]);
    assert_eq!(shape(&set.nodes), ["NEW"]);
    // The slow answer arrives after: nothing shows it.
    let later = gui_for(&bridge, &mut ed, 1500);
    let shown: Vec<_> = later.iter().filter_map(|g| match g { GuiEvent::TreeSet(s) => Some(shape(&s.nodes)), _ => None }).collect();
    assert!(shown.iter().all(|s| s != &["OLD"]), "the stale answer was shown: {shown:?}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_a_server_or_a_file_the_outline_says_so() {
    if !nvim_available() {
        return;
    }
    // No file at all yet.
    let (bridge, mut ed) = picker_session(false, &[]);
    outline_message(&bridge, &mut ed, "No file open");
    bridge.send(UiCommand::Command("qa!".into()));
    // A file no server knows.
    let dir = picker_dir("outline-none");
    let txt = dir.join("plain.txt");
    std::fs::write(&txt, "just text\n").unwrap();
    let (bridge, mut ed) = picker_session(false, &[txt.to_str().unwrap()]);
    let set = outline_message(&bridge, &mut ed, "No outline: no language server for this file");
    assert!(set.nodes.is_empty());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

const OUTLINE_MD: &str = "---\ntitle: x\n# not a heading\n---\n\n# Title\n\n### Skipped level\n\n## Section **bold** and `code` and [link](http://x) and _em_ and snake_case\n\n```sh\n# in code\n```\n\n## Section **bold** and `code` and [link](http://x) and _em_ and snake_case\n\nSetext One\n==========\n\nSetext Two\n----------\n\n# Closed ATX #\n";

#[test]
fn markdown_is_its_headings_with_a_server_or_not() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("outline-md");
    let md = dir.join("n.md");
    std::fs::write(&md, OUTLINE_MD).unwrap();
    let (bridge, mut ed) = picker_session(false, &["--cmd", &format!("cd {}", dir.display()), md.to_str().unwrap()]);
    let set = outline_rows(&bridge, &mut ed);
    let section = "Section bold and code and link and em and snake_case";
    assert_eq!(
        shape(&set.nodes),
        ["Title", "  Skipped level", &format!("  {section}"), &format!("  {section}"), "Setext One", "  Setext Two", "Closed ATX"],
        "atx and setext, nothing from the code block or the front matter; an H3 under an H1 with no H2 is its child"
    );
    let all = flat_nodes(&set.nodes);
    assert_eq!(all.iter().map(|n| n.line.unwrap()).collect::<Vec<_>>(), [6, 8, 10, 16, 18, 21, 24]);
    assert_eq!(all.iter().map(|n| n.kind.as_str()).collect::<Vec<_>>(), ["H1", "H3", "H2", "H2", "H1", "H2", "H1"], "a heading's kind is its level");
    assert_eq!(
        ids(&set.nodes),
        [
            "Title".to_string(),
            format!("Title{SEP}Skipped level"),
            format!("Title{SEP}{section}"),
            format!("Title{SEP}{section}#2"),
            "Setext One".to_string(),
            format!("Setext One{SEP}Setext Two"),
            "Closed ATX".to_string(),
        ],
        "repeats are numbered"
    );
    // Choosing a heading goes to its line.
    tree_accept(&bridge, "outline", &format!("Setext One{SEP}Setext Two"), "open");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 21, 0 }");
    // The same with a server attached: still the headings.
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_picker_lsp.lua");
    bridge.send(UiCommand::Command(format!("luafile {fake}")));
    wait_for(&bridge, &mut ed, "type(fake_lsp)", "function");
    start_fake(&bridge, &mut ed, "one", "{}", 1);
    fake_set(&bridge, &mut ed, format!("lua FAKE.one['textDocument/documentSymbol'] = {}", module_symbols()));
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    let again = outline_rows(&bridge, &mut ed);
    assert_eq!(shape(&again.nodes)[0], "Title");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_cursor_selects_the_innermost_symbol_and_nothing_is_sent_while_it_stays() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _a, _b) = outline_session("outline-follow");
    fake_set(&bridge, &mut ed, format!("lua FAKE.one['textDocument/documentSymbol'] = {}", module_symbols()));
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    outline_rows(&bridge, &mut ed);
    // Inside foo (a function in the module M).
    bridge.send(UiCommand::Command("call cursor(2, 3)".into()));
    let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSelect { id, .. } if id.ends_with("foo")));
    let Some(GuiEvent::TreeSelect { id, path, .. }) = ev else { panic!("no select of foo") };
    assert_eq!((id, path), (format!("M{SEP}foo"), vec!["M".to_string(), format!("M{SEP}foo")]));
    // Elsewhere in foo: the answer is the same, so nothing is sent.
    bridge.send(UiCommand::Command("call cursor(2, 10)".into()));
    assert_eq!(selects(&gui_for(&bridge, &mut ed, 500)), Vec::<String>::new());
    // Into the next one, and the last.
    bridge.send(UiCommand::Command("call cursor(3, 4)".into()));
    let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSelect { .. }));
    assert!(matches!(ev, Some(GuiEvent::TreeSelect { id, .. }) if id == format!("M{SEP}bar")));
    bridge.send(UiCommand::Command("call cursor(4, 1)".into()));
    let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSelect { .. }));
    assert!(matches!(ev, Some(GuiEvent::TreeSelect { id, .. }) if id == "tail"));
    // Follow Cursor off: the cursor moves, the selection stays.
    bridge.send(UiCommand::Command("lua Eshvi.outline.configure({ follow_cursor = false })".into()));
    bridge.send(UiCommand::Command("call cursor(2, 1)".into()));
    assert_eq!(selects(&gui_for(&bridge, &mut ed, 500)), Vec::<String>::new());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn choosing_a_node_moves_the_cursor_in_the_file_window_from_wherever_and_ctrl_o_comes_back() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, a, _b) = outline_session("outline-accept");
    // A long file: `far` is off the screen, `foo` is on it.
    bridge.send(UiCommand::Command("call append(4, repeat(['-- x'], 80)) | call setline(85, 'far()')".into()));
    wait_for(&bridge, &mut ed, "vim.fn.line('$')", "85");
    let far = format!("{{ name = 'far', kind = 12, range = {r}, selectionRange = {r} }}", r = rng(84, 0, 84, 5));
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['textDocument/documentSymbol'] = {{ {{ name = 'foo', kind = 12, range = {r}, selectionRange = {r} }}, {far} }}",
        r = rng(1, 0, 1, 5)
    ));
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    outline_rows(&bridge, &mut ed);
    bridge.send(UiCommand::Command("call cursor(1, 1)".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 1, 0 }");
    let file_win = ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()");
    let top = ask(&bridge, &mut ed, "vim.fn.winsaveview().topline");
    // A terminal window is current (it keeps the last outline); the file's window is another.
    bridge.send(UiCommand::Command("vsplit | terminal".into()));
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "terminal");
    assert_ne!(ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()"), file_win);
    // On the screen: it moves nothing but the cursor.
    tree_accept(&bridge, "outline", "foo", "peek");
    wait_for(&bridge, &mut ed, &format!("vim.api.nvim_get_current_win() == {file_win}"), "true");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 2, 0 }");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.winsaveview().topline"), top, "no scroll for a line in view");
    // Off the screen: centered.
    tree_accept(&bridge, "outline", "far", "open");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 85, 0 }");
    let topline: i64 = ask(&bridge, &mut ed, "vim.fn.winsaveview().topline").parse().unwrap();
    assert!(topline > 60, "the line 85 is in the middle, not at an edge: topline {topline}");
    // The jumplist took where it was.
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 2, 0 }");
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 1, 0 }");
    assert_eq!(ask(&bridge, &mut ed, &format!("vim.fn.expand('%:p') == '{}'", a.display())), "true");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_preview_beside_stands_for_its_file_and_choosing_a_heading_moves_the_files_window() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("outline-side");
    let (md, lua) = (dir.join("n.md"), dir.join("a.lua"));
    std::fs::write(&md, OUTLINE_MD).unwrap();
    std::fs::write(&lua, "local x = 1\n").unwrap();
    let (bridge, mut ed) = picker_session(false, &["--cmd", &format!("cd {}", dir.display()), md.to_str().unwrap()]);
    outline_rows(&bridge, &mut ed);
    let md_win = ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()");
    bridge.send(UiCommand::Command("EshviPreview side".into()));
    wait_for(&bridge, &mut ed, "#vim.api.nvim_list_wins()", "2");
    // Another file in front: the outline is that file's (a Lua file no server knows).
    bridge.send(UiCommand::Command(format!("vsplit {}", lua.display())));
    outline_message(&bridge, &mut ed, "No outline: no language server for this file");
    // The preview window current: its file's headings again.
    bridge.send(UiCommand::Command(
        "lua for _, w in ipairs(vim.api.nvim_list_wins()) do if vim.w[w].eshvi_preview_of then vim.api.nvim_set_current_win(w) end end".into(),
    ));
    let set = outline_rows(&bridge, &mut ed);
    assert_eq!(shape(&set.nodes)[0], "Title");
    // Choosing a heading: the file's window has the cursor, and the keyboard.
    tree_accept(&bridge, "outline", &format!("Setext One{SEP}Setext Two"), "open");
    wait_for(&bridge, &mut ed, &format!("vim.api.nvim_get_current_win() == {md_win}"), "true");
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 21, 0 }");
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.w.eshvi_preview_of)"), "nil", "not the preview's window");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn choosing_a_heading_scrolls_an_in_place_preview_and_leaves_it_shown_and_out_of_insert_mode() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("outline-inplace");
    let md = dir.join("n.md");
    std::fs::write(&md, OUTLINE_MD).unwrap();
    let (bridge, mut ed) = picker_session(false, &["--cmd", &format!("cd {}", dir.display()), md.to_str().unwrap()]);
    outline_rows(&bridge, &mut ed);
    bridge.send(UiCommand::Command("EshviPreview".into()));
    wait_for(&bridge, &mut ed, "tostring(vim.w.eshvi_preview)", "true");
    tree_accept(&bridge, "outline", &format!("Setext One{SEP}Setext Two"), "peek");
    // The preview is told the new line, and is the window's still.
    let shown = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Preview(w) if w.iter().any(|p| p.line == 21 && !p.side)));
    assert!(shown.is_some(), "the preview follows the cursor");
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.w.eshvi_preview)"), "true", "still shown");
    assert_eq!(ask(&bridge, &mut ed, "vim.api.nvim_get_mode().mode"), "n");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn eshvi_outline_commands_reach_the_app() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = picker_session(false, &[]);
    for (command, word) in [("EshviOutline", "toggle"), ("EshviOutline focus", "focus"), ("EshviOutline show", "show"), ("EshviOutline hide", "hide")] {
        bridge.send(UiCommand::Command(command.into()));
        let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Outline(_)));
        assert_eq!(ev, Some(GuiEvent::Outline(word.into())), "{command}");
    }
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn a_source_of_a_configs_own_gives_lazy_children_a_menu_and_what_was_chosen() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = picker_session(false, &[]);
    bridge.send(UiCommand::Command(
        "lua Eshvi.tree.register('fake', { \
           children = function(node, reply) _G.asked = (_G.asked or 0) + 1; vim.defer_fn(function() reply({ { id = node.id .. '/c', label = 'child', kind = 'Field', mine = 7 } }) end, 20) end, \
           accept = function(node, how) _G.chosen = node.id .. ':' .. how .. ':' .. tostring(node.mine) end, \
           menu = function(node) return { { label = 'Copy Path', run = function(n) _G.ran = n.id .. '/' .. tostring(n.mine) end }, { label = 'Other', run = function() _G.other = true end } } end })"
            .into(),
    ));
    bridge.send(UiCommand::Command(
        "lua Eshvi.tree.set('fake', { { id = 'root', label = 'root', hint = 'dim', icon = 'X', hl = 'Normal', tooltip = 'tip', expandable = true, mine = 7 }, { id = 'leaf', label = 'leaf' } }, { expand = 0 })"
            .into(),
    ));
    let set = tree_set_of(&bridge, &mut ed, "fake", |_| true);
    assert_eq!((set.gen, set.expand, set.menu), (1, Some(0), true));
    let root = &set.nodes[0];
    assert_eq!((root.icon.as_str(), root.hl.as_str(), root.tooltip.as_str(), root.expandable), ("X", "Normal", "tip", true));
    assert!(root.fg.is_some(), "the group's foreground comes with it");
    assert!(root.children.is_empty() && !set.nodes[1].expandable);
    // Opened with none yet: they come once, under the gen asked with.
    bridge.send(UiCommand::TreeExpand { view: "fake".into(), id: "root".into(), gen: set.gen });
    let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSet(s) if s.parent.is_some()));
    let Some(GuiEvent::TreeSet(kids)) = ev else { panic!("no children") };
    assert_eq!((kids.parent.as_deref(), kids.gen, shape(&kids.nodes)), (Some("root"), set.gen, vec!["child".to_string()]));
    assert_eq!(kids.nodes[0].kind, "Field");
    // An answer to an old gen carries it, for the app to drop; the nodes are not kept.
    bridge.send(UiCommand::Command("lua Eshvi.tree.set('fake', { { id = 'root', label = 'root', expandable = true } })".into()));
    let newer = tree_set_of(&bridge, &mut ed, "fake", |_| true);
    assert_eq!(newer.gen, 2);
    bridge.send(UiCommand::TreeExpand { view: "fake".into(), id: "root".into(), gen: 1 });
    let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSet(s) if s.parent.is_some()));
    assert!(matches!(ev, Some(GuiEvent::TreeSet(s)) if s.gen == 1), "the old gen goes back with it");
    // What was chosen comes back with the source's own fields.
    tree_accept(&bridge, "fake", "root", "peek");
    wait_for(&bridge, &mut ed, "tostring(_G.chosen)", "root:peek:nil");
    // The menu: its labels, then one run.
    bridge.send(UiCommand::TreeMenu { view: "fake".into(), id: "root".into() });
    let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeMenu { .. }));
    assert_eq!(ev, Some(GuiEvent::TreeMenu { view: "fake".into(), id: "root".into(), items: vec!["Copy Path".into(), "Other".into()] }));
    bridge.send(UiCommand::TreeRun { view: "fake".into(), id: "root".into(), index: 1 });
    wait_for(&bridge, &mut ed, "tostring(_G.ran)", "root/nil");
    assert_eq!(ask(&bridge, &mut ed, "tostring(_G.other)"), "nil", "only the one chosen");
    // Gone.
    bridge.send(UiCommand::Command("lua Eshvi.tree.unregister('fake')".into()));
    let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeUnregister(_)));
    assert_eq!(ev, Some(GuiEvent::TreeUnregister("fake".into())));
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn the_markdown_outline_of_a_long_document_is_quick() {
    if !nvim_available() {
        return;
    }
    // About 4,000 lines and 200 headings, with code blocks (whose `#` lines aren't headings).
    let dir = test_ide_dir("long-markdown");
    std::fs::create_dir_all(&dir).unwrap();
    let mut md = String::from("# Notes\n\n");
    for s in 0..40 {
        md += &format!("## Section {s}\n\nSome text about section {s}, with `code` and a [link](https://example.com).\n\n");
        for t in 0..4 {
            md += &format!("### Part {s}.{t}\n\n- one item\n- another item\n\n```sh\n# not a heading\necho {s}.{t}\n```\n\n");
            md += &"A longer paragraph line that goes on for a while, as notes do.\n".repeat(12);
            md += "\n";
        }
    }
    let notes = dir.join("notes.md");
    std::fs::write(&notes, md).unwrap();
    let (bridge, mut ed) = picker_session(false, &[notes.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.bo.filetype", "markdown");
    bridge.send(UiCommand::Command(
        "lua local t = vim.uv.hrtime(); local h = Eshvi.outline._headings(vim.api.nvim_get_current_buf()); _G.cold = (vim.uv.hrtime() - t) / 1e6; _G.count = #h; \
         t = vim.uv.hrtime(); Eshvi.outline._headings(vim.api.nvim_get_current_buf()); _G.warm = (vim.uv.hrtime() - t) / 1e6"
            .into(),
    ));
    wait_for(&bridge, &mut ed, "type(_G.warm)", "number");
    let (cold, warm, count) = (ask(&bridge, &mut ed, "_G.cold"), ask(&bridge, &mut ed, "_G.warm"), ask(&bridge, &mut ed, "_G.count"));
    let lines = ask(&bridge, &mut ed, "vim.api.nvim_buf_line_count(0)");
    eprintln!("{lines} lines, {count} headings; the first parse {cold} ms, the next {warm} ms");
    assert!(count.parse::<usize>().unwrap() > 40);
    assert!(cold.parse::<f64>().unwrap() < 500.0, "{cold} ms");
    bridge.send(UiCommand::Command("qa!".into()));
}

#[test]
fn a_node_that_was_chosen_stays_the_selection_though_the_cursor_rule_would_pick_its_function() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _a, _b) = outline_session("outline-chosen");
    // `outer(a, b)`: the parameters are variables inside a function, which the cursor rule skips.
    let (o, a) = (rng(0, 0, 0, 15), rng(0, 6, 0, 7));
    fake_set(&bridge, &mut ed, format!(
        "lua FAKE.one['textDocument/documentSymbol'] = {{ {{ name = 'outer', kind = 12, range = {o}, selectionRange = {o}, children = {{ \
           {{ name = 'a', kind = 13, range = {a}, selectionRange = {a} }} }} }} }}"
    ));
    bridge.send(UiCommand::Command("lua Eshvi.outline.refresh()".into()));
    outline_rows(&bridge, &mut ed);
    tree_accept(&bridge, "outline", &format!("outer{SEP}a"), "peek");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 1, 6 }");
    let later = gui_for(&bridge, &mut ed, 600);
    assert_eq!(selects(&later), Vec::<String>::new(), "the selection was moved off the node chosen");
    // Moving the cursor away and back by hand: the rule applies again, and picks the function.
    bridge.send(UiCommand::Command("call cursor(1, 2)".into()));
    let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSelect { .. }));
    assert!(matches!(ev, Some(GuiEvent::TreeSelect { id, .. }) if id == "outer"));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Side bar tree views, and slang-server's hierarchy (crates/nvim-bridge/src/slang/hierarchy.lua) ----

use nvim_bridge::gui::TreeRegister;

/// A session with `top.sv` (40 lines) open, the app's slang support on (the shipped modules: no
/// `config/nvim` on the runtime path), a clipboard that keeps what is copied to it, the fake servers
/// loaded. It makes no view until a server attaches.
fn slang_session(name: &str) -> (Bridge, Editor, std::path::PathBuf, std::path::PathBuf) {
    slang_session_with(name, false)
}

/// `slang_session`, with the native picker on if `native` (for a test that waits for one).
fn slang_session_with(name: &str, native: bool) -> (Bridge, Editor, std::path::PathBuf, std::path::PathBuf) {
    let dir = picker_dir(name);
    let file = dir.join("top.sv");
    let text: String = (0..40).map(|i| format!("// line {i}\n")).collect();
    std::fs::write(&file, text).unwrap();
    let args = vec![
        "--cmd".to_string(),
        format!("cd {}", dir.display()),
        "--cmd".to_string(),
        "lua vim.g.clipboard = { name = 't', copy = { ['+'] = function(l) _G.clip = table.concat(l, '\\n') end }, paste = { ['+'] = function() return { vim.split(_G.clip or '', '\\n'), 'v' } end } }"
            .to_string(),
        file.to_str().unwrap().to_string(),
    ];
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (bridge, mut ed) = picker_session_with(native, true, &args);
    wait_for(&bridge, &mut ed, "vim.fn.bufname('%') ~= ''", "true");
    for lua in ["fake_picker_lsp.lua", "fake_slang.lua"] {
        let path = format!("{}/tests/{lua}", env!("CARGO_MANIFEST_DIR"));
        bridge.send(UiCommand::Command(format!("luafile {path}")));
    }
    wait_for(&bridge, &mut ed, "type(fake_slang)", "function");
    bridge.send(UiCommand::Command("lua fake_watch()".into()));
    wait_for(&bridge, &mut ed, "type(_G.notes)", "table");
    // (Set up by the app after attach.)
    wait_for(&bridge, &mut ed, "tostring(package.loaded['eshvi.slang'] ~= nil)", "true");
    (bridge, ed, dir, file)
}

/// Start the fake slang-server for the current buffer. (It doesn't wait for it to attach: the
/// gui messages it makes would be eaten by the polling; the test waits for them.)
fn attach_slang(bridge: &Bridge, _ed: &mut Editor) {
    bridge.send(UiCommand::Command("lua fake_slang()".into()));
}

/// The next `gui_tree_register` of `view`.
fn tree_register_of(bridge: &Bridge, ed: &mut Editor, view: &str) -> TreeRegister {
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::TreeRegister(r) if r.view == view)) {
        Some(GuiEvent::TreeRegister(r)) => r,
        _ => panic!("no gui_tree_register of {view}"),
    }
}

/// The children of `parent` in `view` (`None`: the roots), once the app has been asked (`TreeExpand`) or
/// told by an update.
fn slang_children(bridge: &Bridge, ed: &mut Editor, id: &str, gen: i64) -> TreeSet {
    bridge.send(UiCommand::TreeExpand { view: "slang".into(), id: id.into(), gen });
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::TreeSet(s) if s.view == "slang" && s.parent.as_deref() == Some(id))) {
        Some(GuiEvent::TreeSet(s)) => s,
        _ => panic!("no children of {id}"),
    }
}

fn slang_roots(bridge: &Bridge, ed: &mut Editor) -> TreeSet {
    tree_set_of(bridge, ed, "slang", |s| !s.nodes.is_empty())
}

fn node_of<'a>(nodes: &'a [TreeNode], id: &str) -> &'a TreeNode {
    nodes.iter().find(|n| n.id == id).unwrap_or_else(|| panic!("no node {id} in {:?}", ids(nodes)))
}

#[test]
fn the_hierarchy_view_comes_with_a_slang_server_and_goes_when_the_last_buffer_lets_go_of_it() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, file) = slang_session("slang-attach");
    // Before any server: no view, so no button leading nowhere.
    let seen = gui_for(&bridge, &mut ed, 300);
    assert!(!seen.iter().any(|g| matches!(g, GuiEvent::TreeRegister(r) if r.view == "slang")), "{seen:?}");
    // Another server (a language server that isn't slang) doesn't make it.
    bridge.send(UiCommand::Command("lua fake_lsp('other', {})".into()));
    wait_for(&bridge, &mut ed, "#vim.lsp.get_clients({ name = 'other', bufnr = 0 })", "1");
    assert!(!gui_for(&bridge, &mut ed, 300).iter().any(|g| matches!(g, GuiEvent::TreeRegister(_))));
    attach_slang(&bridge, &mut ed);
    let reg = tree_register_of(&bridge, &mut ed, "slang");
    let side = reg.side.expect("a view of its own in the side bar");
    assert_eq!((side.title.as_str(), side.icon.as_str(), side.tooltip.as_str()), ("HIERARCHY", "type-hierarchy", "Hierarchy"));
    assert_eq!(side.buttons.iter().map(|b| (b.icon.as_str(), b.tooltip.as_str())).collect::<Vec<_>>(), [("refresh", "Refresh")]);
    assert_eq!(reg.keys, ["/", "y"]);
    // A second buffer on the same server; letting go of one leaves the view, of both takes it.
    let second = dir.join("second.sv");
    std::fs::write(&second, "x\n").unwrap();
    bridge.send(UiCommand::Command(format!("split {}", second.display())));
    bridge.send(UiCommand::Command("lua fake_slang()".into()));
    wait_for(&bridge, &mut ed, "vim.tbl_count(vim.lsp.get_clients({ name = 'slang' })[1].attached_buffers)", "2");
    bridge.send(UiCommand::Command(format!(
        "lua vim.lsp.buf_detach_client(vim.fn.bufnr('{}'), vim.lsp.get_clients({{ name = 'slang' }})[1].id)",
        second.display()
    )));
    wait_for(&bridge, &mut ed, "vim.tbl_count(vim.lsp.get_clients({ name = 'slang' })[1].attached_buffers)", "1");
    let seen = gui_for(&bridge, &mut ed, 400);
    assert!(!seen.iter().any(|g| matches!(g, GuiEvent::TreeUnregister(_))), "one buffer still has the server: {seen:?}");
    bridge.send(UiCommand::Command(format!(
        "lua vim.lsp.buf_detach_client(vim.fn.bufnr('{}'), vim.lsp.get_clients({{ name = 'slang' }})[1].id)",
        file.display()
    )));
    let gone = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeUnregister(v) if v == "slang"));
    assert!(gone.is_some(), "the last buffer let go: the view goes");
    // Attached again: back.
    bridge.send(UiCommand::Command("lua fake_slang()".into()));
    tree_register_of(&bridge, &mut ed, "slang");
    // The server stopped: the view goes too.
    bridge.send(UiCommand::Command("lua vim.lsp.get_clients({ name = 'slang' })[1]:stop()".into()));
    let gone = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeUnregister(v) if v == "slang"));
    assert!(gone.is_some(), "a stopped server takes the view");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// What the design says, as the tests below check it.
fn the_hierarchy_nodes_ids_icons_hints_and_lazy_children(name: &str) {
    let (bridge, mut ed, dir, _file) = slang_session(name);
    attach_slang(&bridge, &mut ed);
    let set = slang_roots(&bridge, &mut ed);
    // Roots: the first step of getScopes(""), server order.
    assert_eq!(ids(&set.nodes), ["top", "pkg"]);
    assert_eq!((set.expand, set.menu, set.message.clone()), (Some(1), true, None));
    let (top, pkg) = (&set.nodes[0], &set.nodes[1]);
    assert_eq!((top.label.as_str(), top.hint.as_str(), top.expandable), ("top", "top_mod", true), "an instance's hint is its declaration");
    assert_eq!((top.hl.as_str(), top.icon.is_empty(), top.fg.is_some()), ("SlangServerInstance", false, true));
    assert_eq!((pkg.hint.as_str(), pkg.hl.as_str(), pkg.expandable), ("", "SlangServerPackage", true));
    assert!(top.children.is_empty(), "children come lazily");
    // top's children: through getScope with its path.
    let kids = slang_children(&bridge, &mut ed, "top", set.gen);
    assert_eq!(ids(&kids.nodes), ["top.clk", "top.q", "top.io", "top.WIDTH", "top.u"]);
    let n = |id: &str| node_of(&kids.nodes, id);
    // Ports by their direction; the hint is the type.
    assert_eq!((n("top.clk").hl.as_str(), n("top.clk").hint.as_str()), ("SlangServerPortInput", "input logic"));
    assert_eq!(n("top.q").hl, "SlangServerPortOutput");
    assert_eq!(n("top.io").hl, "SlangServerPortInout");
    assert!(!n("top.clk").expandable);
    // A param: its value is the tooltip, as `name = value`; an instance array: its declaration, expandable.
    assert_eq!((n("top.WIDTH").hint.as_str(), n("top.WIDTH").tooltip.as_str(), n("top.WIDTH").hl.as_str()), ("int", "WIDTH = 8", "SlangServerParam"));
    assert_eq!((n("top.u").hint.as_str(), n("top.u").expandable, n("top.u").hl.as_str()), ("leaf_mod", true, "SlangServerInstanceArray"));
    assert_eq!(n("top.clk").tooltip, "input logic", "no value: the type");
    // The path of an array element: `[0]` joins without a dot.
    let elements = slang_children(&bridge, &mut ed, "top.u", set.gen);
    assert_eq!(ids(&elements.nodes), ["top.u[0]", "top.u[1]"]);
    assert_eq!(elements.nodes[0].label, "[0]");
    // And a package's child with `::`.
    let package = slang_children(&bridge, &mut ed, "pkg", set.gen);
    assert_eq!(ids(&package.nodes), ["pkg::x"]);
    assert_eq!(package.nodes[0].tooltip, "x = 3");
    // The server was asked with those paths.
    let asked = ask(&bridge, &mut ed, "vim.json.encode(vim.tbl_map(function(c) return c.params.command .. ':' .. vim.inspect(c.params.arguments) end, vim.tbl_filter(function(c) return c.method == 'workspace/executeCommand' end, FAKE_CALLS)))");
    for want in ["slang.getScopes:{ \\\"\\\" }", "slang.getScope:{ \\\"top\\\" }", "slang.getScope:{ \\\"top.u\\\" }", "slang.getScope:{ \\\"pkg\\\" }"] {
        assert!(asked.contains(want), "{want} not in {asked}");
    }
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// The `TreeRegister` of the slang view, waiting for it.
fn slang_register(bridge: &Bridge, ed: &mut Editor) -> nvim_bridge::gui::TreeRegister {
    match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::TreeRegister(r) if r.view == "slang")) {
        Some(GuiEvent::TreeRegister(r)) => r,
        other => panic!("no register: {other:?}"),
    }
}

#[test]
fn the_hierarchy_filters_group_every_kind_and_the_choice_is_remembered_for_the_next_time() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("slang-filter");
    // (Its own state folder: the real one isn't for tests.)
    bridge.send(UiCommand::Command(format!("lua vim.env.XDG_STATE_HOME = '{}/state'", dir.display())));
    wait_for(&bridge, &mut ed, "vim.env.XDG_STATE_HOME ~= nil", "true");
    attach_slang(&bridge, &mut ed);
    let reg = slang_register(&bridge, &mut ed);
    let side = reg.side.expect("a side view");
    let filters: Vec<_> = side.filters.iter().map(|f| (f.id.as_str(), f.label.as_str(), f.hidden)).collect();
    assert_eq!(filters, [
        ("instance", "Instances", false),
        ("scope", "Scopes", false),
        ("interface", "Interface Ports", false),
        ("package", "Packages", false),
        ("port", "Ports", false),
        ("param", "Parameters", false),
        ("signal", "Signals", false),
    ]);
    // Every kind the fake server has answers to one.
    let roots = slang_roots(&bridge, &mut ed);
    assert_eq!(node_of(&roots.nodes, "top").group, "instance");
    assert_eq!(node_of(&roots.nodes, "pkg").group, "package");
    let top = slang_children(&bridge, &mut ed, "top", roots.gen);
    assert_eq!(node_of(&top.nodes, "top.clk").group, "port");
    assert_eq!(node_of(&top.nodes, "top.WIDTH").group, "param");
    assert_eq!(node_of(&top.nodes, "top.u").group, "instance");
    let groups = |nodes: &[TreeNode]| nodes.iter().all(|n| !n.group.is_empty());
    assert!(groups(&roots.nodes) && groups(&top.nodes), "{:?}", top.nodes);

    // Toggled: remembered, so a view registered later starts that way; toggled back, it isn't.
    bridge.send(UiCommand::TreeFilter { view: "slang".into(), group: "port".into(), shown: false });
    bridge.send(UiCommand::TreeFilter { view: "slang".into(), group: "signal".into(), shown: false });
    bridge.send(UiCommand::TreeFilter { view: "slang".into(), group: "signal".into(), shown: true });
    bridge.send(UiCommand::TreeFilter { view: "slang".into(), group: "param".into(), shown: false });
    wait_for(&bridge, &mut ed, "vim.fn.filereadable(vim.fn.stdpath('state') .. '/eshvi_tree_filters.json')", "1");
    wait_for(
        &bridge,
        &mut ed,
        "table.concat(vim.json.decode(table.concat(vim.fn.readfile(vim.fn.stdpath('state') .. '/eshvi_tree_filters.json'))).slang, ',')",
        "param,port",
    );
    bridge.send(UiCommand::Command(
        "lua Eshvi.tree.register('slang', { side = { title = 'H', filters = { { id = 'port', label = 'P' }, { id = 'signal', label = 'S', hidden = true } } } })".into(),
    ));
    let again = slang_register(&bridge, &mut ed).side.expect("a side view");
    let hidden: Vec<_> = again.filters.iter().map(|f| (f.id.as_str(), f.hidden)).collect();
    assert_eq!(hidden, [("port", true), ("signal", false)], "the choice beats the default");
    // A filter the view didn't have when the choice was saved starts with its own default, hidden or not; one it
    // had and the user showed stays shown.
    bridge.send(UiCommand::Command(
        "lua Eshvi.tree.register('slang', { side = { title = 'H', filters = { { id = 'port', label = 'P' }, { id = 'signal', label = 'S', hidden = true }, { id = 'clock', label = 'C', hidden = true }, { id = 'other', label = 'O' } } } })".into(),
    ));
    let added = slang_register(&bridge, &mut ed).side.expect("a side view");
    let hidden: Vec<_> = added.filters.iter().map(|f| (f.id.as_str(), f.hidden)).collect();
    assert_eq!(hidden, [("port", true), ("signal", false), ("clock", true), ("other", false)]);
    // A file from before `_known`: what it switched off stays off, the rest take their defaults.
    bridge.send(UiCommand::Command(
        "lua vim.fn.writefile({ '{\"slang\":[\"port\"]}' }, vim.fn.stdpath('state') .. '/eshvi_tree_filters.json'); \
         Eshvi.tree.register('slang', { side = { title = 'H', filters = { { id = 'port', label = 'P' }, { id = 'clock', label = 'C', hidden = true } } } })"
            .into(),
    ));
    let old = slang_register(&bridge, &mut ed).side.expect("a side view");
    let hidden: Vec<_> = old.filters.iter().map(|f| (f.id.as_str(), f.hidden)).collect();
    assert_eq!(hidden, [("port", true), ("clock", true)]);
}

#[test]
fn the_hierarchy_nodes_have_ids_icons_hints_tooltips_and_lazy_children() {
    if !nvim_available() {
        return;
    }
    the_hierarchy_nodes_ids_icons_hints_and_lazy_children("slang-nodes");
}

#[test]
fn choosing_an_instance_sets_it_active_then_opens_its_own_module_in_the_editor_window() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, file) = slang_session("slang-accept");
    attach_slang(&bridge, &mut ed);
    let set = slang_roots(&bridge, &mut ed);
    // (The nodes the app knows, opened as it would.)
    slang_children(&bridge, &mut ed, "top", set.gen);
    slang_children(&bridge, &mut ed, "top.u", set.gen);
    let editor_win = ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()");
    // The terminal panel is current, as when the view was clicked from it.
    bridge.send(UiCommand::Command("vsplit | terminal".into()));
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "terminal");
    assert_ne!(ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()"), editor_win);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    tree_accept(&bridge, "slang", "top.u[0]", "open");
    // Its own module (the declaration, line 20 from 0) opens in the editor's window, not the terminal's: not the
    // instantiation in the parent (line 5), which is the menu's "Go to Instantiation".
    wait_for(&bridge, &mut ed, &format!("vim.api.nvim_get_current_win() == {editor_win}"), "true");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 21, 4 }");
    assert_eq!(ask(&bridge, &mut ed, &format!("vim.fn.expand('%:p') == '{}'", file.display())), "true");
    assert_eq!(ask(&bridge, &mut ed, "vim.bo[vim.fn.bufnr('term://*')].buftype"), "terminal", "the terminal is as it was");
    // Only the active instance is asked of the server (and the active path is kept for `up`).
    // (`vim.inspect` sorts the keys; `vim.json.encode` doesn't.)
    let calls = ask(&bridge, &mut ed, "table.concat(vim.tbl_map(function(c) return c.params.command .. ' ' .. vim.inspect(c.params.arguments, { newline = '', indent = '' }) end, vim.tbl_filter(function(c) return c.method == 'workspace/executeCommand' end, FAKE_CALLS)), ' | ')");
    assert_eq!(
        calls,
        r#"slang.setActiveInstance { "top.u[0]" }"#
    );
    assert_eq!(ask(&bridge, &mut ed, "require('eshvi.slang.hierarchy').active_path()"), "top.u[0]");
    // A peek does the same (the top's module is at line 10).
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    tree_accept(&bridge, "slang", "top", "peek");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 11, 4 }");
    // Anything that isn't an instance keeps the server showing its place (the array, line 5).
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    tree_accept(&bridge, "slang", "top.u", "open");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 6, 4 }");
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.showHierLocation"), 1);
    // The server's error is the message, as the plugin shows it.
    bridge.send(UiCommand::Command("lua SLANG.fail = 'no such instance'; _G.notes = {}".into()));
    tree_accept(&bridge, "slang", "top.u[1]", "open");
    wait_for(&bridge, &mut ed, "tostring(vim.tbl_contains(_G.notes, 'no such instance|4'))", "true");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_menu_copies_the_path_the_value_and_the_file_and_goes_to_the_declaration_and_the_instantiation() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, file) = slang_session("slang-menu");
    attach_slang(&bridge, &mut ed);
    let set = slang_roots(&bridge, &mut ed);
    slang_children(&bridge, &mut ed, "top", set.gen);
    slang_children(&bridge, &mut ed, "top.u", set.gen);
    let menu = |bridge: &Bridge, ed: &mut Editor, id: &str| -> Vec<String> {
        bridge.send(UiCommand::TreeMenu { view: "slang".into(), id: id.into() });
        match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::TreeMenu { id: i, .. } if i == id)) {
            Some(GuiEvent::TreeMenu { items, .. }) => items,
            _ => panic!("no menu for {id}"),
        }
    };
    assert_eq!(menu(&bridge, &mut ed, "top.WIDTH"), ["Copy Path", "Copy Value", "Copy File Path"], "a param has a value");
    assert_eq!(menu(&bridge, &mut ed, "top.clk"), ["Copy Path", "Copy File Path", "Trace Drivers", "Trace Loads"], "a port can be traced");
    assert_eq!(menu(&bridge, &mut ed, "pkg"), ["Copy Path", "Copy File Path"]);
    let items = menu(&bridge, &mut ed, "top.u[0]");
    assert_eq!(items, ["Copy Path", "Copy File Path", "Go to Declaration", "Go to Instantiation"], "an instance has both places");
    // Copy Path: the register, and a toast that says so.
    bridge.send(UiCommand::Command("lua _G.notes = {}".into()));
    bridge.send(UiCommand::TreeRun { view: "slang".into(), id: "top.u[0]".into(), index: 1 });
    wait_for(&bridge, &mut ed, "tostring(_G.clip)", "top.u[0]");
    wait_for(&bridge, &mut ed, "tostring(vim.tbl_contains(_G.notes, 'Yanked top.u[0]|2'))", "true");
    // Copy Value, and Copy File Path.
    menu(&bridge, &mut ed, "top.WIDTH");
    bridge.send(UiCommand::TreeRun { view: "slang".into(), id: "top.WIDTH".into(), index: 2 });
    wait_for(&bridge, &mut ed, "tostring(_G.clip)", "8");
    menu(&bridge, &mut ed, "top.WIDTH");
    bridge.send(UiCommand::TreeRun { view: "slang".into(), id: "top.WIDTH".into(), index: 3 });
    wait_for(&bridge, &mut ed, &format!("tostring(_G.clip == '{}')", file.display()), "true");
    // Go to Declaration, from a terminal: the editor's window, the jumplist marked.
    let editor_win = ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()");
    bridge.send(UiCommand::Command("call cursor(3, 1) | vsplit | terminal".into()));
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "terminal");
    menu(&bridge, &mut ed, "top.u[0]");
    bridge.send(UiCommand::TreeRun { view: "slang".into(), id: "top.u[0]".into(), index: 3 });
    // The keyboard goes to the editor after it (the copies keep it in the view).
    let told = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeShow { action, .. } if action == "editor"));
    assert!(told.is_some(), "Go to Declaration hands the keyboard to the editor");
    wait_for(&bridge, &mut ed, &format!("vim.api.nvim_get_current_win() == {editor_win}"), "true");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 21, 4 }");
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 3, 0 }");
    // Go to Instantiation: where it is instantiated in the parent (line 5), the keyboard to the editor too.
    menu(&bridge, &mut ed, "top.u[0]");
    bridge.send(UiCommand::TreeRun { view: "slang".into(), id: "top.u[0]".into(), index: 4 });
    let told = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeShow { action, .. } if action == "editor"));
    assert!(told.is_some(), "Go to Instantiation hands the keyboard to the editor");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 6, 4 }");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn up_opens_the_active_instances_instantiation_and_makes_the_instance_around_it_active() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session_with("slang-up", true);
    attach_slang(&bridge, &mut ed);
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'slang' }))", "1");
    let hier = "require('eshvi.slang.hierarchy')";
    let set_active = |bridge: &Bridge, path: &str| bridge.send(UiCommand::Command(format!("lua {hier}.set_active('{path}', function() end)")));
    let calls = |bridge: &Bridge, ed: &mut Editor| {
        ask(bridge, ed, "table.concat(vim.tbl_map(function(c) return c.params.command .. ' ' .. vim.inspect(c.params.arguments, { newline = '', indent = '' }) end, vim.tbl_filter(function(c) return c.method == 'workspace/executeCommand' and c.params.command:find('Active', 1, true) end, FAKE_CALLS)), ' | ')")
    };
    // The active instance is in this module's file: its instantiation (line 5) opens, the parent is active.
    set_active(&bridge, "top.u[0]");
    wait_for(&bridge, &mut ed, &format!("tostring({hier}.active_path())"), "top.u[0]");
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; vim.cmd('call cursor(30, 1)')".into()));
    bridge.send(UiCommand::Command("EshviSlang up".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 6, 4 }");
    assert_eq!(calls(&bridge, &mut ed), r#"slang.setActiveInstance { "top" }"#);
    assert_eq!(ask(&bridge, &mut ed, &format!("{hier}.active_path()")), "top");
    // The jump is in the jumplist.
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 30, 0 }");
    // A top level has nothing above it.
    bridge.send(UiCommand::Command("lua _G.notes = {}".into()));
    bridge.send(UiCommand::Command("EshviSlang up".into()));
    wait_for(&bridge, &mut ed, "tostring(#_G.notes)", "1");
    assert!(ask(&bridge, &mut ed, "_G.notes[1]").contains("nothing above it"));
    // Generate blocks are skipped: an instance in `top.g` goes to `top`.
    bridge.send(UiCommand::Command(
        "lua local f = vim.api.nvim_buf_get_name(0); \
         local function at(l) return { uri = vim.uri_from_fname(f), range = { start = { line = l, character = 4 }, ['end'] = { line = l, character = 9 } } } end; \
         table.insert(SLANG.scopes['top'], { kind = 'Scope', instName = 'g' }); \
         SLANG.scopes['top.g'] = { { kind = 'Instance', instName = 'w', declName = 'w_mod', instLoc = at(7), declLoc = at(25) } }; \
         SLANG.prefixes = { ['top.g.w'] = { 'top', 'top.g', 'top.g.w' } }"
            .into(),
    ));
    set_active(&bridge, "top.g.w");
    wait_for(&bridge, &mut ed, &format!("tostring({hier}.active_path())"), "top.g.w");
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; vim.cmd('call cursor(30, 1)')".into()));
    bridge.send(UiCommand::Command("EshviSlang up".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 8, 4 }");
    assert_eq!(calls(&bridge, &mut ed), r#"slang.setActiveInstance { "top" }"#);
    // No active instance: which instance of the module is asked (the module of the file: its `getModulesInFile`),
    // then the same.
    bridge.send(UiCommand::Command(format!("lua {hier}.active_paths = {{}}; _G.FAKE_CALLS = {{}}; vim.cmd('call cursor(30, 1)')")));
    bridge.send(UiCommand::Command("EshviSlang up".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Go up from which instance?");
    assert_eq!(texts(&open), ["top.u[0]", "top.u[1]"]);
    accept(&bridge, open.id, vec![by_idx(2)], "open");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 6, 4 }");
    assert_eq!(calls(&bridge, &mut ed), r#"slang.setActiveInstance { "top" }"#);
    assert_eq!(calls_of(&bridge, &mut ed, "slang", "slang.getInstancesOfModule"), [r#"["leaf_mod"]"#]);
    // A module with no instance: a warning.
    bridge.send(UiCommand::Command(format!("lua {hier}.active_paths = {{}}; _G.notes = {{}}; SLANG.modules = {{ 'lonely' }}")));
    bridge.send(UiCommand::Command("EshviSlang up".into()));
    wait_for(&bridge, &mut ed, "tostring(#_G.notes)", "1");
    assert!(ask(&bridge, &mut ed, "_G.notes[1]").starts_with("No instances of lonely"));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// Everything the gui was told over `ms` that concerns the `slang` view's rows, in order.
fn slang_sets(events: &[GuiEvent]) -> Vec<&TreeSet> {
    events.iter().filter_map(|g| if let GuiEvent::TreeSet(s) = g { Some(s).filter(|s| s.view == "slang") } else { None }).collect()
}

fn slang_selects(events: &[GuiEvent]) -> Vec<(String, Vec<String>)> {
    events
        .iter()
        .filter_map(|g| match g {
            GuiEvent::TreeSelect { view, id, path } if view == "slang" => Some((id.clone(), path.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn revealing_a_deep_path_pushes_each_level_under_its_parent_and_selects_the_last() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("slang-reveal");
    attach_slang(&bridge, &mut ed);
    slang_roots(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 200);
    bridge.send(UiCommand::Command("lua require('eshvi.slang.hierarchy').reveal('top.u[0].d', { focus = true })".into()));
    let seen = gui_for(&bridge, &mut ed, 700);
    // The view is shown with the keyboard, first.
    assert!(
        matches!(seen.iter().find(|g| matches!(g, GuiEvent::TreeShow { .. })), Some(GuiEvent::TreeShow { view, action, focus }) if view == "slang" && action == "show" && *focus),
        "{seen:?}"
    );
    // Each level, merged, under its parent: the roots, top's, the array's, the element's.
    let sets = slang_sets(&seen);
    assert!(sets.iter().all(|s| s.merge), "pushed unasked, merged");
    let parents: Vec<_> = sets.iter().map(|s| s.parent.clone()).collect();
    assert_eq!(parents, [None, Some("top".into()), Some("top.u".into()), Some("top.u[0]".into()), Some("top.u[0].d".into())]);
    assert_eq!(ids(&sets[2].nodes), ["top.u[0]", "top.u[1]"]);
    assert_eq!(ids(&sets[3].nodes), ["top.u[0].d", "top.u[0].r"]);
    // The last is selected, with its ancestors.
    assert_eq!(slang_selects(&seen), [("top.u[0].d".to_string(), vec!["top".into(), "top.u".into(), "top.u[0]".into(), "top.u[0].d".into()])]);
    // A package member: `::`.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.hierarchy').reveal('pkg::x', {})".into()));
    let seen = gui_for(&bridge, &mut ed, 600);
    assert_eq!(slang_selects(&seen), [("pkg::x".to_string(), vec!["pkg".into(), "pkg::x".into()])]);
    // Without `select`, nothing is selected (the nodes are still pushed).
    bridge.send(UiCommand::Command("lua require('eshvi.slang.hierarchy').reveal('top.u[1]', { select = false })".into()));
    let seen = gui_for(&bridge, &mut ed, 600);
    assert!(slang_selects(&seen).is_empty() && !slang_sets(&seen).is_empty());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_path_that_does_not_resolve_selects_the_deepest_found_and_warns_and_a_newer_reveal_wins() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("slang-partial");
    attach_slang(&bridge, &mut ed);
    slang_roots(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 200);
    bridge.send(UiCommand::Command("lua require('eshvi.slang.hierarchy').reveal('top.nope.x', {})".into()));
    let seen = gui_for(&bridge, &mut ed, 600);
    assert_eq!(slang_selects(&seen), [("top".to_string(), vec!["top".to_string()])], "the deepest found");
    let warned = ask(&bridge, &mut ed, "vim.inspect(_G.notes[#_G.notes])");
    assert_eq!(warned, r#""Could not resolve hierarchy path 'top.nope.x' at 'nope' (last resolved: 'top')|3""#);
    // Two reveals in a row, the server slow: only the second shows.
    bridge.send(UiCommand::Command("lua SLANG.delay = 150".into()));
    bridge.send(UiCommand::Command("lua require('eshvi.slang.hierarchy').reveal('pkg::x', {}); require('eshvi.slang.hierarchy').reveal('top.u[1]', {})".into()));
    let seen = gui_for(&bridge, &mut ed, 1200);
    let selected = slang_selects(&seen);
    assert_eq!(selected.len(), 1, "{selected:?}");
    assert_eq!(selected[0].0, "top.u[1]");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_the_server_says_about_the_active_instance_is_recorded_and_revealed_without_taking_focus() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("slang-active");
    attach_slang(&bridge, &mut ed);
    slang_roots(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 200);
    // (Lenses on in this buffer: the handler refreshes them, without the plugin's deprecated call.)
    bridge.send(UiCommand::Command("lua vim.lsp.codelens.enable(true); _G.notes = {}".into()));
    bridge.send(UiCommand::Command("lua FAKE_DISPATCH.slang.notification('slang/activeInstanceChanged', { hierPath = 'top.u[1]' })".into()));
    let seen = gui_for(&bridge, &mut ed, 700);
    assert_eq!(slang_selects(&seen).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["top.u[1]"]);
    // What the server starts doesn't switch the side bar: no show at all.
    assert!(!seen.iter().any(|g| matches!(g, GuiEvent::TreeShow { .. })), "{seen:?}");
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.tbl_contains(vim.tbl_map(function(n) return n:find('deprecated') ~= nil end, _G.notes), true))"), "false", "no deprecation warning");
    // Recorded: where `<leader>vh` finds it.
    assert_eq!(
        ask(&bridge, &mut ed, "require('eshvi.slang.hierarchy').active_paths[vim.lsp.get_clients({ name = 'slang' })[1].id]"),
        "top.u[1]"
    );
    // A code lens's command reveals as well.
    bridge.send(UiCommand::Command(
        "lua vim.lsp.commands['slang.showInHierarchy']({ command = 'slang.showInHierarchy', arguments = { { hierPath = 'pkg::x' } } }, {})".into(),
    ));
    let seen = gui_for(&bridge, &mut ed, 700);
    assert_eq!(slang_selects(&seen).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["pkg::x"]);
    assert!(!seen.iter().any(|g| matches!(g, GuiEvent::TreeShow { .. })), "a code lens doesn't switch the side bar either");
    // <leader>vh: with the keyboard, on the active instance.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.hierarchy').open()".into()));
    let seen = gui_for(&bridge, &mut ed, 700);
    assert!(seen.iter().any(|g| matches!(g, GuiEvent::TreeShow { focus, .. } if *focus)));
    assert_eq!(slang_selects(&seen).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["top.u[1]"]);
    // With no view (the server let go), the server's words do nothing here.
    bridge.send(UiCommand::Command("lua vim.lsp.get_clients({ name = 'slang' })[1]:stop()".into()));
    wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeUnregister(_))).expect("the view goes");
    bridge.send(UiCommand::Command("lua vim.lsp.handlers['slang/activeInstanceChanged'](nil, { hierPath = 'top' }, { client_id = 99 })".into()));
    let seen = gui_for(&bridge, &mut ed, 400);
    assert!(slang_sets(&seen).is_empty() && slang_selects(&seen).is_empty(), "{seen:?}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_server_error_is_the_message_with_the_refresh_button_to_try_again_which_keeps_what_was_loaded() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("slang-error");
    bridge.send(UiCommand::Command("lua SLANG.fail = 'no build file'".into()));
    attach_slang(&bridge, &mut ed);
    let set = tree_set_of(&bridge, &mut ed, "slang", |s| s.message.is_some());
    assert_eq!((set.message.as_deref(), set.nodes.len()), (Some("No design: no build file"), 0));
    // The design is there now; the refresh button (the first, from 1) loads it.
    bridge.send(UiCommand::Command("lua SLANG.fail = nil".into()));
    bridge.send(UiCommand::TreeButton { view: "slang".into(), index: 1, selected: None, expanded: vec!["top".into()] });
    let seen = gui_for(&bridge, &mut ed, 700);
    let roots: Vec<_> = slang_sets(&seen).into_iter().filter(|s| s.parent.is_none() && !s.nodes.is_empty()).collect();
    assert!(!roots.is_empty(), "{seen:?}");
    assert_eq!(ids(&roots[0].nodes), ["top", "pkg"]);
    // Load some children, change the design, refresh: the roots, then the children, merged.
    let gen = roots[0].gen;
    slang_children(&bridge, &mut ed, "top", gen);
    slang_children(&bridge, &mut ed, "pkg", gen);
    bridge.send(UiCommand::Command("lua SLANG.scopes['top'][4].value = '9'; SLANG.scopes[''][3] = { kind = 'Instance', instName = 'extra', declName = 'e', instLoc = {} }".into()));
    gui_for(&bridge, &mut ed, 100);
    bridge.send(UiCommand::TreeButton { view: "slang".into(), index: 1, selected: None, expanded: vec!["top".into()] });
    let seen = gui_for(&bridge, &mut ed, 700);
    let sets = slang_sets(&seen);
    assert!(sets.iter().all(|s| s.merge), "a refresh merges: nothing folds");
    assert_eq!(ids(&sets[0].nodes), ["top", "pkg", "extra"]);
    let top = sets.iter().find(|s| s.parent.as_deref() == Some("top")).expect("top's children again");
    assert_eq!(node_of(&top.nodes, "top.WIDTH").tooltip, "WIDTH = 9");
    // `pkg` was loaded but is closed (the app said only `top` is open): its children are dropped by an
    // update with none, and not asked for again.
    let pkg = sets.iter().find(|s| s.parent.as_deref() == Some("pkg")).expect("pkg's children dropped");
    assert!(pkg.nodes.is_empty() && pkg.merge);
    let asked = ask(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'slang.getScope' and c.params.arguments[1] == 'pkg' end, FAKE_CALLS))");
    assert_eq!(asked, "1", "pkg's getScope was asked only when it was opened, not by the refresh");
    // A refresh that fails says so (and keeps no rows: the app has the old ones to show until then).
    bridge.send(UiCommand::Command("lua SLANG.fail = 'server gone'".into()));
    bridge.send(UiCommand::TreeButton { view: "slang".into(), index: 1, selected: None, expanded: vec!["top".into()] });
    let set = tree_set_of(&bridge, &mut ed, "slang", |s| s.message.is_some());
    assert_eq!(set.message.as_deref(), Some("No design: server gone"));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_tree_api_round_trips_a_side_view_with_buttons_keys_updates_show_hide_and_the_command() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed) = picker_session(false, &[]);
    bridge.send(UiCommand::Command(
        "lua Eshvi.tree.register('fake', { \
           side = { title = 'FAKE', icon = 'refresh', tooltip = 'Fake', buttons = { \
             { icon = 'refresh', tooltip = 'One', run = function(ctx) _G.pressed = (_G.pressed or '') .. '1'; _G.ctx = (ctx.selected and ctx.selected.id or '-') .. ':' .. table.concat(ctx.expanded, ',') end }, \
             { icon = 'nope', tooltip = 'Two', run = function() _G.pressed = (_G.pressed or '') .. '2' end } } }, \
           keys = { { key = '/', desc = 'Search', run = function(node) _G.keyed = (_G.keyed or '') .. '/' .. (node and node.id or '-') end }, \
                    { key = 'j', desc = 'taken', run = function() _G.bad = true end }, \
                    { key = 'y', desc = 'Yank', run = function(node) _G.keyed = (_G.keyed or '') .. 'y' .. (node and node.mine or '-') end } }, \
           children = function(node, reply) reply({}) end, \
           menu = function(node) return { { label = 'Go', editor = true, run = function() _G.went = true end }, { label = 'Copy', run = function() _G.copied = true end } } end, \
           accept = function(node, how) _G.chosen = node.id .. ':' .. how end })"
            .into(),
    ));
    let reg = tree_register_of(&bridge, &mut ed, "fake");
    let side = reg.side.expect("side");
    assert_eq!((side.title.as_str(), side.icon.as_str()), ("FAKE", "refresh"));
    assert_eq!(side.buttons.iter().map(|b| b.icon.as_str()).collect::<Vec<_>>(), ["refresh", "nope"], "an unknown icon name is the app's to draw");
    assert_eq!(reg.keys, ["/", "y"], "the tree's own keys can't be taken");
    // Buttons: by place, each its own `run`.
    let press = |index: usize, selected: Option<&str>, expanded: &[&str]| UiCommand::TreeButton {
        view: "fake".into(),
        index,
        selected: selected.map(str::to_owned),
        expanded: expanded.iter().map(|s| s.to_string()).collect(),
    };
    bridge.send(press(2, None, &[]));
    bridge.send(press(1, None, &[]));
    bridge.send(press(3, None, &[]));
    wait_for(&bridge, &mut ed, "tostring(_G.pressed)", "21");
    bridge.send(press(1, None, &["x", "y"]));
    wait_for(&bridge, &mut ed, "tostring(_G.ctx)", "-:x,y");
    // Keys: the node comes along, or nil; one that wasn't registered does nothing.
    bridge.send(UiCommand::Command("lua Eshvi.tree.set('fake', { { id = 'a', label = 'a', mine = 5 } })".into()));
    tree_set_of(&bridge, &mut ed, "fake", |_| true);
    bridge.send(UiCommand::TreeKey { view: "fake".into(), id: None, key: "/".into() });
    bridge.send(UiCommand::TreeKey { view: "fake".into(), id: Some("a".into()), key: "y".into() });
    bridge.send(UiCommand::TreeKey { view: "fake".into(), id: Some("a".into()), key: "j".into() });
    bridge.send(UiCommand::TreeKey { view: "fake".into(), id: Some("a".into()), key: "/".into() });
    wait_for(&bridge, &mut ed, "tostring(_G.keyed)", "/-y5/a");
    assert_eq!(ask(&bridge, &mut ed, "tostring(_G.bad)"), "nil");
    // A menu item with `editor` tells the app to give the keyboard to the editor after it runs; one without doesn't.
    gui_for(&bridge, &mut ed, 100);
    for (index, editor) in [(2usize, false), (1, true)] {
        bridge.send(UiCommand::TreeMenu { view: "fake".into(), id: "a".into() });
        wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeMenu { .. })).expect("the menu");
        bridge.send(UiCommand::TreeRun { view: "fake".into(), id: "a".into(), index });
        let seen = gui_for(&bridge, &mut ed, 300);
        let told = seen.iter().any(|g| matches!(g, GuiEvent::TreeShow { action, .. } if action == "editor"));
        assert_eq!(told, editor, "item {index}: {seen:?}");
    }
    assert_eq!(ask(&bridge, &mut ed, "tostring(_G.went) .. tostring(_G.copied)"), "truetrue");
    // Update: the children of `a` pushed unasked, merged, then selected inside them.
    bridge.send(UiCommand::Command(
        "lua Eshvi.tree.update('fake', 'a', { { id = 'a/b', label = 'b', mine = 6, children = { { id = 'a/b/c', label = 'c' } } } }); Eshvi.tree.select('fake', 'a/b/c')".into(),
    ));
    let seen = gui_for(&bridge, &mut ed, 400);
    let pushed = slang_free_sets(&seen, "fake");
    assert_eq!(pushed.len(), 1);
    assert_eq!((pushed[0].parent.as_deref(), pushed[0].merge, pushed[0].gen, ids(&pushed[0].nodes)), (Some("a"), true, 1, vec!["a/b".to_string(), "a/b/c".to_string()]));
    assert!(seen.iter().any(|g| matches!(g, GuiEvent::TreeSelect { id, path, .. } if id == "a/b/c" && path == &["a", "a/b", "a/b/c"])));
    // What was pushed is what is chosen: with the source's own fields.
    tree_accept(&bridge, "fake", "a/b", "open");
    wait_for(&bridge, &mut ed, "tostring(_G.chosen)", "a/b:open");
    // The roots updated: the old children stay for a node the update doesn't give any for, and `a` is kept.
    bridge.send(UiCommand::Command("lua Eshvi.tree.update('fake', nil, { { id = 'a', label = 'A' }, { id = 'z', label = 'z' } })".into()));
    let seen = gui_for(&bridge, &mut ed, 300);
    let roots = slang_free_sets(&seen, "fake");
    assert_eq!((roots[0].parent.clone(), roots[0].merge, ids(&roots[0].nodes)), (None, true, vec!["a".to_string(), "z".to_string()]));
    tree_accept(&bridge, "fake", "a/b", "peek");
    wait_for(&bridge, &mut ed, "tostring(_G.chosen)", "a/b:peek");
    // Show and hide, and the command: the app is told, nothing else happens here.
    bridge.send(UiCommand::Command("lua Eshvi.tree.show('fake', { focus = true }); Eshvi.tree.show('fake'); Eshvi.tree.hide('fake')".into()));
    let seen = gui_for(&bridge, &mut ed, 300);
    let shows: Vec<_> = seen.iter().filter_map(|g| if let GuiEvent::TreeShow { action, focus, .. } = g { Some((action.as_str(), *focus)) } else { None }).collect();
    assert_eq!(shows, [("show", true), ("show", false), ("hide", false)]);
    for (command, want) in [("EshviTree show fake", "show"), ("EshviTree focus fake", "focus"), ("EshviTree hide fake", "hide"), ("EshviTree toggle fake", "toggle")] {
        bridge.send(UiCommand::Command(command.into()));
        let ev = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeShow { .. }));
        assert_eq!(ev, Some(GuiEvent::TreeShow { view: "fake".into(), action: want.into(), focus: false }), "{command}");
    }
    // A view that isn't there: nothing.
    bridge.send(UiCommand::Command("EshviTree toggle nothing | lua Eshvi.tree.show('nothing')".into()));
    assert!(!gui_for(&bridge, &mut ed, 300).iter().any(|g| matches!(g, GuiEvent::TreeShow { .. })));
    // The view completes by name.
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.fn.getcompletion('EshviTree toggle f', 'cmdline'))"), r#"{ "fake" }"#);
    bridge.send(UiCommand::Command("qa!".into()));
}

/// The sets of `view` the gui was sent.
fn slang_free_sets<'a>(events: &'a [GuiEvent], view: &str) -> Vec<&'a TreeSet> {
    events.iter().filter_map(|g| if let GuiEvent::TreeSet(s) = g { Some(s).filter(|s| s.view == view) } else { None }).collect()
}

// ---- The signal trace (crates/nvim-bridge/src/slang/trace.lua) ----

/// `slang_session`, which sets the trace up too (it makes no view before a server attaches).
fn trace_session(name: &str) -> (Bridge, Editor, std::path::PathBuf, std::path::PathBuf) {
    trace_session_with(name, false)
}

fn trace_session_with(name: &str, native: bool) -> (Bridge, Editor, std::path::PathBuf, std::path::PathBuf) {
    let (bridge, ed, dir, file) = slang_session_with(name, native);
    (bridge, ed, dir, file)
}

/// The fake server attached and the trace view registered.
fn trace_ready(bridge: &Bridge, ed: &mut Editor) -> TreeRegister {
    attach_slang(bridge, ed);
    tree_register_of(bridge, ed, "slang_trace")
}

/// Trace `path` and wait for the tree.
fn trace_start(bridge: &Bridge, ed: &mut Editor, path: &str, direction: &str) -> TreeSet {
    bridge.send(UiCommand::Command(format!("lua require('eshvi.slang.trace').trace_path('{path}', '{direction}')")));
    tree_set_of(bridge, ed, "slang_trace", |s| !s.nodes.is_empty())
}

fn trace_sets(events: &[GuiEvent]) -> Vec<&TreeSet> {
    slang_free_sets(events, "slang_trace")
}

/// Any node of the tree, by id.
fn at<'a>(nodes: &'a [TreeNode], id: &str) -> &'a TreeNode {
    flat_nodes(nodes).into_iter().find(|n| n.id == id).unwrap_or_else(|| panic!("no node {id} in {:?}", ids(nodes)))
}

/// Each command the server was sent, with its arguments, in order.
fn server_calls(bridge: &Bridge, ed: &mut Editor) -> String {
    ask(bridge, ed, "table.concat(vim.tbl_map(function(c) return c.params.command .. ' ' .. vim.inspect(c.params.arguments, { newline = '', indent = '' }) end, vim.tbl_filter(function(c) return c.method == 'workspace/executeCommand' end, FAKE_CALLS)), ' | ')")
}

#[test]
fn the_trace_view_comes_after_the_hierarchy_with_a_slang_server_and_goes_with_it() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, file) = trace_session("trace-attach");
    let seen = gui_for(&bridge, &mut ed, 300);
    assert!(!seen.iter().any(|g| matches!(g, GuiEvent::TreeRegister(_))), "no server, no view");
    attach_slang(&bridge, &mut ed);
    let seen = gui_for(&bridge, &mut ed, 800);
    let registered: Vec<_> = seen.iter().filter_map(|g| if let GuiEvent::TreeRegister(r) = g { Some(r) } else { None }).collect();
    assert_eq!(registered.iter().map(|r| r.view.as_str()).collect::<Vec<_>>(), ["slang", "slang_trace"], "the button comes after the hierarchy's");
    let reg = registered[1];
    let side = reg.side.as_ref().expect("a view of its own in the side bar");
    assert_eq!((side.title.as_str(), side.icon.as_str(), side.tooltip.as_str()), ("TRACE", "references", "Signal Trace"));
    assert_eq!(
        side.buttons.iter().map(|b| (b.icon.as_str(), b.tooltip.as_str())).collect::<Vec<_>>(),
        [("arrow-swap", "Switch Drivers / Loads"), ("refresh", "Refresh")]
    );
    assert_eq!(side.filters.iter().map(|f| (f.id.as_str(), f.label.as_str(), f.hidden)).collect::<Vec<_>>(), [("constant", "Constants", true), ("control", "Control Signals", false), ("clock", "Clock Signals", true)]);
    assert_eq!(reg.keys, ["s", "y"]);
    let empty = trace_sets(&seen).into_iter().find(|s| s.message.is_some()).expect("the empty view's message");
    assert_eq!((empty.message.as_deref(), empty.nodes.len()), (Some("Press <leader>vt on a signal"), 0));
    // Letting go of the last buffer takes both views.
    bridge.send(UiCommand::Command(format!(
        "lua vim.lsp.buf_detach_client(vim.fn.bufnr('{}'), vim.lsp.get_clients({{ name = 'slang' }})[1].id)",
        file.display()
    )));
    let seen = gui_for(&bridge, &mut ed, 600);
    let gone: Vec<_> = seen.iter().filter_map(|g| if let GuiEvent::TreeUnregister(v) = g { Some(v.as_str()) } else { None }).collect();
    assert!(gone.contains(&"slang_trace") && gone.contains(&"slang"), "{gone:?}");
    // Back with the server, and a stopped server takes it again.
    bridge.send(UiCommand::Command("lua fake_slang()".into()));
    tree_register_of(&bridge, &mut ed, "slang_trace");
    bridge.send(UiCommand::Command("lua vim.lsp.get_clients({ name = 'slang' })[1]:stop()".into()));
    let gone = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeUnregister(v) if v == "slang_trace"));
    assert!(gone.is_some(), "a stopped server takes the trace view");
    // With no view, a trace does nothing.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').trace_path('top.q', 'drivers')".into()));
    assert!(trace_sets(&gui_for(&bridge, &mut ed, 300)).is_empty());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_trace_from_the_cursor_asks_the_server_for_the_instances_of_the_symbol_there() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-cursor");
    trace_ready(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    // One instance: that signal's drivers, the view shown with the keyboard.
    bridge.send(UiCommand::Command("call cursor(3, 1) | lua require('eshvi.slang.trace').trace('drivers')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(set.nodes[0].id, "top.q");
    assert_eq!(set.nodes[0].hint, "Drivers \u{b7} 1");
    let shown = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeShow { view, .. } if view == "slang_trace"));
    assert_eq!(shown, Some(GuiEvent::TreeShow { view: "slang_trace".into(), action: "show".into(), focus: true }));
    let calls = server_calls(&bridge, &mut ed);
    assert!(calls.contains("slang.getInstances { {position = {character = 0,line = 2},textDocument = {uri = \"file://"), "{calls}");
    // Loads, with the other key.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').trace('loads')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!((set.nodes[0].id.as_str(), set.nodes[0].hint.as_str()), ("top.q", "Loads \u{b7} 2"));
    // Not a signal: a warning, and the view stays as it was.
    bridge.send(UiCommand::Command("lua _G.notes = {}; vim.fn.cursor(1, 1); require('eshvi.slang.trace').trace('drivers')".into()));
    wait_for(&bridge, &mut ed, "tostring(vim.tbl_contains(_G.notes, 'Not a signal in the current design|3'))", "true");
    assert!(trace_sets(&gui_for(&bridge, &mut ed, 300)).is_empty());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn several_instances_pick_the_one_under_the_active_instance_or_ask() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session_with("trace-several", true);
    trace_ready(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    // With an active instance that one of them is under: no question.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.hierarchy').active_paths[vim.lsp.get_clients({ name = 'slang' })[1].id] = 'top.u[1]'".into()));
    bridge.send(UiCommand::Command("call cursor(4, 1) | lua require('eshvi.slang.trace').trace('drivers')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(set.nodes[0].id, "top.u[1].d");
    // An active instance none is under, or none at all: the picker.
    for active in ["'top.nope'", "nil"] {
        bridge.send(UiCommand::Command(format!("lua require('eshvi.slang.hierarchy').active_paths[vim.lsp.get_clients({{ name = 'slang' }})[1].id] = {active}")));
        bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').trace('drivers')".into()));
        let open = next_picker(&bridge, &mut ed);
        assert_eq!((open.title.as_str(), texts(&open)), ("Trace which instance?", vec!["top.u[0].d", "top.u[1].d"]));
        accept(&bridge, open.id, vec![by_idx(1)], "edit");
        let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
        assert_eq!((set.nodes[0].id.as_str(), set.nodes[0].children[0].id.as_str()), ("top.u[0].d", "top.u[0].d>top.clk"));
    }
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_root_is_the_signal_its_children_its_drivers_with_ids_labels_hints_and_counts_and_peeking_decides_the_chevron() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-rows");
    trace_ready(&bridge, &mut ed);
    // Loads of q: two rows, not asked ahead (it can crash the server: `asks_ahead`), so each may have more.
    let set = trace_start(&bridge, &mut ed, "top.q", "loads");
    assert_eq!((set.expand, set.menu, set.message.clone()), (Some(1), true, None));
    let root = &set.nodes[0];
    assert_eq!((root.id.as_str(), root.label.as_str(), root.hint.as_str(), root.tooltip.as_str(), root.open), ("top.q", "q", "Loads \u{b7} 2", "top.q", true));
    assert_eq!(ids(&set.nodes), ["top.q", "top.q>top.out0", "top.q>top.out1"]);
    let out0 = at(&set.nodes, "top.q>top.out0");
    assert_eq!((out0.label.as_str(), out0.hint.as_str(), out0.tooltip.as_str(), out0.expandable, out0.open), ("out0", "top", "top.out0", true, false), "not asked ahead: a chevron, no count");
    assert!(!server_calls(&bridge, &mut ed).contains("getLoadsWithLocation { \"top.out0\" }"), "loads aren't asked ahead");
    // With a server that has the fix, they are: nothing further, so no chevron.
    bridge.send(UiCommand::Command("let g:slang_loads_safe = v:true".into()));
    let set = trace_start(&bridge, &mut ed, "top.q", "loads");
    assert!(!at(&set.nodes, "top.q>top.out0").expandable, "asked ahead with a fixed server");
    bridge.send(UiCommand::Command("let g:slang_loads_safe = v:false".into()));
    assert_eq!(out0.group, "");
    assert!(out0.fg.is_some() && !out0.icon.is_empty() && out0.hl == "SlangServerLogic", "a signal as the hierarchy draws one: {out0:?}");
    // Drivers of q: the chain a, b arrives open; c has two steps and stops, with its count.
    let set = trace_start(&bridge, &mut ed, "top.q", "drivers");
    assert_eq!(ids(&set.nodes), ["top.q", "top.q>top.a", "top.q>top.a>top.b", "top.q>top.a>top.b>top.c"]);
    let (a, b, c) = (at(&set.nodes, "top.q>top.a"), at(&set.nodes, "top.q>top.a>top.b"), at(&set.nodes, "top.q>top.a>top.b>top.c"));
    assert_eq!((a.open, b.open, c.open), (true, true, false), "a chain of single steps is followed and shown open");
    assert_eq!((a.hint.as_str(), c.hint.as_str(), c.expandable), ("1 \u{b7} top", "2 \u{b7} top", true));
    assert!(c.children.is_empty());
    // The place of the row is kept on the node, for accepting (not sent: line only).
    assert_eq!(a.line, Some(12));
    // c asked to open: its answer came with the peek, not asked again; x has nothing, y goes on by itself past the constant.
    let before = server_calls(&bridge, &mut ed).matches("getDriversWithLocation { \"top.c\" }").count();
    bridge.send(UiCommand::TreeExpand { view: "slang_trace".into(), id: c.id.clone(), gen: set.gen });
    let kids = match wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSet(s) if s.view == "slang_trace" && s.parent.as_deref() == Some(c.id.as_str()))) {
        Some(GuiEvent::TreeSet(s)) => s,
        _ => panic!("no children of c"),
    };
    let base = "top.q>top.a>top.b>top.c";
    assert_eq!(
        ids(&kids.nodes),
        [format!("{base}>top.x"), format!("{base}>top.y"), format!("{base}>top.y>pkg::e.K"), format!("{base}>top.y>top.z"), format!("{base}>top.y>top.z>top.y")]
    );
    let after = server_calls(&bridge, &mut ed).matches("getDriversWithLocation { \"top.c\" }").count();
    assert_eq!((before, after), (1, 1), "the answer kept from the peek is used");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_row_that_is_its_own_ancestor_is_a_loop_and_a_constant_has_its_group_and_does_not_count_as_a_step() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-loop");
    trace_ready(&bridge, &mut ed);
    // y is driven by z and by a package enum's member; z is driven by y.
    let set = trace_start(&bridge, &mut ed, "top.y", "drivers");
    assert_eq!(ids(&set.nodes), ["top.y", "top.y>pkg::e.K", "top.y>top.z", "top.y>top.z>top.y"]);
    let (k, z, again) = (at(&set.nodes, "top.y>pkg::e.K"), at(&set.nodes, "top.y>top.z"), at(&set.nodes, "top.y>top.z>top.y"));
    assert_eq!((k.group.as_str(), k.expandable, k.label.as_str()), ("constant", false, "K"));
    assert_ne!(k.hl, z.hl, "a constant has a colour of its own");
    assert_eq!(z.open, true, "z is followed: its constant sibling is not counted as a step");
    assert_eq!(z.group, "");
    assert_eq!((again.hint.as_str(), again.expandable, again.open), ("loop \u{b7} top", false, false), "the chain stops, marked");
    // The constant was not asked about (it would be an error).
    let calls = server_calls(&bridge, &mut ed);
    assert!(!calls.contains("pkg::e.K"), "{calls}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn many_rows_in_two_instances_or_more_are_grouped_by_instance_not_at_eight_and_not_in_one() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-groups");
    trace_ready(&bridge, &mut ed);
    // 9 rows in 2 instances: the instances, sorted, with their counts; the rows inside, closed.
    let set = trace_start(&bridge, &mut ed, "top.big9", "drivers");
    assert_eq!(set.nodes[0].hint, "Drivers \u{b7} 9");
    let groups = &set.nodes[0].children;
    assert_eq!(groups.iter().map(|g| (g.id.as_str(), g.label.as_str(), g.hint.as_str(), g.group.as_str(), g.open)).collect::<Vec<_>>(), [
        ("top.big9>@top.m0", "m0", "4", "", false),
        ("top.big9>@top.m1", "m1", "5", "", false),
    ]);
    assert_eq!(groups[0].children.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), [
        "top.big9>top.m0.s2",
        "top.big9>top.m0.s4",
        "top.big9>top.m0.s6",
        "top.big9>top.m0.s8"
    ]);
    assert_eq!(groups[0].children[0].hint, "m0");
    // Accepting a group does nothing; it has no menu items.
    bridge.send(UiCommand::Command("lua _G.notes = {}".into()));
    let before = server_calls(&bridge, &mut ed);
    tree_accept(&bridge, "slang_trace", "top.big9>@top.m0", "open");
    bridge.send(UiCommand::TreeMenu { view: "slang_trace".into(), id: "top.big9>@top.m0".into() });
    let menu = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeMenu { .. }));
    assert!(matches!(menu, Some(GuiEvent::TreeMenu { items, .. }) if items.is_empty()));
    assert_eq!(server_calls(&bridge, &mut ed), before);
    // Constants aren't an instance: they sit after the groups, and don't make a list span two instances.
    let set = trace_start(&bridge, &mut ed, "top.mix9", "drivers");
    assert_eq!(set.nodes[0].hint, "Drivers \u{b7} 9");
    assert_eq!(
        set.nodes[0].children.iter().map(|g| (g.label.as_str(), g.group.as_str(), g.folder)).collect::<Vec<_>>(),
        [("m0", "", true), ("m1", "", true), ("K", "constant", false)]
    );
    let set = trace_start(&bridge, &mut ed, "top.onek", "drivers");
    assert_eq!(set.nodes[0].children.len(), 10);
    assert!(set.nodes[0].children.iter().all(|n| !n.folder), "one instance and a constant: flat");
    // 8 rows in 2 instances, and 9 in one: a flat list, sorted by path.
    let set = trace_start(&bridge, &mut ed, "top.big8", "drivers");
    assert!(set.nodes[0].children.iter().all(|n| n.group.is_empty() && n.label.starts_with('s')), "{:?}", set.nodes[0].children);
    assert_eq!(set.nodes[0].children.iter().map(|n| n.label.as_str()).collect::<Vec<_>>(), ["s2", "s4", "s6", "s8", "s1", "s3", "s5", "s7"], "by path: m0 before m1");
    let set = trace_start(&bridge, &mut ed, "top.one9", "drivers");
    assert_eq!(set.nodes[0].children.len(), 9);
    assert!(set.nodes[0].children.iter().all(|n| n.hint == "m0"));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_the_first_64_rows_are_peeked_and_a_chain_is_followed_24_deep() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-limits");
    trace_ready(&bridge, &mut ed);
    bridge.send(UiCommand::Command(
        "lua local t = {} for i = 1, 70 do t[i] = string.format('top.w.s%02d', i); SLANG.drivers[t[i]] = { 'top.clk' } end SLANG.drivers['top.wide'] = t \
         for i = 1, 40 do SLANG.drivers['top.n' .. i] = i < 40 and { 'top.n' .. (i + 1) } or {} end".into(),
    ));
    let set = trace_start(&bridge, &mut ed, "top.wide", "drivers");
    let rows = &set.nodes[0].children;
    assert_eq!(rows.len(), 70);
    assert_eq!((rows[63].hint.as_str(), rows[63].expandable), ("1 \u{b7} w", true));
    assert_eq!((rows[64].hint.as_str(), rows[64].expandable), ("w", true), "past 64 it is just expandable, no count");
    let calls = server_calls(&bridge, &mut ed);
    assert_eq!(calls.matches("getDriversWithLocation").count(), 1 + 64 + 64, "the root, 64 rows' next steps, and the one chain each of them follows");
    // A long chain stops at 24 rows below the root, the last of them still expandable.
    let set = trace_start(&bridge, &mut ed, "top.n1", "drivers");
    assert_eq!(flat_nodes(&set.nodes).len(), 25);
    let last = flat_nodes(&set.nodes).pop().unwrap();
    assert_eq!((last.label.as_str(), last.open, last.expandable), ("n25", false, true));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn accepting_a_row_opens_its_place_in_the_editor_window_without_touching_the_active_instance() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, file) = trace_session("trace-accept");
    trace_ready(&bridge, &mut ed);
    trace_start(&bridge, &mut ed, "top.q", "drivers");
    let editor_win = ask(&bridge, &mut ed, "vim.api.nvim_get_current_win()");
    bridge.send(UiCommand::Command("call cursor(3, 1) | vsplit | terminal".into()));
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "terminal");
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    tree_accept(&bridge, "slang_trace", "top.q>top.a", "open");
    wait_for(&bridge, &mut ed, &format!("vim.api.nvim_get_current_win() == {editor_win}"), "true");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 13, 4 }");
    assert_eq!(ask(&bridge, &mut ed, &format!("vim.fn.expand('%:p') == '{}'", file.display())), "true");
    let calls = server_calls(&bridge, &mut ed);
    assert!(!calls.contains("setActiveInstance") && !calls.contains("showHierLocation"), "{calls}");
    // The jumplist was marked: Ctrl-O comes back. And Space (peek) goes the same way.
    bridge.send(UiCommand::Input("<C-o>".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 3, 0 }");
    tree_accept(&bridge, "slang_trace", "top.q>top.a>top.b", "peek");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 14, 4 }");
    // The root has the place of the row it was traced from, when it was traced from one; the cursor's has none.
    trace_start(&bridge, &mut ed, "top.q", "drivers");
    tree_accept(&bridge, "slang_trace", "top.q", "peek");
    gui_for(&bridge, &mut ed, 200);
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))"), "{ 14, 4 }", "no place: nothing moved");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_row_goes_to_where_it_connects_and_asks_when_that_is_several_places() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, file) = trace_session_with("trace-sites", true);
    trace_ready(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command(
        "lua SLANG.drivers['top.s'] = { 'top.once', 'top.twice', 'top.gate' }; SLANG.lines['top.once'] = 20 \
         SLANG.sites = { ['top.once'] = { { line = 5, kind = 'data' } }, \
           ['top.twice'] = { { line = 7, kind = 'data' }, { line = 9, kind = 'control' } }, \
           ['top.gate'] = { { line = 11, kind = 'control' } } }".into(),
    ));
    gui_for(&bridge, &mut ed, 100);
    let set = trace_start(&bridge, &mut ed, "top.s", "drivers");
    let group_of = |id: &str| set.nodes[0].children.iter().find(|n| n.id == id).unwrap().group.clone();
    assert_eq!(group_of("top.s>top.gate"), "control", "only ever a condition");
    assert_eq!(group_of("top.s>top.twice"), "", "a value in one place is more than control");
    assert_eq!(group_of("top.s>top.once"), "");

    // One place: there, not the declaration.
    tree_accept(&bridge, "slang_trace", "top.s>top.once", "open");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 6, 4 }");

    // Several: the picker of them, as go to definition has; Space stays out of it.
    tree_accept(&bridge, "slang_trace", "top.s>top.twice", "open");
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Connections of twice");
    assert_eq!(open.items.iter().map(|i| i.lnum).collect::<Vec<_>>(), [Some(8), Some(10)]);
    accept(&bridge, open.id, vec![by_idx(2)], "edit");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 10, 4 }");
    tree_accept(&bridge, "slang_trace", "top.s>top.twice", "peek");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 8, 4 }");

    // The declaration is still a menu item away, on a row that has somewhere else to go.
    bridge.send(UiCommand::TreeMenu { view: "slang_trace".into(), id: "top.s>top.once".into() });
    match wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeMenu { id, .. } if id == "top.s>top.once")) {
        Some(GuiEvent::TreeMenu { items, .. }) => {
            assert_eq!(items, ["Copy Path", "Go to Declaration", "Trace Drivers", "Trace Loads", "Endpoints\u{2026}", "Show in Hierarchy"]);
        }
        _ => panic!("no menu"),
    }
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: "top.s>top.once".into(), index: 2 });
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 21, 4 }");
    assert_eq!(ask(&bridge, &mut ed, &format!("vim.fn.expand('%:p') == '{}'", file.display())), "true");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// The fake design for the endpoint picker: `top.m.r` reaches logic through a port and a plain copy.
const ENDPOINT_GRAPH: &str = "lua _G.notes = {}; vim.notify = function(m) table.insert(_G.notes, m) end; \
    local function s(line, kind) return { line = line, kind = kind } end \
    SLANG.lines['top.out'] = 30 \
    SLANG.loads['top.m.r'] = { 'top.m.port', 'top.m.read', 'top.m.gated' } \
    SLANG.loads['top.m.port'] = { 'top.m.alias', 'top.m.r', 'top.out' } \
    SLANG.loads['top.m.alias'] = { 'top.m.far', 'top.m.same_line' } \
    SLANG.loads['top.out'] = {} \
    SLANG.loads['top.m.far'] = {}; SLANG.loads['top.m.read'] = {}; SLANG.loads['top.m.gated'] = {}; SLANG.loads['top.m.same_line'] = {} \
    SLANG.drivers['top.m.d'] = { 'top.m.via', 'top.m.en', 'pkg::e.K' } \
    SLANG.drivers['top.m.via'] = { 'top.m.a', 'top.m.b' } \
    SLANG.drivers['top.m.lonely'] = {} \
    SLANG.sites = { ['top.m.port'] = { s(5, 'port') }, ['top.m.alias'] = { s(6, 'wire') }, ['top.m.r'] = { s(6, 'wire') }, \
      ['top.out'] = { s(6, 'port') }, ['top.m.read'] = { s(7, 'data') }, ['top.m.gated'] = { s(9, 'control') }, \
      ['top.m.far'] = { s(11, 'data') }, ['top.m.same_line'] = { s(11, 'data') }, \
      ['top.m.via'] = { s(3, 'port') }, ['top.m.en'] = { s(4, 'control') }, ['pkg::e.K'] = { s(2, 'data') }, \
      ['top.m.a'] = { s(13, 'data') }, ['top.m.b'] = { s(13, 'data') } }";

#[test]
fn the_endpoint_picker_follows_the_wiring_and_lists_where_the_signal_really_ends_up() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session_with("trace-endpoints", true);
    trace_ready(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command(ENDPOINT_GRAPH.into()));
    wait_for(&bridge, &mut ed, "type(_G.notes)", "table");
    // Not without the server that has the sites.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').endpoints_path('top.m.r', 'loads')".into()));
    wait_for(&bridge, &mut ed, "table.concat(_G.notes, ';')", "Endpoints need the patched slang-server");
    bridge.send(UiCommand::Command("let g:slang_loads_safe = v:true | lua _G.notes = {}".into()));

    // Loads: past the port and the copy, conditions included, a line once, the edge of the design too, and the loop ends.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').endpoints_path('top.m.r', 'loads')".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Load Endpoints of r");
    assert_eq!(open.items.iter().map(|i| i.lnum).collect::<Vec<_>>(), [Some(8), Some(10), Some(12), Some(31)]);
    accept(&bridge, open.id, vec![by_idx(3)], "edit");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 12, 4 }");

    // Drivers: one line of logic is a jump; the condition and the constant don't count.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').endpoints_path('top.m.d', 'drivers')".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 14, 4 }");

    // Nothing to show says so.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').endpoints_path('top.m.lonely', 'drivers')".into()));
    wait_for(&bridge, &mut ed, "table.concat(_G.notes, ';')", "No driver endpoints of lonely");

    // Lines that assign a constant are endpoints, at the signal asked about and past the wiring; once with the signals' lines.
    bridge.send(UiCommand::Command("lua SLANG.constants = { ['top.m.d'] = { 20, 13 }, ['top.m.via'] = { 21 }, ['top.m.lonely'] = { 22 } }".into()));
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').endpoints_path('top.m.d', 'drivers')".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Driver Endpoints of d");
    assert_eq!(open.items.iter().map(|i| i.lnum).collect::<Vec<_>>(), [Some(14), Some(21), Some(22)]);
    accept(&bridge, open.id, vec![by_idx(0)], "edit");
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').endpoints_path('top.m.lonely', 'drivers')".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 23, 4 }");
    bridge.send(UiCommand::Command("lua SLANG.constants = nil".into()));

    // From the cursor, and from a row of a trace, in the direction it shows.
    bridge.send(UiCommand::Command("lua SLANG.instances[2] = { 'top.m.d' }".into()));
    bridge.send(UiCommand::Command("call cursor(3, 1) | lua require('eshvi.slang.trace').endpoints('drivers')".into()));
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 14, 4 }");
    let set = trace_start(&bridge, &mut ed, "top.m.r", "loads");
    let row = set.nodes[0].children.iter().find(|n| n.id == "top.m.r>top.m.port").expect("the port's row");
    bridge.send(UiCommand::TreeMenu { view: "slang_trace".into(), id: row.id.clone() });
    wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeMenu { .. }));
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: row.id.clone(), index: 5 });
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Load Endpoints of port");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn switching_asks_the_other_direction_from_the_same_root_and_refresh_keeps_what_is_open() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-switch");
    trace_ready(&bridge, &mut ed);
    let set = trace_start(&bridge, &mut ed, "top.a", "drivers");
    assert_eq!((set.nodes[0].hint.as_str(), ids(&set.nodes)), ("Drivers \u{b7} 1", vec!["top.a".to_string(), "top.a>top.b".into(), "top.a>top.b>top.c".into()]));
    gui_for(&bridge, &mut ed, 100);
    // The first header button, then the `s` key: no change to the side bar.
    bridge.send(UiCommand::TreeButton { view: "slang_trace".into(), index: 1, selected: None, expanded: vec![] });
    let seen = gui_for(&bridge, &mut ed, 600);
    let sets = trace_sets(&seen);
    assert_eq!(sets.len(), 1, "{seen:?}");
    assert_eq!((sets[0].nodes[0].id.as_str(), sets[0].nodes[0].hint.as_str(), ids(&sets[0].nodes)), ("top.a", "Loads \u{b7} 1", vec!["top.a".to_string(), "top.a>top.q".into()]));
    assert!(!seen.iter().any(|g| matches!(g, GuiEvent::TreeShow { .. })), "the view is showing already");
    bridge.send(UiCommand::TreeKey { view: "slang_trace".into(), id: None, key: "s".into() });
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(set.nodes[0].hint, "Drivers \u{b7} 1");
    // Refresh: asked again from the root; only what the app says is open is open (b was closed by the user).
    let refresh = |expanded: &[&str]| UiCommand::TreeButton { view: "slang_trace".into(), index: 2, selected: None, expanded: expanded.iter().map(|s| s.to_string()).collect() };
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(refresh(&["top.a"]));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(ids(&set.nodes), ["top.a", "top.a>top.b"]);
    assert_eq!((set.nodes[0].open, at(&set.nodes, "top.a>top.b").open), (true, false));
    assert!(server_calls(&bridge, &mut ed).starts_with("slang.getDriversWithLocation {"), "asked again");
    // With c open as well (two steps, so not by itself): its children come inline.
    bridge.send(refresh(&["top.a", "top.a>top.b", "top.a>top.b>top.c"]));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(ids(&set.nodes), ["top.a", "top.a>top.b", "top.a>top.b>top.c", "top.a>top.b>top.c>top.x", "top.a>top.b>top.c>top.y"]);
    assert_eq!(at(&set.nodes, "top.a>top.b>top.c").open, true);
    // An error: the server's words, and the tree is left as it was.
    bridge.send(UiCommand::Command("lua _G.notes = {}; SLANG.fail = 'no design'".into()));
    bridge.send(refresh(&[]));
    wait_for(&bridge, &mut ed, "tostring(vim.tbl_contains(_G.notes, 'no design|4'))", "true");
    assert!(trace_sets(&gui_for(&bridge, &mut ed, 300)).is_empty());
    bridge.send(UiCommand::Command("lua SLANG.fail = nil; _G.notes = {}; require('eshvi.slang.trace').trace_path('top.nope', 'drivers')".into()));
    wait_for(&bridge, &mut ed, "tostring(vim.tbl_contains(_G.notes, 'Could not find path in compiled design: top.nope|4'))", "true");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_traces_menu_copies_traces_again_from_a_row_and_shows_it_in_the_hierarchy_and_the_hierarchys_menu_traces_ports_and_signals() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-menu");
    trace_ready(&bridge, &mut ed);
    let menu = |bridge: &Bridge, ed: &mut Editor, view: &str, id: &str| -> Vec<String> {
        bridge.send(UiCommand::TreeMenu { view: view.into(), id: id.into() });
        match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::TreeMenu { id: i, .. } if i == id)) {
            Some(GuiEvent::TreeMenu { items, .. }) => items,
            _ => panic!("no menu for {id}"),
        }
    };
    trace_start(&bridge, &mut ed, "top.y", "drivers");
    assert_eq!(menu(&bridge, &mut ed, "slang_trace", "top.y>pkg::e.K"), ["Copy Path"], "a constant is not traced");
    trace_start(&bridge, &mut ed, "top.q", "drivers");
    let row = "top.q>top.a";
    assert_eq!(menu(&bridge, &mut ed, "slang_trace", row), ["Copy Path", "Trace Drivers", "Trace Loads", "Endpoints\u{2026}", "Show in Hierarchy"]);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: row.into(), index: 1 });
    wait_for(&bridge, &mut ed, "tostring(_G.clip)", "top.a");
    bridge.send(UiCommand::Command("lua _G.clip = ''".into()));
    bridge.send(UiCommand::TreeKey { view: "slang_trace".into(), id: Some("top.q>top.a>top.b".into()), key: "y".into() });
    wait_for(&bridge, &mut ed, "tostring(_G.clip)", "top.b");
    // Show in Hierarchy: the hierarchy view, with the keyboard.
    gui_for(&bridge, &mut ed, 100);
    menu(&bridge, &mut ed, "slang_trace", row);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: row.into(), index: 5 });
    let shown = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeShow { view, .. } if view == "slang"));
    assert_eq!(shown, Some(GuiEvent::TreeShow { view: "slang".into(), action: "show".into(), focus: true }));
    // Trace Loads: that row is the new root.
    menu(&bridge, &mut ed, "slang_trace", row);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: row.into(), index: 3 });
    let loads = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!((loads.nodes[0].id.as_str(), loads.nodes[0].hint.as_str()), ("top.a", "Loads \u{b7} 1"));
    // The hierarchy's own menu: ports and signals get the two items; others don't.
    // (The roots came while the trace was waited for: the first set, gen 1.)
    slang_children(&bridge, &mut ed, "top", 1);
    assert_eq!(menu(&bridge, &mut ed, "slang", "top.q"), ["Copy Path", "Copy File Path", "Trace Drivers", "Trace Loads"]);
    assert_eq!(menu(&bridge, &mut ed, "slang", "top"), ["Copy Path", "Copy File Path", "Go to Declaration", "Go to Instantiation"]);
    menu(&bridge, &mut ed, "slang", "top.q");
    bridge.send(UiCommand::TreeRun { view: "slang".into(), id: "top.q".into(), index: 4 });
    let loads = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!((loads.nodes[0].id.as_str(), loads.nodes[0].hint.as_str()), ("top.q", "Loads \u{b7} 2"));
    menu(&bridge, &mut ed, "slang", "top.clk");
    bridge.send(UiCommand::TreeRun { view: "slang".into(), id: "top.clk".into(), index: 3 });
    let drivers = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!((drivers.nodes[0].id.as_str(), drivers.nodes[0].hint.as_str()), ("top.clk", "Drivers \u{b7} 0"));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The signal trace with svtrace attached ----

/// Start the fake svtrace (phase 1: the three commands only) for the current buffer and wait for its client.
fn attach_svtrace(bridge: &Bridge, ed: &mut Editor) {
    bridge.send(UiCommand::Command("lua fake_svtrace({ phase1 = true })".into()));
    wait_for(bridge, ed, "tostring(#vim.lsp.get_clients({ name = 'svtrace' }))", "1");
}

/// The same with `svtrace.step` and `svtrace.endpoints`.
fn attach_svtrace_steps(bridge: &Bridge, ed: &mut Editor) {
    bridge.send(UiCommand::Command("lua fake_svtrace()".into()));
    wait_for(bridge, ed, "tostring(#vim.lsp.get_clients({ name = 'svtrace' }))", "1");
}

/// How many `workspace/executeCommand` requests `client` (a fake's name) got for the commands starting with `prefix`.
/// (Not `svtrace.status`: the trace asks it before it starts, to see that svtrace has a design.)
fn commands_to(bridge: &Bridge, ed: &mut Editor, client: &str, prefix: &str) -> usize {
    ask(bridge, ed, &format!("tostring(#vim.tbl_filter(function(c) return c.name == '{client}' and c.method == 'workspace/executeCommand' and c.params.command ~= 'svtrace.status' and vim.startswith(c.params.command, '{prefix}') end, FAKE_CALLS))")).parse().unwrap()
}

#[test]
fn with_svtrace_attached_the_trace_commands_go_to_it_and_instances_stay_with_slang_server() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-svtrace-routing");
    trace_ready(&bridge, &mut ed);
    attach_svtrace(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    // Drivers, from the cursor: the instances question is slang-server's, the drivers svtrace's.
    bridge.send(UiCommand::Command("call cursor(3, 1) | lua require('eshvi.slang.trace').trace('drivers')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!((set.nodes[0].id.as_str(), set.nodes[0].hint.as_str()), ("top.q", "Drivers \u{b7} 1"));
    assert_eq!(ids(&set.nodes), ["top.q", "top.q>top.a", "top.q>top.a>top.b", "top.q>top.a>top.b>top.c"]);
    assert_eq!(ask(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.name == 'slang' and c.method == 'workspace/executeCommand' and c.params.command == 'slang.getInstances' end, FAKE_CALLS))"), "1");
    assert_eq!(ask(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.name == 'svtrace' and c.params.command == 'slang.getInstances' end, FAKE_CALLS))"), "0");
    assert!(commands_to(&bridge, &mut ed, "svtrace", "slang.getDriversWithLocation") > 0);
    // Loads and the drivers-with-constants question the same.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').trace('loads')".into()));
    tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    bridge.send(UiCommand::Command("lua SLANG.instances[2] = { 'top.q' }; require('eshvi.slang.trace').endpoints_path('top.q', 'drivers')".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.name == 'svtrace' and c.params.command == 'slang.getDriversWithConstants' end, FAKE_CALLS) > 0)", "true");
    assert!(commands_to(&bridge, &mut ed, "svtrace", "slang.getLoadsWithLocation") > 0);
    // None of the three reached slang-server; it was only asked what it is for.
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getDrivers"), 0);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getLoads"), 0);
    // An error from svtrace is shown as the server's words.
    bridge.send(UiCommand::Command("lua _G.notes = {}; require('eshvi.slang.trace').trace_path('top.nope', 'drivers')".into()));
    wait_for(&bridge, &mut ed, "tostring(vim.tbl_contains(_G.notes, 'Could not find path in compiled design: top.nope|4'))", "true");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn with_svtrace_attached_loads_are_asked_ahead_and_endpoints_work_without_slang_loads_safe() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session_with("trace-svtrace-loads", true);
    trace_ready(&bridge, &mut ed);
    attach_svtrace(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command(ENDPOINT_GRAPH.into()));
    wait_for(&bridge, &mut ed, "type(_G.notes)", "table");
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.g.slang_loads_safe)"), "nil", "the flag is not set");
    // Loads of q: out0 was asked ahead, so it has no chevron.
    let set = trace_start(&bridge, &mut ed, "top.q", "loads");
    assert!(!at(&set.nodes, "top.q>top.out0").expandable);
    assert!(commands_to(&bridge, &mut ed, "svtrace", "slang.getLoadsWithLocation") >= 3);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getLoads"), 0);
    // Endpoints: no "need the patched slang-server", and the picker is made.
    bridge.send(UiCommand::Command("lua _G.notes = {}; require('eshvi.slang.trace').endpoints_path('top.m.r', 'loads')".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Load Endpoints of r");
    assert_eq!(open.items.iter().map(|i| i.lnum).collect::<Vec<_>>(), [Some(8), Some(10), Some(12), Some(31)]);
    assert_eq!(ask(&bridge, &mut ed, "table.concat(_G.notes, ';')"), "");
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getLoads"), 0);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getDrivers"), 0);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_svtrace_the_trace_asks_slang_server_as_before() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-no-svtrace");
    trace_ready(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    trace_start(&bridge, &mut ed, "top.q", "drivers");
    assert!(commands_to(&bridge, &mut ed, "slang", "slang.getDriversWithLocation") > 0);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "slang."), 0);
    // Loads stay unasked-ahead, and endpoints refuse, as before.
    bridge.send(UiCommand::Command("lua _G.notes = {}; require('eshvi.slang.trace').endpoints_path('top.q', 'loads')".into()));
    wait_for(&bridge, &mut ed, "table.concat(_G.notes, ';')", "Endpoints need the patched slang-server|3");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The signal trace with svtrace's whole-graph commands (svtrace.step, svtrace.endpoints) ----

/// The `svtrace.step` requests so far, as `path direction follow ancestors`.
fn step_calls(bridge: &Bridge, ed: &mut Editor) -> String {
    ask(bridge, ed, "table.concat(vim.tbl_map(function(c) local a = c.params.arguments[1]; return a.path .. ' ' .. a.direction .. ' ' .. tostring(a.follow) .. ' [' .. table.concat(a.ancestors, ',') .. ']' end, vim.tbl_filter(function(c) return c.name == 'svtrace' and c.method == 'workspace/executeCommand' and c.params.command == 'svtrace.step' end, FAKE_CALLS)), ' | ')")
}

#[test]
fn with_svtrace_step_a_trace_is_one_request_with_the_chains_open_and_opening_a_row_is_one_more() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-step");
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    let set = trace_start(&bridge, &mut ed, "top.q", "drivers");
    assert_eq!((set.nodes[0].id.as_str(), set.nodes[0].hint.as_str()), ("top.q", "Drivers \u{b7} 1"));
    assert_eq!(ids(&set.nodes), ["top.q", "top.q>top.a", "top.q>top.a>top.b", "top.q>top.a>top.b>top.c"]);
    let (a, b, c) = (at(&set.nodes, "top.q>top.a"), at(&set.nodes, "top.q>top.a>top.b"), at(&set.nodes, "top.q>top.a>top.b>top.c"));
    assert_eq!((a.open, b.open, c.open), (true, true, false));
    assert_eq!((a.hint.as_str(), c.hint.as_str(), c.expandable), ("1 \u{b7} top", "2 \u{b7} top", true));
    // One request for all of it, nothing to the three phase 1 commands, nothing to slang-server.
    assert_eq!(step_calls(&bridge, &mut ed), "top.q drivers true []");
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "slang."), 0);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getDrivers"), 0);
    // Opening c: one request, with the chain above it; y goes on by itself past the constant, z loops back to y.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::TreeExpand { view: "slang_trace".into(), id: c.id.clone(), gen: set.gen });
    let kids = match wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSet(s) if s.view == "slang_trace" && s.parent.as_deref() == Some(c.id.as_str()))) {
        Some(GuiEvent::TreeSet(s)) => s,
        _ => panic!("no children of c"),
    };
    let base = "top.q>top.a>top.b>top.c";
    assert_eq!(
        ids(&kids.nodes),
        [format!("{base}>top.x"), format!("{base}>top.y"), format!("{base}>top.y>pkg::e.K"), format!("{base}>top.y>top.z"), format!("{base}>top.y>top.z>top.y")]
    );
    assert_eq!(step_calls(&bridge, &mut ed), "top.c drivers true [top.q,top.a,top.b,top.c]");
    let (x, y, k, z, looped) = (
        at(&kids.nodes, &format!("{base}>top.x")),
        at(&kids.nodes, &format!("{base}>top.y")),
        at(&kids.nodes, &format!("{base}>top.y>pkg::e.K")),
        at(&kids.nodes, &format!("{base}>top.y>top.z")),
        at(&kids.nodes, &format!("{base}>top.y>top.z>top.y")),
    );
    assert!(!x.expandable, "total 0: nothing further, no chevron");
    assert_eq!((y.open, z.open, y.hint.as_str()), (true, true, "1 \u{b7} top"));
    assert_eq!((k.group.as_str(), k.expandable), ("constant", false));
    assert_eq!((looped.hint.as_str(), looped.expandable), ("loop \u{b7} top", false));
    // The constant doesn't count as a step.
    assert_eq!(set.nodes[0].children.len(), 1);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn with_svtrace_step_a_module_local_constant_comes_from_the_entry_and_many_signals_are_grouped_by_instance() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-step-many");
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    // `top.m.MODE` is an enum member the path can't tell from a signal; svtrace says.
    bridge.send(UiCommand::Command("lua SLANG.drivers['top.v'] = { 'top.m.MODE', 'top.w' }; SLANG.drivers['top.m.MODE'] = {}; SLANG.drivers['top.w'] = {}; SLANG.constant_paths = { ['top.m.MODE'] = true }".into()));
    let set = trace_start(&bridge, &mut ed, "top.v", "drivers");
    assert_eq!(set.nodes[0].hint, "Drivers \u{b7} 1");
    let mode = at(&set.nodes, "top.v>top.m.MODE");
    assert_eq!((mode.group.as_str(), mode.expandable), ("constant", false));
    // Nine signals in two instances: grouped (by the Lua), nothing followed.
    let set = trace_start(&bridge, &mut ed, "top.big9", "drivers");
    let top = &set.nodes[0];
    assert_eq!(top.hint, "Drivers \u{b7} 9");
    assert_eq!(top.children.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["top.big9>@top.m0", "top.big9>@top.m1"]);
    // Eight are not many.
    let set = trace_start(&bridge, &mut ed, "top.big8", "drivers");
    assert_eq!(set.nodes[0].children.len(), 8);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn with_svtrace_step_refresh_asks_without_chains_and_each_open_row_in_turn() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-step-refresh");
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    trace_start(&bridge, &mut ed, "top.a", "drivers");
    let refresh = |expanded: &[&str]| UiCommand::TreeButton { view: "slang_trace".into(), index: 2, selected: None, expanded: expanded.iter().map(|s| s.to_string()).collect() };
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(refresh(&["top.a"]));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(ids(&set.nodes), ["top.a", "top.a>top.b"]);
    assert_eq!((set.nodes[0].open, at(&set.nodes, "top.a>top.b").open), (true, false));
    assert_eq!(step_calls(&bridge, &mut ed), "top.a drivers false []");
    // b and c open: asked for one level after the other, none of it by itself.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(refresh(&["top.a", "top.a>top.b", "top.a>top.b>top.c"]));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(ids(&set.nodes), ["top.a", "top.a>top.b", "top.a>top.b>top.c", "top.a>top.b>top.c>top.x", "top.a>top.b>top.c>top.y"]);
    assert_eq!(step_calls(&bridge, &mut ed), "top.a drivers false [] | top.b drivers false [top.a,top.b] | top.c drivers false [top.a,top.b,top.c]");
    // The row still opens on its own afterwards: one request.
    let y = at(&set.nodes, "top.a>top.b>top.c>top.y");
    assert!(y.expandable && !y.open);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::TreeExpand { view: "slang_trace".into(), id: y.id.clone(), gen: set.gen });
    wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSet(s) if s.view == "slang_trace" && s.parent.as_deref() == Some(y.id.as_str())));
    assert_eq!(step_calls(&bridge, &mut ed), "top.y drivers true [top.a,top.b,top.c,top.y]");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn with_svtrace_step_endpoints_are_one_request_and_an_error_is_shown() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session_with("trace-step-endpoints", true);
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command(ENDPOINT_GRAPH.into()));
    wait_for(&bridge, &mut ed, "type(_G.notes)", "table");
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').endpoints_path('top.m.r', 'loads')".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Load Endpoints of r");
    assert_eq!(open.items.iter().map(|i| i.lnum).collect::<Vec<_>>(), [Some(8), Some(10), Some(12), Some(31)]);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", ""), 1);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.endpoints"), 1);
    // Drivers, with the constants' lines; one line is a jump.
    bridge.send(UiCommand::Command("lua SLANG.constants = { ['top.m.via'] = { 21 } }; require('eshvi.slang.trace').endpoints_path('top.m.d', 'drivers')".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.items.iter().map(|i| i.lnum).collect::<Vec<_>>(), [Some(14), Some(22)]);
    bridge.send(UiCommand::Command("lua SLANG.constants = nil; require('eshvi.slang.trace').endpoints_path('top.m.lonely', 'drivers')".into()));
    wait_for(&bridge, &mut ed, "table.concat(_G.notes, ';')", "No driver endpoints of lonely");
    // An error is the server's words.
    bridge.send(UiCommand::Command("lua _G.notes = {}; require('eshvi.slang.trace').endpoints_path('top.nope', 'drivers')".into()));
    wait_for(&bridge, &mut ed, "tostring(vim.tbl_contains(_G.notes, 'Could not find path in compiled design: top.nope'))", "true");
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "slang."), 0);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_driver_that_is_only_a_clock_is_in_the_clock_group_and_does_not_count() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-step-clock");
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    // A flop: q <= d at clk, with rst_n read in the event list and in an `if` (a clock and a control site).
    bridge.send(UiCommand::Command(
        "lua local function s(line, kind) return { line = line, kind = kind } end \
         SLANG.drivers['top.ff'] = { 'top.clk', 'top.d', 'top.rst_n' }; SLANG.drivers['top.d'] = {}; SLANG.drivers['top.rst_n'] = {} \
         SLANG.loads['top.clk'] = { 'top.ff' }; SLANG.loads['top.ff'] = {} \
         SLANG.sites = { ['top.clk'] = { s(5, 'clock') }, ['top.d'] = { s(6, 'data') }, ['top.rst_n'] = { s(5, 'clock'), s(7, 'control') } }"
            .into(),
    ));
    let set = trace_start(&bridge, &mut ed, "top.ff", "drivers");
    assert_eq!(set.nodes[0].hint, "Drivers \u{b7} 2", "the clock is not a step");
    assert_eq!(at(&set.nodes, "top.ff>top.clk").group, "clock");
    assert_eq!(at(&set.nodes, "top.ff>top.d").group, "");
    assert_eq!(at(&set.nodes, "top.ff>top.rst_n").group, "", "a clock and a control site: not only a clock");
    // The clock's loads: the flop, which is a signal there.
    let set = trace_start(&bridge, &mut ed, "top.clk", "loads");
    assert_eq!((set.nodes[0].hint.as_str(), at(&set.nodes, "top.clk>top.ff").group.as_str()), ("Loads \u{b7} 1", ""));
    // A flop's clock doesn't break a chain: q <= d is still followed.
    bridge.send(UiCommand::Command("lua SLANG.drivers['top.q'] = { 'top.clk', 'top.d' }; SLANG.drivers['top.d'] = { 'top.src' }; SLANG.drivers['top.src'] = {}".into()));
    let set = trace_start(&bridge, &mut ed, "top.q", "drivers");
    assert_eq!(ids(&set.nodes), ["top.q", "top.q>top.clk", "top.q>top.d", "top.q>top.d>top.src"]);
    assert!(at(&set.nodes, "top.q>top.d").open);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_svtrace_without_step_uses_the_phase_1_path() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-phase1");
    trace_ready(&bridge, &mut ed);
    attach_svtrace(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    let set = trace_start(&bridge, &mut ed, "top.q", "drivers");
    assert_eq!(ids(&set.nodes), ["top.q", "top.q>top.a", "top.q>top.a>top.b", "top.q>top.a>top.b>top.c"]);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace."), 0);
    assert!(commands_to(&bridge, &mut ed, "svtrace", "slang.getDriversWithLocation") >= 4, "asked a step at a time");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The signal trace with svtrace's selected paths (bits and fields) and svtrace.atCursor ----

/// A graph with selected paths: `mie` is a field of `top.cs.mstatus_q`, `x[7:0]` a part of `top.x`, `y[3]` a bit of `top.u[0].y`.
const SELECTED_GRAPH: &str = "lua SLANG.signals = { ['top.cs.mstatus_q.mie'] = 'top.cs.mstatus_q', ['top.x[7:0]'] = 'top.x', ['top.x[3]'] = 'top.x', ['top.u[0].y[3]'] = 'top.u[0].y' } \
     SLANG.drivers['top.cs.mstatus_q.mie'] = { 'top.cs.mie_d', 'top.x[7:0]' }; SLANG.drivers['top.cs.mie_d'] = {}; \
     SLANG.drivers['top.x[7:0]'] = { 'top.u[0].y[3]' }; SLANG.drivers['top.u[0].y[3]'] = {} \
     SLANG.at_cursor = { [5] = { 'top.cs.mstatus_q.mie' } }";

#[test]
fn a_trace_from_the_cursor_asks_svtrace_at_cursor_and_the_rows_of_selected_paths_are_labelled_by_their_last_part() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-selected");
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command(SELECTED_GRAPH.into()));
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command("call cursor(6, 1) | lua require('eshvi.slang.trace').trace('drivers')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    // The question is svtrace's, with the document and the position; slang-server is not asked.
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.atCursor"), 1);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getInstances"), 0);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "slang."), 0);
    assert_eq!(ask(&bridge, &mut ed, "tostring(#SLANG.at_cursor_calls) .. ' ' .. SLANG.at_cursor_calls[1].position.line .. ' ' .. tostring(vim.startswith(SLANG.at_cursor_calls[1].textDocument.uri, 'file://'))"), "1 5 true");
    assert_eq!(step_calls(&bridge, &mut ed), "top.cs.mstatus_q.mie drivers true []");
    // Labels are the last part of the selected path; the hint's instance is the signal's.
    let root = &set.nodes[0];
    assert_eq!((root.id.as_str(), root.label.as_str(), root.hint.as_str()), ("top.cs.mstatus_q.mie", "mie", "Drivers \u{b7} 2"));
    let (field, part, bit) = (
        at(&set.nodes, "top.cs.mstatus_q.mie>top.cs.mie_d"),
        at(&set.nodes, "top.cs.mstatus_q.mie>top.x[7:0]"),
        at(&set.nodes, "top.cs.mstatus_q.mie>top.x[7:0]>top.u[0].y[3]"),
    );
    assert_eq!((field.label.as_str(), field.hint.as_str()), ("mie_d", "cs"));
    assert_eq!((part.label.as_str(), part.hint.as_str(), part.open), ("x[7:0]", "1 \u{b7} top", true));
    assert_eq!((bit.label.as_str(), bit.hint.as_str()), ("y[3]", "u[0]"));
    // The root from the cursor knows its signal (atCursor's `signal`): Copy Signal Path, and the hierarchy gets the plain path.
    bridge.send(UiCommand::Command("lua local h = require('eshvi.slang.hierarchy'); local reveal = h.reveal; h.reveal = function(p, o) _G.revealed = p; return reveal(p, o) end".into()));
    let root_menu = |bridge: &Bridge, ed: &mut Editor| -> Vec<String> {
        bridge.send(UiCommand::TreeMenu { view: "slang_trace".into(), id: "top.cs.mstatus_q.mie".into() });
        match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::TreeMenu { id, .. } if id == "top.cs.mstatus_q.mie")) {
            Some(GuiEvent::TreeMenu { items, .. }) => items,
            _ => panic!("no menu for the root"),
        }
    };
    assert_eq!(root_menu(&bridge, &mut ed), ["Copy Path", "Copy Signal Path", "Trace Drivers", "Trace Loads", "Endpoints\u{2026}", "Show in Hierarchy"]);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: "top.cs.mstatus_q.mie".into(), index: 6 });
    wait_for(&bridge, &mut ed, "tostring(_G.revealed)", "top.cs.mstatus_q");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_menu_of_a_selected_path_copies_it_and_its_signal_and_shows_the_signal_in_the_hierarchy() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-selected-menu");
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command(SELECTED_GRAPH.into()));
    let menu = |bridge: &Bridge, ed: &mut Editor, id: &str| -> Vec<String> {
        bridge.send(UiCommand::TreeMenu { view: "slang_trace".into(), id: id.into() });
        match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::TreeMenu { id: i, .. } if i == id)) {
            Some(GuiEvent::TreeMenu { items, .. }) => items,
            _ => panic!("no menu for {id}"),
        }
    };
    // (A root that came from a row knows its signal; one from the cursor has only the path.)
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').trace_path('top.cs.mstatus_q.mie', 'drivers', { signal = 'top.cs.mstatus_q' })".into()));
    tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    let part = "top.cs.mstatus_q.mie>top.x[7:0]";
    let plain = "top.cs.mstatus_q.mie>top.cs.mie_d";
    // Copy Signal Path only where the signal's path is not the path.
    assert_eq!(
        menu(&bridge, &mut ed, part),
        ["Copy Path", "Copy Signal Path", "Trace Drivers", "Trace Loads", "Endpoints\u{2026}", "Show in Hierarchy"]
    );
    assert_eq!(menu(&bridge, &mut ed, plain), ["Copy Path", "Trace Drivers", "Trace Loads", "Endpoints\u{2026}", "Show in Hierarchy"]);
    assert_eq!(menu(&bridge, &mut ed, "top.cs.mstatus_q.mie")[..3], ["Copy Path", "Copy Signal Path", "Trace Drivers"]);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: "top.cs.mstatus_q.mie".into(), index: 2 });
    wait_for(&bridge, &mut ed, "tostring(_G.clip)", "top.cs.mstatus_q");
    menu(&bridge, &mut ed, part);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: part.into(), index: 1 });
    wait_for(&bridge, &mut ed, "tostring(_G.clip)", "top.x[7:0]");
    menu(&bridge, &mut ed, part);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: part.into(), index: 2 });
    wait_for(&bridge, &mut ed, "tostring(_G.clip)", "top.x");
    // The key copies the selected path.
    bridge.send(UiCommand::Command("lua _G.clip = ''".into()));
    bridge.send(UiCommand::TreeKey { view: "slang_trace".into(), id: Some(part.into()), key: "y".into() });
    wait_for(&bridge, &mut ed, "tostring(_G.clip)", "top.x[7:0]");
    // Show in Hierarchy goes to the plain signal.
    bridge.send(UiCommand::Command("lua local h = require('eshvi.slang.hierarchy'); local reveal = h.reveal; h.reveal = function(p, o) _G.revealed = p; return reveal(p, o) end".into()));
    menu(&bridge, &mut ed, part);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: part.into(), index: 6 });
    wait_for(&bridge, &mut ed, "tostring(_G.revealed)", "top.x");
    // Tracing the row goes on with the selected path.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    menu(&bridge, &mut ed, part);
    bridge.send(UiCommand::TreeRun { view: "slang_trace".into(), id: part.into(), index: 3 });
    let again = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!((again.nodes[0].id.as_str(), again.nodes[0].label.as_str()), ("top.x[7:0]", "x[7:0]"));
    assert_eq!(step_calls(&bridge, &mut ed), "top.x[7:0] drivers true []");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn several_instances_from_at_cursor_pick_by_the_active_instance_or_ask_and_endpoints_start_there_too() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session_with("trace-selected-several", true);
    trace_ready(&bridge, &mut ed);
    // (An svtrace that does not merge instances: the picker, as with slang-server.)
    bridge.send(UiCommand::Command("lua fake_svtrace({ paths = false })".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'svtrace' }))", "1");
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command("lua SLANG.at_cursor = { [3] = { 'top.u[0].d', 'top.u[1].d' }, [6] = { 'top.x[3]' } }; SLANG.loads['top.x[3]'] = { 'top.r' }; SLANG.loads['top.r'] = {}; SLANG.sites = { ['top.r'] = { { line = 8, kind = 'data' } } }; SLANG.signals = { ['top.x[3]'] = 'top.x' }".into()));
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; require('eshvi.slang.hierarchy').active_paths[vim.lsp.get_clients({ name = 'slang' })[1].id] = 'top.u[1]'".into()));
    bridge.send(UiCommand::Command("call cursor(4, 1) | lua require('eshvi.slang.trace').trace('drivers')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(set.nodes[0].id, "top.u[1].d", "the one under the active instance, without asking");
    // No active instance: the picker.
    bridge.send(UiCommand::Command("lua require('eshvi.slang.hierarchy').active_paths[vim.lsp.get_clients({ name = 'slang' })[1].id] = nil".into()));
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').trace('drivers')".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!((open.title.as_str(), texts(&open)), ("Trace which instance?", vec!["top.u[0].d", "top.u[1].d"]));
    accept(&bridge, open.id, vec![by_idx(1)], "edit");
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(set.nodes[0].id, "top.u[0].d");
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getInstances"), 0);
    // Endpoints start from the cursor's selected path too: svtrace's walk is asked for it.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command("call cursor(7, 1) | lua require('eshvi.slang.trace').endpoints('loads')".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.name == 'svtrace' and c.params.command == 'svtrace.endpoints' end, FAKE_CALLS))", "1");
    assert_eq!(ask(&bridge, &mut ed, "FAKE_CALLS[#FAKE_CALLS].params.arguments[1].path"), "top.x[3]");
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getInstances"), 0);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// The graph the merged-instance tests share: `top.u[0].d` and `top.u[1].d` are the instances of one module at
/// the cursor's line 3; both have a `w` below them (two steps of their own, so it is not followed) and each its own
/// other driver.
const MERGED_GRAPH: &str = "lua SLANG.at_cursor = { [3] = { 'top.u[0].d', 'top.u[1].d' } }; \
    SLANG.drivers['top.u[0].d'] = { 'top.u[0].w', 'top.clk' }; SLANG.drivers['top.u[1].d'] = { 'top.u[1].w', 'top.q' }; \
    SLANG.drivers['top.u[0].w'] = { 'top.p', 'top.r' }; SLANG.drivers['top.u[1].w'] = { 'top.p', 'top.r' }; \
    SLANG.drivers['top.p'] = {}; SLANG.drivers['top.r'] = {}; \
    SLANG.sites = { ['top.clk'] = { { line = 8, kind = 'data' } }, ['top.q'] = { { line = 9, kind = 'data' } }, ['top.u[0].w'] = { { line = 6, kind = 'data' } }, ['top.u[1].w'] = { { line = 6, kind = 'data' } } }";

/// What the last `svtrace.step` or `svtrace.endpoints` request carried, as `key=value;...`.
fn last_svtrace_call(bridge: &Bridge, ed: &mut Editor, command: &str) -> String {
    ask(
        bridge,
        ed,
        &format!(
            "(function() local calls = vim.tbl_filter(function(c) return c.name == 'svtrace' and c.method == 'workspace/executeCommand' and c.params.command == '{command}' end, FAKE_CALLS); \
             local a = calls[#calls].params.arguments[1]; \
             local function list(v) return type(v) == 'table' and table.concat(v, ',') or tostring(v) end; \
             return 'path=' .. tostring(a.path) .. ';paths=' .. list(a.paths) .. ';prefer=' .. tostring(a.prefer) .. ';ancestors=' .. list(a.ancestors) end)()"
        ),
    )
}

#[test]
fn with_merged_instances_a_trace_from_the_cursor_has_no_picker_one_root_and_rows_for_the_instances_that_differ() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session_with("trace-merged", true);
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command(MERGED_GRAPH.into()));
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    // No picker, one root; the hint says how many instances it is, the tooltip lists them.
    bridge.send(UiCommand::Command("call cursor(4, 1) | lua require('eshvi.slang.trace').trace('drivers')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(set.nodes.len(), 1);
    let root = &set.nodes[0];
    assert_eq!((root.id.as_str(), root.hint.as_str(), root.tooltip.as_str()), ("top.u[0].d", "Drivers \u{b7} 3 \u{b7} 2 instances", "top.u[0].d\ntop.u[1].d"));
    // One request for both instances (no active instance: nothing preferred).
    assert_eq!(last_svtrace_call(&bridge, &mut ed, "svtrace.step"), "path=nil;paths=top.u[0].d,top.u[1].d;prefer=nil;ancestors=");
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.step"), 1);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.atCursor"), 1);
    // The shared row looks as it does for one instance; the rows only one instance has say which.
    let (w, clk, q) = (at(&set.nodes, "top.u[0].d>top.u[0].w"), at(&set.nodes, "top.u[0].d>top.clk"), at(&set.nodes, "top.u[0].d>top.q"));
    // (A row for both instances names neither: its paths are in the tooltip.)
    assert_eq!(w.hint, "2");
    assert_eq!(w.tooltip, "top.u[0].w\ntop.u[1].w");
    assert_eq!((clk.hint.as_str(), clk.tooltip.as_str()), ("u[0] \u{b7} top", "top.clk\nu[0]"));
    assert_eq!((q.hint.as_str(), q.tooltip.as_str()), ("1 \u{b7} u[1] \u{b7} top", "top.q\nu[1]"));
    // Opening the shared row sends both of its paths, and the paths of the rows above as ancestors.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::TreeExpand { view: "slang_trace".into(), id: w.id.clone(), gen: set.gen });
    let kids = match wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TreeSet(s) if s.view == "slang_trace" && s.parent.as_deref() == Some("top.u[0].d>top.u[0].w"))) {
        Some(GuiEvent::TreeSet(s)) => s,
        _ => panic!("no children of w"),
    };
    assert_eq!(ids(&kids.nodes), ["top.u[0].d>top.u[0].w>top.p", "top.u[0].d>top.u[0].w>top.r"]);
    assert_eq!(
        last_svtrace_call(&bridge, &mut ed, "svtrace.step"),
        "path=nil;paths=top.u[0].w,top.u[1].w;prefer=nil;ancestors=top.u[0].d,top.u[1].d,top.u[0].w,top.u[1].w"
    );
    // The children cover both instances of w: no instance hint of theirs.
    assert!(kids.nodes.iter().all(|n| !n.hint.contains(" of ")));
    // Refresh asks again for the same instances (what is open stays open).
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; require('eshvi.slang.trace').refresh({ expanded = { 'top.u[0].d', 'top.u[0].d>top.u[0].w' } })".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.name == 'svtrace' and c.params.command == 'svtrace.step' end, FAKE_CALLS) >= 2)", "true");
    assert_eq!(
        last_svtrace_call(&bridge, &mut ed, "svtrace.step"),
        "path=nil;paths=top.u[0].w,top.u[1].w;prefer=nil;ancestors=top.u[0].d,top.u[1].d,top.u[0].w,top.u[1].w"
    );
    // Endpoints from the cursor are one request for the instances too.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command("call cursor(4, 1) | lua require('eshvi.slang.trace').endpoints('drivers')".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.name == 'svtrace' and c.params.command == 'svtrace.endpoints' end, FAKE_CALLS))", "1");
    assert_eq!(last_svtrace_call(&bridge, &mut ed, "svtrace.endpoints"), "path=nil;paths=top.u[0].d,top.u[1].d;prefer=nil;ancestors=nil");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_active_instance_is_preferred_not_a_filter_and_a_trace_started_on_one_path_stays_one_path() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session_with("trace-merged-prefer", true);
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command(MERGED_GRAPH.into()));
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; require('eshvi.slang.hierarchy').active_paths[vim.lsp.get_clients({ name = 'slang' })[1].id] = 'top.u[1]'".into()));
    bridge.send(UiCommand::Command("call cursor(4, 1) | lua require('eshvi.slang.trace').trace('drivers')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    // Both instances still trace; the active one's paths come first and it is the root's path.
    assert_eq!((set.nodes[0].id.as_str(), set.nodes[0].hint.as_str()), ("top.u[1].d", "Drivers \u{b7} 3 \u{b7} 2 instances"));
    assert_eq!(last_svtrace_call(&bridge, &mut ed, "svtrace.step"), "path=nil;paths=top.u[1].d,top.u[0].d;prefer=top.u[1];ancestors=");
    assert!(ids(&set.nodes).contains(&"top.u[1].d>top.u[1].w".to_string()), "{:?}", ids(&set.nodes));
    // A trace from a row ("trace from here") is that one path.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command("lua require('eshvi.slang.trace').trace_path('top.u[1].w', 'drivers')".into()));
    tree_set_of(&bridge, &mut ed, "slang_trace", |s| s.nodes.first().is_some_and(|n| n.id == "top.u[1].w"));
    assert_eq!(last_svtrace_call(&bridge, &mut ed, "svtrace.step"), "path=top.u[1].w;paths=nil;prefer=nil;ancestors=");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_svtrace_without_at_cursor_leaves_the_cursor_to_slang_server_and_a_selected_label_survives_brackets_and_fields() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-selected-no-cursor");
    trace_ready(&bridge, &mut ed);
    bridge.send(UiCommand::Command("lua fake_svtrace({ no_cursor = true })".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'svtrace' }))", "1");
    gui_for(&bridge, &mut ed, 300);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command("call cursor(3, 1) | lua require('eshvi.slang.trace').trace('drivers')".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(set.nodes[0].id, "top.q");
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getInstances"), 1);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.atCursor"), 0);
    // Labels and instances of paths with brackets, a field after a select, a package and a generate block.
    bridge.send(UiCommand::Command(
        "lua SLANG.drivers['top.g[2].s.f[1:0]'] = { 'top.u[0].v[3].w', 'pkg::p', 'top.g[2].t[0][1]' }; \
         SLANG.drivers['top.u[0].v[3].w'] = {}; SLANG.drivers['pkg::p'] = {}; SLANG.drivers['top.g[2].t[0][1]'] = {}; \
         SLANG.signals = { ['top.g[2].s.f[1:0]'] = 'top.g[2].s', ['top.g[2].t[0][1]'] = 'top.g[2].t' }"
            .into(),
    ));
    let set = trace_start(&bridge, &mut ed, "top.g[2].s.f[1:0]", "drivers");
    let root = "top.g[2].s.f[1:0]";
    assert_eq!(set.nodes[0].label, "f[1:0]");
    let w = at(&set.nodes, &format!("{root}>top.u[0].v[3].w"));
    assert_eq!((w.label.as_str(), w.hint.as_str()), ("w", "v[3]"), "a field after a select; its instance is up to the last dot");
    assert_eq!(at(&set.nodes, &format!("{root}>pkg::p")).label, "p");
    let t = at(&set.nodes, &format!("{root}>top.g[2].t[0][1]"));
    assert_eq!((t.label.as_str(), t.hint.as_str()), ("t[0][1]", "g[2]"));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The hierarchy with svtrace attached ----

#[test]
fn with_svtrace_advertising_the_scope_commands_the_hierarchy_asks_it_and_the_rest_stays_with_slang_server() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("hierarchy-svtrace-scopes");
    // svtrace alone brings the view (and answers it); slang-server attaches after.
    bridge.send(UiCommand::Command("lua fake_svtrace({ scopes = true })".into()));
    // The same nodes as with slang-server alone.
    let set = slang_roots(&bridge, &mut ed);
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'svtrace' }))", "1");
    attach_slang(&bridge, &mut ed);
    assert_eq!(ids(&set.nodes), ["top", "pkg"]);
    let kids = slang_children(&bridge, &mut ed, "top", set.gen);
    assert_eq!(ids(&kids.nodes), ["top.clk", "top.q", "top.io", "top.WIDTH", "top.u"]);
    let elements = slang_children(&bridge, &mut ed, "top.u", set.gen);
    assert_eq!(ids(&elements.nodes), ["top.u[0]", "top.u[1]"]);
    let package = slang_children(&bridge, &mut ed, "pkg", set.gen);
    assert_eq!(ids(&package.nodes), ["pkg::x"]);
    // The scope requests went to svtrace, none to slang-server.
    assert!(commands_to(&bridge, &mut ed, "svtrace", "slang.getScopes") > 0);
    assert!(commands_to(&bridge, &mut ed, "svtrace", "slang.getScope") >= 3);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getScope"), 0);
    // Accepting, a reveal and the active instance are slang-server's.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    tree_accept(&bridge, "slang", "top.u[0]", "open");
    wait_for(&bridge, &mut ed, "vim.inspect(vim.api.nvim_win_get_cursor(0))", "{ 21, 4 }");
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.setActiveInstance"), 1);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.showHierLocation"), 0);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "slang.setActiveInstance"), 0);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "slang.showHierLocation"), 0);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; require('eshvi.slang.hierarchy').reveal('top.u[1]', { show = false })".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.name == 'svtrace' and c.params.command == 'slang.getScopes' and c.params.arguments[1] == 'top.u[1]' end, FAKE_CALLS))", "1");
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getScope"), 0);
    // An error from svtrace is the message, as slang-server's would be.
    bridge.send(UiCommand::Command("lua SLANG.fail = 'svtrace: no design: boom'; _G.notes = {}; require('eshvi.slang.hierarchy').refresh({ expanded = {} })".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang", |s| s.message.as_deref().is_some_and(|m| m.contains("boom")));
    assert!(set.message.unwrap().starts_with("No design: "));
    // svtrace without a design (a build it doesn't support): slang-server's design is shown instead.
    bridge.send(UiCommand::Command(
        "lua SLANG.fail = nil; SLANG.svtrace_fail = 'svtrace: no design: no build configured'; _G.FAKE_CALLS = {}; \
         require('eshvi.slang.hierarchy').refresh({ expanded = {} })"
            .into(),
    ));
    let set = tree_set_of(&bridge, &mut ed, "slang", |s| !s.nodes.is_empty());
    assert_eq!(ids(&set.nodes), ["top", "pkg"]);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "slang.getScopes"), 1);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getScopes"), 1);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn with_an_svtrace_that_lacks_the_scope_commands_or_none_the_hierarchy_asks_slang_server() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("hierarchy-svtrace-no-scopes");
    // svtrace attached but without the commands (the trace's own fake).
    attach_svtrace_steps(&bridge, &mut ed);
    attach_slang(&bridge, &mut ed);
    let set = slang_roots(&bridge, &mut ed);
    assert_eq!(ids(&set.nodes), ["top", "pkg"]);
    slang_children(&bridge, &mut ed, "top", set.gen);
    assert!(commands_to(&bridge, &mut ed, "slang", "slang.getScopes") > 0);
    assert!(commands_to(&bridge, &mut ed, "slang", "slang.getScope") >= 1);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "slang.getScope"), 0);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// While Neovim runs a command that doesn't return (`system()`, `:!cmd`),
/// the calls that don't answer then (focus, resize, paste) must not hold up
/// the keys behind them: Ctrl-C reaches Neovim and ends the command at once.
#[test]
fn keys_get_through_while_neovim_runs_a_command() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 60, 10);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));

    bridge.send(UiCommand::Input(
        ":let g:t0 = reltime() | try | call system('sleep 5') | catch | endtry | let g:took = reltimefloat(reltime(g:t0))<CR>".into(),
    ));
    pump_for(&bridge, &mut ed, Duration::from_millis(500), |_| false);
    // Each of these waits for Neovim to be free again.
    bridge.send(UiCommand::SetFocus(false));
    bridge.send(UiCommand::SetFocus(true));
    bridge.send(UiCommand::TryResize { width: 50, height: 8 });
    bridge.send(UiCommand::Paste("pasted".into()));
    bridge.send(UiCommand::Input("<C-c>".into()));
    let took: f64 = ask(&bridge, &mut ed, "vim.g.took").parse().unwrap();
    assert!(took < 3.0, "the sleep ran {took}s: Ctrl-C waited behind another call");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// A file name with `]==]` and quotes in it goes to `changes_diff` as an
/// argument: the diff opens, and nothing in the name runs.
#[test]
fn a_hostile_file_name_opens_its_diff_and_runs_nothing() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let repo = std::env::temp_dir().join(format!("eshvi-hostile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    let name = "x]==]);vim.g.pwned=1;--[==['q\".txt";
    git(&["init", "-q"]);
    std::fs::write(repo.join(name), "one\n").unwrap();
    git(&["add", "."]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    std::fs::write(repo.join(name), "one\ntwo\n").unwrap();

    let bridge = spawn(&["--clean", "-n"], 100, 24);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let top = repo.canonicalize().unwrap();
    let s = |v: &str| nvim_bridge::RpcValue::from(v);
    let args = vec![s(&top.to_string_lossy()), s("HEAD"), s(name), nvim_bridge::RpcValue::from(false), s("tab")];
    bridge.send(UiCommand::Lua { code: "return Eshvi.changes_diff(...)".into(), args });
    // The file's side shows "two"; the snapshot's side doesn't.
    let two = |ed: &Editor| {
        ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal).any(|w| ed.grid_text(w.grid).iter().any(|l| l.contains("two")))
    };
    assert!(pump_for(&bridge, &mut ed, Duration::from_secs(5), two), "{:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>());
    assert_eq!(ask(&bridge, &mut ed, "vim.g.pwned"), "nil");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(repo);
}

/// Only a save with the file's Changes diff on screen is reported as the end
/// of undoing its hunks (which may remove a file the agents created); the
/// same file saved otherwise is just saved.
#[test]
fn a_save_is_reported_as_from_the_diff_only_in_its_view() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let repo = std::env::temp_dir().join(format!("eshvi-diffsave-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&["add", "."]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    let bridge = spawn(&["--clean", "-n"], 100, 24);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let top = repo.canonicalize().unwrap();
    let s = |v: &str| nvim_bridge::RpcValue::from(v);
    let args = vec![s(&top.to_string_lossy()), s("HEAD"), s("a.txt"), nvim_bridge::RpcValue::from(false), s("tab")];
    bridge.send(UiCommand::Lua { code: "return Eshvi.changes_diff(...)".into(), args });
    wait_for(&bridge, &mut ed, "#vim.api.nvim_list_tabpages()", "2");
    // In the diff view: reported.
    bridge.send(UiCommand::Input("Gox<Esc>:w<CR>".into()));
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::DiffSaved(p) if p.ends_with("a.txt"))).is_some(), "no gui_diff_saved");
    // Out of it: a plain save.
    bridge.send(UiCommand::Command("tabclose".into()));
    wait_for(&bridge, &mut ed, "#vim.api.nvim_list_tabpages()", "1");
    bridge.send(UiCommand::Command(format!("edit {}", top.join("a.txt").display())));
    // (A command takes the lane, keys don't: wait for the first before the second.)
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "a.txt");
    bridge.send(UiCommand::Input("Goy<Esc>:w<CR>".into()));
    let diff_saved = std::cell::Cell::new(false);
    let saved = wait_gui(&bridge, &mut ed, |g| {
        diff_saved.set(diff_saved.get() || matches!(g, GuiEvent::DiffSaved(_)));
        matches!(g, GuiEvent::Saved(_))
    });
    assert!(saved.is_some());
    pump_for(&bridge, &mut ed, Duration::from_millis(300), |_| false);
    assert!(!diff_saved.get(), "a plain save was reported as from the diff");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(repo);
}

/// The diff view's revert goes through Neovim: the change's lines are replaced
/// in the file's buffer and the file is written, with the buffer left clean,
/// and `u` takes it back; a buffer with unsaved changes, or a file that
/// changed since the view read it, is refused and left as it was; a file the
/// agents made is written empty (the GUI then removes it), and one they
/// deleted comes back whole.
#[test]
fn the_diff_views_revert_edits_the_buffer_and_writes_the_file() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-revert-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, "one\nCHANGED\nthree\nfour\n").unwrap();
    let bridge = spawn(&["--clean", "-n"], 100, 24);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let lines = |v: &[&str]| nvim_bridge::RpcValue::Array(v.iter().map(|l| nvim_bridge::RpcValue::from(*l)).collect());
    let revert = |bridge: &Bridge, ed: &mut Editor, path: &std::path::Path, start: i64, expect: &[&str], put: &[&str]| -> (bool, String) {
        let args = vec![nvim_bridge::RpcValue::from(path.to_string_lossy().as_ref()), nvim_bridge::RpcValue::from(start), lines(expect), lines(put)];
        bridge.send(UiCommand::Lua { code: "return Eshvi.diff_revert(...)".into(), args });
        match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::DiffEdit { op, .. } if op == "revert")) {
            Some(GuiEvent::DiffEdit { ok, message, .. }) => (ok, message),
            other => panic!("no answer: {other:?}"),
        }
    };
    bridge.send(UiCommand::Command(format!("edit {}", file.display())));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "a.txt");

    // The change put back: the buffer and the file, and the buffer isn't modified.
    let (ok, _) = revert(&bridge, &mut ed, &file, 1, &["CHANGED"], &["two", "2b"]);
    assert!(ok);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\ntwo\n2b\nthree\nfour\n");
    assert_eq!(ask(&bridge, &mut ed, "table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), ',')"), "one,two,2b,three,four");
    assert_eq!(ask(&bridge, &mut ed, "vim.bo.modified"), "false");
    // Undo takes it back in the buffer (the file keeps what it has until written).
    bridge.send(UiCommand::Command("undo".into()));
    wait_for(&bridge, &mut ed, "table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), ',')", "one,CHANGED,three,four");

    // Unsaved changes of the user's: refused, nothing written. (The buffer is the current one.)
    bridge.send(UiCommand::Input("Gox<Esc>".into()));
    wait_for(&bridge, &mut ed, "vim.bo.modified", "true");
    let before = std::fs::read_to_string(&file).unwrap();
    let (ok, message) = revert(&bridge, &mut ed, &file, 0, &["one"], &["1"]);
    assert!(!ok && message.contains("unsaved changes"), "{message}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    bridge.send(UiCommand::Command("write".into()));
    wait_for(&bridge, &mut ed, "vim.bo.modified", "false");
    std::fs::write(&file, "one\nCHANGED\nthree\nfour\n").unwrap();
    bridge.send(UiCommand::Command("silent! checktime".into()));
    wait_for(&bridge, &mut ed, "table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), ',')", "one,CHANGED,three,four");

    // The file isn't as the view read it: refused.
    let (ok, message) = revert(&bridge, &mut ed, &file, 1, &["SOMETHING ELSE"], &["two"]);
    assert!(!ok && message.contains("changed since"), "{message}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\nCHANGED\nthree\nfour\n");

    // A file in no buffer yet is loaded for it, and a whole file made by the agents goes back to empty.
    let made = dir.join("made.txt");
    std::fs::write(&made, "x\ny\n").unwrap();
    let (ok, _) = revert(&bridge, &mut ed, &made, 0, &["x", "y"], &[]);
    assert!(ok);
    assert_eq!(std::fs::metadata(&made).unwrap().len(), 0, "written empty: the GUI removes it then");
    // The existing removal flow: the GUI removes the file and tells Neovim, whose buffer of it goes.
    let exists = format!("vim.fn.bufexists('{}')", made.display());
    assert_eq!(ask(&bridge, &mut ed, &exists), "1");
    std::fs::remove_file(&made).unwrap();
    let args = vec![nvim_bridge::RpcValue::from(made.to_string_lossy().as_ref()), nvim_bridge::RpcValue::from("made.txt")];
    bridge.send(UiCommand::Lua { code: "return Eshvi.changes_removed(...)".into(), args });
    wait_for(&bridge, &mut ed, &exists, "0");
    // A file the agents deleted comes back as it was, without a blank line more.
    let gone = dir.join("sub").join("gone.txt");
    let (ok, _) = revert(&bridge, &mut ed, &gone, 0, &[], &["a", "b"]);
    assert!(ok);
    assert_eq!(std::fs::read_to_string(&gone).unwrap(), "a\nb\n");
    // And a change at the end of a file with the file's last line.
    let (ok, _) = revert(&bridge, &mut ed, &gone, 1, &["b"], &["B", "C"]);
    assert!(ok);
    assert_eq!(std::fs::read_to_string(&gone).unwrap(), "a\nB\nC\n");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// A discard is guarded: with unsaved changes of the file in Neovim (even in a
/// buffer no window shows) it is refused.
#[test]
fn the_diff_views_discard_is_refused_for_a_file_with_unsaved_changes() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-guard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "one\n").unwrap();
    std::fs::write(&b, "two\n").unwrap();
    let bridge = spawn(&["--clean", "-n", "--cmd", "set hidden"], 100, 24);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let guard = |bridge: &Bridge, ed: &mut Editor, paths: &[&std::path::Path]| -> bool {
        let list = nvim_bridge::RpcValue::Array(paths.iter().map(|p| nvim_bridge::RpcValue::from(p.to_string_lossy().as_ref())).collect());
        bridge.send(UiCommand::Lua { code: "return Eshvi.diff_guard(...)".into(), args: vec![list] });
        match wait_gui(bridge, ed, |g| matches!(g, GuiEvent::DiffEdit { op, .. } if op == "guard")) {
            Some(GuiEvent::DiffEdit { ok, .. }) => ok,
            other => panic!("no answer: {other:?}"),
        }
    };
    bridge.send(UiCommand::Command(format!("edit {}", a.display())));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "a.txt");
    assert!(guard(&bridge, &mut ed, &[&a, &b]), "nothing unsaved");
    bridge.send(UiCommand::Input("Ox<Esc>".into()));
    wait_for(&bridge, &mut ed, "vim.bo.modified", "true");
    assert!(!guard(&bridge, &mut ed, &[&a]));
    // The modified buffer goes out of its window (hidden): still guarded.
    bridge.send(UiCommand::Command(format!("edit {}", b.display())));
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "b.txt");
    assert!(!guard(&bridge, &mut ed, &[&b, &a]));
    assert!(guard(&bridge, &mut ed, &[&b]));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// CODE_REVIEW M6: a Neovim that dies is not a Neovim that quit. Killed by a
/// signal (a crash), or gone with a status other than 0, `Exited` says so,
/// and the window can show it instead of vanishing.
#[test]
fn a_neovim_that_dies_says_how() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let ended = |how: &str| -> Option<Option<String>> {
        let bridge = spawn(&["--clean", "-n"], 60, 10);
        let mut ed = Editor::new();
        assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
        bridge.send(UiCommand::Command(how.into()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            while let Some(ev) = bridge.try_recv() {
                if let BridgeEvent::Exited { error } = ev {
                    return Some(error);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    };
    // (This Neovim ends itself: no other process is signalled.)
    let killed = ended("lua vim.uv.kill(vim.uv.os_getpid(), 'sigkill')").expect("no Exited after the kill");
    assert_eq!(killed.as_deref(), Some("Neovim was killed by signal 9 (SIGKILL)"));
    let status = ended("cquit 3").expect("no Exited after :cquit");
    assert_eq!(status.as_deref(), Some("Neovim exited with status 3"));
    let quit = ended("qa!").expect("no Exited after :qa!");
    assert_eq!(quit, None);
}

/// CODE_REVIEW M2: while no frame takes them (a minimized window, a locked
/// screen), redraw batches don't pile up without end: past a limit they are
/// applied to the editor the UI parked with the bridge, and the UI finds the
/// screen as it is when it comes back.
#[test]
fn redraws_nobody_takes_are_applied_to_the_parked_editor() {
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 60, 10);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    // The frame is over: the editor waits with the bridge, and nothing is taken.
    bridge.park(&mut ed);
    let lua = "for i = 1, 1500 do vim.api.nvim_buf_set_lines(0, 0, -1, false, { 'line ' .. i }) vim.cmd('redraw') end vim.g.done = 1";
    bridge.send(UiCommand::Lua { code: lua.into(), args: vec![] });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut most = 0;
    let mut quiet = 0;
    while Instant::now() < deadline && quiet < 25 {
        let queued = bridge.queued_redraws();
        most = most.max(queued);
        std::thread::sleep(Duration::from_millis(20));
        quiet = if bridge.queued_redraws() == queued { quiet + 1 } else { 0 };
    }
    assert!(most <= 256, "{most} batches were waiting");
    // Back: batches were applied meanwhile, and the rest come as ever.
    assert!(bridge.unpark(&mut ed), "nothing was applied to the parked editor");
    assert!(pump(&bridge, &mut ed, |ed| window_text(ed).first().is_some_and(|l| l.trim_end() == "line 1500")), "{:?}", window_text(&ed));
    bridge.send(UiCommand::Command("qa!".into()));
}

/// CODE_REVIEW L18: moving a folder renames its buffers; one whose new name
/// another buffer already has keeps its old name, and the ones after it
/// still follow (the refused rename used to end the loop).
#[test]
fn buffers_follow_a_move_past_one_whose_name_is_taken() {
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-fileops-taken-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("old")).unwrap();
    for f in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join("old").join(f), "x\n").unwrap();
    }
    let dir = dir.canonicalize().unwrap();
    let (old, new) = (dir.join("old"), dir.join("new"));
    let open = |f: &str| old.join(f).to_string_lossy().into_owned();
    let bridge = spawn(&["--clean", "-n", &open("a.txt"), &open("b.txt"), &open("c.txt")], 100, 10);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    // A buffer in a window already has the name `old/b.txt` moves to.
    bridge.send(UiCommand::Command(format!("split {}", new.join("b.txt").display())));
    // (The split is there before the move, or under load the move can come first and nothing is taken.)
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal).count() == 2));
    std::fs::rename(&old, &new).unwrap();
    bridge.send(UiCommand::Renamed { from: old.clone(), to: new.clone() });
    let names = "join(sort(map(filter(range(1, bufnr('$')), 'buflisted(v:val)'), 'fnamemodify(bufname(v:val), \":p:h:t\") . \"/\" . fnamemodify(bufname(v:val), \":t\")')), ' ')";
    let want = "new/a.txt new/b.txt new/c.txt old/b.txt";
    let mut found = false;
    for _ in 0..25 {
        ed.messages.clear();
        bridge.send(UiCommand::Input(format!(":echo {names}<CR>")));
        if pump_for(&bridge, &mut ed, Duration::from_millis(200), |ed| ed.messages.iter().any(|m| m.text() == want)) {
            found = true;
            break;
        }
    }
    assert!(found, "{:?}", ed.messages.iter().map(|m| m.text()).collect::<Vec<_>>());
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Slang support in the app (phase 7): eshvi.slang ----

/// The `workspace/executeCommand` calls to client `client` of `command`, each as `args` (JSON).
fn calls_of(bridge: &Bridge, ed: &mut Editor, client: &str, command: &str) -> Vec<String> {
    let out = ask(
        bridge,
        ed,
        &format!(
            "table.concat(vim.tbl_map(function(c) return vim.json.encode(c.params.arguments) end, vim.tbl_filter(function(c) return c.name == '{client}' and c.method == 'workspace/executeCommand' and c.params.command == '{command}' end, FAKE_CALLS)), '|')"
        ),
    );
    out.split('|').filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

#[test]
fn svtrace_alone_brings_the_hierarchy_and_the_trace_and_other_servers_bring_nothing() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("slang-svtrace-alone");
    assert!(!gui_for(&bridge, &mut ed, 300).iter().any(|g| matches!(g, GuiEvent::TreeRegister(_))), "no server, no view");
    bridge.send(UiCommand::Command("lua fake_lsp('other', {})".into()));
    assert!(!gui_for(&bridge, &mut ed, 300).iter().any(|g| matches!(g, GuiEvent::TreeRegister(_))));
    bridge.send(UiCommand::Command("lua fake_svtrace({ scopes = true })".into()));
    let seen = gui_for(&bridge, &mut ed, 600);
    let views: Vec<_> = seen.iter().filter_map(|g| if let GuiEvent::TreeRegister(r) = g { Some(r.view.as_str()) } else { None }).collect();
    assert_eq!(views, ["slang", "slang_trace"], "{seen:?}");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn vim_g_eshvi_slang_false_turns_the_slang_support_off() {
    if !nvim_available() {
        return;
    }
    let dir = picker_dir("slang-off");
    let file = dir.join("top.sv");
    std::fs::write(&file, "// x\n").unwrap();
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    let (bridge, mut ed) = picker_session_with(false, true, &["--cmd", "let g:eshvi_slang = v:false", file.to_str().unwrap()]);
    wait_for(&bridge, &mut ed, "vim.g.gui_bridge_chan ~= nil", "true");
    for lua in ["fake_picker_lsp.lua", "fake_slang.lua"] {
        bridge.send(UiCommand::Command(format!("luafile {fake}/{lua}")));
    }
    wait_for(&bridge, &mut ed, "type(fake_slang)", "function");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ask(&bridge, &mut ed, "tostring(package.loaded['eshvi.slang'])"), "nil");
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.fn.exists(':EshviSlang'))"), "0");
    bridge.send(UiCommand::Command("lua fake_slang()".into()));
    assert!(!gui_for(&bridge, &mut ed, 600).iter().any(|g| matches!(g, GuiEvent::TreeRegister(_))), "no view with it off");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn slang_quick_pick_asks_the_native_picker_and_sends_on_select_command_with_the_chosen_value() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session_with("slang-quickpick", true);
    attach_slang(&bridge, &mut ed);
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'slang' }))", "1");
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command(
        "lua local c = vim.lsp.get_clients({ name = 'slang' })[1]; \
         vim.lsp.commands['slang.quickPick']({ command = 'slang.quickPick', arguments = { { placeholder = 'Pick one', onSelectCommand = 'slang.activateInstance', \
         items = { { label = 'a', description = 'first', value = 'va' }, { label = 'b', value = { path = 'vb' } } } } } }, { client_id = c.id, bufnr = vim.api.nvim_get_current_buf() })"
            .into(),
    ));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Pick one");
    assert_eq!(open.items.iter().map(|i| (i.text.as_str(), i.desc.as_str())).collect::<Vec<_>>(), [("a", "first"), ("b", "")]);
    accept(&bridge, open.id, vec![by_idx(2)], "open");
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'slang.activateInstance' end, FAKE_CALLS))", "1");
    assert_eq!(calls_of(&bridge, &mut ed, "slang", "slang.activateInstance"), [r#"[{"path":"vb"}]"#]);
    // Cancelled: nothing is sent. An invalid command is an error notification.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; _G.notes = {}".into()));
    bridge.send(UiCommand::Command(
        "lua vim.lsp.commands['slang.quickPick']({ command = 'slang.quickPick', arguments = { { items = 3 } } }, { client_id = 1 })".into(),
    ));
    wait_for(&bridge, &mut ed, "tostring(#_G.notes)", "1");
    assert!(ask(&bridge, &mut ed, "_G.notes[1]").starts_with("slang-server: invalid quick pick command"));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_hierarchy_search_asks_slang_search_hierarchy_per_query_and_reveals_the_choice() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session_with("slang-search", true);
    attach_slang(&bridge, &mut ed);
    slang_roots(&bridge, &mut ed);
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command("EshviSlang search".into()));
    let open = next_picker(&bridge, &mut ed);
    assert!(open.live && open.items.is_empty());
    assert_eq!((open.title.as_str(), open.hint.as_str()), ("Search hierarchy", "Type to search the hierarchy"));
    assert!(calls_of(&bridge, &mut ed, "slang", "slang.searchHierarchy").is_empty(), "nothing is asked until there is a query");
    bridge.send(UiCommand::PickerQuery { id: open.id, gen: 1, text: "u[".into() });
    let rows = picker_items_for(&bridge, &mut ed, 1);
    assert_eq!(rows.iter().map(|r| (r.text.as_str(), r.desc.as_str())).collect::<Vec<_>>(), [("top.u[0]", "a match"), ("top.u[1]", "a match")]);
    assert_eq!(calls_of(&bridge, &mut ed, "slang", "slang.searchHierarchy"), [r#"["u["]"#]);
    accept(&bridge, open.id, vec![by_idx(2)], "open");
    let seen = gui_for(&bridge, &mut ed, 600);
    assert_eq!(slang_selects(&seen), [("top.u[1]".to_string(), vec!["top".into(), "top.u".into(), "top.u[1]".into()])]);
    assert!(seen.iter().any(|g| matches!(g, GuiEvent::TreeShow { view, focus, .. } if view == "slang" && *focus)));
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn eshvi_slang_subcommands_send_the_servers_requests_and_complete() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, file) = slang_session_with("slang-command", true);
    // No server yet: each says so.
    bridge.send(UiCommand::Command("EshviSlang setBuildFile b.f".into()));
    bridge.send(UiCommand::Command("EshviSlang drivers".into()));
    wait_for(&bridge, &mut ed, "tostring(#_G.notes)", "2");
    assert!(ask(&bridge, &mut ed, "_G.notes[1]").contains("no slang-server LSP client attached"));
    assert!(ask(&bridge, &mut ed, "_G.notes[2]").contains("needs an attached slang-server or svtrace"));
    attach_slang(&bridge, &mut ed);
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'slang' }))", "1");
    // Completion: the subcommands, then a file or true/false.
    assert_eq!(
        ask(&bridge, &mut ed, "table.concat(vim.fn.getcompletion('EshviSlang ', 'cmdline'), ' ')"),
        "hierarchy search drivers loads driverEndpoints loadEndpoints selectActive up setTopLevel setBuildFile addToWaves openWaveform"
    );
    assert_eq!(ask(&bridge, &mut ed, "table.concat(vim.fn.getcompletion('EshviSlang d', 'cmdline'), ' ')"), "drivers driverEndpoints");
    assert_eq!(ask(&bridge, &mut ed, "table.concat(vim.fn.getcompletion('EshviSlang addToWaves t', 'cmdline'), ' ')"), "true");
    assert_eq!(ask(&bridge, &mut ed, "table.concat(vim.fn.getcompletion('EshviSlang setTopLevel to', 'cmdline'), ' ')"), "top.sv");
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    let (dir_s, file_s) = (dir.display().to_string(), file.display().to_string());
    for name in ["b.f", "w.vcd"] {
        std::fs::write(dir.join(name), "").unwrap();
    }
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // A relative file is in the server's workspace, wherever Neovim is (here a subfolder of it).
    bridge.send(UiCommand::Command("cd sub".into()));
    bridge.send(UiCommand::Command("EshviSlang setBuildFile b.f".into()));
    bridge.send(UiCommand::Command("EshviSlang setTopLevel".into()));
    bridge.send(UiCommand::Command("EshviSlang setTopLevel top.sv".into()));
    bridge.send(UiCommand::Command("EshviSlang openWaveform w.vcd".into()));
    wait_for(&bridge, &mut ed, "tostring(#FAKE_CALLS)", "4");
    assert_eq!(calls_of(&bridge, &mut ed, "slang", "slang.setBuildFile"), [format!(r#"["{dir_s}/b.f"]"#)]);
    assert_eq!(calls_of(&bridge, &mut ed, "slang", "slang.setTopLevel"), [format!(r#"["{file_s}"]"#), format!(r#"["{dir_s}/top.sv"]"#)]);
    assert_eq!(calls_of(&bridge, &mut ed, "slang", "slang.openWaveform"), [format!(r#"["{dir_s}/w.vcd"]"#)]);
    // Completion reads the workspace too.
    assert_eq!(ask(&bridge, &mut ed, "table.concat(vim.fn.getcompletion('EshviSlang setBuildFile b', 'cmdline'), ' ')"), "b.f");
    // A file that isn't there is refused before the server, which would take it and be left with no design.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; _G.notes = {}".into()));
    bridge.send(UiCommand::Command("EshviSlang setBuildFile nope.f".into()));
    wait_for(&bridge, &mut ed, "tostring(#_G.notes)", "1");
    assert!(ask(&bridge, &mut ed, "_G.notes[1]").contains(&format!("no such file: {dir_s}/nope.f")));
    assert!(calls_of(&bridge, &mut ed, "slang", "slang.setBuildFile").is_empty());
    bridge.send(UiCommand::Command("cd ..".into()));
    // addToWaves: the instances at the cursor (line 4 has several), a picker, then the choice with `recursive`.
    bridge.send(UiCommand::Command("call cursor(4, 1) | EshviSlang addToWaves true".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Add instance to waves");
    accept(&bridge, open.id, vec![by_idx(1)], "open");
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'slang.addToWaveform' end, FAKE_CALLS))", "1");
    let waves = calls_of(&bridge, &mut ed, "slang", "slang.addToWaveform");
    // (Decoded: the key order of Lua's JSON isn't fixed.)
    assert_eq!(waves.iter().map(|w| serde_json::from_str::<serde_json::Value>(w).unwrap()).collect::<Vec<_>>(), [serde_json::json!([{ "path": "top.u[0].d", "recursive": true }])]);
    // selectActive: the lens on the cursor's line is run, which is slang.quickPick: its picker, then slang.activateInstance.
    bridge.send(UiCommand::Command("call cursor(2, 1) | EshviSlang selectActive".into()));
    let open = next_picker(&bridge, &mut ed);
    assert_eq!(open.title, "Select active instance");
    accept(&bridge, open.id, vec![by_idx(1)], "open");
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'slang.activateInstance' end, FAKE_CALLS))", "1");
    assert_eq!(calls_of(&bridge, &mut ed, "slang", "slang.activateInstance"), [r#"["top.u[0]"]"#]);
    // A line without that lens: a warning.
    bridge.send(UiCommand::Command("lua _G.notes = {}".into()));
    bridge.send(UiCommand::Command("call cursor(5, 1) | EshviSlang selectActive".into()));
    wait_for(&bridge, &mut ed, "tostring(#_G.notes)", "1");
    assert!(ask(&bridge, &mut ed, "_G.notes[1]").starts_with("No active-selection code lens found"));
    // The server refusing is an error notification.
    bridge.send(UiCommand::Command("lua _G.notes = {}; SLANG.build_fail = 'nope'".into()));
    bridge.send(UiCommand::Command("EshviSlang setBuildFile b.f".into()));
    wait_for(&bridge, &mut ed, "tostring(#_G.notes)", "1");
    assert!(ask(&bridge, &mut ed, "_G.notes[1]").starts_with("nope"));
    // The trace commands run the trace (drivers of the signal at line 3).
    bridge.send(UiCommand::Command("lua SLANG.build_fail = nil".into()));
    bridge.send(UiCommand::Command("call cursor(3, 1) | EshviSlang drivers".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(set.nodes[0].id, "top.q");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// A folder with executable stand-ins for `svtrace` and `slang`.
fn fake_bin(name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = picker_dir(name);
    for tool in ["svtrace", "slang"] {
        let path = dir.join(tool);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// A session with `bin` as `ESHVI_BIN`, the app's slang support on and these `--cmd`s (no file given).
fn svtrace_session(bin: &std::path::Path, cmds: &[&str]) -> (Bridge, Editor) {
    let mut all = vec!["--clean".to_string(), "-n".to_string(), "-i".to_string(), "NONE".to_string()];
    for c in cmds {
        all.extend(["--cmd".to_string(), c.to_string()]);
    }
    let bridge = Bridge::spawn(
        SpawnOptions { args: plain_shell(all), width: 80, height: 20, slang: true, bin_dir: Some(bin.to_path_buf()), ..Default::default() },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    wait_for(&bridge, &mut ed, "tostring(package.loaded['eshvi.slang'] ~= nil)", "true");
    (bridge, ed)
}

const SVTRACE_CMD: &str = "vim.inspect(vim.lsp.config.svtrace and vim.lsp.config.svtrace.cmd)";

#[test]
fn svtrace_is_set_up_from_eshvi_bin_and_a_cmd_the_config_set_wins_in_either_order() {
    if !nvim_available() {
        return;
    }
    let bin = fake_bin("svtrace-bin");
    let (b, s) = (bin.display().to_string(), bin.join("slang").display().to_string());
    let shipped = format!(r#"{{ "{b}/svtrace", "--slang", "{s}" }}"#);
    let sv = bin.join("x.sv");
    std::fs::write(&sv, "module x; endmodule\n").unwrap();
    let open = format!("edit {}", sv.display());

    // Nothing from the config: the shipped pair, started with the first SystemVerilog buffer (not before).
    let (bridge, mut ed) = svtrace_session(&bin, &[]);
    assert_eq!(ask(&bridge, &mut ed, SVTRACE_CMD), shipped);
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.lsp.config.svtrace.root_markers) .. vim.inspect(vim.lsp.config.svtrace.filetypes)"), r#"{ ".slang" }{ "systemverilog", "verilog" }"#);
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.lsp.is_enabled('svtrace'))"), "false");
    bridge.send(UiCommand::Command(open.clone()));
    wait_for(&bridge, &mut ed, "tostring(vim.lsp.is_enabled('svtrace'))", "true");
    bridge.send(UiCommand::Command("qa!".into()));

    // The config first (before the app's setup): its cmd stays, the config enables it.
    let (bridge, mut ed) = svtrace_session(&bin, &["lua vim.lsp.config('svtrace', { cmd = { '/mine/svtrace' }, filetypes = { 'verilog' } })"]);
    assert_eq!(ask(&bridge, &mut ed, SVTRACE_CMD), r#"{ "/mine/svtrace" }"#);
    assert_eq!(ask(&bridge, &mut ed, "vim.inspect(vim.lsp.config.svtrace.filetypes)"), r#"{ "verilog" }"#);
    bridge.send(UiCommand::Command(open.clone()));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.lsp.is_enabled('svtrace'))"), "false", "the config did not enable it: the app doesn't");
    bridge.send(UiCommand::Command("qa!".into()));

    // The config after the app's setup (a lazy plugin, on BufReadPre): the same result.
    let (bridge, mut ed) = svtrace_session(&bin, &[]);
    bridge.send(UiCommand::Command("lua vim.lsp.config('svtrace', { cmd = { '/mine/svtrace' } })".into()));
    assert_eq!(ask(&bridge, &mut ed, SVTRACE_CMD), r#"{ "/mine/svtrace" }"#);
    bridge.send(UiCommand::Command(open.clone()));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(ask(&bridge, &mut ed, "tostring(vim.lsp.is_enabled('svtrace'))"), "false");
    // ... and if the config enables it too, it runs with its cmd.
    bridge.send(UiCommand::Command("lua vim.lsp.enable('svtrace')".into()));
    assert_eq!(ask(&bridge, &mut ed, SVTRACE_CMD), r#"{ "/mine/svtrace" }"#);
    bridge.send(UiCommand::Command("qa!".into()));

    // Without the pair beside the app: nothing is set.
    let empty = picker_dir("svtrace-no-bin");
    let (bridge, mut ed) = svtrace_session(&empty, &[]);
    assert_eq!(ask(&bridge, &mut ed, SVTRACE_CMD), "nil");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(bin);
    let _ = std::fs::remove_dir_all(empty);
}

// ---- Chosen builds: svtrace follows the build slang-server uses ----

#[test]
fn a_build_file_or_top_level_set_on_slang_server_is_mirrored_to_svtrace_when_it_succeeds() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("slang-builds");
    attach_slang(&bridge, &mut ed);
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'slang' }))", "1");
    bridge.send(UiCommand::Command("lua fake_svtrace({ scopes = true })".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'svtrace' }))", "1");
    gui_for(&bridge, &mut ed, 300);
    let use_build = |bridge: &Bridge, ed: &mut Editor| calls_of(bridge, ed, "svtrace", "svtrace.useBuild");
    let send_to_server = |bridge: &Bridge, command: &str, arg: &str| {
        bridge.send(UiCommand::Command(format!(
            "lua vim.lsp.get_clients({{ name = 'slang' }})[1]:request('workspace/executeCommand', {{ command = '{command}', arguments = {{ {arg} }} }}, function() end)"
        )));
    };
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));

    // A build file: the same file. (Sent as anything would: a code lens, a quick pick.)
    send_to_server(&bridge, "slang.setBuildFile", "'other.f'");
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'svtrace.useBuild' end, FAKE_CALLS))", "1");
    assert_eq!(use_build(&bridge, &mut ed), [r#"[{"file":"other.f"}]"#]);
    // The hierarchy asks again for the other design.
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'slang.getScopes' end, FAKE_CALLS) > 0)", "true");

    // Explore mode ("" for either) and a top file (no build file) are no design: `file = null`, explore mode
    // marked, as a change of server.json takes it back to the configured build.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}".into()));
    send_to_server(&bridge, "slang.setTopLevel", "''");
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'svtrace.useBuild' end, FAKE_CALLS))", "1");
    send_to_server(&bridge, "slang.setBuildFile", "''");
    send_to_server(&bridge, "slang.setTopLevel", "'/w/top.sv'");
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'svtrace.useBuild' end, FAKE_CALLS))", "3");
    // (Compared as JSON: a Lua table's keys come out in any order.)
    let sent: Vec<serde_json::Value> = use_build(&bridge, &mut ed).iter().map(|c| serde_json::from_str(c).unwrap()).collect();
    let explore = serde_json::json!([{ "file": null, "explore": true }]);
    assert_eq!(sent, [explore.clone(), explore, serde_json::json!([{ "file": null }])]);

    // A request the server refused mirrors nothing, and neither does another command.
    bridge.send(UiCommand::Command("lua _G.FAKE_CALLS = {}; SLANG.build_fail = 'nope'".into()));
    send_to_server(&bridge, "slang.setBuildFile", "'bad.f'");
    send_to_server(&bridge, "slang.activateInstance", "'top.u[0]'");
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'slang.activateInstance' end, FAKE_CALLS))", "1");
    gui_for(&bridge, &mut ed, 300);
    assert!(use_build(&bridge, &mut ed).is_empty(), "a failed request is not mirrored");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_svtrace_that_starts_after_the_build_was_chosen_is_told_which() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = slang_session("slang-builds-late");
    attach_slang(&bridge, &mut ed);
    wait_for(&bridge, &mut ed, "tostring(#vim.lsp.get_clients({ name = 'slang' }))", "1");
    bridge.send(UiCommand::Command(
        "lua vim.lsp.get_clients({ name = 'slang' })[1]:request('workspace/executeCommand', { command = 'slang.setBuildFile', arguments = { 'later.f' } }, function() end)".into(),
    ));
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'slang.setBuildFile' end, FAKE_CALLS))", "1");
    std::thread::sleep(Duration::from_millis(200));
    bridge.send(UiCommand::Command("lua fake_svtrace()".into()));
    wait_for(&bridge, &mut ed, "tostring(#vim.tbl_filter(function(c) return c.params.command == 'svtrace.useBuild' end, FAKE_CALLS))", "1");
    assert_eq!(calls_of(&bridge, &mut ed, "svtrace", "svtrace.useBuild"), [r#"[{"file":"later.f"}]"#]);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_trace_while_svtrace_has_no_design_asks_slang_server_and_goes_back_to_svtrace_with_one() {
    if !nvim_available() {
        return;
    }
    let (bridge, mut ed, dir, _file) = trace_session("trace-no-design");
    trace_ready(&bridge, &mut ed);
    attach_svtrace_steps(&bridge, &mut ed);
    gui_for(&bridge, &mut ed, 300);
    // svtrace has no design (no build chosen): the trace takes slang-server's one-step path, no error shown.
    bridge.send(UiCommand::Command("lua SLANG.svtrace_no_design = true; _G.FAKE_CALLS = {}; _G.notes = {}".into()));
    bridge.send(UiCommand::Command("call cursor(3, 1) | EshviSlang drivers".into()));
    let set = tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(ids(&set.nodes), ["top.q", "top.q>top.a", "top.q>top.a>top.b", "top.q>top.a>top.b>top.c"]);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.step"), 0);
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.atCursor"), 0);
    assert!(commands_to(&bridge, &mut ed, "slang", "slang.getDriversWithLocation") >= 4);
    assert_eq!(commands_to(&bridge, &mut ed, "slang", "slang.getInstances"), 1);
    assert_eq!(ask(&bridge, &mut ed, "#_G.notes"), "0");
    // With a design again (a build chosen): svtrace answers.
    bridge.send(UiCommand::Command("lua SLANG.svtrace_no_design = false; _G.FAKE_CALLS = {}".into()));
    bridge.send(UiCommand::Command("call cursor(3, 1) | EshviSlang drivers".into()));
    tree_set_of(&bridge, &mut ed, "slang_trace", |s| !s.nodes.is_empty());
    assert_eq!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.atCursor"), 1);
    assert!(commands_to(&bridge, &mut ed, "svtrace", "svtrace.step") >= 1);
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// The colorscheme's git and diagnostic colors reach the GUI: the first group of each role
/// that has a foreground (links followed), again on every `:colorscheme`.
#[test]
fn decoration_colors_follow_the_colorscheme() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 40, 10);
    let mut ed = Editor::new();
    let Some(GuiEvent::DecoColors(first)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::DecoColors(_))) else {
        panic!("no gui_deco_colors")
    };
    // Whatever the default scheme has, only roles with a color are listed.
    assert!(first.values().all(|c| *c <= 0xffffff), "{first:?}");

    let lua = "vim.api.nvim_set_hl(0, 'GitSignsAdd', { fg = 0x111111 }) \
               vim.api.nvim_set_hl(0, 'Added', { fg = 0x222222 }) \
               vim.api.nvim_set_hl(0, 'Changed', { bg = 0x333333 }) \
               vim.api.nvim_set_hl(0, 'diffChanged', { link = 'Special' }) \
               vim.api.nvim_set_hl(0, 'Special', { fg = 0x444444 }) \
               vim.api.nvim_set_hl(0, 'DiagnosticWarn', { link = 'Special' }) \
               vim.api.nvim_exec_autocmds('ColorScheme', {})";
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    let Some(GuiEvent::DecoColors(colors)) =
        wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::DecoColors(c) if c.get("added") == Some(&0x111111)))
    else {
        panic!("colors not updated")
    };
    // GitSignsAdd before Added; a link is followed (diffChanged -> Special); a background-only
    // Changed is no color.
    assert_eq!(colors["modified"], 0x444444, "{colors:?}");
    assert_eq!(colors["warn"], 0x444444, "{colors:?}");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// The colorscheme's `DiffAdd`, `DiffDelete`, `DiffChange` and `DiffText` backgrounds reach the GUI
/// for its diff view: at start and on every `:colorscheme`; a group without a background is left out.
#[test]
fn diff_colors_follow_the_colorscheme() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 40, 10);
    let mut ed = Editor::new();
    let Some(GuiEvent::DiffColors(first)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::DiffColors(_))) else {
        panic!("no gui_diff_colors")
    };
    assert!(first.values().all(|c| *c <= 0xffffff), "{first:?}");
    let lua = "vim.api.nvim_set_hl(0, 'DiffAdd', { bg = 0x112211, fg = 0xffffff }) \
               vim.api.nvim_set_hl(0, 'DiffDelete', { fg = 0x221111 }) \
               vim.api.nvim_set_hl(0, 'DiffChange', { link = 'Visual' }) \
               vim.api.nvim_set_hl(0, 'Visual', { bg = 0x333355 }) \
               vim.api.nvim_set_hl(0, 'DiffText', { bg = 0x445566 }) \
               vim.api.nvim_exec_autocmds('ColorScheme', {})";
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    let Some(GuiEvent::DiffColors(colors)) =
        wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::DiffColors(c) if c.get("add") == Some(&0x112211)))
    else {
        panic!("colors not updated")
    };
    // A link is followed, and a foreground-only group has no background to report.
    assert_eq!(colors.get("change"), Some(&0x333355), "{colors:?}");
    assert_eq!(colors.get("text"), Some(&0x445566), "{colors:?}");
    assert_eq!(colors.get("delete"), None, "{colors:?}");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// The GUI's diff view opens a file at a line in an editor window (`Eshvi.open_at`, as the
/// explorer opens files), from wherever the keyboard was.
#[test]
fn the_diff_view_opens_a_file_at_a_line() {
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-open-at-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lines: Vec<String> = (1..=100).map(|i| format!("line {i}")).collect();
    std::fs::write(dir.join("a.txt"), lines.join("\n") + "\n").unwrap();
    let bridge = spawn(&["--clean", "-n"], 80, 24);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let path = nvim_bridge::RpcValue::from(dir.join("a.txt").to_string_lossy().as_ref());
    bridge.send(UiCommand::Lua { code: "return Eshvi.open_at(...)".into(), args: vec![path.clone(), nvim_bridge::RpcValue::from(42_i64)] });
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t') .. vim.fn.line('.')", "a.txt42");
    // Past the end: the last line.
    bridge.send(UiCommand::Lua { code: "return Eshvi.open_at(...)".into(), args: vec![path, nvim_bridge::RpcValue::from(9999_i64)] });
    wait_for(&bridge, &mut ed, "vim.fn.line('.')", "100");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(dir);
}

/// The colorscheme's `g:terminal_color_0..15` reach the GUI for its terminals: at start and
/// on every `:colorscheme`; -1 where unset (a name or nothing, not a color).
#[test]
fn terminal_colors_follow_the_colorscheme() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 40, 10);
    let mut ed = Editor::new();
    let Some(GuiEvent::TermColors(first)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TermColors(_))) else {
        panic!("no gui_term_colors")
    };
    assert!(first.iter().flatten().all(|c| *c <= 0xffffff), "{first:?}");
    let lua = "vim.g.terminal_color_1 = '#e06c75' vim.g.terminal_color_4 = '#61afef' \
               vim.g.terminal_color_9 = 'red' vim.g.terminal_color_15 = '#FFFFFF' \
               vim.api.nvim_exec_autocmds('ColorScheme', {})";
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    let Some(GuiEvent::TermColors(colors)) =
        wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TermColors(c) if c[1] == Some(0xe06c75)))
    else {
        panic!("colors not updated")
    };
    assert_eq!((colors[4], colors[15]), (Some(0x61afef), Some(0xffffff)));
    assert_eq!(colors[9], None, "a color name is not a number");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// `:EshviTermDebug` only tells the GUI, with its action (toggling by default).
#[test]
fn the_terminal_debug_command_tells_the_gui() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 40, 10);
    let mut ed = Editor::new();
    bridge.send(UiCommand::Input(":EshviTermDebug<CR>".into()));
    assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TermDebug(_))), Some(GuiEvent::TermDebug("toggle".into())));
    bridge.send(UiCommand::Input(":EshviTermDebug hide<CR>".into()));
    assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::TermDebug(_))), Some(GuiEvent::TermDebug("hide".into())));
    bridge.send(UiCommand::Command("qa!".into()));
}

// Sessions (`session.lua`): `:mksession` plus a JSON file for what it can't hold.

/// A folder with some files, and the path of a folder for its sessions.
fn session_dir(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("eshvi-session-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in [("a.md", "# A\n\ntext\n"), ("b.txt", "bee\n"), ("c.txt", "see\n"), ("d.md", "# D\n\nmore\n")] {
        std::fs::write(dir.join(file), text).unwrap();
    }
    // (Beside it, not in it: the folder is one of its own sessions' contents.)
    let sessions = std::env::temp_dir().join(format!("eshvi-sessions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sessions);
    (dir, sessions)
}

/// Neovim started in `start` with `files` on its command line and its sessions in
/// `sessions`; `auto` is `[session] auto`.
fn session_nvim(sessions: &std::path::Path, start: &std::path::Path, files: &[&str], auto: bool) -> (Bridge, Editor) {
    let mut args = vec![
        "--clean".to_string(),
        "-n".to_string(),
        "--cmd".to_string(),
        format!("let g:eshvi_session_dir = '{}'", sessions.display()),
        "-c".to_string(),
        format!("cd {}", start.display()),
    ];
    args.extend(files.iter().map(|f| f.to_string()));
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(args),
            width: 100,
            height: 30,
            terminal_enabled: true,
            menu: true,
            session_auto: auto,
            agent: Some(AgentOptions {
                command: vec!["sh".into(), "-c".into(), "echo agent ready; read x; exit 0".into()],
                ide_dir: Some(test_ide_dir(&format!("session-{}", sessions.file_name().unwrap().to_string_lossy()))),
                ..Default::default()
            }),
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    // What the windows are: a file's name (or the buffer type), width and height, by tab.
    bridge.send(UiCommand::Lua {
        code: "function _G.shape()
          local out = {}
          for _, t in ipairs(vim.api.nvim_list_tabpages()) do
            local parts = {}
            for _, w in ipairs(vim.api.nvim_tabpage_list_wins(t)) do
              if vim.api.nvim_win_get_config(w).relative == '' then
                local b = vim.api.nvim_win_get_buf(w)
                local what = vim.bo[b].buftype ~= '' and vim.bo[b].buftype or vim.fn.fnamemodify(vim.api.nvim_buf_get_name(b), ':t')
                table.insert(parts, what .. ':' .. vim.api.nvim_win_get_width(w) .. 'x' .. vim.api.nvim_win_get_height(w))
              end
            end
            table.insert(out, table.concat(parts, ','))
          end
          return table.concat(out, ' | ')
        end"
            .into(),
        args: vec![],
    });
    (bridge, ed)
}

/// Quit and wait until Neovim has gone (its `VimLeavePre` has run).
fn quit(bridge: &Bridge) {
    bridge.send(UiCommand::Command("qa!".into()));
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        while let Some(ev) = bridge.try_recv() {
            if let BridgeEvent::Exited { .. } = ev {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("Neovim did not quit");
}

fn json_file(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

#[test]
fn a_named_session_holds_the_layout_the_previews_and_the_guis_part_and_not_the_panels_windows() {
    use nvim_bridge::gui::{GuiEvent, GuiSession};
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (dir, sessions) = session_dir("named");
    let (bridge, mut ed) = session_nvim(&sessions, &dir, &[], false);
    let run = |cmd: &str| bridge.send(UiCommand::Command(cmd.into()));
    // Tab 1: a.md with its preview beside it, and b.txt; tab 2: d.md previewed in place.
    run("edit a.md");
    run("vsplit b.txt");
    run("wincmd l");
    run("EshviPreview side");
    run("tabnew d.md");
    run("EshviPreview");
    run("tabfirst");
    run("1wincmd w");
    run("1wincmd w");
    // The GUI's part, as it sends it.
    let gui = GuiSession { visible: true, width: 300.0, view: "git".into(), expanded: vec![format!("{}/sub", dir.display())], panel: None, agent: None };
    bridge.send(UiCommand::Lua { code: "return Eshvi.session.set_gui(...)".into(), args: vec![gui.to_value()] });
    pump_for(&bridge, &mut ed, Duration::from_millis(300), |_| false);
    let before = ask(&bridge, &mut ed, "shape()");
    // a.md, its preview and b.txt: the panels are the GUI's, not windows.
    assert!(before.contains("a.md") && before.contains("nofile") && !before.contains("terminal") && !before.contains("quickfix"), "{before}");

    run("EshviSession save work");
    wait_for(&bridge, &mut ed, "vim.fn.filereadable(vim.g.eshvi_session_dir .. '/named/work.json')", "1");
    // Saving put everything back as it was, in the same place.
    assert_eq!(ask(&bridge, &mut ed, "shape()"), before);
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.mode() .. vim.fn.winnr()"), "n1");

    // The `.vim` has the files, not the previews' scratch window.
    let vim = std::fs::read_to_string(sessions.join("named/work.vim")).unwrap();
    assert!(vim.contains("badd +") && vim.contains("a.md") && vim.contains("d.md") && vim.contains("tabnew"), "{vim}");
    assert!(!vim.contains("term://") && !vim.contains("quickfix") && !vim.contains("eshvi_preview"), "{vim}");
    assert_eq!(vim.matches("\nvsplit").count(), 1, "{vim}");
    let data = json_file(&sessions.join("named/work.json"));
    // (The panels are in the GUI's part, as the GUI sends it.)
    assert!(data.get("agent").is_none() && data.get("panel").is_none(), "{data}");
    assert_eq!(data["gui"]["view"], "git", "{data}");
    assert_eq!(data["gui"]["width"], 300.0, "{data}");
    assert_eq!(data["previews"], serde_json::json!([{"tab": 1, "win": 2, "side": true}, {"tab": 2, "win": 1, "side": false}]), "{data}");

    // Something else on the screen, then the session: tabs, splits, files, preview.
    run("tabonly | only | enew");
    wait_for(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0) .. vim.fn.tabpagenr('$')", "11");
    run("EshviSession load work");
    let Some(GuiEvent::Session(back)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Session(_))) else {
        panic!("no gui_session")
    };
    assert_eq!(back, gui);
    wait_for(&bridge, &mut ed, "shape()", &before);
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.tabpagenr('$')"), "2");
    assert_eq!(std::fs::canonicalize(ask(&bridge, &mut ed, "vim.fn.getcwd()")).unwrap(), std::fs::canonicalize(&dir).unwrap());
    // The side preview is a window again, the in-place one on tab 2.
    wait_for(&bridge, &mut ed, "#vim.tbl_filter(function(w) return vim.w[w].eshvi_preview_of ~= nil end, vim.api.nvim_tabpage_list_wins(0))", "1");
    run("tabnext");
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Preview(w) if w.len() == 1 && !w[0].side && w[0].path.ends_with("d.md"))).is_some());
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.mode()"), "n");

    // Named sessions are listed, newest first, and can be deleted.
    run("EshviSession save older");
    std::thread::sleep(Duration::from_millis(50));
    run("EshviSession save newer");
    wait_for(&bridge, &mut ed, "Eshvi.session.list()[1].name", "newer");
    run("EshviSession delete older");
    wait_for(&bridge, &mut ed, "#Eshvi.session.list()", "2");
    assert!(!sessions.join("named/older.vim").exists() && !sessions.join("named/older.json").exists());
    // File › Open Session and Delete Session list them, newest first, and keep up.
    let names = |menu: &str| format!("vim.fn.join(vim.tbl_map(function(m) return m.name end, vim.fn.menu_get('File.{menu}')[1].submenus), ',')");
    wait_for(&bridge, &mut ed, &names("Open\\\\ Session"), "newer,work");
    wait_for(&bridge, &mut ed, &names("Delete\\\\ Session"), "newer,work");
    run("EshviSession delete newer");
    wait_for(&bridge, &mut ed, &names("Open\\\\ Session"), "work");
    run("EshviSession delete work");
    wait_for(&bridge, &mut ed, &names("Open\\\\ Session"), "No Sessions");
    quit(&bridge);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(sessions);
}

/// A session saved before the panels moved into the GUI has `agent` and `panel` keys in its
/// JSON: they are ignored, and the rest loads.
#[test]
fn an_older_sessions_panel_keys_are_ignored() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (dir, sessions) = session_dir("older");
    let (bridge, mut ed) = session_nvim(&sessions, &dir, &["b.txt"], false);
    let run = |cmd: &str| bridge.send(UiCommand::Command(cmd.into()));
    run("vsplit c.txt");
    run("EshviSession save old");
    wait_for(&bridge, &mut ed, "vim.fn.filereadable(vim.g.eshvi_session_dir .. '/named/old.json')", "1");
    let before = ask(&bridge, &mut ed, "shape()");
    let json = sessions.join("named/old.json");
    let mut data = json_file(&json);
    data["agent"] = serde_json::json!({"shown": true, "width": 30});
    data["panel"] = serde_json::json!({"shown": true, "height": 9, "view": "problems"});
    std::fs::write(&json, data.to_string()).unwrap();
    run("only | enew");
    run("EshviSession load old");
    wait_for(&bridge, &mut ed, "shape()", &before);
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.winnr('$')"), "2");
    quit(&bridge);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(sessions);
}

#[test]
fn loading_a_session_refuses_modified_buffers_unless_told_to_discard_them() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (dir, sessions) = session_dir("modified");
    let (bridge, mut ed) = session_nvim(&sessions, &dir, &["b.txt"], false);
    let run = |cmd: &str| bridge.send(UiCommand::Command(cmd.into()));
    run("vsplit c.txt");
    run("EshviSession save two");
    wait_for(&bridge, &mut ed, "#Eshvi.session.list()", "1");
    run("only | edit a.md");
    run("call setline(1, 'changed')");
    run("EshviSession load two");
    // Refused, naming the buffer; nothing changed.
    wait_for(&bridge, &mut ed, "vim.fn.execute('messages'):match('not loaded, these buffers are modified: a.md')", "not loaded, these buffers are modified: a.md");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.expand('%:t') .. vim.fn.winnr('$') .. tostring(vim.bo.modified)"), "a.md1true");
    // With `!` they are discarded.
    run("EshviSession! load two");
    wait_for(&bridge, &mut ed, "vim.fn.winnr('$')", "2");
    assert_eq!(ask(&bridge, &mut ed, "#vim.fn.getbufinfo({ bufmodified = 1 })"), "0");
    let listed = "vim.fn.join(vim.tbl_map(function(b) return vim.fn.fnamemodify(b.name, ':t') end, vim.fn.getbufinfo({ buflisted = 1 })), ',')";
    wait_for(&bridge, &mut ed, listed, "b.txt,c.txt");
    // A session that isn't there.
    run("EshviSession load nothing");
    wait_for(&bridge, &mut ed, "vim.fn.execute('messages'):match('no such session: nothing')", "no such session: nothing");
    quit(&bridge);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(sessions);
}

#[test]
fn the_folders_session_is_saved_on_quit_and_restored_at_start_unless_there_are_files() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (dir, sessions) = session_dir("auto");
    let (bridge, mut ed) = session_nvim(&sessions, &dir, &[], true);
    let run = |cmd: &str| bridge.send(UiCommand::Command(cmd.into()));
    run("edit b.txt");
    run("split c.txt");
    run("tabnew a.md");
    run("tabfirst");
    run("2wincmd w");
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t') .. vim.fn.winnr()", "b.txt2");
    let before = ask(&bridge, &mut ed, "shape()");
    quit(&bridge);
    let files: Vec<_> = std::fs::read_dir(sessions.join("auto")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(files.len(), 2, "{files:?}");
    let json = files.iter().find(|f| f.ends_with(".json")).unwrap();
    let data = json_file(&sessions.join("auto").join(json));
    assert!(data["cwd"].as_str().unwrap().ends_with(dir.file_name().unwrap().to_str().unwrap()), "{data}");
    assert!(data["saved"].as_i64().unwrap() > 1_700_000_000, "{data}");

    // Started in the folder with no files: the same windows, tabs and cursor.
    let (bridge, mut ed) = session_nvim(&sessions, &dir, &[], true);
    wait_for(&bridge, &mut ed, "shape()", &before);
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.tabpagenr('$') .. vim.fn.expand('%:t') .. vim.fn.winnr()"), "2b.txt2");
    quit(&bridge);

    // Started with a file: nothing is restored, and quitting leaves the session as it was.
    let (bridge, mut ed) = session_nvim(&sessions, &dir, &["a.md"], true);
    pump_for(&bridge, &mut ed, Duration::from_millis(500), |_| false);
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.tabpagenr('$') .. vim.fn.winnr('$') .. vim.fn.expand('%:t')"), "11a.md");
    quit(&bridge);
    let (bridge, mut ed) = session_nvim(&sessions, &dir, &[], true);
    wait_for(&bridge, &mut ed, "shape()", &before);
    quit(&bridge);

    // Automatic sessions off: nothing is restored.
    let (bridge, mut ed) = session_nvim(&sessions, &dir, &[], false);
    pump_for(&bridge, &mut ed, Duration::from_millis(500), |_| false);
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.tabpagenr('$') .. vim.fn.winnr('$') .. vim.fn.expand('%:t')"), "11");
    quit(&bridge);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(sessions);
}

#[test]
fn started_from_the_dock_the_latest_session_comes_back() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (one, sessions) = session_dir("dock-one");
    let (two, _) = session_dir("dock-two");
    for (dir, file) in [(&one, "b.txt"), (&two, "c.txt")] {
        let (bridge, mut ed) = session_nvim(&sessions, dir, &[], true);
        bridge.send(UiCommand::Command(format!("edit {file}")));
        wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", file);
        quit(&bridge);
        // (Sessions are dated to the second.)
        std::thread::sleep(Duration::from_millis(1100));
    }
    // From `/`: the latest, which is the second folder's, and Neovim is in that folder now.
    let (bridge, mut ed) = session_nvim(&sessions, std::path::Path::new("/"), &[], true);
    wait_for(&bridge, &mut ed, "vim.fn.expand('%:t')", "c.txt");
    let cwd = ask(&bridge, &mut ed, "vim.fn.getcwd()");
    assert_eq!(std::fs::canonicalize(&cwd).unwrap(), std::fs::canonicalize(&two).unwrap(), "{cwd}");
    quit(&bridge);
    // `/` gets no session of its own: still the two folders'.
    let autos = std::fs::read_dir(sessions.join("auto")).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".vim")).count();
    assert_eq!(autos, 2);
    // With automatic sessions off, `/` starts empty.
    let (bridge, mut ed) = session_nvim(&sessions, std::path::Path::new("/"), &[], false);
    pump_for(&bridge, &mut ed, Duration::from_millis(500), |_| false);
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.expand('%:t') .. vim.fn.getcwd()"), "/");
    quit(&bridge);
    let _ = std::fs::remove_dir_all(one);
    let _ = std::fs::remove_dir_all(two);
    let _ = std::fs::remove_dir_all(sessions);
}

// The bottom panel's Neovim side (`panel.lua`): the commands tell the GUI, the diagnostics go to it.

fn native_nvim(args: &[&str], width: usize, height: usize) -> Bridge {
    Bridge::spawn(
        SpawnOptions {
            args: plain_shell(args.iter().map(|s| s.to_string()).collect()),
            width,
            height,
            terminal_enabled: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap()
}

/// The panel commands keep their arguments but only tell the GUI (`gui_panel`), and make no
/// window.
#[test]
fn the_panel_commands_only_notify_the_gui() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = native_nvim(&["--clean", "-n"], 60, 12);
    let mut ed = Editor::new();
    let panel = |kind: &str, args: &[&str]| GuiEvent::Panel { kind: kind.into(), args: args.iter().map(|s| s.to_string()).collect() };
    for (cmd, want) in [
        ("EshviPanel", panel("panel", &[])),
        ("EshviPanel hide", panel("panel", &["hide"])),
        ("EshviTerminal new", panel("terminal", &["new"])),
        ("EshviTerminal height 9", panel("terminal", &["height", "9"])),
        ("EshviProblems show file", panel("problems", &["show", "file"])),
        ("EshviProblems filter two words", panel("problems", &["filter", "two words"])),
    ] {
        bridge.send(UiCommand::Command(cmd.into()));
        assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Panel { .. })), Some(want), "{cmd}");
    }
    // No Neovim window for the panel, and no `Eshvi.panel_state` for sessions.
    assert_eq!(ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)"), "1");
    assert_eq!(ask(&bridge, &mut ed, "Eshvi.panel_state"), "nil");
    // (`:copen` below: not moved into any panel.)
    // :copen is Neovim's own quickfix window in the editor area, not moved anywhere.
    bridge.send(UiCommand::Command("copen".into()));
    wait_for(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)", "2");
    assert_eq!(ask(&bridge, &mut ed, "vim.bo.buftype"), "quickfix");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// A plain `:terminal` (the user's, a plugin's) is an ordinary buffer in an ordinary editor
/// window: its screen is drawn as a grid and typing reaches it. Only the panels moved out.
#[test]
fn a_plain_terminal_is_an_ordinary_window_that_draws_and_takes_input() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let bridge = native_nvim(&["--clean", "-n"], 60, 12);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    bridge.send(UiCommand::Input(":terminal sh<CR>".into()));
    wait_for(&bridge, &mut ed, "vim.bo.buftype", "terminal");
    // Typing in Terminal mode goes to the shell, and its output is on the window's grid.
    bridge.send(UiCommand::Input("i".into()));
    wait_for(&bridge, &mut ed, "vim.fn.mode()", "t");
    bridge.send(UiCommand::Input("echo plain-$((40 + 2))<CR>".into()));
    assert!(
        pump_for(&bridge, &mut ed, Duration::from_secs(5), |ed| {
            ed.windows.values().filter(|w| w.kind == grid::WindowKind::Normal).any(|w| ed.grid_text(w.grid).iter().any(|l| l.contains("plain-42")))
        }),
        "the terminal's output is drawn in its window"
    );
    // It is one editor window; no panel window was made for it.
    assert_eq!(ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)"), "1");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// `gui_problems` carries every diagnostic (hints left out) once they stopped changing for 200 ms.
#[test]
fn problems_are_reported_debounced_with_hints_left_out() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-problems-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("lib.rs");
    std::fs::write(&file, "fn main() {}\nlet x = 1;\n").unwrap();
    let bridge = native_nvim(&["--clean", "-n", &file.to_string_lossy()], 60, 12);
    let mut ed = Editor::new();
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Windows(w) if !w.is_empty())).is_some());
    // Three changes in a row, then a quiet moment: one report holding the last.
    let lua = "local ns = vim.api.nvim_create_namespace('t') local S = vim.diagnostic.severity \
               vim.diagnostic.set(ns, 0, { { lnum = 0, col = 0, severity = S.ERROR, message = 'old' } }) \
               vim.diagnostic.set(ns, 0, { { lnum = 0, col = 0, severity = S.ERROR, message = 'old2' } }) \
               vim.diagnostic.set(ns, 0, { \
                 { lnum = 1, col = 4, end_col = 5, severity = S.ERROR, message = 'bad', source = 'rustc', code = 'E1' }, \
                 { lnum = 0, col = 0, severity = S.WARN, message = 'meh' }, \
                 { lnum = 0, col = 2, severity = S.INFO, message = 'fyi', code = 7 }, \
                 { lnum = 0, col = 3, severity = S.HINT, message = 'hint' } })";
    let started = Instant::now();
    bridge.send(UiCommand::Command(format!("lua {lua}")));
    let Some(GuiEvent::Problems(items)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Problems(_))) else {
        panic!("no gui_problems")
    };
    assert!(started.elapsed() >= Duration::from_millis(190), "debounced: {:?}", started.elapsed());
    assert_eq!(items.len(), 3, "hints are left out, and only the last set is there: {items:?}");
    let bad = items.iter().find(|p| p.message == "bad").unwrap();
    assert!(bad.file.ends_with("lib.rs"));
    assert_eq!((bad.lnum, bad.col, bad.end_lnum, bad.end_col, bad.severity), (2, 5, 2, 6, 1));
    assert_eq!((bad.source.as_str(), bad.code.as_str()), ("rustc", "E1"));
    let fyi = items.iter().find(|p| p.message == "fyi").unwrap();
    assert_eq!((fyi.severity, fyi.code.as_str()), (3, "7"));
    // Nothing more follows the one report.
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Problems(_))).is_none());
    bridge.send(UiCommand::Command("qa!".into()));
}

/// `Eshvi.navigate` moves between windows, and tells the GUI only at an edge.
#[test]
fn navigate_moves_windows_and_tells_the_gui_at_an_edge() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = spawn(&["--clean", "-n"], 60, 12);
    let mut ed = Editor::new();
    bridge.send(UiCommand::Command("vsplit".into()));
    bridge.send(UiCommand::Command("1wincmd w".into()));
    wait_for(&bridge, &mut ed, "vim.fn.winnr()", "1");
    // Left window: ⌃l moves to the right one without a word to the GUI.
    bridge.send(UiCommand::Command("lua Eshvi.navigate('l')".into()));
    wait_for(&bridge, &mut ed, "vim.fn.winnr()", "2");
    assert!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Navigate(_))).is_none());
    // At the right edge: ⌃l stays and says so; so do ⌃j and ⌃k with one row of windows.
    bridge.send(UiCommand::Command("lua Eshvi.navigate('l')".into()));
    assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Navigate(_))), Some(GuiEvent::Navigate("l".into())));
    bridge.send(UiCommand::Command("lua Eshvi.navigate('j')".into()));
    assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Navigate(_))), Some(GuiEvent::Navigate("j".into())));
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.winnr()"), "2");
    // Entering from the GUI: the window at the edge.
    bridge.send(UiCommand::Command("999wincmd h".into()));
    wait_for(&bridge, &mut ed, "vim.fn.winnr()", "1");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// The bottom panel's state is the GUI's part of a session: saved, and given back on load; the
/// session code has no panel window to close around `:mksession`.
#[test]
fn the_bottom_panel_goes_in_the_guis_session_part() {
    use nvim_bridge::gui::{GuiEvent, GuiSession, PanelSession};
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (dir, sessions) = session_dir("native");
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(vec![
                "--clean".into(),
                "-n".into(),
                "--cmd".into(),
                format!("let g:eshvi_session_dir = '{}'", sessions.display()),
                "-c".into(),
                format!("cd {}", dir.display()),
                "a.md".into(),
            ]),
            width: 100,
            height: 30,
            terminal_enabled: true,
            menu: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let gui = GuiSession {
        visible: true,
        width: 280.0,
        view: "explorer".into(),
        expanded: vec![],
        panel: Some(PanelSession { shown: true, height: 222.0, view: "terminal".into() }),
        agent: None,
    };
    bridge.send(UiCommand::Lua { code: "return Eshvi.session.set_gui(...)".into(), args: vec![gui.to_value()] });
    pump_for(&bridge, &mut ed, Duration::from_millis(200), |_| false);
    bridge.send(UiCommand::Command("EshviSession save native".into()));
    wait_for(&bridge, &mut ed, "vim.fn.filereadable(vim.g.eshvi_session_dir .. '/named/native.json')", "1");
    let data = json_file(&sessions.join("named/native.json"));
    assert_eq!(data["gui"]["panel"]["shown"], true, "{data}");
    assert_eq!(data["gui"]["panel"]["height"], 222.0, "{data}");
    assert_eq!(data["gui"]["panel"]["view"], "terminal", "{data}");
    assert!(data.get("panel").is_none() || data["panel"].is_null(), "no Neovim panel window in it: {data}");
    bridge.send(UiCommand::Command("enew".into()));
    bridge.send(UiCommand::Command("EshviSession load native".into()));
    let Some(GuiEvent::Session(back)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Session(_))) else {
        panic!("no gui_session")
    };
    assert_eq!(back, gui);
    bridge.send(UiCommand::Command("qa!".into()));
}

// The agent panel's Neovim side (`agent.lua`).

fn native_agent_nvim(name: &str, args: Vec<String>) -> Bridge {
    Bridge::spawn(
        SpawnOptions {
            args: plain_shell(args),
            width: 100,
            height: 30,
            terminal_enabled: true,
            menu: true,
            agent: Some(AgentOptions {
                command: vec!["sh".into(), "-c".into(), "read x".into()],
                ide_dir: Some(test_ide_dir(name)),
                ..Default::default()
            }),
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap()
}

/// `:EshviAgent` keeps its arguments but only tells the GUI (`gui_panel`, kind "agent"): no window,
/// no terminal buffer, no `Eshvi.agent_state` for sessions, no agent window for diffs; the agents'
/// environment (the IDE connection) is sent once.
#[test]
fn eshvi_agent_only_notifies_the_gui() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let bridge = native_agent_nvim("native-cmd", vec!["--clean".into(), "-n".into()]);
    let mut ed = Editor::new();
    let Some(GuiEvent::AgentEnv(env)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::AgentEnv(_))) else {
        panic!("no gui_agent_env")
    };
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert!(get("CLAUDE_CODE_SSE_PORT").is_some_and(|p| p.parse::<u16>().is_ok()), "{env:?}");
    assert_eq!(get("ENABLE_IDE_INTEGRATION").as_deref(), Some("true"));
    let agent = |args: &[&str]| GuiEvent::Panel { kind: "agent".into(), args: args.iter().map(|s| s.to_string()).collect() };
    for (cmd, want) in [
        ("EshviAgent", agent(&[])),
        ("EshviAgent focus", agent(&["focus"])),
        ("EshviAgent new codex", agent(&["new", "codex"])),
        ("EshviAgent resume", agent(&["resume"])),
        ("EshviAgent close 7", agent(&["close", "7"])),
        ("EshviAgent restart", agent(&["restart"])),
    ] {
        bridge.send(UiCommand::Command(cmd.into()));
        assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Panel { .. })), Some(want), "{cmd}");
    }
    assert_eq!(ask(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)"), "1");
    assert_eq!(ask(&bridge, &mut ed, "#vim.api.nvim_list_bufs()"), "1");
    assert_eq!(ask(&bridge, &mut ed, "Eshvi.agent_state"), "nil");
    assert_eq!(ask(&bridge, &mut ed, "Eshvi.agent_win"), "nil");
    bridge.send(UiCommand::Command("qa!".into()));
}

/// The shown agent's process and folder come from the GUI: that pid is what "send to the agent"
/// goes by, and the folder is what a relative `agent_open` path is from; the file opens in the
/// editor window.
#[test]
fn the_pid_the_gui_pushes_is_the_agent_ide_lua_sends_to_and_agent_open_uses_the_editor() {
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("eshvi-native-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "one\ntwo\nthree\n").unwrap();
    let bridge = native_agent_nvim("native-pid", vec!["--clean".into(), "-n".into()]);
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    wait_for(&bridge, &mut ed, "type(Eshvi.agent_pid)", "function");
    assert_eq!(ask(&bridge, &mut ed, "Eshvi.agent_pid()"), "nil");
    let push = |pid: i64, cwd: &str| {
        bridge.send(UiCommand::Lua { code: "return Eshvi.agent_set_shown(...)".into(), args: vec![nvim_bridge::RpcValue::from(pid), nvim_bridge::RpcValue::from(cwd)] })
    };
    push(4242, &dir.to_string_lossy());
    wait_for(&bridge, &mut ed, "Eshvi.agent_pid()", "4242");
    // No agent (pid 0): nothing to send to.
    push(0, "");
    wait_for(&bridge, &mut ed, "tostring(Eshvi.agent_pid())", "nil");
    push(4243, &dir.to_string_lossy());
    wait_for(&bridge, &mut ed, "Eshvi.agent_pid()", "4243");
    // A relative path is from the agent's folder; it opens in a normal window, at the line.
    bridge.send(UiCommand::Command("vsplit".into()));
    wait_for(&bridge, &mut ed, "#vim.api.nvim_tabpage_list_wins(0)", "2");
    assert_eq!(ask(&bridge, &mut ed, "tostring(Eshvi.agent_open('notes.txt', 2, 1))"), "true");
    wait_for(&bridge, &mut ed, "vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ':t')", "notes.txt");
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.line('.')"), "2");
    assert_eq!(ask(&bridge, &mut ed, "tostring(Eshvi.agent_open('missing.txt'))"), "false");
    bridge.send(UiCommand::Command("qa!".into()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The agent panel's state is the GUI's part of a session.
#[test]
fn the_agent_panel_goes_in_the_guis_session_part() {
    use nvim_bridge::gui::{AgentSession, GuiEvent, GuiSession};
    if !nvim_available() || cfg!(windows) {
        return;
    }
    let (dir, sessions) = session_dir("native-agent");
    let bridge = native_agent_nvim(
        "native-session",
        vec!["--clean".into(), "-n".into(), "--cmd".into(), format!("let g:eshvi_session_dir = '{}'", sessions.display()), "-c".into(), format!("cd {}", dir.display()), "a.md".into()],
    );
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| ed.windows.values().any(|w| w.kind == grid::WindowKind::Normal)));
    let gui = GuiSession {
        visible: true,
        width: 280.0,
        view: "explorer".into(),
        expanded: vec![],
        panel: None,
        agent: Some(AgentSession { shown: true, width: 412.0 }),
    };
    bridge.send(UiCommand::Lua { code: "return Eshvi.session.set_gui(...)".into(), args: vec![gui.to_value()] });
    pump_for(&bridge, &mut ed, Duration::from_millis(200), |_| false);
    bridge.send(UiCommand::Command("EshviSession save agent".into()));
    wait_for(&bridge, &mut ed, "vim.fn.filereadable(vim.g.eshvi_session_dir .. '/named/agent.json')", "1");
    let data = json_file(&sessions.join("named/agent.json"));
    assert_eq!(data["gui"]["agent"]["shown"], true, "{data}");
    assert_eq!(data["gui"]["agent"]["width"], 412.0, "{data}");
    assert!(data.get("agent").is_none() || data["agent"].is_null(), "no Neovim agent window in it: {data}");
    bridge.send(UiCommand::Command("enew".into()));
    bridge.send(UiCommand::Command("EshviSession load agent".into()));
    let Some(GuiEvent::Session(back)) = wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Session(_))) else {
        panic!("no gui_session")
    };
    assert_eq!(back, gui);
    bridge.send(UiCommand::Command("qa!".into()));
}

/// `[neovim] copy_on_select`: a selection the mouse made (a drag, a double click) is sent to
/// the GUI's clipboard when the button is let go, and the registers stay as they were.
#[test]
fn a_mouse_selection_in_the_editor_is_copied_without_a_yank() {
    use nvim_bridge::gui::GuiEvent;
    if !nvim_available() {
        return;
    }
    let bridge = Bridge::spawn(
        SpawnOptions {
            args: plain_shell(["--clean", "-n", "-c", "set mouse=a", "-c", "call setline(1, ['hello world', 'second line'])"].map(String::from).to_vec()),
            width: 60,
            height: 12,
            copy_on_select: true,
            ..Default::default()
        },
        Arc::new(|| {}),
    )
    .unwrap();
    let mut ed = Editor::new();
    assert!(pump(&bridge, &mut ed, |ed| first_line(ed).starts_with("hello world")));
    let grid = ed.windows.values().find(|w| w.kind == grid::WindowKind::Normal).unwrap().grid;
    let mouse = |action: &str, row: usize, col: usize| UiCommand::InputMouse { button: "left".into(), action: action.into(), modifier: String::new(), grid, row, col };
    // A drag over "hello" (each event a moment apart, as a hand makes them).
    for (action, col) in [("press", 0), ("drag", 2), ("drag", 4), ("release", 4)] {
        bridge.send(mouse(action, 0, col));
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Copy(_))), Some(GuiEvent::Copy("hello".into())));
    assert_eq!(ask(&bridge, &mut ed, "vim.fn.getreg('\"')"), "", "not a yank");
    // Leave Visual mode, then a double click on "second" selects the word.
    bridge.send(UiCommand::Input("<Esc>".into()));
    for action in ["press", "release", "press", "release"] {
        bridge.send(mouse(action, 1, 2));
    }
    assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Copy(_))), Some(GuiEvent::Copy("second".into())));
    // A plain click selects nothing and copies nothing.
    bridge.send(UiCommand::Input("<Esc>".into()));
    bridge.send(mouse("press", 0, 7));
    bridge.send(mouse("release", 0, 7));
    assert_eq!(wait_gui(&bridge, &mut ed, |g| matches!(g, GuiEvent::Copy(_))), None);
    bridge.send(UiCommand::Command("qa!".into()));
}
