#![doc = include_str!("../README.md")]
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
