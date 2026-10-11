//! Terminal setup and panic-safe restore.
//!
//! Order matters: the graphics query and the keyboard-enhancement query both read stdin, so they
//! run before [`crate::Runtime::spawn_terminal_reader`] starts consuming input.

use std::io::{self, Stdout, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::output::{ByteCounter, CountingWriter};

/// The terminal type a [`TerminalSession`] drives. Its writer counts every byte sent.
pub type SessionTerminal = Terminal<CrosstermBackend<CountingWriter<Stdout>>>;

/// What to enable when the session starts.
#[derive(Clone, Debug)]
pub struct TerminalOptions {
    pub mouse: bool,
    pub bracketed_paste: bool,
    pub focus_change: bool,
    /// Ask for the kitty keyboard protocol, so key releases and repeats are reported.
    pub keyboard_enhancement: bool,
    /// Probe for kitty zlib compression, and use it when the terminal accepts it. On by default:
    /// UI frames are mostly flat colour and compress about 85-fold for 2–3 ms of encode time at
    /// 2688 × 1836 px, which took Ghostty from 3 to 33 fps. Photographic content compresses far
    /// worse and costs more CPU.
    /// `RATATUI_SLINT_KITTY_COMPRESSION=1` (or `0`) overrides this.
    pub kitty_compression: bool,
    /// Force a protocol instead of detecting one. The `RATATUI_SLINT_PROTOCOL` environment
    /// variable (`halfblocks`, `sixel`, `kitty`, `iterm2`) overrides this.
    pub protocol: Option<ProtocolType>,
    pub query_timeout: Duration,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            mouse: true,
            bracketed_paste: true,
            focus_change: true,
            keyboard_enhancement: true,
            kitty_compression: true,
            protocol: None,
            query_timeout: Duration::from_millis(1000),
        }
    }
}

/// An initialised terminal plus what was learned about it. Restores the terminal on drop and on
/// panic.
pub struct TerminalSession {
    pub terminal: SessionTerminal,
    pub picker: Picker,
    /// Whether the kitty keyboard protocol was enabled. Pass to
    /// [`crate::InputRouter::set_keyboard_enhanced`].
    pub keyboard_enhanced: bool,
    /// Total bytes written to the terminal.
    pub bytes: ByteCounter,
}

const MOUSE: u8 = 1;
const PASTE: u8 = 2;
const FOCUS: u8 = 4;
const KEYBOARD: u8 = 8;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicU8 = AtomicU8::new(0);
static HOOK: Once = Once::new();

/// The terminal's current cell size in pixels, from the window-size ioctl. Unlike the startup
/// query this does not read stdin, so it is safe while the input reader thread runs. Returns
/// `None` when the terminal does not report pixel sizes (some multiplexers report zero).
///
/// Call it on `Event::Resize` and pass the result to [`crate::SlintPane::set_font_size`]: font
/// zoom changes the pixel size per cell without necessarily changing the cell grid.
pub fn cell_size() -> Option<ratatui_image::FontSize> {
    let ws = crossterm::terminal::window_size().ok()?;
    if ws.width == 0 || ws.height == 0 || ws.columns == 0 || ws.rows == 0 {
        return None;
    }
    let (w, h) = (ws.width / ws.columns, ws.height / ws.rows);
    (w > 0 && h > 0).then(|| ratatui_image::FontSize::new(w, h))
}

/// Whether the terminal is iTerm2. `LC_TERMINAL` is set by iTerm2 and forwarded by default SSH
/// configurations; `TERM_PROGRAM` is not forwarded over SSH.
pub fn is_iterm2() -> bool {
    std::env::var("LC_TERMINAL").is_ok_and(|v| v == "iTerm2")
        || std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "iTerm.app")
}

/// Parses a protocol name as used by `RATATUI_SLINT_PROTOCOL`.
pub fn parse_protocol(name: &str) -> Option<ProtocolType> {
    match name.to_ascii_lowercase().as_str() {
        "halfblocks" | "half-blocks" | "blocks" => Some(ProtocolType::Halfblocks),
        "sixel" => Some(ProtocolType::Sixel),
        "kitty" => Some(ProtocolType::Kitty),
        "iterm2" | "iterm" => Some(ProtocolType::Iterm2),
        _ => None,
    }
}

