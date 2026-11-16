//! Pure data model for a Neovim UI client: grids, cells, highlights,
//! cursor, modes and windows. No UI or RPC dependencies.

mod editor;
mod event;
mod grid;
mod hl;

pub use editor::{Cmdline, Cursor, Editor, Margins, Message, PopupMenu, Scrolled, StatusSegment, Viewport, Window, WindowKind, DEFAULT_GRID};
pub use event::{ChunkStyle, FloatAnchor, OptionValue, PmItem, RedrawEvent, StyledChunk};
pub use grid::{Cell, Grid, LineCell};
pub use hl::{CursorShape, DefaultColors, HlAttr, HlId, ModeInfo, ResolvedColors, Underline};
