// Release builds on Windows are GUI apps: no console window beside them.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bookmarks;
mod changes;
mod changes_popup;
mod cmdline;
mod config;
mod debug;
mod decorations;
mod diff_view;
mod explorer;
mod explorer_view;
mod file_clipboard;
mod focus;
mod fonts;
mod git;
mod icons;
mod images;
mod input;
mod links;
mod login_path;
mod lsp_popups;
mod markdown;
mod menu;
mod messages;
mod picker;
mod popup;
mod popupmenu;
mod preview;
mod problems;
mod renderer;
mod scm_view;
mod scrollbar;
mod side_bar;
mod style;
mod terminal_view;
mod trash_can;
mod tree_view;
mod tty;

fn usage() -> ! {
    eprintln!("usage: eshvi [--nvim PATH] [--font-size N] [--geometry WxH] [--] [nvim args...]");
    std::process::exit(2);
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    login_path::adopt();

    let (config, config_error) = config::load();
    let mut opts = app::Options { nvim: "nvim".into(), nvim_args: vec![], font_size: None, config, config_error };
    let mut size = [1100.0_f32, 720.0];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--nvim" => opts.nvim = args.next().unwrap_or_else(|| usage()),
            "--font-size" => opts.font_size = Some(args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage())),
            "--geometry" => {
                let g = args.next().unwrap_or_else(|| usage());
                let (w, h) = g.split_once('x').unwrap_or_else(|| usage());
                size = [w.parse().unwrap_or_else(|_| usage()), h.parse().unwrap_or_else(|_| usage())];
            }
            "-h" | "--help" => usage(),
            "--" => {
                opts.nvim_args.extend(args.by_ref());
            }
            _ => opts.nvim_args.push(a),
        }
    }

    let mut viewport = egui::ViewportBuilder::default().with_title("Eshvi").with_inner_size(size);
    if let Some(icon) = icon() {
        viewport = viewport.with_icon(icon);
    }
    if opts.config.window.titlebar().is_some() {
        // Content extends under a transparent title bar; the app paints its
        // own thin bar there and the traffic-light buttons stay.
        viewport = viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    let native = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "eshvi",
        native,
        Box::new(|cc| {
            // Cmd/Ctrl +/- are for Neovim (and our own zoom), not egui's.
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            // Images in Markdown previews (local files only).
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::VimApp::new(cc, opts)?))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// The app icon (the window's, and the Dock's when not run from a bundle):
/// a porthole onto a prompt and cursor. `assets/icon` has the
/// SVG it is drawn from and the `.icns` for an app bundle.
pub fn icon() -> Option<egui::IconData> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../../../assets/icon/eshvi-512.png")));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    (info.color_type == png::ColorType::Rgba).then(|| {
        rgba.truncate(info.buffer_size());
        egui::IconData { rgba, width: info.width, height: info.height }
    })
}
