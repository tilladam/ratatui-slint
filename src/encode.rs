//! Turns rendered pixel snapshots into terminal image protocols, inline or on a worker thread.
//!
//! The worker holds at most one pending job. Submitting a new frame replaces a job that has not
//! started yet, so a slow protocol (sixel) drops intermediate frames instead of building latency.
//! Every job carries the pane's layout generation; results from an older generation are dropped.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::picker::ProtocolType;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, ResizeEncodeRender};

use crate::platform::Waker;

/// Where encoding runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncodeMode {
    /// Worker thread for sixel, kitty and iTerm2; inline for half-blocks. The default.
    #[default]
    Auto,
    /// Always on the UI thread. Deterministic; used by tests.
    Inline,
    /// Always on a worker thread.
    Threaded,
}

impl EncodeMode {
    /// Whether frames for `protocol` encode on a worker thread.
    pub(crate) fn threaded(self, protocol: ProtocolType) -> bool {
        match self {
            EncodeMode::Inline => false,
            EncodeMode::Threaded => true,
            EncodeMode::Auto => protocol != ProtocolType::Halfblocks,
        }
    }
}

#[cfg(not(feature = "transparency"))]
pub(crate) type Pixel = slint::Rgb8Pixel;
#[cfg(feature = "transparency")]
pub(crate) type Pixel = slint::platform::software_renderer::PremultipliedRgbaColor;

/// A rendered frame as it leaves the UI thread: a copy of the framebuffer plus what is needed
/// to turn it into a protocol. Conversion, hashing and encoding all happen in [`encode`].
pub(crate) struct Frame {
    pub pixels: Vec<Pixel>,
    pub width: u32,
    pub height: u32,
    /// Keep the alpha channel (kitty and iTerm2 with the `transparency` feature).
    pub keep_alpha: bool,
    /// What transparent pixels are composited over when alpha is not kept.
    #[cfg_attr(not(feature = "transparency"), allow(dead_code))]
    pub background: [u8; 3],
    pub font: ratatui_image::FontSize,
    pub protocol_type: ratatui_image::protocol::StatefulProtocolType,
}

impl Frame {
    #[cfg(not(feature = "transparency"))]
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 3);
        for p in &self.pixels {
            out.extend_from_slice(&[p.r, p.g, p.b]);
        }
        out
    }

    #[cfg(feature = "transparency")]
    fn bytes(&self) -> Vec<u8> {
        let [br, bg, bb] = self.background;
        let mut out = Vec::with_capacity(self.pixels.len() * if self.keep_alpha { 4 } else { 3 });
        for p in &self.pixels {
            if self.keep_alpha {
                // Slint renders premultiplied alpha; image encoders expect straight alpha.
                let un = |c: u8| {
                    if p.alpha == 0 {
                        0
                    } else {
                        ((c as u32 * 255) / p.alpha as u32).min(255) as u8
                    }
                };
                out.extend_from_slice(&[un(p.red), un(p.green), un(p.blue), p.alpha]);
            } else {
                // Composite the premultiplied colour over the background.
                let inv = 255 - p.alpha as u32;
                let over = |c: u8, b: u8| (c as u32 + (b as u32 * inv) / 255).min(255) as u8;
                out.extend_from_slice(&[over(p.red, br), over(p.green, bg), over(p.blue, bb)]);
            }
        }
        out
    }

    fn to_image(&self) -> Option<DynamicImage> {
        to_image(self.width, self.height, self.bytes(), self.keep_alpha)
    }
}

/// An encoded frame ready to draw.
pub(crate) enum Displayed {
    /// Encoded by ratatui-image.
    Stateful(StatefulProtocol),
    /// An escape sequence encoded here (iTerm2 and sixel), drawn from the area's first cell.
    Raw { data: String, cells: Size },
    /// The upload is separate from placeholders so later widgets cannot erase it. The pane
    /// queues it before any placeholder of the image is drawn.
    Kitty {
        protocol: StatefulProtocol,
        upload: Option<String>,
    },
}

impl Displayed {
    /// The cell size the frame was encoded for.
    pub fn cells(&self) -> Size {
        match self {
            Displayed::Stateful(p) | Displayed::Kitty { protocol: p, .. } => {
                p.size_for(Resize::Fit(None), Size::new(u16::MAX, u16::MAX))
            }
            Displayed::Raw { cells, .. } => *cells,
        }
    }

