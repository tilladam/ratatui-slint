//! Spike: a Slint component rendered by Slint's software renderer into a
//! ratatui area, displayed via ratatui-image, with input routed back.
//!
//! `cargo run`              -> interactive (q quits, + grows bar, click counts)
//! `cargo run -- headless`  -> scripted run on ratatui's TestBackend, prints checks,
//!                             writes frame.png (raw Slint framebuffer)
//! `cargo run --release -- bench` -> per-protocol render/encode timing, 1200 x 600 px pane

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use image::{DynamicImage, RgbImage};
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Image, Resize};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::Rgb8Pixel;
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent, WindowEventDispatchResult};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, PlatformError, SharedString};

slint::slint! {
    export component Gauge inherits Window {
        in-out property <float> value: 0.2;
        in-out property <int> clicks: 0;
        background: #1e2630;
        fs := FocusScope {
            key-pressed(e) => {
                if (e.text == "+") { root.value = Math.min(1.0, root.value + 0.1); return accept; }
                if (e.text == "-") { root.value = Math.max(0.0, root.value - 0.1); return accept; }
                reject
            }
            TouchArea { clicked => { root.clicks += 1; } }
        }
        // continuously animated element: proves per-frame repaint
        Rectangle {
            width: 24px; height: 24px; y: 12px; border-radius: 6px;
            background: #4a90e2;
            x: (Math.mod(animation-tick() / 1ms, 2000) / 2000) * (root.width - self.width);
        }
        Text { x: 12px; y: 44px; text: "clicks: " + root.clicks; color: white; font-size: 16px; }
        // property animation: proves transitions run off our clock
        Rectangle {
            x: 0; y: root.height - 10px; height: 10px;
            width: root.width * root.value;
            background: @linear-gradient(90deg, #3bd16f 0%, #f5c542 100%);
            animate width { duration: 300ms; easing: ease-in-out; }
        }
        init => { fs.focus(); }
    }
}

/// Platform: every Slint window becomes a MinimalSoftwareWindow; the widget
/// that triggered creation picks it up from `last_created`.
struct TermPlatform {
    last_created: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for TermPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        *self.last_created.borrow_mut() = Some(w.clone());
        Ok(w)
    }
}

/// One Slint component living in a ratatui Rect.
struct SlintPane {
    window: Rc<MinimalSoftwareWindow>,
    buf: Vec<Rgb8Pixel>,
    px: (u32, u32),
    area: Rect,
    font: FontSize,
    protocol: Option<Protocol>,
    frames: u32,
}

impl SlintPane {
    fn new(window: Rc<MinimalSoftwareWindow>, font: FontSize) -> Self {
        Self { window, buf: Vec::new(), px: (0, 0), area: Rect::default(), font, protocol: None, frames: 0 }
    }

    /// Size the Slint window to exactly the pixels the cell area covers.
    fn layout(&mut self, area: Rect) {
        let px = (area.width as u32 * self.font.width as u32, area.height as u32 * self.font.height as u32);
        if px != self.px {
            self.px = px;
            self.buf = vec![Rgb8Pixel::default(); (px.0 * px.1) as usize];
            self.window.set_size(PhysicalSize::new(px.0, px.1));
            self.window.request_redraw();
        }
        self.area = area;
    }

