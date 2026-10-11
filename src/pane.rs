//! One Slint component living in a ratatui `Rect`.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::{StatefulProtocol, StatefulProtocolType};
use ratatui_image::{FontSize, ResizeEncodeRender};
use slint::platform::WindowEvent;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::{ComponentHandle, PhysicalSize};

use crate::encode::{EncodeMode, Encoder, Job, to_image};
use crate::platform::Waker;

#[cfg(not(feature = "transparency"))]
type Pixel = slint::Rgb8Pixel;
#[cfg(feature = "transparency")]
type Pixel = slint::platform::software_renderer::PremultipliedRgbaColor;

/// Largest pane edge in physical pixels. Larger areas are treated as hidden instead of
/// allocating an unbounded framebuffer.
pub const MAX_PANE_PIXELS: u32 = 8192;

/// Identifies a pane for input routing and focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// How logical Slint pixels map to the physical pixels a pane covers.
///
/// The terminal font fixes the physical size (cells × cell pixels). The scale factor is a separate
/// choice, applied with `WindowEvent::ScaleFactorChanged`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    /// A fixed factor. `Fixed(1.0)`, the default, makes one logical pixel one terminal pixel.
    Fixed(f32),
    /// Scale a design of this logical size so it fits the pane, keeping its aspect ratio.
    /// Suits UIs laid out for a fixed screen, such as an embedded touch panel.
    Fit { width: f32, height: f32 },
}

impl Default for Scale {
    fn default() -> Self {
        Scale::Fixed(1.0)
    }
}

impl Scale {
    fn factor(self, px: (u32, u32)) -> f32 {
        let f = match self {
            Scale::Fixed(f) => f,
            Scale::Fit { width, height } if width > 0.0 && height > 0.0 => {
                (px.0 as f32 / width).min(px.1 as f32 / height)
            }
            Scale::Fit { .. } => 1.0,
        };
        if f.is_finite() && f > 0.01 { f } else { 1.0 }
    }
}

/// Physical pixel size of a cell area, or `None` if it is empty or too large.
pub(crate) fn pixel_size(area: Rect, font: FontSize) -> Option<(u32, u32)> {
    let w = u32::from(area.width).checked_mul(u32::from(font.width))?;
    let h = u32::from(area.height).checked_mul(u32::from(font.height))?;
    (w > 0 && h > 0 && w <= MAX_PANE_PIXELS && h <= MAX_PANE_PIXELS).then_some((w, h))
}

/// Timings of the most recent frame, for diagnostics and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// Slint software rendering into the framebuffer.
    pub render: std::time::Duration,
    /// Framebuffer copy into an image plus protocol construction, on the UI thread.
    pub snapshot: std::time::Duration,
    /// Protocol encoding, on whichever thread ran it.
    pub encode: std::time::Duration,
}

/// The object-safe view of a pane that the input router needs.
pub trait PaneInput {
    fn id(&self) -> PaneId;
    /// The cell area the pane occupies on screen.
    fn area(&self) -> Rect;
    /// Terminal cell size in pixels.
    fn font_size(&self) -> FontSize;
    fn slint_window(&self) -> &slint::Window;
}

/// A Slint component rendered into a ratatui area.
///
/// Create it with [`crate::Runtime::create_pane`]. Lay it out with [`SlintPane::set_area`],
/// call [`SlintPane::update`] once per frame, and draw it with [`crate::SlintWidget`].
pub struct SlintPane<C: ComponentHandle> {
    component: C,
    window: Rc<MinimalSoftwareWindow>,
    id: PaneId,
    picker: Picker,
    font: FontSize,
    scale: Scale,
    scale_factor: f32,
    area: Rect,
    px: Option<(u32, u32)>,
    pixels: Vec<Pixel>,
    generation: u64,
    template: StatefulProtocolType,
    encoder: Encoder,
    encode_mode: EncodeMode,
    waker: Waker,
    displayed: Option<(u64, StatefulProtocol)>,
    needs_redraw: bool,
    background: [u8; 3],
    frames: u64,
    last_error: Option<String>,
    stats: FrameStats,
}

