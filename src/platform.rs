//! The Slint platform and the loop that drives it from a ratatui application.
//!
//! [`Runtime`] installs a Slint platform on the current thread. Every Slint window becomes a
//! [`MinimalSoftwareWindow`]; nothing runs Slint's own event loop. Instead the host loop calls
//! [`Runtime::next_event`], which blocks on one channel that carries terminal input, closures
//! from [`slint::invoke_from_event_loop`], encoder completions and application wake-ups.

use std::cell::RefCell;
use std::io::Write;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossterm::event::Event;
use ratatui_image::picker::Picker;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Clipboard, EventLoopProxy, Platform, WindowAdapter};
use slint::{ComponentHandle, EventLoopError, PlatformError};

use crate::Error;
use crate::pane::SlintPane;

/// Messages that wake the host loop.
pub(crate) enum Msg {
    Terminal(Event),
    Invoke(Box<dyn FnOnce() + Send>),
    Wake,
    Quit,
}

/// Cheap, cloneable, `Send` handle that wakes the host loop from any thread.
#[derive(Clone)]
pub struct Waker {
    tx: Sender<Msg>,
}

impl Waker {
    /// Wakes the loop. Returns `false` if the loop is gone.
    pub fn wake(&self) -> bool {
        self.tx.send(Msg::Wake).is_ok()
    }
}

/// What woke [`Runtime::next_event`].
#[derive(Debug)]
pub enum LoopEvent {
    /// Input from the terminal reader thread.
    Terminal(Event),
    /// A [`Waker`], an encoder, or a closure posted with `slint::invoke_from_event_loop` (the
    /// closure has already run).
    Wake,
    /// A Slint timer is due or the next animation frame is due.
    Timeout,
    /// `slint::quit_event_loop()` was called, the terminal reader failed, or every sender is gone.
    Quit,
}

/// State shared between the platform object Slint owns and the [`Runtime`] the host owns.
pub(crate) struct Shared {
    windows: RefCell<Vec<Weak<MinimalSoftwareWindow>>>,
    clipboard: RefCell<Option<String>>,
    selection: RefCell<Option<String>>,
    pending_output: RefCell<Vec<u8>>,
}

impl Shared {
    fn live_windows(&self) -> Vec<Rc<MinimalSoftwareWindow>> {
        let mut windows = self.windows.borrow_mut();
        windows.retain(|w| w.strong_count() > 0);
        windows.iter().filter_map(Weak::upgrade).collect()
    }

    pub(crate) fn find_window(&self, window: &slint::Window) -> Option<Rc<MinimalSoftwareWindow>> {
        self.live_windows()
            .into_iter()
            .find(|w| std::ptr::eq(w.window(), window))
    }
}

thread_local! {
    static CURRENT: RefCell<Weak<Shared>> = const { RefCell::new(Weak::new()) };
}

/// Sets the cached clipboard text, used for bracketed paste. Returns `false` without a runtime.
pub(crate) fn set_clipboard_cache(text: String) -> bool {
    CURRENT.with(|c| match c.borrow().upgrade() {
        Some(shared) => {
            *shared.clipboard.borrow_mut() = Some(text);
            true
        }
        None => false,
    })
}

/// Queues bytes for the terminal, written by [`Runtime::write_pending_output`] after the frame.
pub(crate) fn queue_output(bytes: &[u8]) {
    CURRENT.with(|c| {
        if let Some(shared) = c.borrow().upgrade() {
            shared.pending_output.borrow_mut().extend_from_slice(bytes);
        }
    });
}

/// Only one platform per process may register Slint's event-loop proxy, because Slint keeps
/// that proxy in a process-wide slot while the platform itself is per thread.
static PROXY_CLAIMED: AtomicBool = AtomicBool::new(false);

struct Proxy {
    tx: Sender<Msg>,
}

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        self.tx
            .send(Msg::Quit)
            .map_err(|_| EventLoopError::EventLoopTerminated)
    }

    fn invoke_from_event_loop(
        &self,
        event: Box<dyn FnOnce() + Send>,
    ) -> Result<(), EventLoopError> {
        self.tx
            .send(Msg::Invoke(event))
            .map_err(|_| EventLoopError::EventLoopTerminated)
    }
}

