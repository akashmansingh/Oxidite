//! Pure VT parser, terminal state, grid, and scrollback for Oxidite.

#![allow(dead_code)]

pub mod grid;
pub mod handler;
pub mod parser;
pub mod snapshot;
pub mod terminal;

pub use grid::{Attrs, Cell, Color, Cursor, CursorShape, Grid, Row};
pub use handler::{EraseMode, Handler, Mode, SgrAttr};
pub use parser::Parser;
pub use snapshot::{serialize_grid_snapshot, serialize_snapshot};
pub use terminal::{Modes, Terminal};
