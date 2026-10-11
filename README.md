# ratatui-slint

Embed [Slint](https://slint.dev) components as pixel widgets inside a
[ratatui](https://ratatui.rs) application. Slint renders into a software
framebuffer; the pane is shown through [ratatui-image](https://github.com/ratatui/ratatui-image)
(kitty, sixel, iTerm2 or half-blocks) and driven from the ratatui event loop.
Mouse and keyboard events are routed to the pane under focus, and keys Slint
rejects fall back to ratatui.

Status: proof of concept. See [PLAN.md](PLAN.md) for the architecture, the
verified API surface, measurements and the phased plan.

## Spike

```sh
cd spike
cargo run              # in kitty, Ghostty or WezTerm
cargo run -- headless  # no terminal needed
```

## Licence

This crate is MIT licensed, see [LICENSE](LICENSE). Slint itself is available
under GPL-3.0, a royalty-free licence or a commercial licence; users of this
crate pick the Slint licence that fits their product.
