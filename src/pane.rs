//! One Slint component living in a ratatui `Rect`.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui_image::FontSize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocolType;
use slint::platform::WindowEvent;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::{ComponentHandle, PhysicalSize};

use crate::encode::{Displayed, EncodeMode, Encoder, Frame, Job, Pixel};
use crate::platform::Waker;

/// Process-wide kitty image id counter, seeded so concurrent programs rarely collide.
static NEXT_KITTY_ID: AtomicU64 = AtomicU64::new(0);

fn next_kitty_id() -> u32 {
    let _ = NEXT_KITTY_ID.compare_exchange(0, seed(), Ordering::Relaxed, Ordering::Relaxed);
    loop {
        let id = NEXT_KITTY_ID.fetch_add(1, Ordering::Relaxed) as u32;
        if id != 0 {
            return id;
        }
    }
}

fn seed() -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    (t ^ (u64::from(std::process::id()) << 20)) | 1
}

/// The kitty delete command that frees an image and its data.
fn kitty_delete(id: u32, tmux: bool) -> String {
    let seq = format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
    crate::output::passthrough(seq, tmux)
}

/// Largest pane edge in physical pixels. Larger areas are treated as hidden instead of
/// allocating an unbounded framebuffer.
pub const MAX_PANE_PIXELS: u32 = 8192;

/// Identifies a pane for input routing and focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Logical pixels per terminal text row assumed by [`Scale::Auto`]. A heuristic: about the line
/// height of 12–14 pt text at 1×.
pub const AUTO_ROW_HEIGHT: f32 = 16.0;

/// How logical Slint pixels map to the physical pixels a pane covers.
///
/// The terminal font fixes the physical size (cells × cell pixels). The scale factor is a separate
/// choice, applied with `WindowEvent::ScaleFactorChanged`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum Scale {
    /// The default. Factor = cell height ÷ [`AUTO_ROW_HEIGHT`], so Slint text sizes look like
    /// terminal text: about 1.0 on a 1× display, about 2.0 on Retina, where terminals report cell
    /// sizes in device pixels. Follows font zoom through [`SlintPane::set_font_size`].
    #[default]
    Auto,
    /// A fixed factor. `Fixed(1.0)` makes one logical pixel one terminal pixel.
    Fixed(f32),
    /// Scale a design of this logical size so it fits the pane, keeping its aspect ratio.
    /// Suits UIs laid out for a fixed screen, such as an embedded touch panel.
    Fit {
        /// Design width in logical pixels.
        width: f32,
        /// Design height in logical pixels.
        height: f32,
    },
}

