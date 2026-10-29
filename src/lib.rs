//! zellij-hints — pure core: no `zellij-tile` dependency, unit-testable on
//! the host (`cargo test`). The wasm plugin shell lives in `src/main.rs`.
//!
//! Pipeline: pane viewport rows + ANSI dump → `Screen` (logical lines of
//! styled `Cell`s, each keeping its exact visual rows) → pattern matching
//! on logical line text → on-screen positions straight from those rows →
//! prefix-free labels ordered by cursor proximity.

pub mod geometry;
pub mod grid;
pub mod labels;
pub mod patterns;

pub use geometry::{char_display_width, lay_row, Glyph, Placement};
pub use grid::{Cell, Color, Line, Screen, Style};
pub use labels::generate as generate_labels;
pub use patterns::{Match, MatchKind, PatternSet};

use std::collections::HashSet;

/// A match with its on-screen position and assigned label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabeledMatch {
    /// Logical line index within the screen.
    pub line: usize,
    /// Char index within the logical line where the match starts.
    pub char_start: usize,
    /// Match text (already punctuation-trimmed) — what gets copied/pasted.
    pub text: String,
    pub kind: MatchKind,
    /// Global visual row (counting wrapped rows of all lines above).
    pub row: usize,
    /// Visual column of the match's first character.
    pub col: usize,
    /// Display width (columns) of the match text.
    pub width: usize,
    pub label: String,
}

/// Options for [`compute_hints`].
pub struct HintOptions<'a> {
    /// Alphabet for labels, e.g. the 26 lowercase letters.
    pub alphabet: &'a [char],
    /// Which pattern kinds to enable.
    pub kinds: &'a [MatchKind],
    /// Cursor position (visual row, col) if visible; used for label ordering.
    /// `None` → order from the bottom-left of the screen (prompt position).
    pub cursor: Option<(usize, usize)>,
    /// Merge duplicate match texts into a single label (tmux-thumbs
    /// `--unique`): only the occurrence nearest the cursor is labeled.
    pub unique: bool,
}