impl<C: ComponentHandle> SlintPane<C> {
    pub(crate) fn new(component: C, window: Rc<MinimalSoftwareWindow>, picker: Picker, waker: Waker) -> Self {
        let font = picker.font_size();
        // One protocol state for the life of the pane. Each frame is encoded into a clone of it,
        // so a kitty pane keeps one image id instead of allocating a new one per frame.
        let template = picker.new_resize_protocol(image::DynamicImage::new_rgb8(1, 1)).protocol_type_owned();
        let encode_mode = EncodeMode::Auto;
        let encoder = Encoder::new(Self::threaded(encode_mode, &picker), waker.clone());
        let pane = Self {
            component,
            window,
            id: PaneId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            picker,
            font,
            scale: Scale::default(),
            scale_factor: 1.0,
            area: Rect::default(),
            px: None,
            pixels: Vec::new(),
            generation: 0,
            template,
            encoder,
            encode_mode,
            waker,
            displayed: None,
            needs_redraw: false,
            background: [0, 0, 0],
            frames: 0,
            last_error: None,
            stats: FrameStats::default(),
        };
        pane.window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        pane
    }

    fn threaded(mode: EncodeMode, picker: &Picker) -> bool {
        match mode {
            EncodeMode::Inline => false,
            EncodeMode::Threaded => true,
            EncodeMode::Auto => picker.protocol_type() != ProtocolType::Halfblocks,
        }
    }

    /// The Slint component.
    pub fn component(&self) -> &C {
        &self.component
    }

    pub fn id(&self) -> PaneId {
        self.id
    }

    /// The cell area set by the last layout.
    pub fn area(&self) -> Rect {
        self.area
    }

    /// The physical pixel size Slint renders at, or `None` while the pane is hidden.
    pub fn pixel_size(&self) -> Option<(u32, u32)> {
        self.px
    }

    /// The scale factor currently applied to the Slint window.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// Terminal frames encoded so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Cell size of the frame currently on display, or `None` before the first frame.
    pub fn displayed_cells(&self) -> Option<Size> {
        self.displayed.as_ref().map(|(_, p)| p.size_for(ratatui_image::Resize::Fit(None), Size::new(u16::MAX, u16::MAX)))
    }

    /// Timings of the most recent rendered and encoded frame.
    pub fn stats(&self) -> FrameStats {
        self.stats
    }

    /// The last encoder error, if the most recent frame failed to encode.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// The protocol this pane encodes to.
    pub fn protocol_type(&self) -> ProtocolType {
        self.picker.protocol_type()
    }

    /// The raw framebuffer, row-major, at [`SlintPane::pixel_size`].
    pub fn framebuffer(&self) -> &[Pixel] {
        &self.pixels
    }

    /// Sets the scale policy. Takes effect immediately.
    pub fn set_scale(&mut self, scale: Scale) {
        self.scale = scale;
        if let Some(px) = self.px {
            self.apply_scale(px);
        }
    }

    /// Chooses where encoding runs. Pending frames from the old encoder are dropped.
    pub fn set_encode_mode(&mut self, mode: EncodeMode) {
        self.encode_mode = mode;
        self.encoder = Encoder::new(Self::threaded(mode, &self.picker), self.waker.clone());
        self.generation += 1;
        self.window.request_redraw();
    }

    /// Colour that transparent pixels are composited over for protocols without alpha.
    pub fn set_background(&mut self, rgb: [u8; 3]) {
        self.background = rgb;
        self.window.request_redraw();
    }

    /// Terminal cell size in pixels that the pane renders at.
    pub fn font_size(&self) -> FontSize {
        self.font
    }

    /// Changes the terminal cell size, for example after the user zoomed the terminal font.
    /// The pane re-renders at the new pixel size and pointer mapping follows it.
    pub fn set_font_size(&mut self, font: FontSize) {
        let same = (font.width, font.height) == (self.font.width, self.font.height);
        if same || font.width == 0 || font.height == 0 {
            return;
        }
        self.font = font;
        let area = self.area;
        self.area = Rect::default();
        self.px = None;
        self.set_area(area);
    }

    /// Moves or resizes the pane. A zero-sized or oversized area hides it.
    ///
    /// Moving without resizing keeps the encoded image; resizing re-renders and bumps the layout
    /// generation so frames still encoding at the old size are discarded.
    pub fn set_area(&mut self, area: Rect) {
        if area == self.area {
            return;
        }
        self.area = area;
        self.needs_redraw = true;
        let px = pixel_size(area, self.font);
        if px == self.px {
            return;
        }
        self.px = px;
        self.generation += 1;
        match px {
            Some(px) => {
                self.pixels = vec![Pixel::default(); px.0 as usize * px.1 as usize];
                self.apply_scale(px);
            }
            None => {
                self.pixels = Vec::new();
                self.displayed = None;
            }
        }
        // A layout change made outside the loop must still produce a frame.
        self.waker.wake();
    }

