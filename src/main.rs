//! zellij-hints plugin shell (wasm32-wasip1).
//!
//! Architecture (blueprint: zellij-copy-mode — reimplemented from scratch,
//! that repo is unlicensed):
//!   1. A keybind opens us as a floating pane. From `PaneUpdate` we find the
//!      target (the focused, tiled terminal pane in our tab) and resize our
//!      own floating pane to exactly cover it, borderless. The target keeps
//!      running live underneath — nothing is modified, nothing to restore.
//!   2. We read the target's viewport twice: `get_pane_scrollback` gives
//!      plain text synchronously (monochrome, shown immediately), then
//!      `DumpScreen --ansi` writes the colored viewport to a file in our
//!      initial cwd which we read back through the `/host` mount and re-parse
//!      with `vte` — upgrading to full color.
//!   3. The pure core (`zellij_hints` lib) finds URL/path matches, resolves
//!      overlaps by priority, assigns prefix-free labels ordered by cursor
//!      proximity. We render the viewport dimmed with labels over the match
//!      starts; typing a label copies it (uppercase: copies + pastes it into
//!      the target pane; URLs: open in the browser).
//!   4. Exit drops the key intercept and closes our own pane.

use std::collections::BTreeMap;
use std::path::PathBuf;

use zellij_tile::prelude::*;

use zellij_hints::{
    compute_hints, wrap_line, HintOptions, LabeledMatch, MatchKind, Screen,
};

register_plugin!(State);

const DUMP_NAME: &str = ".zellij-hints-dump.ansi";
const MAX_DUMP_POLLS: u32 = 40;

// ---------------------------------------------------------------------------
// Config (flattened KDL plugin config: BTreeMap<String, String>)
// ---------------------------------------------------------------------------

struct Config {
    alphabet: Vec<char>,
    /// Label colors per match kind (RGB).
    url_fg: (u8, u8, u8),
    path_fg: (u8, u8, u8),
    dim: bool,
    paste_on_uppercase: bool,
    /// Host command that opens a URL (macOS: `open`, Linux: `xdg-open`).
    open_command: String,
    kinds: Vec<MatchKind>,
    /// Identical match texts share one label.
    unique: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            alphabet: "abcdefghijklmnopqrstuvwxyz".chars().collect(),
            url_fg: (0, 175, 255),   // blue-ish
            path_fg: (80, 250, 123), // green
            dim: true,
            paste_on_uppercase: true,
            open_command: "open".into(),
            kinds: vec![MatchKind::Url, MatchKind::Path],
            unique: true,
        }
    }
}

fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

impl Config {
    fn from_map(map: &BTreeMap<String, String>) -> Config {
        let mut cfg = Config::default();
        let get = |k: &str| map.get(k).map(|s| s.as_str());
        if let Some(a) = get("alphabet") {
            let chars: Vec<char> = a.chars().filter(|c| c.is_alphabetic()).collect();
            if !chars.is_empty() {
                cfg.alphabet = chars;
            }
        }
        if let Some(c) = get("url_fg").and_then(parse_hex_color) {
            cfg.url_fg = c;
        }
        if let Some(c) = get("path_fg").and_then(parse_hex_color) {
            cfg.path_fg = c;
        }
        if let Some("false") = get("dim") {
            cfg.dim = false;
        }
        if let Some("false") = get("paste_on_uppercase") {
            cfg.paste_on_uppercase = false;
        }
        if let Some(cmd) = get("open_command") {
            if !cmd.is_empty() {
                cfg.open_command = cmd.into();
            }
        }
        if let Some(p) = get("patterns") {
            cfg.kinds = MatchKind::parse_list(p);
        }
        if let Some("false") = get("unique") {
            cfg.unique = false;
        }
        cfg
    }
}

// ---------------------------------------------------------------------------
// Plugin state
// ---------------------------------------------------------------------------

struct State {
    self_id: Option<PaneId>,
    host_cwd: PathBuf,

    perms_granted: bool,
    error: Option<String>,

    /// The pane we're hinting: focused, tiled, non-plugin terminal in our tab.
    target: Option<PaneId>,
    /// Outer geometry of the target: (x, y, cols, rows).
    target_geom: Option<(usize, usize, usize, usize)>,
    /// Where the target's *content* starts relative to its outer geometry
    /// (non-zero when pane frames are on).
    content_off: (usize, usize),
    content_cols: usize,
    /// Cursor position in the viewport (col, row), if visible.
    cursor: Option<(usize, usize)>,

    screen: Option<Screen>,
    colored: bool,
    dump_polls: u32,

