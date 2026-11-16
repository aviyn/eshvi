//! Images from the kitty graphics protocol, the part snacks.image uses:
//! images are sent as PNG (a file path or the data, in chunks), given a
//! *virtual* placement of so many columns and rows (`a=p,U=1`), and shown
//! by Unicode placeholder cells in the grid: U+10EEEE, with diacritics for
//! the cell's row and column in the image, the image id in the
//! foreground color and the placement id in the underline color. So the
//! image moves with the text (scrolling, floats) and the renderer just
//! draws each placeholder cell's slice of it.

use std::collections::HashMap;
use std::sync::OnceLock;

use egui::{ColorImage, TextureHandle, TextureOptions};

use crate::tty::{base64_decode, GraphicsCommand};

/// The placeholder character.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Larger images are scaled down (GPU texture limits, memory).
const MAX_SIDE: usize = 4096;

/// A transmitted image, ready to draw.
#[derive(Clone)]
pub struct Image {
    pub texture: TextureHandle,
    /// Pixels.
    pub size: [usize; 2],
}

/// Images and their virtual placements, by kitty id.
#[derive(Default)]
pub struct Images {
    images: HashMap<u32, Image>,
    /// `(image id, placement id)` → `(columns, rows)`.
    placements: HashMap<(u32, u32), (u32, u32)>,
    /// A transmission arriving in chunks (`m=1`): its first command and the
    /// payload so far.
    chunked: Option<(GraphicsCommand, Vec<u8>)>,
    /// Bumped on every change, so the renderer knows to look again.
    pub generation: u64,
}

/// What the renderer needs for one placement.
#[derive(Clone)]
pub struct Placed {
    pub image: Image,
    pub cols: u32,
    pub rows: u32,
}

impl Images {
    /// Apply a command; returns a reply for the terminal, if it wants one.
    pub fn apply(&mut self, ctx: &egui::Context, cmd: GraphicsCommand) -> Option<String> {
        // Later chunks carry only `m` and the payload.
        let cmd = match self.chunked.take() {
            Some((first, mut data)) => {
                data.extend_from_slice(&cmd.payload);
                if cmd.get("m") == Some("1") {
                    self.chunked = Some((first, data));
                    return None;
                }
                GraphicsCommand { payload: data, ..first }
            }
            None if cmd.get("m") == Some("1") => {
                self.chunked = Some((cmd.clone(), cmd.payload));
                return None;
            }
            None => cmd,
        };
        self.generation += 1;
        log::debug!("graphics {:?} ({} payload bytes)", cmd.keys, cmd.payload.len());
        let id = cmd.num("i").unwrap_or(0);
        let reply = |ok: Result<(), String>| {
            // q=1 silences OK, q=2 errors too.
            let quiet = cmd.num("q").unwrap_or(0);
            match ok {
                Ok(()) if quiet == 0 => Some(format!("\x1b_Gi={id};OK\x1b\\")),
                Err(e) if quiet < 2 => Some(format!("\x1b_Gi={id};{e}\x1b\\")),
                _ => None,
            }
        };
        match cmd.get("a").unwrap_or("t") {
            "t" | "T" => {
                let result = decode(&cmd).map(|img| {
                    let size = img.size;
                    let texture = ctx.load_texture(format!("kitty-image-{id}"), img, TextureOptions::LINEAR);
                    self.images.insert(id, Image { texture, size });
                });
                if cmd.get("a") == Some("T") {
                    self.place(&cmd);
                }
                reply(result)
            }
            "p" => {
                self.place(&cmd);
                reply(Ok(()))
            }
            "d" => {
                let p = cmd.num("p");
                match cmd.get("d").unwrap_or("a") {
                    "a" => self.placements.clear(),
                    "A" => {
                        self.placements.clear();
                        self.images.clear();
                    }
                    d @ ("i" | "I") => {
                        self.placements.retain(|&(i, pid), _| i != id || p.is_some_and(|p| p != pid));
                        if d == "I" && p.is_none() {
                            self.images.remove(&id);
                        }
                    }
                    _ => {}
                }
                None
            }
            // A query: can this image be loaded?
            "q" => reply(decode(&cmd).map(|_| ())),
            _ => None,
        }
    }