    fn apply_scale(&mut self, px: (u32, u32)) {
        let factor = self.scale.factor(px);
        if factor != self.scale_factor {
            self.scale_factor = factor;
            self.window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: factor });
        }
        self.window.set_size(PhysicalSize::new(px.0, px.1));
        self.window.request_redraw();
    }

    /// Renders if Slint marked the window dirty, and collects finished encodes.
    ///
    /// Returns `true` when the terminal needs to be redrawn: a new image is ready, or the pane
    /// moved. Call after [`crate::Runtime::begin_frame`] returned `true`.
    pub fn update(&mut self) -> bool {
        let Some((w, h)) = self.px else {
            return std::mem::take(&mut self.needs_redraw);
        };
        let pixels = &mut self.pixels;
        let start = std::time::Instant::now();
        let drawn = self.window.draw_if_needed(|renderer| {
            renderer.render(pixels, w as usize);
        });
        if drawn {
            self.stats.render = start.elapsed();
            let start = std::time::Instant::now();
            if let Some(image) = to_image(w, h, self.snapshot_bytes(), self.keeps_alpha()) {
                let protocol =
                    StatefulProtocol::new(image, self.font, self.picker_background(), self.template.clone());
                let size = Size::new(self.area.width, self.area.height);
                self.stats.snapshot = start.elapsed();
                self.encoder.submit(Job { generation: self.generation, size, protocol });
            }
        }
        if let Some(done) = self.encoder.take_latest(self.generation) {
            self.stats.encode = done.elapsed;
            self.last_error = done.error;
            if self.last_error.is_none() {
                self.displayed = Some((self.generation, done.protocol));
                self.frames += 1;
                self.needs_redraw = true;
            }
        }
        std::mem::take(&mut self.needs_redraw)
    }

    fn picker_background(&self) -> Option<image::Rgba<u8>> {
        let [r, g, b] = self.background;
        Some(image::Rgba([r, g, b, 255]))
    }

    /// Kitty and iTerm2 carry an alpha channel; half-blocks and sixel get composited.
    fn keeps_alpha(&self) -> bool {
        cfg!(feature = "transparency")
            && matches!(self.picker.protocol_type(), ProtocolType::Kitty | ProtocolType::Iterm2)
    }

    #[cfg(not(feature = "transparency"))]
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 3);
        for p in &self.pixels {
            out.extend_from_slice(&[p.r, p.g, p.b]);
        }
        out
    }

    #[cfg(feature = "transparency")]
    fn snapshot_bytes(&self) -> Vec<u8> {
        let alpha = self.keeps_alpha();
        let [br, bg, bb] = self.background;
        let mut out = Vec::with_capacity(self.pixels.len() * if alpha { 4 } else { 3 });
        for p in &self.pixels {
            if alpha {
                // Slint renders premultiplied alpha; image encoders expect straight alpha.
                let un = |c: u8| if p.alpha == 0 { 0 } else { ((c as u32 * 255) / p.alpha as u32).min(255) as u8 };
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

    /// Draws the most recent encoded image. Used by [`crate::SlintWidget`].
    fn render_into(&mut self, area: Rect, buf: &mut Buffer) {
        if area != self.area {
            // Laid out by the widget alone: adopt the area; the next frame renders at that size.
            self.set_area(area);
        }
        let Some((generation, protocol)) = &mut self.displayed else { return };
        if *generation != self.generation {
            // Encoded for an older size; only draw it if it still fits.
            let size = protocol.size_for(ratatui_image::Resize::Fit(None), Size::new(area.width, area.height));
            if size.width > area.width || size.height > area.height {
                return;
            }
        }
        protocol.render(area, buf);
    }
}

/// Lets [`crate::SlintWidget`] draw any pane regardless of its component type.
pub trait PaneRender {
    fn render_pane(&mut self, area: Rect, buf: &mut Buffer);
}

impl<C: ComponentHandle> PaneRender for SlintPane<C> {
    fn render_pane(&mut self, area: Rect, buf: &mut Buffer) {
        self.render_into(area, buf);
    }
}

impl<C: ComponentHandle> PaneInput for SlintPane<C> {
    fn id(&self) -> PaneId {
        self.id
    }
    fn area(&self) -> Rect {
        self.area
    }
    fn font_size(&self) -> FontSize {
        self.font
    }
    fn slint_window(&self) -> &slint::Window {
        self.component.window()
    }
}

impl<C: ComponentHandle> Drop for SlintPane<C> {
    fn drop(&mut self) {
        // `show()` stored a strong reference to the component in the window; only `hide()`
        // releases it. Without this the component and window outlive the pane.
        let _ = self.component.hide();
    }
}
