//! Turns rendered pixel snapshots into terminal image protocols, inline or on a worker thread.
//!
//! The worker holds at most one pending job. Submitting a new frame replaces a job that has not
//! started yet, so a slow protocol (sixel) drops intermediate frames instead of building latency.
//! Every job carries the pane's layout generation; results from an older generation are dropped.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{ResizeEncodeRender, Resize};

use crate::platform::Waker;

/// Where encoding runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncodeMode {
    /// Worker thread for sixel, kitty and iTerm2; inline for half-blocks. The default.
    #[default]
    Auto,
    /// Always on the UI thread. Deterministic; used by tests.
    Inline,
    /// Always on a worker thread.
    Threaded,
}

pub(crate) struct Job {
    pub generation: u64,
    pub size: Size,
    pub protocol: StatefulProtocol,
}

pub(crate) struct Done {
    pub generation: u64,
    pub protocol: StatefulProtocol,
    pub error: Option<String>,
    pub elapsed: std::time::Duration,
}

fn encode(mut job: Job) -> Done {
    let start = std::time::Instant::now();
    // The image already matches the cell box, so Fit only downsamples for half-blocks.
    job.protocol.resize_encode(&Resize::Fit(None), job.size);
    let error = job.protocol.last_encoding_result().and_then(|r| r.err()).map(|e| e.to_string());
    Done { generation: job.generation, protocol: job.protocol, error, elapsed: start.elapsed() }
}

pub(crate) struct Slot {
    job: Option<Job>,
    stop: bool,
}

pub(crate) enum Encoder {
    Inline { done: Option<Box<Done>> },
    Threaded { slot: Arc<(Mutex<Slot>, Condvar)>, rx: Receiver<Done> },
}

impl Encoder {
    pub fn new(threaded: bool, waker: Waker) -> Self {
        if !threaded {
            return Encoder::Inline { done: None };
        }
        let slot = Arc::new((Mutex::new(Slot { job: None, stop: false }), Condvar::new()));
        let (tx, rx): (Sender<Done>, Receiver<Done>) = mpsc::channel();
        let worker_slot = slot.clone();
        std::thread::Builder::new()
            .name("ratatui-slint-encode".into())
            .spawn(move || {
                let (lock, cvar) = &*worker_slot;
                loop {
                    let job = {
                        let mut s = lock.lock().unwrap();
                        while s.job.is_none() && !s.stop {
                            s = cvar.wait(s).unwrap();
                        }
                        if s.stop {
                            return;
                        }
                        s.job.take().unwrap()
                    };
                    if tx.send(encode(job)).is_err() {
                        return;
                    }
                    waker.wake();
                }
            })
            .expect("spawn encoder thread");
        Encoder::Threaded { slot, rx }
    }

    /// Queues a frame, replacing any frame that has not started encoding.
    pub fn submit(&mut self, job: Job) {
        match self {
            Encoder::Inline { done } => *done = Some(Box::new(encode(job))),
            Encoder::Threaded { slot, .. } => {
                let (lock, cvar) = &**slot;
                lock.lock().unwrap().job = Some(job);
                cvar.notify_one();
            }
        }
    }

    /// The newest finished frame for `generation`, if any. Older generations are discarded.
    pub fn take_latest(&mut self, generation: u64) -> Option<Done> {
        match self {
            Encoder::Inline { done } => done.take().filter(|d| d.generation == generation).map(|d| *d),
            Encoder::Threaded { rx, .. } => {
                rx.try_iter().filter(|d| d.generation == generation).last()
            }
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if let Encoder::Threaded { slot, .. } = self {
            let (lock, cvar) = &**slot;
            if let Ok(mut s) = lock.lock() {
                s.stop = true;
                s.job = None;
            }
            cvar.notify_one();
        }
    }
}

/// Converts a rendered frame to the image type the encoders take.
pub(crate) fn to_image(width: u32, height: u32, bytes: Vec<u8>, has_alpha: bool) -> Option<DynamicImage> {
    if has_alpha {
        image::RgbaImage::from_raw(width, height, bytes).map(DynamicImage::ImageRgba8)
    } else {
        image::RgbImage::from_raw(width, height, bytes).map(DynamicImage::ImageRgb8)
    }
}
