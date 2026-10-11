//! btop-style system monitor: a ratatui process table beside a Slint pane with animated per-core
//! gauges, a memory gauge and a CPU history graph.
//!
//! `cargo run --release --example sysmon`
//!
//! Keys: `q` quits, `p` pauses sampling (watch the bytes/s counter drop to zero while idle),
//! Up/Down scroll the process table. Click the Slint pane to focus it; Ctrl+G leaves it.
//! Set `RATATUI_SLINT_PROTOCOL=halfblocks|sixel|kitty|iterm2` to force a protocol.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell as TCell, Paragraph, Row, Table, TableState};
use ratatui_slint::terminal::{TerminalOptions, TerminalSession};
use ratatui_slint::{InputRouter, LoopEvent, PaneInput, Routed, Runtime, SlintWidget};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use sysinfo::{ProcessesToUpdate, System};

slint::slint! {
    export component Monitor inherits Window {
        in property <[float]> cores;
        in property <float> mem;
        in property <string> mem-text;
        in property <float> cpu;
        in property <string> history-line;
        in property <string> history-fill;
        in property <bool> paused;
        background: #0f1318;

        VerticalLayout {
            padding: 10px;
            spacing: 8px;
            HorizontalLayout {
                Text { text: "CPU " + Math.round(root.cpu * 100) + "%"; color: #c8d3dc; font-size: 16px; }
                Rectangle {}
                Text { text: root.paused ? "paused" : "live"; color: root.paused ? #f5c542 : #3bd16f; font-size: 14px; }
            }
            HorizontalLayout {
                spacing: 3px;
                vertical-stretch: 2;
                for usage in root.cores: Rectangle {
                    background: #1b222a;
                    border-radius: 3px;
                    Rectangle {
                        y: parent.height - self.height;
                        height: parent.height * usage;
                        border-radius: 3px;
                        background: usage > 0.85 ? #e5534b : usage > 0.5 ? #f5c542 : #3bd16f;
                        animate height, background { duration: 450ms; easing: ease-out; }
                    }
                }
            }
            Text { text: "Memory " + root.mem-text; color: #c8d3dc; font-size: 14px; }
            Rectangle {
                height: 14px;
                background: #1b222a;
                border-radius: 7px;
                Rectangle {
                    x: 0;
                    width: parent.width * root.mem;
                    border-radius: 7px;
                    background: @linear-gradient(90deg, #4a90e2 0%, #a371f7 100%);
                    animate width { duration: 450ms; easing: ease-out; }
                }
            }
            Text { text: "CPU history (60 s)"; color: #c8d3dc; font-size: 14px; }
            Rectangle {
                vertical-stretch: 2;
                background: #141a20;
                border-radius: 4px;
                Path {
                    width: 100%; height: 100%;
                    viewbox-width: 100; viewbox-height: 100;
                    commands: root.history-fill;
                    fill: #4a90e233;
                }
                Path {
                    width: 100%; height: 100%;
                    viewbox-width: 100; viewbox-height: 100;
                    commands: root.history-line;
                    stroke: #4a90e2;
                    stroke-width: 2px;
                }
            }
        }
    }
}

const HISTORY: usize = 60;

struct Proc {
    pid: u32,
    name: String,
    cpu: f32,
    mem_mb: u64,
}

struct Sampler {
    sys: System,
    history: VecDeque<f32>,
    procs: Vec<Proc>,
}

impl Sampler {
    fn sample(&mut self, ui: &Monitor) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys.refresh_processes(ProcessesToUpdate::All, true);

        let cores: Vec<f32> = self.sys.cpus().iter().map(|c| c.cpu_usage() / 100.0).collect();
        let avg = cores.iter().sum::<f32>() / cores.len().max(1) as f32;
        let model = ui.get_cores();
        if model.row_count() == cores.len() {
            for (i, v) in cores.iter().enumerate() {
                model.set_row_data(i, *v);
            }
        } else {
            ui.set_cores(ModelRc::new(VecModel::from(cores)));
        }
        ui.set_cpu(avg);

        let (used, total) = (self.sys.used_memory(), self.sys.total_memory().max(1));
        ui.set_mem(used as f32 / total as f32);
        ui.set_mem_text(format!("{:.1} / {:.1} GiB", gib(used), gib(total)).into());

        self.history.push_back(avg);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        let (line, fill) = history_paths(&self.history);
        ui.set_history_line(line);
        ui.set_history_fill(fill);

        let mut procs: Vec<Proc> = self
            .sys
            .processes()
            .values()
            .map(|p| Proc {
                pid: p.pid().as_u32(),
                name: p.name().to_string_lossy().into_owned(),
                cpu: p.cpu_usage(),
                mem_mb: p.memory() / (1024 * 1024),
            })
            .collect();
        procs.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
        self.procs = procs;
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// SVG path commands in a 100 × 100 viewbox: the history line and the area under it.
fn history_paths(history: &VecDeque<f32>) -> (SharedString, SharedString) {
    if history.len() < 2 {
        return (SharedString::new(), SharedString::new());
    }
    let step = 100.0 / (HISTORY - 1) as f32;
    let x0 = 100.0 - step * (history.len() - 1) as f32;
    let pts: Vec<(f32, f32)> =
        history.iter().enumerate().map(|(i, v)| (x0 + i as f32 * step, 100.0 - v.clamp(0.0, 1.0) * 96.0 - 2.0)).collect();
    let mut line = format!("M {:.2} {:.2}", pts[0].0, pts[0].1);
    for (x, y) in &pts[1..] {
        line.push_str(&format!(" L {x:.2} {y:.2}"));
    }
    let fill = format!("{line} L 100 100 L {x0:.2} 100 Z");
    (line.into(), fill.into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rt = Runtime::new()?;
    let mut session = TerminalSession::init(TerminalOptions::default())?;
    rt.spawn_terminal_reader();
    let mut pane = rt.create_pane(&session.picker, Monitor::new)?;
    let mut router = InputRouter::new();
    router.set_keyboard_enhanced(session.keyboard_enhanced);

    let sampler = Rc::new(RefCell::new(Sampler { sys: System::new_all(), history: VecDeque::new(), procs: Vec::new() }));
    let table_dirty = Rc::new(Cell::new(true));
    let timer = slint::Timer::default();
    {
        let ui = pane.component().as_weak();
        let (sampler, dirty) = (sampler.clone(), table_dirty.clone());
        sampler.borrow_mut().sample(&pane.component().clone_strong());
        timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
            if let Some(ui) = ui.upgrade() {
                sampler.borrow_mut().sample(&ui);
                dirty.set(true);
            }
        });
    }

    let mut table = TableState::default().with_selected(Some(0));
    let mut paused = false;
    let (mut frames, started) = (0u64, Instant::now());
    let mut rate = ByteRate::default();
    let res: Result<(), Box<dyn std::error::Error>> = (|| loop {
        match rt.next_event_capped(Some(Duration::from_secs(1))) {
            LoopEvent::Quit => return Ok(()),
            LoopEvent::Terminal(ev) => {
                if let Event::Resize(..) = ev
                    && pane.protocol_type() != ratatui_slint::ratatui_image::picker::ProtocolType::Halfblocks
                    && let Some(font) = ratatui_slint::terminal::cell_size()
                {
                    pane.set_font_size(font); // font zoom changes pixels per cell
                }
                let routed = router.route(&ev, &[&pane as &dyn PaneInput]);
                table_dirty.set(true);
                if routed == Routed::Unhandled
                    && let Event::Key(k) = ev
                    && k.kind != KeyEventKind::Release
                {
                    match k.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('p') => {
                            paused = !paused;
                            if paused { timer.stop() } else { timer.restart() }
                            pane.component().set_paused(paused);
                        }
                        KeyCode::Down => table.select_next(),
                        KeyCode::Up => table.select_previous(),
                        _ => {}
                    }
                }
            }
            LoopEvent::Wake | LoopEvent::Timeout => {}
        }
        let bytes_per_sec = rate.sample(session.bytes.get());
        if !rt.begin_frame() {
            continue;
        }
        let size = session.terminal.size()?;
        let [main, status] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(Rect::from(size));
        let [left, right] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main);
        let block = Block::default().borders(Borders::ALL).title(" Slint ");
        let inner = block.inner(right);
        pane.set_area(inner);
        let pane_changed = pane.update();
        if pane_changed {
            frames += 1;
        }
        if !(pane_changed || table_dirty.replace(false)) {
            continue;
        }
        let focus = if router.focused().is_some() { "pane focused (Ctrl+G leaves)" } else { "click pane to focus" };
        let fps = frames as f64 / started.elapsed().as_secs_f64().max(0.001);
        let status_line = format!(
            " {:?} | {fps:.1} fps | {} B/s to terminal | {focus} | p pause, q quit",
            pane.protocol_type(),
            bytes_per_sec
        );
        let sampler = sampler.borrow();
        session.terminal.draw(|f| {
            let rows = sampler.procs.iter().take(200).map(|p| {
                Row::new(vec![
                    TCell::from(p.pid.to_string()),
                    TCell::from(p.name.clone()),
                    TCell::from(format!("{:5.1}", p.cpu)),
                    TCell::from(format!("{:6}", p.mem_mb)),
                ])
            });
            let t = Table::new(rows, [Constraint::Length(7), Constraint::Min(10), Constraint::Length(6), Constraint::Length(7)])
                .header(Row::new(vec!["PID", "NAME", "CPU%", "MEM MB"]).style(Style::default().add_modifier(Modifier::BOLD)))
                .row_highlight_style(Style::default().bg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title(" Processes "));
            f.render_stateful_widget(t, left, &mut table);
            f.render_widget(block, right);
            f.render_stateful_widget(SlintWidget, inner, &mut pane);
            f.render_widget(Paragraph::new(Line::from(status_line.clone())).style(Style::default().fg(Color::Gray)), status);
        })?;
        rt.write_pending_output(session.terminal.backend_mut())?;
    })();
    drop(pane); // queues deletion of its kitty image
    rt.write_pending_output(session.terminal.backend_mut())?;
    session.restore()?;
    res
}

/// Bytes per second written to the terminal, over the last second.
#[derive(Default)]
struct ByteRate {
    last: Option<(Instant, u64)>,
    rate: u64,
}

impl ByteRate {
    fn sample(&mut self, total: u64) -> u64 {
        let now = Instant::now();
        match self.last {
            Some((t, b)) if now.duration_since(t) >= Duration::from_secs(1) => {
                self.rate = ((total - b) as f64 / now.duration_since(t).as_secs_f64()) as u64;
                self.last = Some((now, total));
            }
            None => self.last = Some((now, total)),
            _ => {}
        }
        self.rate
    }
}