    /// The Kitty upload, the first time it is asked for.
    pub fn take_upload(&mut self) -> Option<String> {
        match self {
            Displayed::Kitty { upload, .. } => upload.take(),
            _ => None,
        }
    }

    pub fn render(
        &mut self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        damage: bool,
    ) {
        match self {
            Displayed::Stateful(p) | Displayed::Kitty { protocol: p, .. } => p.render(area, buf),
            Displayed::Raw { data, cells } => {
                use ratatui::buffer::CellDiffOption;
                let w = cells.width.min(area.width);
                let h = cells.height.min(area.height);
                for y in area.y..area.y + h {
                    for x in area.x..area.x + w {
                        let Some(cell) = buf.cell_mut((x, y)) else {
                            continue;
                        };
                        if (x, y) == (area.x, area.y) {
                            cell.set_fg(if damage {
                                ratatui::style::Color::Black
                            } else {
                                ratatui::style::Color::Reset
                            });
                            cell.set_symbol(data)
                                .set_diff_option(CellDiffOption::ForcedWidth(
                                    std::num::NonZeroU16::MIN,
                                ));
                        } else {
                            // The image covers these cells; ratatui must not write over it.
                            cell.set_diff_option(CellDiffOption::Skip);
                        }
                    }
                }
            }
        }
    }
}

/// The sixel sequence for `image`, which covers `cells`.
///
/// ratatui-image pads sixel images to whole cells. Sixel data comes in 6-pixel bands, so a cell
/// height that is not a multiple of 6 makes the image reach a few pixels into the next row; on
/// the terminal's last row that scrolls the screen. The image is therefore cut to whole bands.
pub(crate) fn sixel_sequence(image: &DynamicImage, tmux: bool) -> Result<String, String> {
    let rgba = image.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height() / 6 * 6);
    if w == 0 || h == 0 {
        return Err("pane too small for a sixel band".into());
    }
    let bytes = &rgba.as_raw()[..w as usize * h as usize * 4];
    let seq = icy_sixel::sixel_encode(
        bytes,
        w as usize,
        h as usize,
        &icy_sixel::EncodeOptions::default(),
    )
    .map_err(|e| format!("sixel encoding error: {e}"))?;
    Ok(crate::output::passthrough(seq, tmux))
}

/// The iTerm2 inline-image sequence for `image`, sized to `cells`.
///
/// ratatui-image sizes iTerm2 images in pixels from the image dimensions. iTerm2 reports cell
/// sizes in device pixels, and on a Retina display the image did not appear. Cell units fill the
/// pane exactly, whatever iTerm2 takes a pixel to be.
pub(crate) fn iterm2_sequence(
    image: &DynamicImage,
    cells: Size,
    tmux: bool,
) -> Result<String, String> {
    use base64::Engine;
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let payload = base64::engine::general_purpose::STANDARD.encode(&png);
    let seq = format!(
        "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=0;doNotMoveCursor=1:{payload}\x07",
        png.len(),
        cells.width,
        cells.height,
    );
    Ok(crate::output::passthrough(seq, tmux))
}

pub(crate) struct Job {
    pub generation: u64,
    pub size: Size,
    pub frame: Frame,
    /// Kitty image id of this frame; `None` for other protocols.
    pub kitty_id: Option<u32>,
}

pub(crate) struct Done {
    pub generation: u64,
    pub protocol: Displayed,
    pub error: Option<String>,
    pub elapsed: std::time::Duration,
    pub kitty_id: Option<u32>,
}

