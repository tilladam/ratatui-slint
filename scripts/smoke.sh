#!/bin/sh
# Runs every example under the fake terminal (scripts/fake_terminal.py) for each protocol it can
# simulate, with scripted input and expected screen text. A stand-in for the real-terminal matrix:
# it proves startup, input, rendering per protocol, quit and terminal restore, but not how a real
# terminal decodes or displays the images.
set -eu
cd "$(dirname "$0")/.."
cargo build --quiet --examples
cargo build --quiet -p live-preview
# Compile outside the pty timeout: Cargo may wait for another build's target lock.
SESSION_TEST=$(cargo test --quiet -p ratatui-slint --test session_lifecycle --no-run --message-format=json | python3 -c 'import sys,json; print(next(m["executable"] for m in map(json.loads,sys.stdin) if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "session_lifecycle"))')
FT="python3 scripts/fake_terminal.py --timeout 30 --require graphics --require restore --require input-restore"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0
run() {
    name=$1; shift
    if out=$("$@" 2>&1); then
        echo "ok    $name"
    else
        echo "FAIL  $name"; echo "$out" | sed 's/^/      /'; fail=1
    fi
}
for p in none sixel kitty iterm2; do
    run "termbench/$p" $FT --protocol $p -- ./target/debug/examples/termbench 2
    run "overlay/$p" $FT --protocol $p --script "2:key o;3:key o;4:key q" -- ./target/debug/examples/overlay
    run "hello/$p" $FT --protocol $p --script "2.5:click 5 2;3:key +;3.5:key \x07;4:key q" -- ./target/debug/examples/hello
    run "sysmon/$p" $FT --protocol $p --script "3:key p;5:mark a;7:mark b;7.5:key q" --expect-idle a:b --expect "Processes" -- ./target/debug/examples/sysmon
    run "device_console/$p" $FT --protocol $p --script "3:click 29 19;4.5:key q" --expect "console: connected to hvac-unit-3" -- ./target/debug/examples/device_console
    cp examples/live-preview/demo.slint "$TMP/ui.slint"
    run "live-preview/$p" $FT --protocol $p --script "3:run sed -i.bak 's/Live preview/Reloaded/' $TMP/ui.slint;5:key q" --expect "reload #1" -- ./target/debug/live-preview "$TMP/ui.slint"
done
run "panic restores terminal" $FT --expect-exit 101 --env RATATUI_SLINT_PANIC_AFTER_MS=1500 -- ./target/debug/examples/hello
run "enhanced keys and focus" $FT --protocol kitty --keyboard-enhanced --require keyboard-restore --env RATATUI_SLINT_EXPECT_LEVEL=0.4 --script "2:press 5 2;2.2:focus lost;2.4:focus gained;2.6:click 5 2;3:enhanced 43;3.2:enhanced 43 1 3;3.5:enhanced 103 5;4:enhanced 113" -- ./target/debug/examples/hello
run "session ownership" python3 scripts/fake_terminal.py --timeout 30 --require restore --require input-restore -- "$SESSION_TEST" --ignored --nocapture
exit $fail
