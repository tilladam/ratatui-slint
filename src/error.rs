use std::fmt;

/// Errors from setting up the runtime or creating panes.
#[derive(Debug)]
pub enum Error {
    /// The Slint platform could not be installed, usually because one is already set on this thread.
    Platform(String),
    /// The component factory failed, or the component could not be shown.
    Component(String),
    /// Reading from or writing to the terminal failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Platform(e) => write!(f, "cannot install the Slint platform: {e}"),
            Error::Component(e) => write!(f, "cannot create the Slint component: {e}"),
            Error::Io(e) => write!(f, "terminal I/O failed: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