fn encode(job: Job) -> Result<Done, String> {
    use ratatui_image::protocol::StatefulProtocolType;
    let start = std::time::Instant::now();
    let image = job
        .frame
        .to_image()
        .ok_or("invalid framebuffer dimensions")?;
    let (protocol, error) = match job.frame.protocol_type {
        StatefulProtocolType::ITerm2(iterm2) => {
            match iterm2_sequence(&image, job.size, iterm2.is_tmux) {
                Ok(data) => (
                    Displayed::Raw {
                        data,
                        cells: job.size,
                    },
                    None,
                ),
                Err(e) => (
                    Displayed::Raw {
                        data: String::new(),
                        cells: job.size,
                    },
                    Some(e),
                ),
            }
        }
        StatefulProtocolType::Sixel(sixel) => match sixel_sequence(&image, sixel.is_tmux) {
            Ok(data) => (
                Displayed::Raw {
                    data,
                    cells: job.size,
                },
                None,
            ),
            Err(e) => (
                Displayed::Raw {
                    data: String::new(),
                    cells: job.size,
                },
                Some(e),
            ),
        },
        protocol_type => {
            // No background: the image already fills the cell box exactly, and transparency is
            // composited above. A background would make ratatui-image blend a full-size copy.
            let mut protocol = StatefulProtocol::new(image, job.frame.font, None, protocol_type);
            // The image already matches the cell box, so Fit only downsamples for half-blocks.
            protocol.resize_encode(&Resize::Fit(None), job.size);
            let error = protocol
                .last_encoding_result()
                .and_then(|r| r.err())
                .map(|e| e.to_string());
            let displayed = if job.kitty_id.is_some() && error.is_none() {
                // ratatui-image exposes the upload only on first render. Consume it into
                // a scratch cell on the encoder thread; future renders produce placeholders
                // only. Keep the upload outside the ratatui diff/composition pipeline.
                let area = ratatui::layout::Rect::new(0, 0, 1, 1);
                let mut scratch = ratatui::buffer::Buffer::empty(area);
                protocol.render(area, &mut scratch);
                let (upload, _) = scratch[(0, 0)]
                    .symbol()
                    .split_once('\u{10eeee}')
                    .ok_or("kitty encoder did not produce an upload/placeholder pair")?;
                if upload.is_empty() {
                    return Err("kitty encoder produced an empty upload".into());
                }
                Displayed::Kitty {
                    protocol,
                    upload: Some(upload.to_owned()),
                }
            } else {
                Displayed::Stateful(protocol)
            };
            (displayed, error)
        }
    };
    Ok(Done {
        generation: job.generation,
        protocol,
        error,
        elapsed: start.elapsed(),
        kitty_id: job.kitty_id,
    })
}

pub(crate) struct Slot {
    job: Option<Job>,
    stop: bool,
}

pub(crate) enum Encoder {
    Inline {
        done: Option<Box<Result<Done, String>>>,
    },
    Threaded {
        slot: Arc<(Mutex<Slot>, Condvar)>,
        rx: Receiver<Result<Done, String>>,
        failure: Option<String>,
    },
}

impl Encoder {
    pub fn new(threaded: bool, waker: Waker) -> std::io::Result<Self> {
        if !threaded {
            return Ok(Encoder::Inline { done: None });
        }
        let slot = Arc::new((
            Mutex::new(Slot {
                job: None,
                stop: false,
            }),
            Condvar::new(),
        ));
        let (tx, rx) = mpsc::channel();
        let worker_slot = slot.clone();
        std::thread::Builder::new()
            .name("ratatui-slint-encode".into())
            .spawn(move || {
                worker_boundary(&worker_slot, &tx, &waker, || {
                    encode_worker(&worker_slot, &tx, &waker)
                });
            })?;
        Ok(Encoder::Threaded {
            slot,
            rx,
            failure: None,
        })
    }

    /// Queues a frame, replacing any frame that has not started encoding.
    pub fn submit(&mut self, job: Job) -> Result<(), String> {
        match self {
            Encoder::Inline { done } => *done = Some(Box::new(encode(job))),
            Encoder::Threaded { slot, failure, .. } => {
                if let Some(error) = failure {
                    return Err(error.clone());
                }
                let (lock, cvar) = &**slot;
                let mut s = lock.lock().map_err(|_| "encoder state poisoned")?;
                if s.stop {
                    return Err("encoder worker stopped".into());
                }
                s.job = Some(job);
                cvar.notify_one();
            }
        }
        Ok(())
    }

