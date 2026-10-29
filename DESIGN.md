# zellij-hints — Design (Phase 1)

A Zellij plugin (kitty-hints / tmux-thumbs style): overlay the focused pane with
short letter labels on pattern matches. Typing a label acts on the match; the
target pane is never modified.

Target: **Zellij 0.45.1**, `zellij-tile = "0.45.1"`, `wasm32-wasip1`.
All API claims below were verified against the 0.45.1 sources of `zellij-tile`,
`zellij-utils`, and `zellij-server` (not assumed from docs).

## v1 scope (decided 2026-10-29)

- **Patterns**: URLs + paths only (incl. `path:line[:col]`, `~/`, `./`,
  `../`, diff `a/`,`b/` headers). Other pattern types (SHA, UUID, IP,
  docker, hex colors, numbers) and custom regexes are follow-ups — the
  priority/overlap machinery in `src/patterns.rs` already supports adding
  them.
- **Both kinds hinted simultaneously**, distinguishable by label color
  (URLs blue, paths green).
- **Labels**: full a–z alphabet; 1 key ≤26 matches, 2 keys beyond
  (vim-easymotion style), prefix-free.
- **Actions**: lowercase → URL: open in browser, path: copy; uppercase →
  URL: copy, path: copy + paste into the pane.
- Trigger: pane-mode `u` (leader `Ctrl y`). MIT license, published at
  https://github.com/viktomas/zellij-hints.

## 1. Fork zellij-copy-mode or start fresh? → **Start fresh**

- **zellij-copy-mode has no license** (no LICENSE file; GitHub reports
  `license: None`). We cannot legally copy its code. Its *architecture* is our
  blueprint, but the code will be written from scratch.
- ~85% of copy-mode is vi-motions/selection/search that hints doesn't need. The
  part we DO need (overlay positioning, scrollback+ANSI-dump ingestion, vte
  grid, permissions) is ~300 lines and fully understood.
- tmux-thumbs is MIT — its pattern list (src/state.rs, 15 patterns) and
  alphabet/hint-generation scheme can be adapted with attribution.

## 2. Getting viewport text + exact geometry

Proven blueprint (copy-mode), reimplemented:

1. **Launch**: keybind opens us as a floating pane via
   `LaunchOrFocusPlugin "file:…/zellij-hints.wasm"` — floating x/y/width/height
   in the keybind are ignored; the plugin sizes itself at runtime.
2. **Find the target**: from `Event::PaneUpdate`, pick the pane that is
   `is_focused && !is_plugin && !is_suppressed && !is_floating` **in our own
   tab** (tab-scoping disambiguates multi-tab sessions where several panes
   report `is_focused`). Capture from `PaneInfo`:
   - outer geometry: `pane_x, pane_y, pane_columns, pane_rows` (what we cover)
   - content geometry: `pane_content_x, pane_content_y, pane_content_columns,
     pane_content_rows` (where text starts — differs when pane frames are on)
   - `cursor_coordinates_in_pane: Option<(x, y)>` for label ordering
3. **Cover the target**: `change_floating_panes_coordinates` with
   `FloatingPaneCoordinates::new(x, y, cols, rows, borderless=true)` so our
   floating pane exactly overlays the target. The target keeps running live
   underneath — nothing to restore on exit, no ghost panes, no layout shift.
4. **Plain text (immediate)**: `get_pane_scrollback(target, false)` →
   `PaneContents { viewport: Vec<String>, cursor: Option<(usize, usize)> }`.
   One entry per **visual grid row**. Available synchronously, monochrome.
5. **Colors (async upgrade)**: `run_action(Action::DumpScreen { file_path:
   <plugin initial_cwd>/.zellij-hints-dump.ansi, include_scrollback: false,
   ansi: true, pane_id: Some(target) })`, then poll the file through the `/host`
   sandbox mount on `Event::Timer` (50 ms × ≤40 attempts; `Event::ActionComplete`
   as backup trigger). On success, re-parse with the `vte` crate into a grid of
   styled cells (fg/bg 256+truecolor, bold/italic/underline/reverse). On
   timeout: keep the monochrome grid (graceful degradation).
   - No plugin API returns ANSI content directly in 0.45.1 (`get_pane_scrollback`
     is text-only) — the file round-trip is the only color path. **Limitation,
     not a hack**: it's how copy-mode does it and the dump is an official action.

### Wrapped lines — key discovery

`DumpScreen --ansi` emits `\n` only at **canonical** line boundaries; rows
produced by line wrapping are concatenated without a newline (verified in
`grid.rs` `dump_screen_with_ansi!` macro). So:

- We match on **logical lines** → a path wrapped across visual rows is still
  matched (important for compiler output in narrow panes).
- To get on-screen positions we **re-wrap** each logical line at
  `pane_content_columns` using `unicode-width` (wide char = 2 cols, combining =
  0), mirroring Zellij's grid wrapping. This reconstructs the visual rows and
  gives (row, col) for every match character.

### Wide (CJK/emoji) characters

- Column math uses `unicode-width` (never `chars().count()`).
- A label overwriting the first cell(s) of a 2-cell-wide char can render
  artifacts; matches starting on a wide char get their label drawn on the
  character's **two** cells when the label is 1 char (avoid splitting a wide
  char in half).
- `get_pane_scrollback`'s viewport rows serve as a sanity cross-check for the
  re-wrap during testing.

## 3. Drawing labels without shifting layout

We own the overlay pane's full render (`println!` with SGR), so nothing can
shift — we repaint the same cells with different styles.

- **Overwrite the first characters of the match** with the label (tmux-thumbs /
  kitty behavior), rendered in a configurable fg color (default: bright green)
  on the original bg. This is the default: it never depends on free space
  beside the match.
- If the match's display width < the label's width (e.g. a 4-digit number with
  a 2-char label), draw the label **immediately right of the match** when the
  next cell is a space; otherwise overwrite anyway (kitty does the same).
- Non-match content: original colors + SGR 2 (dim/faint). Match text: original
  colors, undimmed. Status line at the bottom row: typed label prefix,
  multi-select state, match count.

## 4. Permissions (names verified in `plugin_permission.proto` + server mapping)

Requested once in `load()` via `request_permission(&[…])`; Zellij shows a single
approval prompt. Needed:

| Permission | Why |
|---|---|
| `ReadApplicationState` | `PaneUpdate` info: target id/geometry/cursor |
| `ChangeApplicationState` | `change_floating_panes_coordinates` (cover target) |
| `InterceptInput` | `intercept_key_presses` / `clear_key_presses_intercepts` (0.45 maps these here — copy-mode on 0.44 didn't need it listed; 0.45 does) |
| `ReadPaneContents` | `get_pane_scrollback` |
| `WriteToClipboard` | `copy_to_clipboard` |
| `RunActionsAsUser` | `run_action` — `DumpScreen` (colors) and `WriteCharsToPaneId` (paste) |

Reading the dump file back via `/host` (= plugin cwd) needs no extra permission
(sandbox-allowed, same as copy-mode). `close_self` needs no permission.

## 5. Paste (uppercase label)

`run_action(Action::WriteCharsToPaneId { chars, pane_id: target })` — exists in
0.45.1 (`data.rs`), writes bytes directly to the target pane's PTY regardless of
which pane holds focus, and triggers `ClearScroll` for the client (target
viewport snaps to bottom — desirable before pasting). Sequence: write chars →
`copy_to_clipboard` → `close_self()`.

## 6. Patterns — priority order, overlaps resolved first-claim-wins

v1 implements the first three rows (URL, diff paths, path:line). The rest
are follow-ups — the overlap machinery is already generic over a priority-
ordered pattern list.

Adapted from tmux-thumbs (MIT) + task spec, highest priority first:

1. **url**: `(https?://|git@|git://|ssh://|ftp://|file://)[^ ]+`
2. **path:line[:col]**: path (rule below) followed by `:\d+(:\d+)?`
3. **path**: absolute (`/…`), relative (contains `/`, no leading `//`), or
   `~/…`; segments of word chars + `.-_@~[]`; rejects trailing punctuation
4. **diff paths**: `diff --git a/X b/Y`, `--- a/X`, `+++ b/Y` (capture the `b/`
   side)
5. **git sha**: `[0-9a-f]{7,40}` with boundary guards (not part of a longer hex
   string — else the hex-color/uuid rules win where they overlap)
6. **uuid**: 8-4-4-4-12 hex
7. **ipv4** / **ipv6**
8. **docker**: `sha256:<64 hex>` and `name:tag` forms
9. **hex color**: `#[0-9a-fA-F]{6}\b`
10. **number**: `[0-9]{4,}`

Post-processing (kitty-style): strip trailing punctuation `.,:;)]}"'` from
match text (`path.rs:12:` → `path.rs:12`; `(https://x.y)` → `https://x.y`).

**Custom regexes** from KDL config, first capture group if present.
Default priority: **before** built-ins (explicit user intent) — configurable.

## 7. Labels

- Alphabet default `asdfghjkl` (homerow), configurable.
- Generation (tmux-thumbs scheme, prefix-free): fill 1-char labels from the
  alphabet; when more are needed, expand to 2-char labels whose **first char is
  an alphabet letter not used as a 1-char label** (so no label is a prefix of
  another); then 3-char the same way. With 9 letters: 9 one-char, 72 two-char,
  504 three-char — plenty; beyond that, extra matches are ignored (status note).
- Shortest labels → matches nearest the cursor (fallback: bottom of screen);
  scan outward from the cursor position.
- Identical match text shares one label (dedupe by text, first occurrence
  labeled).

## 8. Keys

- lowercase label → URL: open in browser · path: copy match, close
- UPPERCASE label → URL: copy · path: copy + paste into target pane
  (`WriteCharsToPaneId`), close (config toggle `paste_on_uppercase`)
- `Esc` / `Ctrl-c` → cancel
- `Backspace` → undo typed label prefix
- multi-select (`Tab`, `Enter` join) — follow-up

## 9. Config (KDL)

Plugin config arrives in `load()` as `BTreeMap<String, String>` — named args
and child nodes with a single scalar value (verified in
`kdl_layout_parser.rs::parse_plugin_user_configuration`; repeated child node
names collide, so lists must be single strings):

```kdl
// in config.kdl keybinds:
pane {
    bind "u" {
        LaunchOrFocusPlugin "file:/Users/tomas/workspace/private/zellij-hints/target/wasm32-wasip1/release/zellij-hints.wasm" {
            alphabet "asdfghjkl"
            label_fg "#00ff00"
            dim true
            patterns "url,path,sha"          // comma-separated enabled types; "all" default
            custom_regexes "TODO-\\d+,(?P<issue>\\S+-\\d+)"  // comma-separated, first capture group wins
            paste_on_uppercase true
            scrollback false                  // visible viewport only (default)
        }
    }
}
```

## 10. Architecture & testing

```
zellij-hints/
├── Cargo.toml            # zellij-tile 0.45.1, vte, unicode-width, regex (all cross-compile to wasip1)
├── src/
│   ├── lib.rs            # pure core — NO zellij-tile dep
│   ├── patterns.rs       # regex set, overlap resolution, trailing-punct trim
│   ├── labels.rs         # prefix-free label generation + cursor-proximity ordering
│   ├── geometry.rs       # logical→visual re-wrap, wide-char-aware columns
│   ├── grid.rs           # vte ANSI parser → styled cell grid
│   └── main.rs           # thin plugin shell: lifecycle, overlay, keys, clipboard
└── test/sample-output.sh # prints paths, URLs, git log --oneline, compiler errors for manual testing
```

Pure-core modules unit-tested natively (`cargo test` on host, no wasm):
overlapping matches, wrapped paths, trailing punctuation, prefix-freeness,
identical-text dedupe, wide-char columns.

## 11. Known limitations

- **Alternate-screen apps**: works (viewport = alt screen content), but content
  is a snapshot; if the app redraws while the overlay is open, labels go stale
  (same as tmux-thumbs). No snapshot refresh planned for v1.
- **Floating target panes**: v1 targets the focused **tiled** pane
  (copy-mode's filter). If only a floating pane is focused we fall back to the
  last focused tiled pane in the tab; covering floating panes may z-fight with
  our own floating layer — needs a manual test, likely v2.
- **Very many matches**: >~500 matches (with default alphabet) are unlabeled.
- **Colored dump race**: poll ≤2 s; then monochrome fallback.
- **Re-wrap fidelity**: our width-based re-wrap mirrors Zellij's wrapping for
  normal text; exotic cases (explicit cursor moves, partial clears mid-line)
  could misalign columns by a cell. Cross-checked against
  `get_pane_scrollback` rows in tests.
- **Multi-client sessions**: `is_focused` is per-client; we take the first
  focused pane in our tab.

## 12. Follow-ups (out of scope for v1)

- **Open in `$EDITOR` at line**: natively supported — `open_file(FileToOpen {
  path, line_number })` (OpenFiles permission). Likely v1.1: `e`-prefixed label
  or a config action map.
- **Per-binding pattern filter** (kitty `--type`): trivial once config exists —
  define multiple plugin aliases (`hints-url`, `hints-path`) with different
  `patterns` values, bind different keys.
- **Native regex highlights**: 0.45.1 added `set_pane_regex_highlights`
  (ChangeApplicationState) — highlights matches *in the live pane* with
  hover/click context, no overlay. Can't draw labels (styles only, no text
  replacement), but an interesting mouse-click UX complement.
- **Multi-select paste** (write all selected into pane, not just clipboard).