    ready: bool,
    hints: Vec<LabeledMatch>,
    typed: String,
    /// Set whenever something changed that warrants a repaint; consumed
    /// (cleared) by the `update` return value. Without this, the painted
    /// overlay only ever refreshed via resize side-effects.
    dirty: bool,

    cfg: Config,

    /// Our own pane's render size.
    rows: usize,
    cols: usize,
}

impl Default for State {
    fn default() -> Self {
        State {
            self_id: None,
            host_cwd: PathBuf::new(),
            perms_granted: false,
            error: None,
            target: None,
            target_geom: None,
            content_off: (0, 0),
            content_cols: 0,
            cursor: None,
            screen: None,
            colored: false,
            dump_polls: 0,
            ready: false,
            hints: Vec::new(),
            typed: String::new(),
            dirty: false,
            cfg: Config::default(),
            rows: 0,
            cols: 0,
        }
    }
}

impl State {
    // --- lifecycle ---------------------------------------------------------

    fn dump_host_path(&self) -> String {
        // DumpScreen writes a raw host path; we read it back through the
        // `/host` mount, which maps to the plugin's initial cwd.
        format!("{}/{DUMP_NAME}", self.host_cwd.display())
    }

    fn dump_sandbox_path(&self) -> String {
        format!("/host/{DUMP_NAME}")
    }

    /// Resize our own floating pane to exactly cover the target pane,
    /// borderless — the overlay appears in place, no layout shift.
    fn cover_target(&self) {
        let Some(PaneId::Plugin(id)) = self.self_id else {
            return;
        };
        let Some((x, y, cols, rows)) = self.target_geom else {
            return;
        };
        if let Some(coords) = FloatingPaneCoordinates::new(
            Some(x.to_string()),
            Some(y.to_string()),
            Some(cols.to_string()),
            Some(rows.to_string()),
            None,
            Some(true), // borderless
        ) {
            change_floating_panes_coordinates(vec![(PaneId::Plugin(id), coords)]);
        }
    }

    /// Start once permissions + target are known: grab the monochrome
    /// viewport synchronously, then kick off the colored ANSI dump.
    fn maybe_start(&mut self) {
        if !self.perms_granted || self.target.is_none() || self.screen.is_some() {
            return;
        }
        let target = self.target.unwrap();
        match get_pane_scrollback(target, false) {
            Ok(contents) => {
                self.cursor = contents.cursor;
                self.screen = Some(Screen::from_viewport_rows(
                    &contents.viewport,
                    self.content_cols,
                ));
                self.compute_hints();
            }
            Err(e) => self.error = Some(format!("hints: scrollback error: {e}")),
        }

        // Colored upgrade: DumpScreen --ansi (viewport only) → file → /host.
        let mut ctx = BTreeMap::new();
        ctx.insert("hints_dump".to_string(), "1".to_string());
        run_action(
            actions::Action::DumpScreen {
                file_path: Some(self.dump_host_path()),
                include_scrollback: false,
                pane_id: Some(target),
                ansi: true,
            },
            ctx,
        );
        self.dump_polls = 0;
        set_timeout(0.05);
    }

    /// Poll for the ANSI dump; on arrival replace the grid with the colored
    /// one and recompute hints. Gives up after ~2s (monochrome stays).
    fn poll_dump(&mut self) {
        if self.colored || self.screen.is_none() {
            return;
        }
        self.dump_polls += 1;
        match std::fs::read(self.dump_sandbox_path()) {
            Ok(bytes) if !bytes.is_empty() => {
                let screen = Screen::from_ansi(&bytes);
                if !screen.lines.is_empty() {
                    self.screen = Some(screen);
                    self.colored = true;
                    let _ = std::fs::remove_file(self.dump_sandbox_path());
                    self.compute_hints();
                    return;
                }
            }
            _ => {}
        }
        if self.dump_polls < MAX_DUMP_POLLS {
            set_timeout(0.05);
        }
    }

    fn compute_hints(&mut self) {
        let Some(screen) = self.screen.clone() else {
            return;
        };
        // `cursor` from PaneContents is (col, row) in the viewport.
        let cursor = self.cursor.map(|(x, y)| (y, x));
        let opts = HintOptions {
            alphabet: &self.cfg.alphabet,
            kinds: &self.cfg.kinds,
            cursor,
            unique: self.cfg.unique,
        };
        self.hints = compute_hints(&screen, self.content_cols, &opts);
        self.ready = true;
        self.dirty = true;
    }

    fn exit(&mut self) {
        // Non-destructive: the target was never touched; just un-grab keys
        // and close our own floating pane.
        clear_key_presses_intercepts();
        close_self();
    }

    // --- actions -----------------------------------------------------------

