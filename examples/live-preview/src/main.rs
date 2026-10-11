//! Hot-reloading `.slint` preview in a terminal pane, for designing embedded UIs next to an
//! editor such as Helix or Neovim (in a terminal split or tmux pane).
//!
//! `cargo run --release -p live-preview -- path/to/ui.slint [ComponentName]`
//!
//! Without a path it previews the bundled `demo.slint`. The file and every `.slint` file next to
//! it are watched; on save the UI is recompiled with `slint-interpreter` and swapped in. Compile
//! errors keep the last good UI on screen and show the diagnostics.
//!
//! Keys (when the preview is not focused): `r` reloads, `s` cycles the scale (1×, 2×, fit to
//! 800 × 480), `q` quits. Click the preview to interact with it; Ctrl+G leaves it.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui_slint::terminal::{TerminalOptions, TerminalSession};
use ratatui_slint::{InputRouter, LoopEvent, PaneInput, Routed, Runtime, Scale, SlintPane, SlintWidget};
use slint_interpreter::{ComponentDefinition, ComponentInstance, DiagnosticLevel};

/// The interpreter's compile future does no real I/O waiting; poll it to completion.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

struct Build {
    definition: Option<ComponentDefinition>,
    diagnostics: Vec<(DiagnosticLevel, String)>,
    took: Duration,
}

fn compile(path: &Path, component: Option<&str>) -> Build {
    let start = Instant::now();
    let compiler = slint_interpreter::Compiler::default();
    let result = block_on(compiler.build_from_path(path));
    let diagnostics = result
        .diagnostics()
        .map(|d| {
            let (line, col) = d.line_column();
            let file = d.source_file().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            (d.level(), format!("{file}:{line}:{col}: {}", d.message()))
        })
        .collect();
    let definition = match component {
        Some(name) => result.component(name),
        None => result.component_names().last().and_then(|n| result.component(n)),
    };
    Build { definition, diagnostics, took: start.elapsed() }
}

/// Newest modification time of the watched `.slint` files.
fn newest_mtime(dir: &Path, file: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::metadata(file).and_then(|m| m.modified()).ok();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "slint")
                && let Ok(t) = e.metadata().and_then(|m| m.modified())
            {
                newest = newest.max(Some(t));
            }
        }
    }
    newest
}

