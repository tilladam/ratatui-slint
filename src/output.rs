//! Terminal output helpers: OSC 52 clipboard sequences and a byte-counting writer.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;

/// The OSC 52 sequence that puts `text` on the system clipboard. Inside tmux it is wrapped in a
/// DCS passthrough, which tmux forwards when `allow-passthrough` is on.
pub fn osc52(text: &str) -> String {
    let payload = base64::engine::general_purpose::STANDARD.encode(text);
    let seq = format!("\x1b]52;c;{payload}\x07");
    if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq
    }
}

/// Wraps a writer and counts the bytes written through it.
///
/// Wrap the terminal backend's writer in this to measure what actually reaches the terminal:
/// `CrosstermBackend::new(CountingWriter::new(std::io::stdout()))`.
pub struct CountingWriter<W> {
    inner: W,
    count: Arc<AtomicU64>,
}

impl<W> CountingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner, count: Arc::default() }
    }

    /// A handle that reads the running total from anywhere.
    pub fn counter(&self) -> ByteCounter {
        ByteCounter(self.count.clone())
    }

    pub fn inner(&self) -> &W {
        &self.inner
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Shared view of a [`CountingWriter`]'s total.
#[derive(Clone, Debug, Default)]
pub struct ByteCounter(Arc<AtomicU64>);

impl ByteCounter {
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_encodes_base64() {
        // SAFETY: test-local check; TMUX may be set when tests run inside tmux.
        if std::env::var_os("TMUX").is_none() {
            assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
        }
    }

    #[test]
    fn counting_writer_counts() {
        let mut w = CountingWriter::new(Vec::new());
        let c = w.counter();
        w.write_all(b"abc").unwrap();
        assert_eq!(c.get(), 3);
    }
}