struct TermPlatform {
    shared: Rc<Shared>,
    proxy_tx: Option<Sender<Msg>>,
}

impl Platform for TermPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        self.shared
            .windows
            .borrow_mut()
            .push(Rc::downgrade(&window));
        Ok(window)
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        self.proxy_tx
            .clone()
            .map(|tx| Box::new(Proxy { tx }) as Box<dyn EventLoopProxy>)
    }

    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        match clipboard {
            Clipboard::DefaultClipboard => {
                *self.shared.clipboard.borrow_mut() = Some(text.to_owned());
                // OSC 52 copy. Queued, never written from inside a Slint dispatch: the host
                // writes it between frames through the same writer as the terminal output.
                let seq = crate::output::osc52(text);
                self.shared
                    .pending_output
                    .borrow_mut()
                    .extend_from_slice(seq.as_bytes());
            }
            Clipboard::SelectionClipboard => {
                *self.shared.selection.borrow_mut() = Some(text.to_owned())
            }
            _ => {}
        }
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        // Terminals cannot be read back reliably (OSC 52 read is disabled almost everywhere),
        // so paste comes from the local cache, filled by copies and by bracketed paste.
        match clipboard {
            Clipboard::DefaultClipboard => self.shared.clipboard.borrow().clone(),
            Clipboard::SelectionClipboard => self.shared.selection.borrow().clone(),
            _ => None,
        }
    }
}

/// Owns the Slint platform for this thread and the host loop's wake channel.
pub struct Runtime {
    shared: Rc<Shared>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    frame_interval: Duration,
    last_frame: Option<Instant>,
    frame_pending: bool,
    reader: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    owns_proxy: bool,
}

