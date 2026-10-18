# Changelog

## 0.1.0 — unreleased

Initial release: embed Slint components in ratatui with software-rendered Kitty, sixel, iTerm2,
and half-block output, bounded background encoding, focus/input routing, and terminal sessions.

Pre-release hardening fixes last-pane shutdown, session ownership and reader teardown, focus-loss
capture cancellation, pointer modifiers, popup transfer, raw-image invalidation, and Kitty upload
loss under overlays. Framebuffer access has one stable borrowed view across Cargo features;
alpha output requires explicit per-pane opt-in. Input/thread errors are surfaced and error causes
preserved. Stable Kitty IDs and unconditional broad image decoders have been removed.

Linux/macOS are the release targets. The current real-terminal validation matrix is pending;
Windows, tmux and SSH remain unverified. See docs/INTEGRATION.md and docs/VALIDATION.md.