impl Scale {
    /// `RATATUI_SLINT_SCALE`: `auto` or a number for a fixed factor. Replaces the default only.
    fn from_env() -> Option<Self> {
        let v = std::env::var("RATATUI_SLINT_SCALE").ok()?;
        match v.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Scale::Auto),
            n => n
                .parse::<f32>()
                .ok()
                .filter(|f| f.is_finite() && *f > 0.0)
                .map(Scale::Fixed),
        }
    }

    fn factor(self, px: (u32, u32), font: FontSize) -> f32 {
        let f = match self {
            Scale::Auto => f32::from(font.height) / AUTO_ROW_HEIGHT,
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
#[non_exhaustive]
pub struct FrameStats {
    /// Slint software rendering into the framebuffer.
    pub render: std::time::Duration,
    /// Framebuffer copy handed to the encoder, on the UI thread.
    pub snapshot: std::time::Duration,
    /// Image conversion and protocol encoding, on whichever thread ran it.
    pub encode: std::time::Duration,
}

/// The object-safe view of a pane that the input router needs.
/// Implementations may wrap a SlintPane, preserving its identity and window. Areas and cell
/// sizes must describe that window accurately, and identities must be unique among routed panes.
pub trait PaneInput {
    /// The pane's identity, stable for its lifetime.
    fn id(&self) -> PaneId;
    /// The cell area the pane occupies on screen.
    fn area(&self) -> Rect;
    /// Terminal cell size in pixels.
    fn font_size(&self) -> FontSize;
    /// The Slint window that receives the pane's events.
    fn slint_window(&self) -> &slint::Window;
}

/// Borrowed row-major framebuffer storage, with dimensions from SlintPane::pixel_size().
/// The type and variants are available in every feature configuration. Alpha values, when
/// present, are premultiplied; output may still be opaque according to the pane's policy.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Framebuffer<'a> {
    /// Opaque RGB storage.
    Rgb(&'a [slint::Rgb8Pixel]),
    /// Premultiplied RGBA storage.
    PremultipliedRgba(&'a [slint::platform::software_renderer::PremultipliedRgbaColor]),
}

impl<'a> Framebuffer<'a> {
    /// Number of physical pixels.
    pub fn len(self) -> usize {
        match self {
            Self::Rgb(p) => p.len(),
            Self::PremultipliedRgba(p) => p.len(),
        }
    }
    /// Whether the pane has no framebuffer (before layout or while hidden).
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
    /// RGB storage, if this build renders without an alpha channel.
    pub fn rgb(self) -> Option<&'a [slint::Rgb8Pixel]> {
        match self {
            Self::Rgb(p) => Some(p),
            _ => None,
        }
    }
    /// RGBA storage, if this build renders with an alpha channel.
    pub fn premultiplied_rgba(
        self,
    ) -> Option<&'a [slint::platform::software_renderer::PremultipliedRgbaColor]> {
        match self {
            Self::PremultipliedRgba(p) => Some(p),
            _ => None,
        }
    }
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
    waker: Waker,
    displayed: Option<(u64, Displayed)>,
    needs_redraw: bool,
    background: [u8; 3],
    frames: u64,
    last_error: Option<String>,
    stats: FrameStats,
    transparent: bool,
    damage: bool,
    damage_pending: bool,
    reencode: bool,
    /// Kitty id of the frame on display, and ids of replaced frames awaiting deletion.
    shown_kitty_id: Option<u32>,
    retired_kitty_ids: Vec<u32>,
    /// A sixel pane on the terminal's last row leaves that row out of its image: the terminal
    /// moves the cursor below a sixel image, which on the last row scrolls the whole screen.
    sixel_skips_last_row: bool,
}

impl<C: ComponentHandle> SlintPane<C> {
    pub(crate) fn new(
        component: C,
        window: Rc<MinimalSoftwareWindow>,
        picker: Picker,
        waker: Waker,
    ) -> Result<Self, crate::Error> {
        let font = picker.font_size();
        // Other protocols reuse this template; Kitty gets a fresh ID for each encoding.
        let template = picker
            .new_resize_protocol(image::DynamicImage::new_rgb8(1, 1))
            .protocol_type_owned();
        let encode_mode = EncodeMode::Auto;
        let encoder = Encoder::new(encode_mode.threaded(picker.protocol_type()), waker.clone())
            .map_err(|error| {
                let _ = component.hide();
                crate::Error::from(error)
            })?;
        let pane = Self {
            component,
            window,
            id: PaneId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            picker,
            font,
            scale: Scale::from_env().unwrap_or_default(),
            scale_factor: 1.0,
            area: Rect::default(),
            px: None,
            pixels: Vec::new(),
            generation: 0,
            template,
            encoder,
            waker,
            displayed: None,
            needs_redraw: false,
            background: [0, 0, 0],
            frames: 0,
            last_error: None,
            stats: FrameStats::default(),
            transparent: false,
            damage: false,
            damage_pending: false,
            reencode: false,
            shown_kitty_id: None,
            retired_kitty_ids: Vec::new(),
            sixel_skips_last_row: false,
        };
        pane.window
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        Ok(pane)
    }

    /// The Slint component.
    pub fn component(&self) -> &C {
        &self.component
    }

    /// The pane's identity, stable for its lifetime.
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
        self.displayed.as_ref().map(|(_, p)| p.cells())
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
    pub fn framebuffer(&self) -> Framebuffer<'_> {
        #[cfg(not(feature = "transparency"))]
        {
            Framebuffer::Rgb(&self.pixels)
        }
        #[cfg(feature = "transparency")]
        {
            Framebuffer::PremultipliedRgba(&self.pixels)
        }
    }

    /// Sets the scale policy. Takes effect immediately.
    pub fn set_scale(&mut self, scale: Scale) {
        if self.scale == scale {
            return;
        }
        self.scale = scale;
        if let Some(px) = self.px {
            self.apply_scale(px);
            self.waker.wake();
        }
    }

    /// Chooses where encoding runs. Pending frames from the old encoder are dropped.
    pub fn set_encode_mode(&mut self, mode: EncodeMode) -> Result<(), crate::Error> {
        let encoder = Encoder::new(
            mode.threaded(self.picker.protocol_type()),
            self.waker.clone(),
        )?;
        self.encoder = encoder;
        self.generation += 1;
        self.window.request_redraw();
        self.waker.wake();
        Ok(())
    }

    /// Opts into alpha output for Kitty/iTerm2. Opaque output is the default even when another
    /// dependency enables the Cargo feature. Returns an error if the capability is unavailable.
    pub fn set_transparency(&mut self, enabled: bool) -> Result<(), crate::Error> {
        if enabled && !cfg!(feature = "transparency") {
            return Err(crate::Error::TransparencyUnavailable);
        }
        if self.transparent != enabled {
            self.transparent = enabled;
            self.generation += 1;
            self.window.request_redraw();
            self.waker.wake();
        }
        Ok(())
    }

    /// Invalidates terminal image contents after host damage (for example removing an overlay).
    /// Wakes the runtime and makes update() request a draw even when Slint has not changed.
    /// Sixel/iTerm2 reuse encoded bytes. Kitty reencodes retained pixels under a fresh image ID
    /// to recover even after terminal image storage was cleared. Half-blocks use normal diffing.
    pub fn invalidate(&mut self) {
        self.damage_pending = true;
        self.needs_redraw = true;
        if self.picker.protocol_type() == ProtocolType::Kitty {
            self.reencode = true;
        }
        self.waker.wake();
    }

    /// Colour that transparent pixels are composited over for protocols without alpha.
    pub fn set_background(&mut self, rgb: [u8; 3]) {
        if self.background == rgb {
            return;
        }
        self.background = rgb;
        self.window.request_redraw();
        self.waker.wake();
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
        // Moving needs a terminal frame even when its pixel size and encoded image are unchanged.
        self.waker.wake();
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
    }

    fn apply_scale(&mut self, px: (u32, u32)) {
        let factor = self.scale.factor(px, self.font);
        if factor != self.scale_factor {
            self.scale_factor = factor;
            self.window.dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: factor,
            });
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
        }
        let reencode = std::mem::take(&mut self.reencode);
        if drawn || reencode {
            // Only a copy of the framebuffer happens here; conversion, hashing and encoding run
            // in the encoder (on its worker thread unless the mode is inline).
            let start = std::time::Instant::now();
            let (protocol_type, kitty_id) = self.frame_protocol_type();
            // Leave out the last cell row when a sixel image would otherwise end on the
            // terminal's last row.
            let cut_rows = u16::from(self.sixel_skips_last_row && self.area.height > 1);
            let height = h - u32::from(cut_rows) * u32::from(self.font.height);
            let frame = Frame {
                pixels: self.pixels[..w as usize * height as usize].to_vec(),
                width: w,
                height,
                keep_alpha: self.keeps_alpha(),
                background: self.background,
                font: self.font,
                protocol_type,
            };
            let size = Size::new(self.area.width, self.area.height - cut_rows);
            self.stats.snapshot = start.elapsed();
            if let Err(error) = self.encoder.submit(Job {
                generation: self.generation,
                size,
                frame,
                kitty_id,
            }) {
                self.record_error(error);
            }
        }
        let latest = match self.encoder.take_latest(self.generation) {
            Ok(done) => done,
            Err(error) => {
                self.record_error(error);
                None
            }
        };
        if let Some(done) = latest {
            self.stats.encode = done.elapsed;
            self.last_error = done.error;
            if self.last_error.is_none() {
                if done.kitty_id != self.shown_kitty_id
                    && let Some(old) = self.shown_kitty_id
                {
                    self.retired_kitty_ids.push(old);
                }
                self.shown_kitty_id = done.kitty_id;
                self.displayed = Some((self.generation, done.protocol));
                self.frames += 1;
                self.needs_redraw = true;
            }
        }
        std::mem::take(&mut self.needs_redraw)
    }

    fn record_error(&mut self, error: String) {
        if self.last_error.as_ref() != Some(&error) {
            self.last_error = Some(error);
            self.needs_redraw = true;
        }
    }

    /// Kitty panes always get a fresh ID; replacing an existing ID flickers on real terminals.
    fn frame_protocol_type(&self) -> (StatefulProtocolType, Option<u32>) {
        match &self.template {
            StatefulProtocolType::Kitty(_) => {
                let id = next_kitty_id();
                let compress = self
                    .picker
                    .capabilities()
                    .contains(&ratatui_image::picker::Capability::KittyCompression);
                let kitty = ratatui_image::protocol::kitty::StatefulKitty::new(
                    id,
                    self.picker.tmux_detected(),
                    compress,
                );
                (StatefulProtocolType::Kitty(kitty), Some(id))
            }
            _ => (self.template.clone(), None),
        }
    }

    /// Kitty and iTerm2 carry an alpha channel; half-blocks and sixel get composited.
    fn keeps_alpha(&self) -> bool {
        self.transparent
            && cfg!(feature = "transparency")
            && matches!(
                self.picker.protocol_type(),
                ProtocolType::Kitty | ProtocolType::Iterm2
            )
    }

    /// Draws the most recent encoded image. Used by [`crate::SlintWidget`].
    fn render_into(&mut self, area: Rect, buf: &mut Buffer) {
        if area != self.area {
            // Laid out by the widget alone: adopt the area; the next frame renders at that size.
            self.set_area(area);
        }
        // Only the widget sees the whole screen. A sixel image ending on its last row would scroll
        // it, so such a pane drops that row: re-render shorter, and draw nothing until then.
        let at_bottom = self.picker.protocol_type() == ProtocolType::Sixel
            && area.bottom() == buf.area.bottom();
        if at_bottom != self.sixel_skips_last_row {
            self.sixel_skips_last_row = at_bottom;
            self.generation += 1;
            self.window.request_redraw();
            self.waker.wake();
        }
        let Some((generation, protocol)) = &mut self.displayed else {
            return;
        };
        if at_bottom && protocol.cells().height >= area.height {
            return;
        }
        if *generation != self.generation {
            // Encoded for an older size; only draw it if it still fits.
            let size = protocol.cells();
            if size.width > area.width || size.height > area.height {
                return;
            }
        }
        if area.intersection(buf.area).is_empty() {
            return;
        }
        if std::mem::take(&mut self.damage_pending) {
            self.damage = !self.damage;
        }
        protocol.render(area, buf, self.damage);
        // The new frame is in this draw; images it replaced can go once the draw is written.
        for id in self.retired_kitty_ids.drain(..) {
            crate::platform::queue_output(kitty_delete(id, self.picker.tmux_detected()).as_bytes());
        }
    }
}

/// Lets [`crate::SlintWidget`] draw any pane regardless of its component type.
/// Wrappers may implement this to customize composition; keep rendering on the Runtime thread
/// and preserve the pending-output flush after the terminal draw.
pub trait PaneRender {
    /// Renders the pane into `area` of `buf`.
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
        for id in self.retired_kitty_ids.drain(..) {
            crate::platform::queue_output(kitty_delete(id, self.picker.tmux_detected()).as_bytes());
        }
        if let Some(id) = self.shown_kitty_id {
            crate::platform::queue_output(kitty_delete(id, self.picker.tmux_detected()).as_bytes());
        }
    }
}
