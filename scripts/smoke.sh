#!/bin/sh
# Runs every example under the fake terminal (scripts/fake_terminal.py) for each protocol it can
# simulate, with scripted input and expected screen text. A stand-in for the real-terminal matrix:
# it proves startup, input, rendering per protocol, quit and terminal restore, but not how a real
# terminal decodes or displays the images.
set -eu
cd "$(dirname "$0")/.."
cargo build --quiet --examples
cargo build --quiet -p live-preview
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
for p in none sixel kitty; do
    run "hello/$p" $FT --protocol $p --script "2.5:click 5 2;3:key +;3.5:key \x07;4:key q" -- ./target/debug/examples/hello
    run "sysmon/$p" $FT --protocol $p --script "3:key p;5:mark a;7:mark b;7.5:key q" --expect-idle a:b --expect "Processes" -- ./target/debug/examples/sysmon
    run "device_console/$p" $FT --protocol $p --script "3:click 29 19;4.5:key q" --expect "console: connected to hvac-unit-3" -- ./target/debug/examples/device_console
    cp examples/live-preview/demo.slint "$TMP/ui.slint"
    run "live-preview/$p" $FT --protocol $p --script "3:run sed -i.bak 's/Live preview/Reloaded/' $TMP/ui.slint;5:key q" --expect "reload #1" -- ./target/debug/live-preview "$TMP/ui.slint"
done
run "panic restores terminal" $FT --expect-exit 101 --env RATATUI_SLINT_PANIC_AFTER_MS=1500 -- ./target/debug/examples/hello
exit $fail