/// Instantiates the built component in a new pane, replacing the old one. Returns an error text.
fn load(
    rt: &Runtime,
    picker: &ratatui_slint::ratatui_image::picker::Picker,
    build: &Build,
    pane: &mut Option<SlintPane<ComponentInstance>>,
    scale: Scale,
) -> Option<String> {
    let def = build.definition.as_ref()?;
    match rt.create_pane(picker, || def.create()) {
        Ok(mut p) => {
            p.set_scale(scale);
            *pane = Some(p); // the old pane is dropped and its window hidden
            None
        }
        Err(e) => Some(e.to_string()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path: PathBuf = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("demo.slint"));
    let component_name = args.next();
    let path = path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    let mut rt = Runtime::new()?;
    let mut session = TerminalSession::init(TerminalOptions::default())?;
    rt.spawn_terminal_reader();
    let picker = session.picker.clone();
    let mut router = InputRouter::new();
    router.set_keyboard_enhanced(session.keyboard_enhanced);

    // File watcher: polls mtimes, wakes the loop on change.
    let (tx, rx) = mpsc::channel::<()>();
    {
        let waker = rt.waker();
        let (dir, path) = (dir.clone(), path.clone());
        std::thread::spawn(move || {
            let mut last = newest_mtime(&dir, &path);
            loop {
                std::thread::sleep(Duration::from_millis(300));
                let now = newest_mtime(&dir, &path);
                if now != last {
                    last = now;
                    if tx.send(()).is_err() || !waker.wake() {
                        break;
                    }
                }
            }
        });
    }

    let scales = [Scale::Fixed(1.0), Scale::Fixed(2.0), Scale::Fit { width: 800.0, height: 480.0 }];
    let mut scale_index = 0;
    let mut pane: Option<SlintPane<ComponentInstance>> = None;
    let mut build = compile(&path, component_name.as_deref());
    let mut reloads = 0u32;
    let mut load_error = load(&rt, &picker, &build, &mut pane, scales[scale_index]);
    // The definition of the UI on screen; a failed rebuild keeps it.
    let mut shown = build.definition.clone();
    let mut dirty = true;

    let res: Result<(), Box<dyn std::error::Error>> = (|| loop {
        let event = rt.next_event();
        let mut reload = rx.try_iter().count() > 0;
        match event {
            LoopEvent::Quit => return Ok(()),
            LoopEvent::Terminal(ev) => {
                dirty = true;
                if let Event::Resize(..) = ev
                    && picker.protocol_type() != ratatui_slint::ratatui_image::picker::ProtocolType::Halfblocks
                    && let Some(font) = ratatui_slint::terminal::cell_size()
                    && let Some(p) = pane.as_mut()
                {
                    p.set_font_size(font); // font zoom changes pixels per cell
                }
                let routed = match &pane {
                    Some(p) => router.route(&ev, &[p as &dyn PaneInput]),
                    None => Routed::Unhandled,
                };
                if routed == Routed::Unhandled
                    && let Event::Key(k) = ev
                    && k.kind != KeyEventKind::Release
                {
                    match k.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('r') => reload = true,
                        KeyCode::Char('s') => {
                            scale_index = (scale_index + 1) % scales.len();
                            if let Some(p) = pane.as_mut() {
                                p.set_scale(scales[scale_index]);
                            }
                        }
                        _ => {}
                    }
                }
            }
            LoopEvent::Wake | LoopEvent::Timeout => {}
        }
        if reload {
            build = compile(&path, component_name.as_deref());
            reloads += 1;
            dirty = true;
            if build.definition.is_some() {
                load_error = load(&rt, &picker, &build, &mut pane, scales[scale_index]);
                if load_error.is_none() {
                    shown = build.definition.clone();
                }
            }
        }
        if !rt.begin_frame() {
            continue;
        }
        let size = session.terminal.size()?;
        let [main, status] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(Rect::from(size));
        let [left, right] = Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)]).areas(main);
        let block = Block::default().borders(Borders::ALL).title(format!(
            " {} ",
            shown.as_ref().map(|d| d.name().to_string()).unwrap_or_else(|| "no component".into())
        ));
        let inner = block.inner(left);
        let mut changed = false;
        if let Some(p) = pane.as_mut() {
            p.set_area(inner);
            changed = p.update();
        }
        if !(changed || std::mem::take(&mut dirty)) {
            continue;
        }

        let mut info: Vec<Line> = vec![
            Line::from(vec![Span::styled("file  ", Style::default().fg(Color::Gray)), Span::raw(path.display().to_string())]),
            Line::from(vec![
                Span::styled("build ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{:.0} ms, reload #{reloads}", build.took.as_secs_f64() * 1e3)),
            ]),
            Line::from(vec![Span::styled("scale ", Style::default().fg(Color::Gray)), Span::raw(format!("{:?}", scales[scale_index]))]),
            Line::from(""),
        ];
        if let Some(e) = &load_error {
            info.push(Line::styled(e.clone(), Style::default().fg(Color::Red)));
        }
        if build.diagnostics.is_empty() {
            info.push(Line::styled("no diagnostics", Style::default().fg(Color::Green)));
        }
        for (level, msg) in &build.diagnostics {
            let color = if *level == DiagnosticLevel::Error { Color::Red } else { Color::Yellow };
            info.push(Line::styled(msg.clone(), Style::default().fg(color)));
        }
        if build.definition.is_none() && shown.is_some() {
            info.push(Line::styled("build failed: showing the last good UI", Style::default().fg(Color::Yellow)));
        }
        if let (Some(def), Some(p)) = (&shown, &pane) {
            info.push(Line::from(""));
            info.push(Line::styled("properties", Style::default().add_modifier(Modifier::BOLD)));
            for (name, _) in def.properties() {
                let value = p.component().get_property(&name).map(|v| format!("{v:?}")).unwrap_or_default();
                info.push(Line::from(format!("{name} = {value}")));
            }
        }
        let focus = if router.focused().is_some() { "preview focused (Ctrl+G leaves)" } else { "click preview to use it" };
        let status_line = format!(" {:?} | {focus} | r reload, s scale, q quit", picker.protocol_type());
        session.terminal.draw(|f| {
            f.render_widget(block, left);
            if let Some(p) = pane.as_mut() {
                f.render_stateful_widget(SlintWidget, inner, p);
            }
            f.render_widget(
                Paragraph::new(info).wrap(Wrap { trim: false }).block(Block::default().borders(Borders::ALL).title(" Build ")),
                right,
            );
            f.render_widget(Paragraph::new(status_line).style(Style::default().fg(Color::Gray)), status);
        })?;
        rt.write_pending_output(session.terminal.backend_mut())?;
    })();
    drop(pane);
    session.restore()?;
    res
}