impl Runtime {
    /// Installs the platform on the current thread.
    ///
    /// Slint keeps the platform per thread, so each thread may own one runtime. The first runtime
    /// in the process also becomes the target of `slint::invoke_from_event_loop`.
    pub fn new() -> Result<Self, Error> {
        let (tx, rx) = mpsc::channel();
        let shared = Rc::new(Shared {
            windows: RefCell::default(),
            clipboard: RefCell::default(),
            selection: RefCell::default(),
            pending_output: RefCell::default(),
        });
        let owns_proxy = !PROXY_CLAIMED.swap(true, Ordering::SeqCst);
        let platform = TermPlatform {
            shared: shared.clone(),
            proxy_tx: owns_proxy.then(|| tx.clone()),
        };
        if let Err(e) = slint::platform::set_platform(Box::new(platform)) {
            if owns_proxy {
                PROXY_CLAIMED.store(false, Ordering::SeqCst);
            }
            return Err(Error::Platform(format!("{e:?}")));
        }
        CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&shared));
        Ok(Self {
            shared,
            tx,
            rx,
            frame_interval: Duration::from_secs(1) / 30,
            last_frame: None,
            frame_pending: true,
            reader: None,
            owns_proxy,
        })
    }

    /// Whether `slint::invoke_from_event_loop` and `slint::quit_event_loop` reach this runtime.
    pub fn receives_invoke_from_event_loop(&self) -> bool {
        self.owns_proxy
    }

    /// A handle that wakes [`Runtime::next_event`] from any thread.
    pub fn waker(&self) -> Waker {
        Waker {
            tx: self.tx.clone(),
        }
    }

    /// Creates a component with `factory` and wraps it in a pane.
    ///
    /// The pane's window is found by matching `component.window()` against the windows the
    /// platform created, so factories that create helper components are handled.
    pub fn create_pane<C, E>(
        &self,
        picker: &Picker,
        factory: impl FnOnce() -> Result<C, E>,
    ) -> Result<SlintPane<C>, Error>
    where
        C: ComponentHandle,
        E: std::fmt::Display,
    {
        let component = factory().map_err(|e| Error::Component(e.to_string()))?;
        component
            .show()
            .map_err(|e| Error::Component(e.to_string()))?;
        let Some(window) = self.shared.find_window(component.window()) else {
            let _ = component.hide();
            return Err(Error::Component(
                "component window was not created by this runtime's platform".into(),
            ));
        };
        Ok(SlintPane::new(
            component,
            window,
            picker.clone(),
            self.waker(),
        ))
    }

    /// Number of Slint windows still alive. Zero once every pane has been dropped.
    pub fn live_windows(&self) -> usize {
        self.shared.live_windows().len()
    }

    /// Caps how often [`Runtime::begin_frame`] reports a frame as due. Default 30.
    pub fn set_frame_rate(&mut self, fps: u32) {
        self.frame_interval = Duration::from_secs(1) / fps.max(1);
    }

    /// Starts a thread that reads crossterm events into the loop channel.
    ///
    /// Call this after any terminal query that reads stdin, such as
    /// [`Picker::from_query_stdio`]; the reader would otherwise swallow the replies.
    pub fn spawn_terminal_reader(&mut self) {
        if self.reader.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let tx = self.tx.clone();
        let flag = stop.clone();
        let handle = std::thread::Builder::new()
            .name("ratatui-slint-input".into())
            .spawn(move || {
                read_terminal_events(&flag, &tx, crossterm::event::poll, crossterm::event::read)
            })
            .expect("spawn input thread");
        self.reader = Some((stop, handle));
    }

    /// Stops the terminal reader thread and waits for it, so it no longer reads stdin.
    pub fn stop_terminal_reader(&mut self) {
        if let Some((stop, handle)) = self.reader.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
    }

    /// Feeds an event into the loop as if the terminal had produced it. Used by tests and by
    /// hosts that read input themselves.
    pub fn inject(&self, event: Event) {
        let _ = self.tx.send(Msg::Terminal(event));
    }

    /// How long the loop may sleep: until the next Slint timer, or until the next frame when
    /// Slint is animating or a frame is pending. `None` means block until input.
    ///
    /// An overdue timer returns zero. Timers run in [`Runtime::next_event`] after the wait,
    /// ensuring that their changes get a frame before the loop goes back to sleep.
    pub fn timeout(&self) -> Option<Duration> {
        let animating = self.animating();
        let mut timeout = slint::platform::duration_until_next_timer_update();
        if self.frame_pending || animating {
            let until_frame = self.next_frame_at().map_or(Duration::ZERO, |at| {
                at.saturating_duration_since(Instant::now())
            });
            timeout = Some(timeout.map_or(until_frame, |t| t.min(until_frame)));
        }
        timeout
    }

    /// Blocks until something happens, then advances Slint's timers and animations.
    ///
    /// Closures posted with `slint::invoke_from_event_loop` run here, on the Slint thread.
    pub fn next_event(&mut self) -> LoopEvent {
        self.next_event_capped(None)
    }

    /// Like [`Runtime::next_event`] but never sleeps longer than `max`.
    pub fn next_event_capped(&mut self, max: Option<Duration>) -> LoopEvent {
        let timeout = match (self.timeout(), max) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let msg = match timeout {
            Some(t) => match self.rx.recv_timeout(t) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(Msg::Quit),
            },
            None => Some(self.rx.recv().unwrap_or(Msg::Quit)),
        };
        let event = match msg {
            None => LoopEvent::Timeout,
            Some(m) => self.handle(m),
        };
        // Any wake-up may have changed Slint state, so the next begin_frame() gets to look.
        slint::platform::update_timers_and_animations();
        self.frame_pending = true;
        event
    }

    fn handle(&mut self, msg: Msg) -> LoopEvent {
        match msg {
            Msg::Terminal(ev) => LoopEvent::Terminal(ev),
            Msg::Invoke(f) => {
                f();
                LoopEvent::Wake
            }
            Msg::Wake => LoopEvent::Wake,
            Msg::Quit => LoopEvent::Quit,
        }
    }

    /// Marks that something outside Slint changed and the host wants a frame.
    pub fn request_frame(&mut self) {
        self.frame_pending = true;
    }

    /// Returns `true` when a frame should be produced now: one is pending or Slint is animating,
    /// and the frame cap allows it. Starts the frame on an absolute cadence, so slow frames and
    /// input bursts cannot push the cap off its grid.
    pub fn begin_frame(&mut self) -> bool {
        if !(self.frame_pending || self.animating()) {
            return false;
        }
        let now = Instant::now();
        self.last_frame = Some(match self.next_frame_at() {
            Some(due) if now < due => return false,
            // Stay on the grid unless we fell more than one interval behind.
            Some(due) if now.duration_since(due) < self.frame_interval => due,
            _ => now,
        });
        self.frame_pending = false;
        true
    }

    /// When the next frame may start; `None` before the first frame.
    fn next_frame_at(&self) -> Option<Instant> {
        self.last_frame.map(|last| last + self.frame_interval)
    }

    /// Whether the last render left animations running, so another frame is needed.
    ///
    /// Slint tracks this per thread, not per window, and resets it whenever timers and animations
    /// advance; it is meaningful between a render and the next clock update.
    pub fn animating(&self) -> bool {
        self.shared
            .live_windows()
            .first()
            .is_some_and(|w| w.window().has_active_animations())
    }

    /// Writes queued terminal output to `out`: OSC 52 clipboard copies and deletion of kitty
    /// images that a newer frame replaced. Call after every `terminal.draw`, with the same writer
    /// the terminal backend uses; skipping it lets replaced kitty images pile up in the terminal.
    pub fn write_pending_output(&self, out: &mut impl Write) -> std::io::Result<()> {
        let bytes = std::mem::take(&mut *self.shared.pending_output.borrow_mut());
        if !bytes.is_empty() {
            out.write_all(&bytes)?;
            out.flush()?;
        }
        Ok(())
    }

    /// Text that a copy inside a pane put on the clipboard, if any.
    pub fn clipboard_text(&self) -> Option<String> {
        self.shared.clipboard.borrow().clone()
    }

    /// Replaces the clipboard cache that Slint pastes from.
    pub fn set_clipboard_text(&self, text: impl Into<String>) {
        *self.shared.clipboard.borrow_mut() = Some(text.into());
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop_terminal_reader();
    }
}

