//! Styled-cell grid parsed from pane content.
//!
//! Two input paths (both produce the same `Screen`):
//! - ANSI dump (`DumpScreen --ansi`): full SGR colors; **logical** lines —
//!   the dump only emits `\n` at canonical line boundaries, so rows produced
//!   by terminal line-wrapping arrive concatenated (matching across wraps
//!   works; `geometry::wrap_line` recovers on-screen positions).
//! - Plain viewport rows (`get_pane_scrollback`): monochrome fallback; rows
//!   are joined into logical lines with a full-width heuristic.

use vte::{Params, Parser, Perform};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Faint (SGR 2). Never set by parsing — used by the overlay renderer
    /// to dim non-match content.
    pub dim: bool,
}

impl Style {
    /// SGR escape that establishes this style from a clean slate.
    pub fn sgr(&self) -> String {
        let mut codes: Vec<String> = vec!["0".into()];
        if self.bold {
            codes.push("1".into());
        }
        if self.dim {
            codes.push("2".into());
        }
        if self.italic {
            codes.push("3".into());
        }
        if self.underline {
            codes.push("4".into());
        }
        match self.fg {
            Color::Default => {}
            Color::Indexed(i) => codes.push(format!("38;5;{i}")),
            Color::Rgb(r, g, b) => codes.push(format!("38;2;{r};{g};{b}")),
        }
        match self.bg {
            Color::Default => {}
            Color::Indexed(i) => codes.push(format!("48;5;{i}")),
            Color::Rgb(r, g, b) => codes.push(format!("48;2;{r};{g};{b}")),
        }
        format!("\x1b[{}m", codes.join(";"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            style: Style::default(),
        }
    }
}

/// The pane as logical lines of styled cells.
#[derive(Clone, Debug, Default)]
pub struct Screen {
    pub lines: Vec<Vec<Cell>>,
}

impl Screen {
    /// Parse an ANSI screen dump (`DumpScreen --ansi` output).
    pub fn from_ansi(bytes: &[u8]) -> Screen {
        let mut sink = GridSink::default();
        let mut parser = Parser::new();
        for &b in bytes {
            parser.advance(&mut sink, b);
        }
        Screen {
            lines: sink.finish(),
        }
    }

    /// Build from plain viewport rows (monochrome fallback). Rows that are
    /// full-width and don't end in a space were almost certainly produced by
    /// line wrapping, so they're joined into one logical line — matching then
    /// works across wraps even without the colored dump.
    pub fn from_viewport_rows(rows: &[String], width: usize) -> Screen {
        let mut lines: Vec<Vec<Cell>> = Vec::new();
        let mut cur: Vec<Cell> = Vec::new();
        for row in rows {
            let chars: Vec<char> = row.chars().collect();
            let wrapped = width > 0
                && chars.len() == width
                && chars.last().is_some_and(|c| *c != ' ');
            for ch in chars {
                cur.push(Cell {
                    ch,
                    style: Style::default(),
                });
            }
            if !wrapped {
                lines.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        // Drop trailing all-blank lines (below where output ended).
        while lines
            .last()
            .is_some_and(|l| l.iter().all(|c| c.ch == ' '))
        {
            lines.pop();
        }
        Screen { lines }
    }

    pub fn text_of_line(&self, idx: usize) -> String {
        self.lines
            .get(idx)
            .map(|l| l.iter().map(|c| c.ch).collect())
            .unwrap_or_default()
    }
}

/// vte sink: raw ANSI bytes in, logical lines of styled cells out.
#[derive(Default)]
struct GridSink {
    lines: Vec<Vec<Cell>>,
    cur: Vec<Cell>,
    style: Style,
}

impl GridSink {
    fn finish(mut self) -> Vec<Vec<Cell>> {
        if !self.cur.is_empty() {
            self.lines.push(std::mem::take(&mut self.cur));
        }
        // A pane dump includes the whole viewport; everything below where the
        // output ended is blank. Trim it so it doesn't show as dead space.
        while self
            .lines
            .last()
            .is_some_and(|l| l.iter().all(|c| c.ch == ' '))
        {
            self.lines.pop();
        }
        self.lines
    }

    /// Interpret one SGR sequence into `self.style`. Handles the subset the
    /// dump can contain: reset, bold/italic/underline, fg/bg (basic,
    /// bright, 256-color and truecolor).
    fn sgr(&mut self, params: &Params) {
        // Flatten sub-params (`38:5:196` and `38;5;196` both work).
        let flat: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
        if flat.is_empty() {
            self.style = Style::default();
            return;
        }
        let mut i = 0;
        while i < flat.len() {
            match flat[i] {
                0 => self.style = Style::default(),
                1 => self.style.bold = true,
                3 => self.style.italic = true,
                4 => self.style.underline = true,
                22 => self.style.bold = false,
                23 => self.style.italic = false,
                24 => self.style.underline = false,
                30..=37 => self.style.fg = Color::Indexed((flat[i] - 30) as u8),
                39 => self.style.fg = Color::Default,
                90..=97 => self.style.fg = Color::Indexed((flat[i] - 90 + 8) as u8),
                40..=47 => self.style.bg = Color::Indexed((flat[i] - 40) as u8),
                49 => self.style.bg = Color::Default,
                100..=107 => self.style.bg = Color::Indexed((flat[i] - 100 + 8) as u8),
                38 | 48 => {
                    let is_fg = flat[i] == 38;
                    let color = match flat.get(i + 1) {
                        Some(5) => flat.get(i + 2).map(|v| Color::Indexed(*v as u8)),
                        Some(2) => match (flat.get(i + 2), flat.get(i + 3), flat.get(i + 4)) {
                            (Some(r), Some(g), Some(b)) => {
                                Some(Color::Rgb(*r as u8, *g as u8, *b as u8))
                            }
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(c) = color {
                        if is_fg {
                            self.style.fg = c;
                        } else {
                            self.style.bg = c;
                        }
                    }
                    // Skip consumed params regardless of parse success.
                    match flat.get(i + 1) {
                        Some(5) => i += 2,
                        Some(2) => i += 4,
                        _ => {}
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

impl Perform for GridSink {
    fn print(&mut self, c: char) {
        self.cur.push(Cell {
            ch: c,
            style: self.style,
        });
    }

    fn execute(&mut self, byte: u8) {
        // In the dump, `\n` is a canonical line boundary. `\r` and other C0
        // controls carry no layout meaning for a line grid.
        if byte == b'\n' {
            self.lines.push(std::mem::take(&mut self.cur));
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _i: &[u8], _ignore: bool, action: char) {
        if action == 'm' {
            self.sgr(params);
        }
        // Cursor movement sequences (CUP, CUU, EL, …) are not replayed: the
        // dump linearizes the viewport, and our wrap reconstruction ignores
        // them. Known limitation for apps that paint with cursor moves.
    }
}