    /// Only virtual placements (`U=1`) are shown: by placeholder cells.
    fn place(&mut self, cmd: &GraphicsCommand) {
        if cmd.get("U") != Some("1") {
            return;
        }
        let (cols, rows) = (cmd.num("c").unwrap_or(1).max(1), cmd.num("r").unwrap_or(1).max(1));
        self.placements.insert((cmd.num("i").unwrap_or(0), cmd.num("p").unwrap_or(0)), (cols, rows));
    }

    /// The placement a placeholder cell points at; placement 0 (none in the
    /// underline color) matches any placement of the image.
    pub fn placed(&self, image: u32, placement: u32) -> Option<Placed> {
        let (cols, rows) = match self.placements.get(&(image, placement)) {
            Some(p) => *p,
            None if placement == 0 => *self.placements.iter().find(|((i, _), _)| *i == image)?.1,
            None => return None,
        };
        Some(Placed { image: self.images.get(&image)?.clone(), cols, rows })
    }
}

/// The image a transmission carries: a PNG (`f=100`), or raw RGB(A)
/// (`f=24`/`f=32`, `s`×`v` pixels), from the payload or a file it names.
fn decode(cmd: &GraphicsCommand) -> Result<ColorImage, String> {
    let payload = base64_decode(&cmd.payload);
    let data = match cmd.get("t").unwrap_or("d") {
        "d" => payload,
        "f" | "t" => {
            let path = String::from_utf8_lossy(&payload).into_owned();
            let data = std::fs::read(&path).map_err(|e| format!("ENOENT:{e}"))?;
            if cmd.get("t") == Some("t") && path.contains("tty-graphics-protocol") {
                let _ = std::fs::remove_file(&path);
            }
            data
        }
        other => return Err(format!("EINVAL:unsupported medium {other}")),
    };
    if cmd.get("o").is_some() {
        return Err("EINVAL:compression not supported".into());
    }
    let image = match cmd.num("f").unwrap_or(32) {
        100 => decode_png(&data)?,
        f @ (24 | 32) => {
            let (w, h) = (cmd.num("s").unwrap_or(0) as usize, cmd.num("v").unwrap_or(0) as usize);
            let channels = f as usize / 8;
            // (Checked: a size made to wrap around would pass the test
            // against the data's length and fail inside `ColorImage`.)
            let bytes = w.checked_mul(h).and_then(|n| n.checked_mul(channels)).filter(|&n| n > 0 && n <= data.len());
            let Some(bytes) = bytes else { return Err("EINVAL:bad size".into()) };
            if channels == 4 {
                ColorImage::from_rgba_unmultiplied([w, h], &data[..bytes])
            } else {
                ColorImage::from_rgb([w, h], &data[..bytes])
            }
        }
        f => return Err(format!("EINVAL:unsupported format {f}")),
    };
    Ok(shrink(image))
}

fn decode_png(data: &[u8]) -> Result<ColorImage, String> {
    let err = |e: png::DecodingError| format!("EBADPNG:{e}");
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(err)?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("EBADPNG:too large")?];
    let info = reader.next_frame(&mut buf).map_err(err)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("EBADPNG:palette not expanded".into()),
    };
    Ok(ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}

/// Scale an image down by a whole factor until it fits `MAX_SIDE`
/// (averaging each block of pixels).
fn shrink(image: ColorImage) -> ColorImage {
    let [w, h] = image.size;
    let factor = w.max(h).div_ceil(MAX_SIDE);
    if factor <= 1 {
        return image;
    }
    // (At least a pixel each way: a 5000×1 strip is not an image of no size.)
    let (nw, nh) = ((w / factor).max(1), (h / factor).max(1));
    let mut pixels = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for sy in (y * factor..(y + 1) * factor).take_while(|&sy| sy < h) {
                for sx in (x * factor..(x + 1) * factor).take_while(|&sx| sx < w) {
                    let p = image.pixels[sy * w + sx].to_array();
                    for (s, c) in sum.iter_mut().zip(p) {
                        *s += c as u32;
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            pixels.push(egui::Color32::from_rgba_premultiplied(
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                (sum[3] / n) as u8,
            ));
        }
    }
    ColorImage { size: [nw, nh], pixels, ..image }
}

/// A placeholder cell: which part of which image it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tile {
    pub image: u32,
    pub placement: u32,
    pub row: u32,
    pub col: u32,
}

