use std::sync::atomic::{AtomicU64, Ordering};

use crate::hl::HlId;

/// Global counter so row versions never repeat, even across grid
/// destroy/recreate. Renderers cache per-row layouts keyed by version.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

fn next_version() -> u64 {
    NEXT_VERSION.fetch_add(1, Ordering::Relaxed)
}

/// One screen cell. `text` is a single grapheme cluster; it is empty for
/// the right half of a double-width character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub hl_id: HlId,
}

impl Default for Cell {
    fn default() -> Self {
        Self { text: " ".to_string(), hl_id: 0 }
    }
}

/// A cell as sent by `grid_line`, with the highlight already resolved
/// (Neovim omits repeated hl ids) and an explicit repeat count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineCell {
    pub text: String,
    pub hl_id: HlId,
    pub repeat: usize,
}

#[derive(Clone, Debug)]
pub struct Grid {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
    row_versions: Vec<u64>,
}

impl Grid {
    pub fn new(width: usize, height: usize) -> Self {
        let v = next_version();
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
            row_versions: vec![v; height],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn row(&self, row: usize) -> &[Cell] {
        let start = row * self.width;
        &self.cells[start..start + self.width]
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if row < self.height && col < self.width {
            Some(&self.cells[row * self.width + col])
        } else {
            None
        }
    }

    /// Version of a row; changes whenever the row's content changes.
    pub fn row_version(&self, row: usize) -> u64 {
        self.row_versions[row]
    }

    /// Text of a row, for tests and debugging.
    pub fn row_text(&self, row: usize) -> String {
        self.row(row).iter().map(|c| c.text.as_str()).collect()
    }

    fn touch(&mut self, row: usize) {
        self.row_versions[row] = next_version();
    }

    /// `grid_resize`: keep the overlapping top-left content.
    pub fn resize(&mut self, width: usize, height: usize) {
        if width == self.width && height == self.height {
            return;
        }
        let mut cells = vec![Cell::default(); width * height];
        for row in 0..height.min(self.height) {
            for col in 0..width.min(self.width) {
                cells[row * width + col] = std::mem::take(&mut self.cells[row * self.width + col]);
            }
        }
        self.cells = cells;
        self.width = width;
        self.height = height;
        let v = next_version();
        self.row_versions = vec![v; height];
    }

    /// `grid_clear`
    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
        let v = next_version();
        self.row_versions.fill(v);
    }