impl TerminalSession {
    /// Enters raw mode and the alternate screen, queries graphics support, and enables input
    /// reporting.
    pub fn init(options: TerminalOptions) -> io::Result<Self> {
        install_panic_hook();
        crossterm::terminal::enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        // Any failure from here on must leave the terminal as it was.
        Self::init_inner(options).inspect_err(|_| {
            let _ = restore_terminal();
        })
    }

    fn init_inner(options: TerminalOptions) -> io::Result<Self> {
        let mut out = io::stdout();
        crossterm::execute!(out, EnterAlternateScreen)?;

        let kitty_compression = match std::env::var("RATATUI_SLINT_KITTY_COMPRESSION").as_deref() {
            Ok("1" | "true" | "yes") => true,
            Ok("0" | "false" | "no") => false,
            _ => options.kitty_compression,
        };
        let query = QueryStdioOptions {
            timeout: options.query_timeout,
            kitty_compression,
            ..QueryStdioOptions::default()
        };
        // Without an answer the cell size is unknown; half-blocks at 10 × 20 px still work.
        let mut picker = Picker::from_query_stdio_with_options(query).unwrap_or_else(|_| Picker::halfblocks());
        let forced = std::env::var("RATATUI_SLINT_PROTOCOL").ok().and_then(|v| parse_protocol(&v)).or(options.protocol);
        if let Some(p) = forced {
            picker.set_protocol_type(p);
        } else if picker.protocol_type() == ProtocolType::Kitty && is_iterm2() {
            // iTerm2 answers the kitty query but not the compression probe, and reads raw kitty
            // frames slowly: 1.2 fps full screen, against 14 fps with its own protocol (measured
            // 2026-10-11).
            picker.set_protocol_type(ProtocolType::Iterm2);
        }

        let mut enabled = 0;
        if options.mouse {
            crossterm::execute!(out, EnableMouseCapture)?;
            enabled |= MOUSE;
        }
        if options.bracketed_paste {
            crossterm::execute!(out, EnableBracketedPaste)?;
            enabled |= PASTE;
        }
        if options.focus_change {
            crossterm::execute!(out, EnableFocusChange)?;
            enabled |= FOCUS;
        }
        let keyboard_enhanced = options.keyboard_enhancement
            && crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        if keyboard_enhanced {
            crossterm::execute!(
                out,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                        // Without this, crossterm can report Shift+= instead of '+'.
                        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                )
            )?;
            enabled |= KEYBOARD;
        }
        ENABLED.store(enabled, Ordering::SeqCst);

        let writer = CountingWriter::new(io::stdout());
        let bytes = writer.counter();
        let mut terminal = Terminal::new(CrosstermBackend::new(writer))?;
        terminal.clear()?;
        Ok(Self { terminal, picker, keyboard_enhanced, bytes })
    }

    /// Restores the terminal. Safe to call more than once.
    pub fn restore(&mut self) -> io::Result<()> {
        let _ = self.terminal.show_cursor();
        restore_terminal()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Undoes everything [`TerminalSession::init`] enabled. Idempotent; also runs from the panic hook.
pub fn restore_terminal() -> io::Result<()> {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    let enabled = ENABLED.swap(0, Ordering::SeqCst);
    let mut out = io::stdout();
    if enabled & KEYBOARD != 0 {
        let _ = crossterm::execute!(out, PopKeyboardEnhancementFlags);
    }
    if enabled & FOCUS != 0 {
        let _ = crossterm::execute!(out, DisableFocusChange);
    }
    if enabled & PASTE != 0 {
        let _ = crossterm::execute!(out, DisableBracketedPaste);
    }
    if enabled & MOUSE != 0 {
        let _ = crossterm::execute!(out, DisableMouseCapture);
    }
    let _ = crossterm::execute!(out, LeaveAlternateScreen, crossterm::cursor::Show);
    let _ = out.flush();
    crossterm::terminal::disable_raw_mode()
}

fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal();
            previous(info);
        }));
    });
}
