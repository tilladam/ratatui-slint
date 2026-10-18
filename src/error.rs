use std::fmt;

/// Errors from runtime/pane setup and configuration.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The Slint platform could not be installed, usually because one is already set on this thread.
    Platform(slint::platform::SetPlatformError),
    /// The component factory failed, or its window belongs to another runtime.
    Component(String),
    /// Slint could not show a component window.
    Window(slint::PlatformError),
    /// I/O or worker thread creation failed.
    Io(std::io::Error),
    /// Alpha output was requested without the transparency Cargo feature.
    TransparencyUnavailable,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Platform(e) => write!(f, "cannot install the Slint platform: {e}"),
            Error::Component(e) => write!(f, "cannot create the Slint component: {e}"),
            Error::Io(e) => write!(f, "I/O failed: {e}"),
            Error::TransparencyUnavailable => {
                f.write_str("enable the transparency Cargo feature for alpha output")
            }
            Error::Window(e) => write!(f, "cannot show the Slint component: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Platform(e) => Some(e),
            Self::Window(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Component(_) | Self::TransparencyUnavailable => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
