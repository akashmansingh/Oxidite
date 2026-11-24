//! Serializes Terminal screen state into repaint escape sequences.
//!
//! When a client attaches or reconnects and its requested offset is outside
//! the replay buffer, the server sends a Snapshot frame containing escape
//! sequences that repaint the full screen state identically without client-side custom logic.

#![allow(clippy::similar_names, clippy::if_not_else)]

use crate::grid::{Attrs, Color, Grid};
use crate::terminal::Terminal;

/// Serializes visible terminal screen state into ANSI repaint bytes.
#[must_use]
pub fn serialize_snapshot(terminal: &Terminal) -> Vec<u8> {
    serialize_grid_snapshot(&terminal.grid, terminal.modes.cursor_visible)
}

/// Serializes a Grid into ANSI repaint bytes.
#[must_use]
pub fn serialize_grid_snapshot(grid: &Grid, cursor_visible: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(grid.rows * grid.cols * 2 + 128);

    // 1. Hide cursor during screen repaint
    out.extend_from_slice(b"\x1b[?25l");
    // 2. Clear entire screen
    out.extend_from_slice(b"\x1b[2J");

    let mut cur_fg = Color::Default;
    let mut cur_bg = Color::Default;
    let mut cur_attrs = Attrs::empty();

    for row_idx in 0..grid.rows {
        // Move cursor to row (1-indexed), column 1
        let row_num = row_idx + 1;
        out.extend_from_slice(format!("\x1b[{row_num};1H").as_bytes());

        let row = &grid[row_idx];
        for col_idx in 0..grid.cols {
            let cell = &row[col_idx];

            // If attributes or colors changed, emit SGR
            if cell.attrs != cur_attrs || cell.fg != cur_fg || cell.bg != cur_bg {
                emit_sgr_diff(&mut out, cur_attrs, cell.attrs, cell.fg, cell.bg);
                cur_attrs = cell.attrs;
                cur_fg = cell.fg;
                cur_bg = cell.bg;
            }

            // Emit character
            let mut char_buf = [0u8; 4];
            let encoded = cell.ch.encode_utf8(&mut char_buf);
            out.extend_from_slice(encoded.as_bytes());
        }
    }

    // Reset SGR attributes at end of screen
    out.extend_from_slice(b"\x1b[0m");

    // Position cursor at terminal cursor coordinates (1-indexed)
    let cursor_row = (grid.cursor.row + 1).min(grid.rows);
    let cursor_col = (grid.cursor.col + 1).min(grid.cols);
    out.extend_from_slice(format!("\x1b[{cursor_row};{cursor_col}H").as_bytes());

    // Restore cursor visibility
    if cursor_visible {
        out.extend_from_slice(b"\x1b[?25h");
    } else {
        out.extend_from_slice(b"\x1b[?25l");
    }

    out
}

fn emit_sgr_diff(out: &mut Vec<u8>, old_attrs: Attrs, new_attrs: Attrs, fg: Color, bg: Color) {
    if !new_attrs.contains(old_attrs) {
        out.extend_from_slice(b"\x1b[0m");
        if new_attrs.contains(Attrs::BOLD) {
            out.extend_from_slice(b"\x1b[1m");
        }
        if new_attrs.contains(Attrs::DIM) {
            out.extend_from_slice(b"\x1b[2m");
        }
        if new_attrs.contains(Attrs::ITALIC) {
            out.extend_from_slice(b"\x1b[3m");
        }
        if new_attrs.contains(Attrs::UNDERLINE) {
            out.extend_from_slice(b"\x1b[4m");
        }
        if new_attrs.contains(Attrs::INVERSE) {
            out.extend_from_slice(b"\x1b[7m");
        }
        if new_attrs.contains(Attrs::HIDDEN) {
            out.extend_from_slice(b"\x1b[8m");
        }
        if new_attrs.contains(Attrs::STRIKETHROUGH) {
            out.extend_from_slice(b"\x1b[9m");
        }
    } else {
        let added = new_attrs - old_attrs;
        if added.contains(Attrs::BOLD) {
            out.extend_from_slice(b"\x1b[1m");
        }
        if added.contains(Attrs::DIM) {
            out.extend_from_slice(b"\x1b[2m");
        }
        if added.contains(Attrs::ITALIC) {
            out.extend_from_slice(b"\x1b[3m");
        }
        if added.contains(Attrs::UNDERLINE) {
            out.extend_from_slice(b"\x1b[4m");
        }
        if added.contains(Attrs::INVERSE) {
            out.extend_from_slice(b"\x1b[7m");
        }
        if added.contains(Attrs::HIDDEN) {
            out.extend_from_slice(b"\x1b[8m");
        }
        if added.contains(Attrs::STRIKETHROUGH) {
            out.extend_from_slice(b"\x1b[9m");
        }
    }

    emit_color(out, fg, true);
    emit_color(out, bg, false);
}

fn emit_color(out: &mut Vec<u8>, color: Color, is_fg: bool) {
    let base = if is_fg { 38 } else { 48 };
    match color {
        Color::Default => {
            let code = if is_fg { 39 } else { 49 };
            out.extend_from_slice(format!("\x1b[{code}m").as_bytes());
        }
        Color::Indexed(idx) => {
            if idx < 8 {
                let code = (if is_fg { 30 } else { 40 }) + idx;
                out.extend_from_slice(format!("\x1b[{code}m").as_bytes());
            } else if idx < 16 {
                let code = (if is_fg { 90 } else { 100 }) + (idx - 8);
                out.extend_from_slice(format!("\x1b[{code}m").as_bytes());
            } else {
                out.extend_from_slice(format!("\x1b[{base};5;{idx}m").as_bytes());
            }
        }
        Color::Rgb(r, g, b) => {
            out.extend_from_slice(format!("\x1b[{base};2;{r};{g};{b}m").as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;
    use crate::terminal::Terminal;

    #[test]
    fn test_serialize_grid_snapshot_produces_non_empty_bytes() {
        let mut term = Terminal::new(80, 24);
        let mut parser = Parser::new();
        parser.advance(
            &mut term,
            b"Hello World!\r\nLine 2 with \x1b[1;31mRed Bold\x1b[0m text.",
        );

        let snapshot = serialize_snapshot(&term);
        assert!(!snapshot.is_empty(), "snapshot should not be empty");
        assert!(snapshot.starts_with(b"\x1b[?25l\x1b[2J"));
        // When parsed into a second terminal, contents match visible screen
        let mut second = Terminal::new(80, 24);
        let mut second_parser = Parser::new();
        second_parser.advance(&mut second, &snapshot);

        assert_eq!(second.grid[0][0].ch, 'H');
        assert_eq!(second.grid[0][1].ch, 'e');
        assert_eq!(second.grid[0][11].ch, '!');
        assert_eq!(second.grid[1][0].ch, 'L');
        assert_eq!(second.grid[1][12].ch, 'R');
    }
}
