//! Debug helper: `ESHVI_SCREENSHOT=out.png` saves a screenshot once the
//! UI has settled (no new flush for a while), then quits. Optional
//! `ESHVI_SCREENSHOT_DELAY_MS` sets the settle time (default 800) and
//! `ESHVI_SCREENSHOT_AFTER_MS` a minimum time since startup (the shot is
//! taken at most 3 s after it, settled or not).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const GIVE_UP: Duration = Duration::from_secs(3);

pub struct Screenshotter {
    path: PathBuf,
    settle: Duration,
    not_before: Instant,
    last_flush: Option<Instant>,
    requested: bool,
    done: Arc<AtomicBool>,
}

impl Screenshotter {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("ESHVI_SCREENSHOT")?;
        let settle = std::env::var("ESHVI_SCREENSHOT_DELAY_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(800);
        let after: u64 = std::env::var("ESHVI_SCREENSHOT_AFTER_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        Some(Self {
            path: path.into(),
            settle: Duration::from_millis(settle),
            not_before: Instant::now() + Duration::from_millis(after),
            last_flush: None,
            requested: false,
            done: Arc::new(AtomicBool::new(false)),
        })
    }

    /// True once the screenshot was written; the app then quits Neovim.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    pub fn on_flush(&mut self) {
        self.last_flush = Some(Instant::now());
    }

    pub fn after_paint(&mut self, ctx: &egui::Context) {
        let Some(t) = self.last_flush else { return };
        if self.requested {
            // The shot arrives with a later frame; without Neovim redrawing
            // (an error screen) nothing else asks for one.
            if !self.is_done() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            return;
        }
        let now = Instant::now();
        if now < self.not_before {
            ctx.request_repaint_after(self.not_before - now);
            return;
        }
        // Something that never stops redrawing (a spinner, LSP progress)
        // only delays the shot by `GIVE_UP`.
        let waited = t.elapsed();
        if waited < self.settle && now < self.not_before + GIVE_UP {
            ctx.request_repaint_after(self.settle - waited);
            return;
        }
        self.requested = true;
        let path = self.path.clone();
        let ctx2 = ctx.clone();
        let done = self.done.clone();
        ctx.request_screenshot(move |image| {
            if let Err(e) = save_png(&path, &image) {
                eprintln!("screenshot failed: {e}");
            } else {
                eprintln!("screenshot saved to {}", path.display());
            }
            done.store(true, Ordering::Relaxed);
            ctx2.request_repaint();
        });
        ctx.request_repaint();
    }
}

fn save_png(path: &std::path::Path, image: &egui::ColorImage) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let [w, h] = image.size;
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    writer.write_image_data(&bytes)?;
    Ok(())
}