/// Read a placeholder cell's text (U+10EEEE and up to three diacritics:
/// row, column, the image id's highest byte). Missing ones continue the
/// cell to the left (`prev`), as the protocol allows.
pub fn tile(text: &str, fg: u32, sp: u32, prev: Option<Tile>) -> Option<Tile> {
    let mut chars = text.chars();
    if chars.next() != Some(PLACEHOLDER) {
        return None;
    }
    let mut marks = chars.map(diacritic_index);
    let (row, col, msb) = (marks.next().flatten(), marks.next().flatten(), marks.next().flatten());
    let image = (fg & 0xff_ffff) | msb.map_or(0, |b| b << 24);
    let prev = prev.filter(|p| p.image == image && p.placement == sp && row.is_none_or(|r| r == p.row));
    Some(Tile {
        image,
        placement: sp,
        row: row.or(prev.map(|p| p.row)).unwrap_or(0),
        col: col.or(prev.map(|p| p.col + 1)).unwrap_or(0),
    })
}

/// The protocol's row/column diacritics, in order (index = number).
const DIACRITICS: &str = "0305,030D,030E,0310,0312,033D,033E,033F,0346,034A,034B,034C,0350,0351,0352,0357,035B,0363,0364,0365,0366,0367,0368,0369,036A,036B,036C,036D,036E,036F,0483,0484,0485,0486,0487,0592,0593,0594,0595,0597,0598,0599,059C,059D,059E,059F,05A0,05A1,05A8,05A9,05AB,05AC,05AF,05C4,0610,0611,0612,0613,0614,0615,0616,0617,0657,0658,0659,065A,065B,065D,065E,06D6,06D7,06D8,06D9,06DA,06DB,06DC,06DF,06E0,06E1,06E2,06E4,06E7,06E8,06EB,06EC,0730,0732,0733,0735,0736,073A,073D,073F,0740,0741,0743,0745,0747,0749,074A,07EB,07EC,07ED,07EE,07EF,07F0,07F1,07F3,0816,0817,0818,0819,081B,081C,081D,081E,081F,0820,0821,0822,0823,0825,0826,0827,0829,082A,082B,082C,082D,0951,0953,0954,0F82,0F83,0F86,0F87,135D,135E,135F,17DD,193A,1A17,1A75,1A76,1A77,1A78,1A79,1A7A,1A7B,1A7C,1B6B,1B6D,1B6E,1B6F,1B70,1B71,1B72,1B73,1CD0,1CD1,1CD2,1CDA,1CDB,1CE0,1DC0,1DC1,1DC3,1DC4,1DC5,1DC6,1DC7,1DC8,1DC9,1DCB,1DCC,1DD1,1DD2,1DD3,1DD4,1DD5,1DD6,1DD7,1DD8,1DD9,1DDA,1DDB,1DDC,1DDD,1DDE,1DDF,1DE0,1DE1,1DE2,1DE3,1DE4,1DE5,1DE6,1DFE,20D0,20D1,20D4,20D5,20D6,20D7,20DB,20DC,20E1,20E7,20E9,20F0,2CEF,2CF0,2CF1,2DE0,2DE1,2DE2,2DE3,2DE4,2DE5,2DE6,2DE7,2DE8,2DE9,2DEA,2DEB,2DEC,2DED,2DEE,2DEF,2DF0,2DF1,2DF2,2DF3,2DF4,2DF5,2DF6,2DF7,2DF8,2DF9,2DFA,2DFB,2DFC,2DFD,2DFE,2DFF,A66F,A67C,A67D,A6F0,A6F1,A8E0,A8E1,A8E2,A8E3,A8E4,A8E5,A8E6,A8E7,A8E8,A8E9,A8EA,A8EB,A8EC,A8ED,A8EE,A8EF,A8F0,A8F1,AAB0,AAB2,AAB3,AAB7,AAB8,AABE,AABF,AAC1,FE20,FE21,FE22,FE23,FE24,FE25,FE26,10A0F,10A38,1D185,1D186,1D187,1D188,1D189,1D1AA,1D1AB,1D1AC,1D1AD,1D242,1D243,1D244";