    /// Render if Slint says something changed; re-encode for the terminal.
    fn update(&mut self, picker: &Picker) -> bool {
        let (w, h) = self.px;
        if w == 0 || h == 0 {
            return false;
        }
        let buf = &mut self.buf;
        let drawn = self.window.draw_if_needed(|r| {
            r.render(buf, w as usize);
        });
        if drawn {
            let raw: Vec<u8> = self.buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
            let img = DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, raw).unwrap());
            self.protocol = picker
                .new_protocol(img, Size::new(self.area.width, self.area.height), Resize::Fit(None))
                .ok();
            self.frames += 1;
        }
        drawn
    }

    fn contains(&self, col: u16, row: u16) -> bool {
        col >= self.area.x && col < self.area.right() && row >= self.area.y && row < self.area.bottom()
    }

    /// Cell -> logical pixel at the cell centre (scale factor 1 here).
    fn to_logical(&self, col: u16, row: u16) -> LogicalPosition {
        let sf = self.window.scale_factor();
        let x = ((col - self.area.x) as f32 + 0.5) * self.font.width as f32;
        let y = ((row - self.area.y) as f32 + 0.5) * self.font.height as f32;
        LogicalPosition::new(x / sf, y / sf)
    }

    fn click(&self, col: u16, row: u16) {
        let position = self.to_logical(col, row);
        let button = PointerEventButton::Left;
        self.window.dispatch_event(WindowEvent::PointerPressed { position, button });
        self.window.dispatch_event(WindowEvent::PointerReleased { position, button });
    }

    /// Returns whether Slint consumed the key; unconsumed keys fall back to ratatui.
    fn key(&self, text: SharedString) -> bool {
        let r = self.window.dispatch_event_with_result(WindowEvent::KeyPressed { text: text.clone() });
        let _ = self.window.dispatch_event_with_result(WindowEvent::KeyReleased { text });
        matches!(r, Ok(WindowEventDispatchResult::Accepted))
    }
}

fn map_key(code: crossterm::event::KeyCode) -> Option<SharedString> {
    use crossterm::event::KeyCode as K;
    Some(match code {
        K::Char(c) => c.to_string().into(),
        K::Enter => Key::Return.into(),
        K::Tab => Key::Tab.into(),
        K::Backspace => Key::Backspace.into(),
        K::Esc => Key::Escape.into(),
        K::Left => Key::LeftArrow.into(),
        K::Right => Key::RightArrow.into(),
        K::Up => Key::UpArrow.into(),
        K::Down => Key::DownArrow.into(),
        _ => return None,
    })
}

fn draw(f: &mut ratatui::Frame, pane: &mut SlintPane, status: &str) {
    let [top, bottom] = Layout::vertical([Constraint::Min(6), Constraint::Length(3)]).areas(f.area());
    let block = Block::default().borders(Borders::ALL).title(" Slint inside ratatui ");
    let inner = block.inner(top);
    f.render_widget(block, top);
    pane.layout(inner);
    if let Some(p) = &pane.protocol {
        f.render_widget(Image::new(p), inner);
    }
    f.render_widget(Paragraph::new(status.to_string()).block(Block::default().borders(Borders::ALL)), bottom);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|a| a == "bench") { bench(); return Ok(()); }
    let headless = std::env::args().any(|a| a == "headless");
    let last = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TermPlatform { last_created: last.clone() }))
        .map_err(|e| format!("{e:?}"))?;

    let gauge = Gauge::new()?;
    gauge.show()?;
    let window = last.borrow_mut().take().expect("window adapter created");

    if headless { headless_run(gauge, window) } else { interactive_run(gauge, window) }
}

