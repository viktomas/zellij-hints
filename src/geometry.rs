//! On-screen geometry: turn logical lines (wrapped rows concatenated by the
//! dump) back into visual rows of a fixed width, wide-character aware.

use crate::grid::Cell;
use unicode_width::UnicodeWidthChar;

/// Display width of a character in terminal columns (0 for combining marks
/// and controls, 1 for most, 2 for CJK/emoji).
pub fn char_display_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Where a character sits on screen: `(visual_row, column)` within the block
/// of rows that one logical line wraps into.
pub type Placement = (usize, usize);

/// Split a logical line into visual rows `width` columns wide.
///
/// Returns the visual rows (each padded to `width` with unstyled spaces) and,
/// for every character of `line`, its placement. Mirrors how a terminal
/// wraps: a wide char that doesn't fit in the last column moves to the next
/// row (leaving a blank cell behind); zero-width chars attach to the current
/// position without advancing.
pub fn wrap_line(line: &[Cell], width: usize) -> (Vec<Vec<Cell>>, Vec<Placement>) {
    let mut rows: Vec<Vec<Cell>> = vec![Vec::new()];
    let mut placements: Vec<Placement> = Vec::with_capacity(line.len());
    let mut col = 0usize;

    for cell in line {
        let w = char_display_width(cell.ch);
        if w > 0 && col + w > width {
            // Wrap: pad the current row, start a new one.
            if let Some(last) = rows.last_mut() {
                while last.len() < width {
                    last.push(Cell::default());
                }
            }
            rows.push(Vec::new());
            col = 0;
        }
        if w == 0 {
            // Zero-width chars (combining marks) attach to the preceding
            // char: they share its placement and don't advance the column.
            let p = placements.last().copied().unwrap_or((rows.len() - 1, 0));
            placements.push(p);
            if let Some(last) = rows.last_mut() {
                last.push(*cell);
            }
            continue;
        }
        placements.push((rows.len() - 1, col));
        if let Some(last) = rows.last_mut() {
            last.push(*cell);
        }
        col += w;
    }

    // Pad the final row so callers can rely on full-width rows.
    if let Some(last) = rows.last_mut() {
        while last.len() < width {
            last.push(Cell::default());
        }
    }

    (rows, placements)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(s: &str) -> Vec<Cell> {
        s.chars()
            .map(|ch| Cell {
                ch,
                style: Default::default(),
            })
            .collect()
    }

    #[test]
    fn short_line_is_one_row() {
        let (rows, placements) = wrap_line(&cells("hi"), 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 10);
        assert_eq!(placements, vec![(0, 0), (0, 1)]);
    }

    #[test]
    fn wrapped_line_splits_at_width() {
        let text = "abcdefghijklm"; // 13 chars, width 5 → rows of 5,5,3
        let (rows, placements) = wrap_line(&cells(text), 5);
        assert_eq!(rows.len(), 3);
        assert_eq!(placements[4], (0, 4));
        assert_eq!(placements[5], (1, 0));
        assert_eq!(placements[12], (2, 2));
    }

    #[test]
    fn wide_char_never_splits() {
        // CJK chars are 2 columns wide; with width 5 the second wide char
        // starts at col 2, the third would start at col 4 and needs 2 cols,
        // so it wraps.
        let text = "漢字漢字"; // 4 wide chars
        let (rows, placements) = wrap_line(&cells(text), 5);
        assert_eq!(placements[0], (0, 0));
        assert_eq!(placements[1], (0, 2));
        assert_eq!(placements[2], (1, 0)); // wrapped
        // Row 0: 漢字 + 1 pad cell = 5 columns
        assert_eq!(rows[0].len(), 5);
    }

    #[test]
    fn zero_width_char_shares_position() {
        // 'e' + combining acute (U+0301, zero width) + 'a' — written with
        // escapes so the source can't accidentally hold precomposed é.
        let text = "\u{65}\u{301}a";
        assert_eq!(text.chars().count(), 3);
        let (rows, placements) = wrap_line(&cells(text), 10);
        assert_eq!(placements[0], (0, 0));
        assert_eq!(placements[1], (0, 0)); // combining attaches to 'e'
        assert_eq!(placements[2], (0, 1));
        assert_eq!(rows.len(), 1);
    }
}

