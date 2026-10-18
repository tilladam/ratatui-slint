#!/usr/bin/env python3
"""Run a TUI example inside a pseudo-terminal that answers terminal queries.

Answers the queries ratatui-image and crossterm send at startup (device attributes, cell size,
cursor position, kitty graphics), so an example can be smoke-tested without a real terminal.
Sends scripted input, records everything the program writes, and reports checks.

    scripts/fake_terminal.py [--protocol none|sixel|kitty|iterm2] [--cols 120] [--rows 40]
                             [--script "2:click 70 10;3:key +;5:key q"] [--timeout 20]
                             [--log out.bin] -- ./target/debug/examples/sysmon

Script steps are "<seconds>:<action>", separated by ';'. Actions:
  key <text>        send text (escapes like \\x1b allowed)
  click <col> <row> SGR mouse press and release, 1-based terminal cells
  enhanced <code> [modifiers] [kind]  CSI-u key event (defaults: 1, 1)
  focus lost|gained                  terminal focus report
  press|release <col> <row>          individual mouse events
  paste <text>      bracketed paste
  mark <name>       record the output byte count under <name> (for idle measurements)
  run <command>     run a shell command (e.g. edit a watched file)
"""

import argparse
import fcntl
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time


def enhanced_key(code, modifiers=1, kind=1):
    """CSI-u key event: modifier mask plus one; kind 1=press, 2=repeat, 3=release."""
    return f"\x1b[{code};{modifiers}:{kind}u".encode()


def parse_script(text):
    steps = []
    for part in filter(None, (p.strip() for p in text.split(";"))):
        t, action = part.split(":", 1)
        steps.append((float(t), action.strip()))
    return sorted(steps)