fn headless_run(gauge: Gauge, window: Rc<MinimalSoftwareWindow>) -> Result<(), Box<dyn std::error::Error>> {
    let picker = Picker::halfblocks(); // 10x20 px per cell, no terminal query
    let mut pane = SlintPane::new(window, picker.font_size());
    let mut term = Terminal::new(ratatui::backend::TestBackend::new(40, 14))?;
    let mut snapshots = Vec::new();
    for i in 0..6 {
        slint::platform::update_timers_and_animations();
        term.draw(|f| draw(f, &mut pane, "headless"))?;
        pane.update(&picker);
        term.draw(|f| draw(f, &mut pane, "headless"))?;
        snapshots.push(format!("{:?}", term.backend().buffer()));
        if i == 1 {
            pane.click(pane.area.x + 2, pane.area.y + 2);
            pane.click(pane.area.x + 2, pane.area.y + 2);
            let consumed_plus = pane.key("+".into());
            let consumed_x = pane.key("x".into());
            println!("key '+' consumed by Slint: {consumed_plus}; key 'x' consumed by Slint: {consumed_x}");
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    // Dump the raw Slint framebuffer to check text/gradient rendering.
    let raw: Vec<u8> = pane.buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    RgbImage::from_raw(pane.px.0, pane.px.1, raw).unwrap().save("frame.png")?;
    let distinct = snapshots.windows(2).filter(|w| w[0] != w[1]).count();
    println!("pane: {} x {} cells -> {} x {} px", pane.area.width, pane.area.height, pane.px.0, pane.px.1);
    println!("frames rendered by Slint: {}", pane.frames);
    println!("consecutive terminal snapshots that differ: {distinct} of {}", snapshots.len() - 1);
    println!("clicks property = {}, value property = {:.1}", gauge.get_clicks(), gauge.get_value());
    Ok(())
}

fn interactive_run(gauge: Gauge, window: Rc<MinimalSoftwareWindow>) -> Result<(), Box<dyn std::error::Error>> {
    use crossterm::event::{self, Event, KeyCode, MouseButton, MouseEventKind};
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    let mut pane = SlintPane::new(window, picker.font_size());
    let started = Instant::now();
    let res = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            slint::platform::update_timers_and_animations();
            terminal.draw(|f| draw(f, &mut pane, ""))?; // lays out the pane
            if pane.update(&picker) {
                let fps = pane.frames as f32 / started.elapsed().as_secs_f32();
                let status = format!("{:?} | {fps:.1} fps | clicks {} | +/- bar, q quit", picker.protocol_type(), gauge.get_clicks());
                terminal.draw(|f| draw(f, &mut pane, &status))?;
            }
            let wait = slint::platform::duration_until_next_timer_update()
                .unwrap_or(Duration::from_millis(33))
                .min(Duration::from_millis(33));
            if event::poll(wait)? {
                match event::read()? {
                    Event::Key(k) if k.code == KeyCode::Char('q') => break,
                    Event::Key(k) => {
                        if let Some(t) = map_key(k.code) {
                            pane.key(t);
                        }
                    }
                    Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) && pane.contains(m.column, m.row) => {
                        pane.click(m.column, m.row)
                    }
                    Event::Resize(..) => pane.window.request_redraw(),
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
    ratatui::restore();
    res
}

#[allow(deprecated)]
pub fn bench() {
    use ratatui_image::picker::ProtocolType;
    let last = Rc::new(RefCell::new(None));
    let _ = slint::platform::set_platform(Box::new(TermPlatform { last_created: last.clone() }));
    let g = Gauge::new().unwrap();
    g.show().unwrap();
    let window = last.borrow_mut().take().unwrap();
    let font = FontSize::new(10, 20);
    let area = Rect::new(0, 0, 120, 30); // 1200 x 600 px
    for proto in [ProtocolType::Halfblocks, ProtocolType::Sixel, ProtocolType::Kitty] {
        let mut picker = Picker::from_fontsize(font);
        picker.set_protocol_type(proto);
        let mut pane = SlintPane::new(window.clone(), font);
        pane.layout(area);
        let n = 30;
        let (mut t_render, mut t_encode, mut bytes) = (Duration::ZERO, Duration::ZERO, 0usize);
        for _ in 0..n {
            std::thread::sleep(Duration::from_millis(17));
            slint::platform::update_timers_and_animations();
            let t0 = Instant::now();
            let buf = &mut pane.buf;
            window.draw_if_needed(|r| { r.render(buf, 1200); });
            let t1 = Instant::now();
            let raw: Vec<u8> = pane.buf.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
            let img = DynamicImage::ImageRgb8(RgbImage::from_raw(1200, 600, raw).unwrap());
            let p = picker.new_protocol(img, Size::new(120, 30), Resize::Fit(None)).unwrap();
            let mut tb = ratatui::buffer::Buffer::empty(area);
            use ratatui::widgets::Widget;
            Image::new(&p).render(area, &mut tb);
            let t2 = Instant::now();
            bytes += tb.content.iter().map(|c| c.symbol().len()).sum::<usize>();
            t_render += t1 - t0;
            t_encode += t2 - t1;
        }
        println!("{proto:?}: render {:.2} ms, encode+widget {:.2} ms, ~{} KB/frame escape payload",
            t_render.as_secs_f64() * 1e3 / n as f64, t_encode.as_secs_f64() * 1e3 / n as f64, bytes / n / 1024);
    }
}