    /// `grid_line`: write cells starting at `col_start`. Cells past the
    /// right edge are ignored.
    pub fn put_line(&mut self, row: usize, col_start: usize, cells: &[LineCell]) {
        if row >= self.height {
            return;
        }
        let base = row * self.width;
        let mut col = col_start;
        'outer: for lc in cells {
            for _ in 0..lc.repeat {
                if col >= self.width {
                    break 'outer;
                }
                let cell = &mut self.cells[base + col];
                if cell.text != lc.text {
                    cell.text.clone_from(&lc.text);
                }
                cell.hl_id = lc.hl_id;
                col += 1;
            }
        }
        self.touch(row);
    }

    /// `grid_scroll`: move the region `[top, bot) x [left, right)` up by
    /// `rows` (down if negative). Rows scrolled into view keep their old
    /// content; Neovim sends `grid_line` for them afterwards.
    pub fn scroll(&mut self, top: usize, bot: usize, left: usize, right: usize, rows: i64) {
        let bot = bot.min(self.height);
        let right = right.min(self.width);
        if rows == 0 || top >= bot || left >= right {
            return;
        }
        let w = self.width;
        let copy_row = |cells: &mut Vec<Cell>, dst: usize, src: usize| {
            for col in left..right {
                cells[dst * w + col] = cells[src * w + col].clone();
            }
        };
        if rows > 0 {
            let n = rows as usize;
            for dst in top..bot.saturating_sub(n) {
                copy_row(&mut self.cells, dst, dst + n);
                self.touch(dst);
            }
        } else {
            let n = rows.unsigned_abs() as usize;
            for dst in (top + n..bot).rev() {
                copy_row(&mut self.cells, dst, dst - n);
                self.touch(dst);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lc(text: &str, hl: HlId, repeat: usize) -> LineCell {
        LineCell { text: text.into(), hl_id: hl, repeat }
    }

    fn grid_from(rows: &[&str]) -> Grid {
        let w = rows[0].len();
        let mut g = Grid::new(w, rows.len());
        for (r, s) in rows.iter().enumerate() {
            let cells: Vec<_> = s.chars().map(|c| lc(&c.to_string(), 0, 1)).collect();
            g.put_line(r, 0, &cells);
        }
        g
    }

    fn texts(g: &Grid) -> Vec<String> {
        (0..g.height()).map(|r| g.row_text(r)).collect()
    }

    #[test]
    fn grid_line_writes_cells_and_highlights() {
        let mut g = Grid::new(6, 2);
        g.put_line(1, 1, &[lc("a", 3, 1), lc("b", 4, 1)]);
        assert_eq!(g.row_text(1), " ab   ");
        assert_eq!(g.cell(1, 1).unwrap().hl_id, 3);
        assert_eq!(g.cell(1, 2).unwrap().hl_id, 4);
        assert_eq!(g.row_text(0), "      ");
    }

    #[test]
    fn grid_line_repeat_expands() {
        let mut g = Grid::new(6, 1);
        g.put_line(0, 0, &[lc("x", 1, 3), lc("-", 2, 2)]);
        assert_eq!(g.row_text(0), "xxx-- ");
        assert_eq!(g.cell(0, 4).unwrap().hl_id, 2);
    }

    #[test]
    fn grid_line_clips_at_right_edge() {
        let mut g = Grid::new(4, 1);
        g.put_line(0, 2, &[lc("y", 1, 10)]);
        assert_eq!(g.row_text(0), "  yy");
    }

    #[test]
    fn grid_line_double_width() {
        let mut g = Grid::new(4, 1);
        g.put_line(0, 0, &[lc("漢", 0, 1), lc("", 0, 1), lc("a", 0, 1)]);
        assert_eq!(g.row_text(0), "漢a ");
        assert_eq!(g.cell(0, 1).unwrap().text, "");
    }

    #[test]
    fn grid_line_bumps_row_version_only_for_that_row() {
        let mut g = Grid::new(3, 2);
        let (v0, v1) = (g.row_version(0), g.row_version(1));
        g.put_line(1, 0, &[lc("z", 0, 1)]);
        assert_eq!(g.row_version(0), v0);
        assert_ne!(g.row_version(1), v1);
    }

    #[test]
    fn grid_scroll_up_full_width() {
        let mut g = grid_from(&["aaa", "bbb", "ccc", "ddd"]);
        g.scroll(0, 4, 0, 3, 1);
        // Last row keeps old content until nvim redraws it.
        assert_eq!(texts(&g), ["bbb", "ccc", "ddd", "ddd"]);
    }

    #[test]
    fn grid_scroll_down_full_width() {
        let mut g = grid_from(&["aaa", "bbb", "ccc", "ddd"]);
        g.scroll(0, 4, 0, 3, -2);
        assert_eq!(texts(&g), ["aaa", "bbb", "aaa", "bbb"]);
    }

    #[test]
    fn grid_scroll_region_only() {
        let mut g = grid_from(&["0123", "4567", "89ab", "cdef"]);
        // Scroll rows 1..3, cols 1..3 up by one.
        g.scroll(1, 3, 1, 3, 1);
        assert_eq!(texts(&g), ["0123", "49a7", "89ab", "cdef"]);
    }

    #[test]
    fn grid_scroll_region_down_partial_cols() {
        let mut g = grid_from(&["0123", "4567", "89ab", "cdef"]);
        g.scroll(0, 4, 2, 4, -1);
        assert_eq!(texts(&g), ["0123", "4523", "8967", "cdab"]);
    }

    #[test]
    fn grid_scroll_more_than_height_is_noop_for_moves() {
        let mut g = grid_from(&["aa", "bb"]);
        g.scroll(0, 2, 0, 2, 5);
        assert_eq!(texts(&g), ["aa", "bb"]);
    }

    #[test]
    fn resize_keeps_top_left() {
        let mut g = grid_from(&["abc", "def"]);
        g.resize(2, 3);
        assert_eq!(texts(&g), ["ab", "de", "  "]);
    }

    #[test]
    fn clear_blanks_everything() {
        let mut g = grid_from(&["abc"]);
        g.clear();
        assert_eq!(g.row_text(0), "   ");
    }
}
