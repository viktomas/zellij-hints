# zellij-hints

A [Zellij](https://zellij.dev) plugin in the spirit of the kitty `hints`
kitten and [tmux-thumbs](https://github.com/fcsonline/tmux-thumbs): overlay
the focused pane with short letter labels on every URL and file path;
typing a label acts on that match without touching the pane itself.

- **lowercase label** → URL: open in browser · path: copy to clipboard
- **UPPERCASE label** → URL: copy to clipboard · path: copy **and paste**
  into the pane (`WriteCharsToPaneId`, so it works even though the plugin
  holds focus)
- **Esc / Ctrl-c** cancel · **Backspace** undoes a partially typed label
- ≤26 matches → 1 key; more → 2 keys (vim-easymotion style); labels are
  prefix-free so typing is unambiguous, shortest labels nearest the cursor
- Non-match content is dimmed, labels colored per type; URLs and paths are
  hinted simultaneously (blue vs green by default)
- The target pane is **never modified** — the plugin is a borderless
  floating pane resized to exactly cover the target, which keeps running
  underneath

## Demo

![zellij-hints demo](demo.gif)

*`Ctrl y`, `u` labels every URL and path (blue/green); an UPPERCASE label copies and pastes a path into the prompt, `Esc` cancels, a lowercase label copies.*

## Build

Requires Rust with the `wasm32-wasip1` target and Zellij **0.45.x**:

```sh
rustup target add wasm32-wasip1
cargo build --target wasm32-wasip1 --release
# → target/wasm32-wasip1/release/zellij-hints.wasm
```

Unit tests (pattern matching, label generation, wrapping — no wasm needed):

```sh
cargo test
```

## Bind it

In `~/.config/zellij/config.kdl`, e.g. under the `pane` mode so `Ctrl y` → `u`
triggers it (path below assumes the repo was cloned to
`~/workspace/private/zellij-hints`):

```kdl
keybinds {
    pane {
        bind "u" {
            LaunchOrFocusPlugin "file:/Users/tomas/workspace/private/zellij-hints/target/wasm32-wasip1/release/zellij-hints.wasm" {
                // alphabet "asdfghjkl"        // default: a-z
                // url_fg "#00afff"            // label color for URLs
                // path_fg "#50fa7b"           // label color for paths
                // dim true                    // dim non-match content
                // paste_on_uppercase true     // uppercase path = copy+paste
                // open_command "open"         // macOS; xdg-open on Linux
                // patterns "url,path"         // or "all"
                // unique true                 // identical texts share a label
            }
        }
    }
}
```

The first launch asks for plugin permissions (one prompt):
`ReadApplicationState`, `ChangeApplicationState`, `InterceptInput`,
`ReadPaneContents`, `WriteToClipboard`, `WriteToStdin`, `RunCommands`,
`RunActionsAsUser` — see `DESIGN.md` for what each is for.

## Try it

```sh
./test/sample-output.sh   # prints URLs, paths, compiler errors, diffs
# then: Ctrl y, u, type a label
```

## How it works

See `DESIGN.md`. Short version: the plugin finds the focused tiled terminal
pane from `PaneUpdate`, resizes its own floating pane to cover it exactly,
reads the viewport with `get_pane_scrollback` (instant, monochrome) and
upgrades to color via a `DumpScreen --ansi` file round-trip (the only
ANSI-content path available to plugins in 0.45). Matching/labeling is a pure
Rust core (`src/lib.rs`, no `zellij-tile` dep) that is unit-tested natively.

Architecture blueprint: [zellij-copy-mode](https://github.com/zackradisic/zellij-copy-mode)
(that repo is unlicensed — its ideas are reused, its code is not).
Pattern shapes adapted from tmux-thumbs (MIT).

## Limitations (v1)

- **Scope**: URLs and paths only (paths must contain a `/`; bare `main.rs`
  is not hinted — trivial to add later). No git SHAs / UUIDs / numbers yet.
- **Snapshot**: content is captured when the overlay opens; if the target
  redraws underneath, labels go stale. Re-trigger to refresh.
- **Floating panes** are not hinted (falls back to the focused tiled pane).
- **Alternate-screen apps** (vim, htop): works on the visible snapshot, but
  apps that paint with cursor moves may misalign the reconstruction.
- Very many matches (> alphabet², 676 by default) leave extras unlabeled.
- With pane frames on, the frame is covered while the overlay is up.

## Follow-ups

- More pattern types (git SHA, UUID, IP, docker, hex colors, numbers) — the
  priority/overlap machinery already supports them (`src/patterns.rs`).
- Bare filenames, custom regexes from KDL config (first capture group).
- Multi-select (`Tab` toggle, `Enter` joins with spaces).
- "Open in `$EDITOR` at line" — `open_file(FileToOpen { line_number })`
  already supports it (paths with `:line` are matched today).
- Per-binding pattern filters, kitty `--type` style: bind the same wasm
  under several plugin aliases with different `patterns` config.
- `Tab` completion of ambiguous label prefixes.

## License

MIT — see [LICENSE](LICENSE).