def last_screen(d, rows, cols):
    """Replays the output on a character grid and returns the screen as it was just before the
    program left the alternate screen. Handles cursor positioning, CR/LF and erase; ignores
    colours and images; combining marks take no cell."""
    import unicodedata
    end = d.rfind(b"\x1b[?1049l")
    d = d[:end] if end >= 0 else d
    d = re.sub(rb"\x1b[_P].*?\x1b\\", b"", d, flags=re.S)
    d = re.sub(rb"\x1b\][^\x07]*\x07", b"", d, flags=re.S)
    grid = [[" "] * cols for _ in range(rows)]
    r = c = 0
    text = d.decode("utf-8", "replace")
    i = 0
    csi = re.compile(r"\x1b\[([0-9;?<>=]*)([ -/]*)([@-~])")
    while i < len(text):
        ch = text[i]
        if ch == "\x1b":
            m = csi.match(text, i)
            if m:
                params, _, final = m.groups()
                if final == "H":
                    parts = [p for p in params.split(";")]
                    r = (int(parts[0]) if parts and parts[0] else 1) - 1
                    c = (int(parts[1]) if len(parts) > 1 and parts[1] else 1) - 1
                elif final == "J" and params in ("2", "3"):
                    grid = [[" "] * cols for _ in range(rows)]
                i = m.end()
            else:
                i += 2
            continue
        if ch == "\r":
            c = 0
        elif ch == "\n":
            r += 1
        elif unicodedata.combining(ch) or ord(ch) < 32:
            pass
        else:
            if 0 <= r < rows and 0 <= c < cols:
                grid[r][c] = ch
            c += 1
        i += 1
    return "\n".join("".join(row) for row in grid)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--protocol", default="none", choices=["none", "sixel", "kitty", "iterm2"])
    ap.add_argument("--keyboard-enhanced", action="store_true", help="answer kitty keyboard negotiation")
    ap.add_argument("--cols", type=int, default=120)
    ap.add_argument("--rows", type=int, default=40)
    ap.add_argument("--cell", default="10x20", help="cell size in pixels, WxH")
    ap.add_argument("--script", default="")
    ap.add_argument("--timeout", type=float, default=20)
    ap.add_argument("--log")
    ap.add_argument("--screen", action="store_true", help="print the last screen")
    ap.add_argument("--env", action="append", default=[], help="NAME=VALUE for the child")
    ap.add_argument("--expect", action="append", default=[], help="text that must appear on screen")
    ap.add_argument("--expect-exit", type=int, default=0, help="expected child exit code")
    ap.add_argument("--require", action="append", default=[], choices=["graphics", "restore", "input-restore", "keyboard-restore"])
    ap.add_argument("--expect-idle", action="append", default=[], help="START:END marks between which zero bytes may be written")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    cmd = args.cmd[1:] if args.cmd and args.cmd[0] == "--" else args.cmd
    cw, ch = (int(v) for v in args.cell.split("x"))

    pid, fd = pty.fork()
    if pid == 0:
        for kv in args.env:
            k, v = kv.split("=", 1)
            os.environ[k] = v
        if args.protocol == "iterm2":
            os.environ["RATATUI_SLINT_PROTOCOL"] = "iterm2"
        os.environ["TERM"] = "xterm-256color"
        os.execvp(cmd[0], cmd)

    winsize = struct.pack("HHHH", args.rows, args.cols, args.cols * cw, args.rows * ch)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, winsize)

    out = bytearray()
    scanned = 0
    marks = {}
    replies = []
    steps = parse_script(args.script)
    start = time.monotonic()
    status = None

    def send(data):
        os.write(fd, data)

    while True:
        elapsed = time.monotonic() - start
        while steps and steps[0][0] <= elapsed:
            _, action = steps.pop(0)
            verb, _, rest = action.partition(" ")
            if verb == "key":
                send(rest.encode().decode("unicode_escape").encode())
            elif verb == "enhanced":
                parts = rest.split()
                code = int(parts[0])
                modifiers = int(parts[1]) if len(parts) > 1 else 1
                kind = int(parts[2]) if len(parts) > 2 else 1
                send(enhanced_key(code, modifiers, kind))
            elif verb == "focus":
                send(b"\x1b[I" if rest == "gained" else b"\x1b[O")
            elif verb in ("press", "release"):
                c, r = rest.split()
                send(f"\x1b[<0;{c};{r}{'M' if verb == 'press' else 'm'}".encode())
            elif verb == "click":
                c, r = rest.split()
                send(f"\x1b[<0;{c};{r}M\x1b[<0;{c};{r}m".encode())
            elif verb == "paste":
                send(b"\x1b[200~" + rest.encode() + b"\x1b[201~")
            elif verb == "mark":
                marks[rest] = len(out)
            elif verb == "run":
                import subprocess
                subprocess.run(rest, shell=True, check=False)
        if elapsed > args.timeout:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
            status = "timeout"
            break
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                data = b""
            if not data:
                _, st = os.waitpid(pid, 0)
                status = os.waitstatus_to_exitcode(st)
                break
            out += data
            # Answer queries found in the new output.
            chunk = bytes(out[max(0, scanned - 16):])
            scanned = len(out)
            for m in re.finditer(rb"\x1b_G([^\x1b]*)\x1b\\", chunk):
                if b"a=q" in m.group(1) and args.protocol == "kitty":
                    idm = re.search(rb"i=(\d+)", m.group(1))
                    send(b"\x1b_Gi=" + (idm.group(1) if idm else b"31") + b";OK\x1b\\")
                    replies.append("kitty-ok")
            if b"\x1b[16t" in chunk:
                send(f"\x1b[6;{ch};{cw}t".encode())
                replies.append("cell-size")
            if b"\x1b[14t" in chunk:
                send(f"\x1b[4;{args.rows * ch};{args.cols * cw}t".encode())
            if b"\x1b[?u" in chunk:
                if args.keyboard_enhanced:
                    send(b"\x1b[?0u")
                    replies.append("keyboard")
            if b"\x1b[c" in chunk:
                send(b"\x1b[?62;4;22c" if args.protocol == "sixel" else b"\x1b[?62;22c")
                replies.append("da1")
            if b"\x1b[5n" in chunk:
                send(b"\x1b[0n")
            for _ in range(chunk.count(b"\x1b[6n")):
                send(b"\x1b[1;1R")
                replies.append("cursor")
        try:
            done, st = os.waitpid(pid, os.WNOHANG)
            if done:
                status = os.waitstatus_to_exitcode(st)
                # Drain what is left.
                while True:
                    r, _, _ = select.select([fd], [], [], 0.05)
                    if not r:
                        break
                    try:
                        data = os.read(fd, 65536)
                    except OSError:
                        break
                    if not data:
                        break
                    out += data
                break
        except ChildProcessError:
            break

    if args.log:
        with open(args.log, "wb") as f:
            f.write(out)
    d = bytes(out)
    checks = {
        "exit status": status,
        "seconds": round(time.monotonic() - start, 1),
        "bytes written": len(d),
        "query replies": ",".join(sorted(set(replies))),
        "entered alt screen": b"\x1b[?1049h" in d,
        "left alt screen": b"\x1b[?1049l" in d,
        "mouse capture off": b"\x1b[?1000l" in d,
        "bracketed paste off": b"\x1b[?2004l" in d,
        "focus reporting off": b"\x1b[?1004l" in d,
        "half-block glyphs": "▀".encode() in d,
        "sixel data": b"\x1bP" in d and b"q\"" in d,
        "iterm2 data": b"\x1b]1337;File=inline=1;" in d,
        "keyboard pushed": bool(re.search(rb"\x1b\[>[0-9]+u", d)),
        "keyboard popped": b"\x1b[<1u" in d,
        "kitty transmit": b"\x1b_Ga=T" in d or b",a=T," in d,
    }
    screen = last_screen(d, args.rows, args.cols)
    failed = validate_checks(checks, screen, marks, args)
    names = sorted(marks, key=marks.get)
    for a, b in zip(names, names[1:]):
        checks[f"bytes {a}->{b}"] = marks[b] - marks[a]
        if 0 < marks[b] - marks[a] <= 2000:
            checks[f"content {a}->{b}"] = repr(d[marks[a]:marks[b]])
    for k, v in checks.items():
        print(f"{k:24} {v}")
    if args.screen:
        print(screen)
    if failed:
        print("failed: " + ", ".join(failed))
    sys.exit(1 if failed else 0)


def validate_checks(checks, screen, marks, args):
    failed = []
    if checks["exit status"] != args.expect_exit:
        failed.append("exit status")
    screen_flat = " ".join(screen.split())
    for text in args.expect:
        key = f"expect {text!r}"
        checks[key] = " ".join(text.split()) in screen_flat
        if not checks[key]:
            failed.append(key)
    required = {
        "graphics": [{"none": "half-block glyphs", "sixel": "sixel data", "kitty": "kitty transmit", "iterm2": "iterm2 data"}[args.protocol]],
        "restore": ["entered alt screen", "left alt screen"],
        "input-restore": ["mouse capture off", "bracketed paste off", "focus reporting off"],
        "keyboard-restore": ["keyboard pushed", "keyboard popped"],
    }
    for name in args.require:
        failed.extend(key for key in required[name] if not checks[key])
    for interval in args.expect_idle:
        start, end = interval.split(":", 1)
        key = f"idle {start}->{end}"
        checks[key] = start in marks and end in marks and marks[start] == marks[end]
        if not checks[key]:
            failed.append(key)
    return failed


if __name__ == "__main__":
    main()
