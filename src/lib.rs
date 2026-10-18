//! Embed Slint components as pixel widgets inside a ratatui application.
//!
//! Slint renders each component with its software renderer into a framebuffer the size of the
//! pane's cells. The frame is shown through ratatui-image (kitty, sixel, iTerm2 or half-blocks).
//! The ratatui loop drives Slint's timers, animations and rendering; Slint never runs its own
//! event loop.
//!
//! ```no_run
//! use ratatui_slint::{InputRouter, LoopEvent, PaneInput, Routed, Runtime, SlintWidget};
//! use ratatui_slint::terminal::{TerminalOptions, TerminalSession};
//!
//! slint::slint! { export component Hello inherits Window { Text { text: "hello"; } } }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut rt = Runtime::new()?;
//! let mut session = TerminalSession::init(TerminalOptions::default())?;
//! rt.spawn_terminal_reader(); // after the terminal queries
//! let mut pane = rt.create_pane(&session.picker, Hello::new)?;
//! let mut router = InputRouter::new();
//! router.set_keyboard_enhanced(session.keyboard_enhanced);
//! loop {
//!     match rt.next_event() {
//!         LoopEvent::Terminal(ev) => {
//!             if router.route(&ev, &[&pane as &dyn PaneInput]) == Routed::Unhandled {
//!                 if matches!(ev, crossterm::event::Event::Key(k) if k.code == crossterm::event::KeyCode::Char('q')) {
//!                     break;
//!                 }
//!             }
//!         }
//!         LoopEvent::Quit => break,
//!         _ => {}
//!     }
//!     if rt.begin_frame() {
//!         let area = session.terminal.size()?.into();
//!         pane.set_area(area);
//!         if pane.update() {
//!             session.terminal.draw(|f| f.render_stateful_widget(SlintWidget, area, &mut pane))?;
//!         }
//!     }
//!     rt.write_pending_output(session.terminal.backend_mut())?;
//! }
//! # Ok(()) }
//! ```

#![warn(missing_docs)]

mod encode;
mod error;
pub mod input;
pub mod output;
mod pane;
mod platform;
pub mod terminal;
mod widget;

pub use encode::EncodeMode;
pub use error::Error;
pub use input::{InputRouter, KeyBinding, Routed};
pub use pane::{
    AUTO_ROW_HEIGHT, FrameStats, Framebuffer, MAX_PANE_PIXELS, PaneId, PaneInput, PaneRender,
    Scale, SlintPane,
};
pub use platform::{LoopEvent, Runtime, Waker};
pub use widget::SlintWidget;

pub use ratatui_image;
pub use slint;