fn read_terminal_events(
    stop: &AtomicBool,
    tx: &Sender<Msg>,
    mut poll: impl FnMut(Duration) -> std::io::Result<bool>,
    mut read: impl FnMut() -> std::io::Result<Event>,
) {
    while !stop.load(Ordering::Relaxed) {
        let result = match poll(Duration::from_millis(50)) {
            Ok(false) => continue,
            Ok(true) => read(),
            Err(e) => Err(e),
        };
        match result {
            Ok(event) => {
                if tx.send(Msg::Terminal(event)).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                // Runtime and its platform retain senders, so exiting the reader alone cannot
                // disconnect the channel. Wake a blocked host explicitly.
                if !stop.load(Ordering::Relaxed) {
                    let _ = tx.send(Msg::Quit);
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_reader_errors_terminate_the_host_loop() {
        for poll_fails in [true, false] {
            let (tx, rx) = mpsc::channel();
            read_terminal_events(
                &AtomicBool::new(false),
                &tx,
                |_| {
                    if poll_fails {
                        Err(std::io::Error::other("poll failed"))
                    } else {
                        Ok(true)
                    }
                },
                || Err(std::io::Error::other("read failed")),
            );
            assert!(matches!(rx.try_recv(), Ok(Msg::Quit)));
        }
    }

    #[test]
    fn stopping_terminal_reader_does_not_quit_the_host() {
        let (tx, rx) = mpsc::channel();
        read_terminal_events(
            &AtomicBool::new(true),
            &tx,
            |_| panic!("stopped reader polled"),
            || panic!("stopped reader read"),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn terminal_reader_retries_interrupts_and_forwards_events() {
        let (tx, rx) = mpsc::channel();
        let mut calls = 0;
        read_terminal_events(
            &AtomicBool::new(false),
            &tx,
            |_| {
                calls += 1;
                match calls {
                    1 => Err(std::io::ErrorKind::Interrupted.into()),
                    2 => Ok(true),
                    _ => Err(std::io::Error::other("disconnected")),
                }
            },
            || Ok(Event::FocusLost),
        );
        assert!(matches!(rx.try_recv(), Ok(Msg::Terminal(Event::FocusLost))));
        assert!(matches!(rx.try_recv(), Ok(Msg::Quit)));
    }
}