    fn trigger(&mut self, hint: &LabeledMatch, upper: bool) {
        match hint.kind {
            MatchKind::Url => {
                if upper {
                    copy_to_clipboard(hint.text.clone());
                }
                let ctx = BTreeMap::new();
                run_command(&[&self.cfg.open_command, &hint.text], ctx);
            }
            MatchKind::Path => {
                copy_to_clipboard(hint.text.clone());
                if upper && self.cfg.paste_on_uppercase {
                    if let Some(target) = self.target {
                        write_chars_to_pane_id(&hint.text, target);
                    }
                }
            }
        }
        self.exit();
    }

    // --- keys --------------------------------------------------------------

    fn handle_key(&mut self, key: KeyWithModifier) {
        let ctrl = key.key_modifiers.contains(&KeyModifier::Ctrl);
        let shift = key.key_modifiers.contains(&KeyModifier::Shift);
        match key.bare_key {
            BareKey::Esc => self.exit(),
            BareKey::Char('c') if ctrl => self.exit(),
            BareKey::Backspace => {
                if self.typed.pop().is_some() {
                    self.dirty = true;
                }
            }
            BareKey::Char(c) => {
                let lower = c.to_ascii_lowercase();
                if !self.cfg.alphabet.contains(&lower) {
                    return; // not a label key — ignore
                }
                self.typed.push(lower);
                let upper = shift || c.is_uppercase();
                if let Some(hint) = self.hints.iter().find(|h| h.label == self.typed) {
                    let hint = hint.clone();
                    self.trigger(&hint, upper);
                } else if !self.hints.iter().any(|h| h.label.starts_with(&self.typed)) {
                    // No label has this prefix — dead end, start over.
                    self.typed.clear();
                }
                self.dirty = true;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// ZellijPlugin
// ---------------------------------------------------------------------------

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.cfg = Config::from_map(&configuration);
        let ids = get_plugin_ids();
        self.self_id = Some(PaneId::Plugin(ids.plugin_id));
        self.host_cwd = ids.initial_cwd;

        request_permission(&[
            // PaneUpdate: target pane id/geometry/cursor.
            PermissionType::ReadApplicationState,
            // Resize our own floating pane over the target.
            PermissionType::ChangeApplicationState,
            // Grab the keyboard while the overlay is up (0.45 maps
            // intercept_key_presses here).
            PermissionType::InterceptInput,
            // get_pane_scrollback.
            PermissionType::ReadPaneContents,
            // copy_to_clipboard.
            PermissionType::WriteToClipboard,
            // write_chars_to_pane_id (paste).
            PermissionType::WriteToStdin,
            // run_command (open URLs on the host).
            PermissionType::RunCommands,
            // run_action(DumpScreen) for the colored dump.
            PermissionType::RunActionsAsUser,
        ]);
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::PaneUpdate,
            EventType::Key,
            EventType::ActionComplete,
            EventType::Timer,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.perms_granted = true;
                self.maybe_start();
                true
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.error = Some("hints: permissions denied".into());
                true
            }
            Event::PaneUpdate(manifest) => {
                // Which tab are we in? (the tab containing our plugin pane)
                let my_tab = manifest.panes.iter().find_map(|(tab, panes)| {
                    panes
                        .iter()
                        .any(|p| p.is_plugin && Some(PaneId::Plugin(p.id)) == self.self_id)
                        .then_some(*tab)
                });
                // Target: the focused, tiled, live terminal pane in our tab.
                // Tab-scoping disambiguates multi-tab sessions where several
                // panes report is_focused (one per tab).
                let mut found = None;
                for (tab, panes) in &manifest.panes {
                    if Some(*tab) != my_tab {
                        continue;
                    }
                    for p in panes {
                        if p.is_focused
                            && !p.is_plugin
                            && !p.is_suppressed
                            && !p.is_floating
                            && !p.exited
                        {
                            found = Some((p.id, p));
                            break;
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                }
                if let Some((id, p)) = found {
                    self.target = Some(PaneId::Terminal(id));
                    self.target_geom = Some((p.pane_x, p.pane_y, p.pane_columns, p.pane_rows));
                    // Content offset within the outer geometry (non-zero
                    // when pane frames are on).
                    self.content_off = (
                        p.pane_content_x.saturating_sub(p.pane_x),
                        p.pane_content_y.saturating_sub(p.pane_y),
                    );
                    self.content_cols = p.pane_content_columns;
                    if self.cursor.is_none() {
                        self.cursor = p.cursor_coordinates_in_pane;
                    }
                    // Resize ASAP — if this lands before the first paint the
                    // overlay appears flash-free.
                    self.cover_target();
                }
                self.maybe_start();
                std::mem::take(&mut self.dirty)
            }
            // Backup path for the dump poll; the timer is primary.
            Event::ActionComplete(..) => {
                self.poll_dump();
                std::mem::take(&mut self.dirty)
            }
            Event::Timer(_) => {
                self.poll_dump();
                std::mem::take(&mut self.dirty)
            }
            Event::Key(key) | Event::InterceptedKeyPress(key) => {
                if self.ready {
                    self.handle_key(key);
                    // Repaint when the typed prefix changed (footer) or the
                    // plugin exited.
                    std::mem::take(&mut self.dirty)
                } else if let BareKey::Esc = key.bare_key {
                    // Let the user dismiss an error/stuck overlay.
                    self.exit();
                    false
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.rows = rows;
        self.cols = cols;

        if let Some(err) = &self.error {
            println!("\x1b[0m\x1b[38;5;203m {err}\r");
            return;
        }
        let Some(screen) = self.screen.as_ref() else {
            return; // nothing loaded yet — stay blank briefly
        };
        if !self.ready {
            return;
        }

        let (ox, oy) = self.content_off;
        let content_cols = self.content_cols.min(cols.saturating_sub(ox));

        // Build a full render grid of (char, style). Base pass: the screen's
        // wrapped visual rows, dimmed outside matches; then labels are
        // overdrawn at match starts.
        let mut grid: Vec<Vec<(char, zellij_hints::Style)>> =
            vec![vec![(' ', Default::default()); cols]; rows];

        // Which char ranges of each line belong to a hint (keep undimmed).
        use std::collections::HashMap;
        let mut spans: HashMap<usize, Vec<(usize, usize)>> = HashMap::new(); // line → ranges
        for h in &self.hints {
            spans
                .entry(h.line)
                .or_default()
                .push((h.char_start, h.char_start + h.text.chars().count()));
        }
        let in_hint = |line: usize, ci: usize| {
            spans
                .get(&line)
                .is_some_and(|rs| rs.iter().any(|&(s, e)| ci >= s && ci < e))
        };

        let mut row_cursor = oy;
        'lines: for (line_idx, line) in screen.lines.iter().enumerate() {
            let (visual_rows, placements) = wrap_line(line, self.content_cols);
            for (vrow, vcells) in visual_rows.iter().enumerate() {
                if row_cursor >= rows {
                    break 'lines;
                }
                for (i, cell) in vcells.iter().take(content_cols).enumerate() {
                    let mut style = cell.style;
                    let ci = placements
                        .iter()
                        .position(|&(r, c)| r == vrow && c == i)
                        .unwrap_or(usize::MAX);
                    let highlighted = ci != usize::MAX && in_hint(line_idx, ci);
                    if self.cfg.dim && !highlighted {
                        style.dim = true;
                    }
                    grid[row_cursor][ox + i] = (cell.ch, style);
                }
                row_cursor += 1;
            }
        }

        // Overdraw labels at match starts (configurable color per kind,
        // bold, original background kept).
        for h in &self.hints {
            let r = oy + h.row;
            let c = ox + h.col;
            let (fr, fg, fb) = match h.kind {
                MatchKind::Url => self.cfg.url_fg,
                MatchKind::Path => self.cfg.path_fg,
            };
            for (i, lc) in h.label.chars().enumerate() {
                if r < rows && c + i < cols {
                    let style = zellij_hints::Style {
                        fg: zellij_hints::Color::Rgb(fr, fg, fb),
                        bg: grid[r][c + i].1.bg,
                        bold: true,
                        italic: false,
                        underline: false,
                        dim: false,
                    };
                    grid[r][c + i] = (lc, style);
                }
            }
        }

        // While typing (or when there's nothing to select), a compact footer
        // replaces the bottom row; otherwise the pane looks exactly like a
        // screenshot of the target — labels are the only signal, Esc always
        // exits.
        let footer = if self.hints.is_empty() {
            Some(" hints: no matches · Esc ".to_string())
        } else if !self.typed.is_empty() {
            Some(format!(" hints: {} ", self.typed))
        } else {
            None
        };

        // Emit the grid as SGR runs, one line per println.
        for r in 0..rows {
            if r == rows - 1 {
                if let Some(msg) = &footer {
                    let pad = cols.saturating_sub(msg.chars().count());
                    println!(
                        "\x1b[1m\x1b[48;5;24m\x1b[38;5;231m{}{}\x1b[0m\r",
                        " ".repeat(pad),
                        msg
                    );
                    continue;
                }
            }
            let mut out = String::new();
            let mut last: Option<zellij_hints::Style> = None;
            for c in 0..cols {
                let (ch, style) = grid[r][c];
                if last != Some(style) {
                    out.push_str(&style.sgr());
                    last = Some(style);
                }
                out.push(ch);
            }
            out.push_str("\x1b[0m");
            println!("{out}\r");
        }
    }
}
