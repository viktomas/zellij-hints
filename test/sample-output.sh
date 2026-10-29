#!/usr/bin/env bash
# Sample output for manually testing zellij-hints.
# Run this in a zellij pane, then trigger the plugin (default: Ctrl y, u)
# and check:
#   - URLs and paths get labels (blue vs green)
#   - labels are 1 key; run the "many matches" section to see 2-key labels
#   - lowercase path label copies it (paste somewhere to verify)
#   - UPPERCASE path label copies AND pastes into the pane
#   - lowercase URL label opens the browser
#   - Esc cancels, Backspace undoes a partial label

cat <<'EOF'
== plain URLs and paths ==
Docs: https://zellij.org/docs and https://docs.rs/zellij-tile/0.45.1
See (https://sw.kovidgoyal.net/kitty/kittens/hints/) for inspiration.
error: mismatched types found in src/main.rs:42:17
warning: unused variable in ./src/lib.rs:8:9
edit ~/notes/todo.md for the follow-ups
config at /usr/local/etc/zellij/config.kdl
relative import ../shared/util.rs:12

== diff output ==
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
run git clone git@github.com:fcsonline/tmux-thumbs.git for reference

== trailing punctuation (should be trimmed) ==
broken link: https://example.com/a.
error at src/grammar.rs:12:34:
quoted "~/notes/week.md"

== wrapped lines (narrow your pane to ~40 cols) ==
error[E0308]: mismatched types found in src/very/deeply/nested/directory/structure/mod.rs:128:4 expected `usize`, found `String`

== no matches (footer should say so) ==
just some plain words without anything hintable here

EOF

# >26 matches: labels should become two keys (za, zb, ...)
for i in $(seq -w 1 30); do
    echo "file_$i: src/case_$i.rs:1$i"
done

echo
echo "== done — trigger hints now (Ctrl y, u) =="