fn diacritic_index(c: char) -> Option<u32> {
    static TABLE: OnceLock<HashMap<char, u32>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        DIACRITICS
            .split(',')
            .enumerate()
            .filter_map(|(i, hex)| Some((char::from_u32(u32::from_str_radix(hex, 16).ok()?)?, i as u32)))
            .collect()
    });
    table.get(&c).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_that_wraps_around_is_refused_and_a_strip_keeps_a_pixel() {
        // s × v × 4 is 2^66: zero once wrapped, which any payload is "long enough" for.
        let wraps = cmd("a=t,f=32,i=1,s=4294967295,v=4294967295", "AAAA");
        assert_eq!(decode(&wraps).err().as_deref(), Some("EINVAL:bad size"));
        assert_eq!(decode(&cmd("a=t,f=24,i=1,s=0,v=3", "AAAA")).err().as_deref(), Some("EINVAL:bad size"));
        // Wider than `MAX_SIDE` and one pixel high: shrunk, still an image.
        let strip = ColorImage::new([5000, 1], vec![egui::Color32::from_rgb(10, 20, 30); 5000]);
        let small = shrink(strip);
        assert_eq!(small.size, [2500, 1]);
        assert_eq!(small.pixels.len(), 2500);
        assert_eq!(small.pixels[0], egui::Color32::from_rgb(10, 20, 30));
    }

    fn cmd(control: &str, payload: &str) -> GraphicsCommand {
        let keys = control.split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.into(), v.into())).collect();
        GraphicsCommand { keys, payload: payload.as_bytes().to_vec() }
    }

    /// A 2×1 RGBA image, as base64 raw pixels.
    const RED_GREEN: &str = "/wAA/wD/AP8=";

    #[test]
    fn transmit_place_and_delete() {
        let ctx = egui::Context::default();
        let mut images = Images::default();
        assert_eq!(images.apply(&ctx, cmd("a=t,f=32,s=2,v=1,i=5,q=2", RED_GREEN)), None);
        assert!(images.placed(5, 1).is_none(), "not placed yet");
        images.apply(&ctx, cmd("a=p,U=1,i=5,p=1,c=8,r=3,q=2", ""));
        let placed = images.placed(5, 1).unwrap();
        assert_eq!((placed.cols, placed.rows, placed.image.size), (8, 3, [2, 1]));
        // Placement 0 in a cell matches any placement of the image.
        assert!(images.placed(5, 0).is_some());
        images.apply(&ctx, cmd("a=d,d=i,i=5,p=1,q=2", ""));
        assert!(images.placed(5, 1).is_none());
        // Bad data is answered unless silenced.
        assert!(images.apply(&ctx, cmd("a=t,f=100,i=6", "AAAA")).is_some_and(|r| r.starts_with("\x1b_Gi=6;EBADPNG")));
    }

    #[test]
    fn chunks_are_joined() {
        let ctx = egui::Context::default();
        let mut images = Images::default();
        images.apply(&ctx, cmd("a=T,f=32,s=2,v=1,i=9,U=1,c=1,r=1,m=1,q=2", &RED_GREEN[..4]));
        assert!(images.placed(9, 0).is_none());
        images.apply(&ctx, cmd("m=1", &RED_GREEN[4..8]));
        images.apply(&ctx, cmd("m=0", &RED_GREEN[8..]));
        assert_eq!(images.placed(9, 0).unwrap().image.size, [2, 1]);
    }

    #[test]
    fn placeholder_cells() {
        let d = |i: usize| DIACRITICS.split(',').nth(i).and_then(|h| char::from_u32(u32::from_str_radix(h, 16).ok()?)).unwrap();
        let text = |marks: &[usize]| std::iter::once(PLACEHOLDER).chain(marks.iter().map(|&i| d(i))).collect::<String>();
        let t = tile(&text(&[2, 5]), 0x00_0007, 3, None).unwrap();
        assert_eq!(t, Tile { image: 7, placement: 3, row: 2, col: 5 });
        // No column: the next one after the cell to the left.
        assert_eq!(tile(&text(&[2]), 7, 3, Some(t)).unwrap().col, 6);
        assert_eq!(tile(&text(&[]), 7, 3, Some(t)).unwrap(), Tile { col: 6, ..t });
        // The image id's highest byte.
        assert_eq!(tile(&text(&[0, 0, 1]), 7, 0, None).unwrap().image, 0x0100_0007);
        assert!(tile("x", 7, 0, None).is_none());
    }
}