/// Full pipeline: find matches, resolve overlaps, order by proximity to the
/// cursor (or screen bottom), assign prefix-free labels.
///
/// Each line already stores its exact visual rows (as captured from the
/// pane), so match positions are read straight off those rows — the text
/// is never re-wrapped.
pub fn compute_hints(screen: &Screen, opts: &HintOptions) -> Vec<LabeledMatch> {
    let patterns = PatternSet::new(opts.kinds);
    let matches = patterns.find(screen);

    // Remember each line's row offset and the per-char placements so
    // matches can be located on screen.
    let mut row_offsets = Vec::with_capacity(screen.lines.len());
    let mut all_placements = Vec::with_capacity(screen.lines.len());
    let mut total_rows = 0usize;
    for line in &screen.lines {
        row_offsets.push(total_rows);
        all_placements.push(line.placements());
        total_rows += line.rows.len().max(1);
    }

    // Anchor for ordering: the cursor if visible, else bottom-left (where
    // the shell prompt usually is).
    let anchor = opts.cursor.unwrap_or((total_rows.saturating_sub(1), 0));

    // Place every match on screen.
    let mut placed: Vec<(Match, usize, usize)> = Vec::new(); // (match, row, col)
    for m in &matches {
        if m.line >= all_placements.len() {
            continue;
        }
        let Some(&(vrow, vcol)) = all_placements[m.line].get(m.char_start) else {
            continue;
        };
        placed.push((m.clone(), row_offsets[m.line] + vrow, vcol));
    }

    // Sort by distance to the anchor (ties: reading order) — shortest labels
    // then go to the nearest matches.
    placed.sort_by_key(|(_, row, col)| {
        (row.abs_diff(anchor.0) + col.abs_diff(anchor.1), *row, *col)
    });

    // Dedupe identical texts after sorting: the occurrence nearest the
    // anchor comes first and keeps the (shorter) label.
    if opts.unique {
        let mut seen: HashSet<String> = HashSet::new();
        placed.retain(|(m, _, _)| seen.insert(m.text.clone()));
    }

    let labels = generate_labels(placed.len(), opts.alphabet);
    placed
        .into_iter()
        .zip(labels)
        .filter_map(|((m, row, col), label)| {
            if label.is_empty() {
                return None; // alphabet capacity exhausted
            }
            let width = m.text.chars().map(char_display_width).sum();
            Some(LabeledMatch {
                line: m.line,
                char_start: m.char_start,
                text: m.text,
                kind: m.kind,
                row,
                col,
                width,
                label,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_of(lines: &[&str]) -> Screen {
        Screen {
            lines: lines
                .iter()
                .map(|l| Line {
                    rows: vec![l
                        .chars()
                        .map(|ch| Cell {
                            ch,
                            style: Default::default(),
                        })
                        .collect()],
                })
                .collect(),
        }
    }

    fn alpha() -> Vec<char> {
        "abcdefghijklmnopqrstuvwxyz".chars().collect()
    }

    fn opts<'a>(
        alphabet: &'a [char],
        cursor: Option<(usize, usize)>,
    ) -> HintOptions<'a> {
        HintOptions {
            alphabet,
            kinds: &[MatchKind::Url, MatchKind::Path],
            cursor,
            unique: true,
        }
    }

    #[test]
    fn labels_ordered_by_distance_to_cursor() {
        let screen = screen_of(&["a/x.rs first", "a/y.rs second", "a/z.rs third"]);
        // Cursor on the middle line → the nearest match gets "a".
        let hints = compute_hints(&screen, &opts(&alpha(), Some((1, 0))));
        assert_eq!(hints.len(), 3);
        assert_eq!(hints[0].text, "a/y.rs");
        assert_eq!(hints[0].label, "a");
        assert_eq!(hints[0].row, 1);
    }

    #[test]
    fn no_cursor_orders_from_bottom() {
        let screen = screen_of(&["a/x.rs first", "a/y.rs second", "a/z.rs third"]);
        let hints = compute_hints(&screen, &opts(&alpha(), None));
        assert_eq!(hints[0].text, "a/z.rs"); // last line first
        assert_eq!(hints[0].label, "a");
    }

    #[test]
    fn duplicate_text_shares_one_label() {
        let screen = screen_of(&["a/x.rs one", "a/x.rs two", "a/y.rs three"]);
        let hints = compute_hints(&screen, &opts(&alpha(), None));
        // a/y.rs (bottom) gets "a", a/x.rs gets one label only (nearest: line 1)
        let texts: Vec<&str> = hints.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(texts, vec!["a/y.rs", "a/x.rs"]);
        assert_eq!(hints[0].label, "a");
        assert_eq!(hints[1].label, "b");
    }

    #[test]
    fn wrapped_path_is_matched_and_placed() {
        // A 57-char line wrapped at width 20 into rows "error: see src/ve",
        // "_long_directory_nam" and "e/file.rs:42": the path spans all three
        // rows; its start sits at row 0, column 11. The rows are the exact
        // visual rows — no re-wrapping.
        let text = "error: see src/very_long_directory_name/file.rs:42";
        let chars: Vec<char> = text.chars().collect();
        let rows: Vec<Vec<Cell>> = chars
            .chunks(20)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|&ch| Cell {
                        ch,
                        style: Default::default(),
                    })
                    .collect()
            })
            .collect();
        let screen = Screen {
            lines: vec![Line { rows }],
        };
        let hints = compute_hints(&screen, &opts(&alpha(), None));
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].text, "src/very_long_directory_name/file.rs:42");
        // "src/" starts at char 11 → row 0 (cols 0..19), col 11.
        assert_eq!(hints[0].row, 0);
        assert_eq!(hints[0].col, 11);
        assert_eq!(hints[0].width, 39);
    }

    #[test]
    fn hints_use_viewport_text_across_wrap_boundaries() {
        // The dump drops the space at the wrap boundary, so its text reads
        // "…src/foo.rs andmore". Merged with the exact viewport rows, the
        // match must still be "src/foo.rs" — not "src/foo.rsmore" — and
        // sit at the right column.
        let viewport = vec!["see src/foo.rs and ".to_string(), "more".to_string()];
        let dump = b"see \x1b[38;5;2msrc/foo.rs\x1b[0m andmore\x1b[m";
        let screen = Screen::from_viewport_with_ansi(&viewport, dump).expect("merge");
        let hints = compute_hints(&screen, &opts(&alpha(), None));
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].text, "src/foo.rs");
        assert_eq!(hints[0].row, 0);
        assert_eq!(hints[0].col, 4);
    }

    #[test]
    fn wide_chars_keep_columns_aligned() {
        // 漢 is 2 columns wide, so "src/x.rs" starts at column 5, not 4.
        let screen = screen_of(&["漢字 src/x.rs"]);
        let hints = compute_hints(&screen, &opts(&alpha(), None));
        assert_eq!(hints[0].col, 5);
    }
}