    /// Newest finished frame for this generation; terminal worker errors are never discarded.
    pub fn take_latest(&mut self, generation: u64) -> Result<Option<Done>, String> {
        match self {
            Encoder::Inline { done } => done
                .take()
                .map(|done| *done)
                .transpose()
                .map(|done| done.filter(|d| d.generation == generation)),
            Encoder::Threaded { rx, failure, .. } => {
                if let Some(error) = failure {
                    return Err(error.clone());
                }
                let mut newest = None;
                loop {
                    match rx.try_recv() {
                        Ok(done) => {
                            let done = done.inspect_err(|error| {
                                *failure = Some(error.clone());
                            })?;
                            if done.generation == generation {
                                newest = Some(done);
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => return Ok(newest),
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let error = "encoder worker disconnected".to_owned();
                            *failure = Some(error.clone());
                            return Err(error);
                        }
                    }
                }
            }
        }
    }
}

fn worker_boundary(
    slot: &(Mutex<Slot>, Condvar),
    tx: &Sender<Result<Done, String>>,
    waker: &Waker,
    run: impl FnOnce() -> Result<(), String>,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    let failure = match result {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(_) => Some("encoder worker panicked".to_owned()),
    };
    if let Some(error) = failure {
        slot.0.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        let _ = tx.send(Err(error));
        waker.wake();
    }
}

fn encode_worker(
    slot: &(Mutex<Slot>, Condvar),
    tx: &Sender<Result<Done, String>>,
    waker: &Waker,
) -> Result<(), String> {
    let (lock, cvar) = slot;
    loop {
        let job = {
            let mut s = lock.lock().map_err(|_| "encoder state poisoned")?;
            while s.job.is_none() && !s.stop {
                s = cvar.wait(s).map_err(|_| "encoder state poisoned")?;
            }
            if s.stop {
                return Ok(());
            }
            s.job.take().ok_or("encoder wake without a job")?
        };
        if tx.send(encode(job)).is_err() {
            return Ok(());
        }
        waker.wake();
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if let Encoder::Threaded { slot, .. } = self {
            let (lock, cvar) = &**slot;
            let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
            s.stop = true;
            s.job = None;
            cvar.notify_one();
        }
    }
}

/// Converts frame bytes to the image type the encoders take.
fn to_image(width: u32, height: u32, bytes: Vec<u8>, has_alpha: bool) -> Option<DynamicImage> {
    if has_alpha {
        image::RgbaImage::from_raw(width, height, bytes).map(DynamicImage::ImageRgba8)
    } else {
        image::RgbImage::from_raw(width, height, bytes).map(DynamicImage::ImageRgb8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_threads_every_protocol_but_half_blocks() {
        assert!(!EncodeMode::Auto.threaded(ProtocolType::Halfblocks));
        assert!(EncodeMode::Threaded.threaded(ProtocolType::Halfblocks));
        for protocol in [
            ProtocolType::Sixel,
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
        ] {
            assert!(EncodeMode::Auto.threaded(protocol), "{protocol:?}");
            assert!(!EncodeMode::Inline.threaded(protocol), "{protocol:?}");
        }
    }

    #[test]
    fn worker_panic_and_poison_wake_host_and_preserve_cause() {
        let mut runtime = crate::Runtime::new().unwrap();
        let slot = Arc::new((
            Mutex::new(Slot {
                job: None,
                stop: false,
            }),
            Condvar::new(),
        ));
        let (tx, rx) = mpsc::channel();
        let worker_slot = slot.clone();
        let waker = runtime.waker();
        std::thread::spawn(move || {
            worker_boundary(&worker_slot, &tx, &waker, || {
                let _guard = worker_slot.0.lock().unwrap();
                panic!("test encoder failure");
            });
        })
        .join()
        .unwrap();
        assert!(matches!(
            runtime.next_event_capped(Some(std::time::Duration::from_secs(1))),
            crate::LoopEvent::Wake
        ));
        let mut encoder = Encoder::Threaded {
            slot,
            rx,
            failure: None,
        };
        for _ in 0..2 {
            assert!(
                matches!(encoder.take_latest(99), Err(ref e) if e == "encoder worker panicked")
            );
        }
        drop(encoder); // poisoned state must not cause a second panic
    }

    #[test]
    fn disconnected_worker_is_reported() {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let slot = Arc::new((
            Mutex::new(Slot {
                job: None,
                stop: false,
            }),
            Condvar::new(),
        ));
        let mut encoder = Encoder::Threaded {
            slot,
            rx,
            failure: None,
        };
        assert!(matches!(encoder.take_latest(0), Err(ref e) if e.contains("disconnected")));
    }
}
