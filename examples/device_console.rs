//! Device remote console: a ratatui operations console (logs, services, config editor) with the
//! device's touch panel live and clickable in a pane.
//!
//! `cargo run --release --example device_console`
//!
//! The panel is designed for an 800 × 480 touch screen and scaled into the pane with
//! `Scale::Fit`. The same `.slint` file would drive the device's own display. Over half-blocks
//! the panel's text is unreadable, so the console adds a ratatui readout of its key values.
//!
//! Keys: click the panel to use it (Ctrl+G leaves). Otherwise Tab switches between the config
//! editor and the log, Up/Down move, Enter edits a config value and Enter again applies it,
//! Esc cancels, `q` quits.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Row, Table};
use ratatui_image::picker::ProtocolType;
use ratatui_slint::terminal::{TerminalOptions, TerminalSession};
use ratatui_slint::{InputRouter, LoopEvent, PaneInput, Routed, Runtime, Scale, SlintWidget};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

slint::slint! {
    import { Button, Switch, Slider } from "std-widgets.slint";

    component Card inherits Rectangle {
        in property <string> title;
        background: #1a2129;
        border-radius: 16px;
        Text { x: 20px; y: 14px; text: root.title; color: #8a99a8; font-size: 18px; }
        @children
    }

    export component DevicePanel inherits Window {
        in-out property <float> temperature: 21.5;
        in-out property <float> setpoint: 22.0;
        in-out property <bool> heating: true;
        in-out property <bool> eco: false;
        in-out property <float> fan: 40;
        in property <float> power-kw: 1.2;
        in property <[float]> power-history: [0.4, 0.6, 0.9, 1.1, 1.0, 1.2];
        callback setpoint-changed(float);
        callback mode-changed(string, bool);
        callback fan-changed(float);
        width: 800px;
        height: 480px;
        background: #0e1217;

        Text { x: 28px; y: 18px; text: "Living room · HVAC unit 3"; color: #e6edf3; font-size: 26px; }
        Text {
            x: parent.width - self.width - 28px; y: 22px;
            text: root.heating ? (root.eco ? "Heating · eco" : "Heating") : "Standby";
            color: root.heating ? #f0883e : #8a99a8; font-size: 20px;
        }

        Card {
            x: 24px; y: 70px; width: 380px; height: 386px; title: "Temperature";
            Text { x: 20px; y: 54px; text: Math.round(root.temperature * 10) / 10 + "°"; color: #e6edf3; font-size: 96px; }
            Text { x: 24px; y: 178px; text: "Target " + Math.round(root.setpoint * 10) / 10 + "°"; color: #58a6ff; font-size: 26px; }
            // Range bar 15–30 °C with the current temperature and the target marked.
            Rectangle {
                x: 24px; y: 230px; width: 332px; height: 14px; border-radius: 7px;
                background: @linear-gradient(90deg, #388bfd 0%, #3fb950 45%, #f0883e 100%);
                Rectangle {
                    x: (root.temperature - 15) / 15 * parent.width - 4px; y: -6px;
                    width: 8px; height: 26px; border-radius: 4px; background: #e6edf3;
                    animate x { duration: 500ms; easing: ease-out; }
                }
                Rectangle {
                    x: (root.setpoint - 15) / 15 * parent.width - 2px; y: 18px;
                    width: 4px; height: 10px; background: #58a6ff;
                    animate x { duration: 250ms; }
                }
            }
            HorizontalLayout {
                x: 24px; y: 290px; width: 332px; height: 70px; spacing: 20px;
                Button {
                    text: "−";
                    clicked => { root.setpoint = Math.max(15, root.setpoint - 0.5); root.setpoint-changed(root.setpoint); }
                }
                Button {
                    text: "+";
                    clicked => { root.setpoint = Math.min(30, root.setpoint + 0.5); root.setpoint-changed(root.setpoint); }
                }
            }
        }

        Card {
            x: 420px; y: 70px; width: 356px; height: 180px; title: "Mode";
            VerticalLayout {
                x: 20px; y: 50px; width: 316px; height: 120px; spacing: 10px;
                Switch {
                    text: "Heating";
                    checked <=> root.heating;
                    toggled => { root.mode-changed("heating", self.checked); }
                }
                Switch {
                    text: "Eco mode";
                    checked <=> root.eco;
                    toggled => { root.mode-changed("eco", self.checked); }
                }
                HorizontalLayout {
                    spacing: 10px;
                    Text { text: "Fan"; color: #e6edf3; vertical-alignment: center; }
                    Slider {
                        minimum: 0; maximum: 100;
                        value <=> root.fan;
                        released(v) => { root.fan-changed(v); }
                    }
                }
            }
        }

        Card {
            x: 420px; y: 266px; width: 356px; height: 190px; title: "Power";
            Text { x: 20px; y: 44px; text: Math.round(root.power-kw * 100) / 100 + " kW"; color: #e6edf3; font-size: 40px; }
            HorizontalLayout {
                x: 20px; y: 100px; width: 316px; height: 72px; spacing: 4px; alignment: end;
                for v in root.power-history: Rectangle {
                    width: 16px;
                    Rectangle {
                        y: parent.height - self.height;
                        height: parent.height * Math.min(1, v / 3);
                        border-radius: 3px;
                        background: #3fb950;
                        animate height { duration: 400ms; }
                    }
                }
            }
        }
    }
}

/// Messages from the simulated device thread.
enum DeviceMsg {
    Log(String),
    Services(Vec<(String, f32, u32)>),
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Config,
    Log,
}

struct ConfigEntry {
    key: &'static str,
    value: String,
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rt = Runtime::new()?;
    let mut session = TerminalSession::init(TerminalOptions::default())?;
    rt.spawn_terminal_reader();
    let mut pane = rt.create_pane(&session.picker, DevicePanel::new)?;
    pane.set_scale(Scale::Fit {
        width: 800.0,
        height: 480.0,
    });
    let halfblocks = pane.protocol_type() == ProtocolType::Halfblocks;
    let mut router = InputRouter::new();
    router.set_keyboard_enhanced(session.keyboard_enhanced);

    // Device simulation on its own thread. Sensor readings go straight into the panel through
    // slint::invoke_from_event_loop; logs and service stats go through a channel plus a Waker.
    let (tx, rx) = mpsc::channel::<DeviceMsg>();
    let waker = rt.waker();
    let panel = pane.component().as_weak();
    std::thread::spawn(move || {
        let mut t: f32 = 0.0;
        let mut temp: f32 = 21.0;
        for tick in 0u64.. {
            std::thread::sleep(Duration::from_millis(700));
            t += 0.7;
            temp += (t * 0.9).sin() * 0.08;
            let power = 1.1 + (t * 0.5).sin().abs() * 1.4;
            let panel = panel.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(p) = panel.upgrade() {
                    p.set_temperature(temp);
                    p.set_power_kw(power);
                    let hist = p.get_power_history();
                    let mut v: Vec<f32> = hist.iter().collect();
                    v.push(power);
                    if v.len() > 16 {
                        v.remove(0);
                    }
                    p.set_power_history(ModelRc::new(VecModel::from(v)));
                }
            });
            if tick.is_multiple_of(3) {
                let _ = tx.send(DeviceMsg::Log(format!(
                    "{} sensor: temperature {temp:.2} °C, power {power:.2} kW",
                    timestamp()
                )));
            }
            let services = vec![
                ("hvac-control".into(), 2.0 + (t.sin() * 1.5).abs(), 18),
                ("sensor-bus".into(), 0.6 + (t * 1.7).cos().abs(), 6),
                ("ui-panel".into(), 4.0 + (t * 0.3).sin().abs() * 3.0, 42),
                ("ota-agent".into(), 0.1, 11),
                ("sshd".into(), 0.0, 4),
            ];
            if tx.send(DeviceMsg::Services(services)).is_err() || !waker.wake() {
                break;
            }
        }
    });

    let mut log: VecDeque<String> = VecDeque::new();
    let push_log = |log: &mut VecDeque<String>, line: String| {
        log.push_back(line);
        while log.len() > 500 {
            log.pop_front();
        }
    };
    // Panel callbacks write to the log through a channel too: they run inside Slint dispatch.
    let (ui_tx, ui_rx) = mpsc::channel::<String>();
    {
        let tx = ui_tx.clone();
        pane.component().on_setpoint_changed(move |v| {
            let _ = tx.send(format!("{} panel: setpoint -> {v:.1} °C", timestamp()));
        });
        let tx = ui_tx.clone();
        pane.component().on_mode_changed(move |mode, on| {
            let _ = tx.send(format!(
                "{} panel: {mode} -> {}",
                timestamp(),
                if on { "on" } else { "off" }
            ));
        });
        let tx = ui_tx;
        pane.component().on_fan_changed(move |v| {
            let _ = tx.send(format!("{} panel: fan -> {v:.0} %", timestamp()));
        });
    }
    push_log(
        &mut log,
        format!(
            "{} console: connected to hvac-unit-3 ({:?})",
            timestamp(),
            pane.protocol_type()
        ),
    );

    let mut config = [
        ConfigEntry {
            key: "setpoint",
            value: "22.0".into(),
        },
        ConfigEntry {
            key: "eco",
            value: "false".into(),
        },
        ConfigEntry {
            key: "fan",
            value: "40".into(),
        },
        ConfigEntry {
            key: "hostname",
            value: "hvac-unit-3".into(),
        },
        ConfigEntry {
            key: "log-level",
            value: "info".into(),
        },
    ];
    let mut config_state = ListState::default().with_selected(Some(0));
    let mut editing: Option<String> = None;
    let mut focus = Focus::Config;
    let mut log_scroll: usize = 0;
    let mut services: Vec<(String, f32, u32)> = Vec::new();
    let mut dirty = true;

    let res: Result<(), Box<dyn std::error::Error>> = (|| loop {
        match rt.next_event() {
            LoopEvent::Quit => return Ok(()),
            LoopEvent::Terminal(ev) => {
                dirty = true;
                if let Event::Resize(..) = ev
                    && pane.protocol_type()
                        != ratatui_slint::ratatui_image::picker::ProtocolType::Halfblocks
                    && let Some(font) = ratatui_slint::terminal::cell_size()
                {
                    pane.set_font_size(font); // font zoom changes pixels per cell
                }
                if router.route(&ev, &[&pane as &dyn PaneInput]) == Routed::Unhandled
                    && let Event::Key(k) = ev
                    && k.kind != KeyEventKind::Release
                {
                    if let Some(buf) = editing.as_mut() {
                        match k.code {
                            KeyCode::Enter => {
                                let i = config_state.selected().unwrap_or(0);
                                let value = editing.take().unwrap_or_default();
                                apply_config(&pane, config[i].key, &value, &mut log);
                                config[i].value = value;
                            }
                            KeyCode::Esc => editing = None,
                            KeyCode::Backspace => {
                                buf.pop();
                            }
                            KeyCode::Char(c) => buf.push(c),
                            _ => {}
                        }
                    } else {
                        match k.code {
                            KeyCode::Char('q') => return Ok(()),
                            KeyCode::Tab => {
                                focus = if focus == Focus::Config {
                                    Focus::Log
                                } else {
                                    Focus::Config
                                }
                            }
                            KeyCode::Down if focus == Focus::Config => config_state.select_next(),
                            KeyCode::Up if focus == Focus::Config => config_state.select_previous(),
                            KeyCode::Up if focus == Focus::Log => log_scroll += 1,
                            KeyCode::Down if focus == Focus::Log => {
                                log_scroll = log_scroll.saturating_sub(1)
                            }
                            KeyCode::Enter if focus == Focus::Config => {
                                let i = config_state.selected().unwrap_or(0).min(config.len() - 1);
                                editing = Some(config[i].value.clone());
                            }
                            _ => {}
                        }
                    }
                }
            }
            LoopEvent::Wake | LoopEvent::Timeout => {}
        }
        for msg in rx.try_iter() {
            dirty = true;
            match msg {
                DeviceMsg::Log(line) => push_log(&mut log, line),
                DeviceMsg::Services(s) => services = s,
            }
        }
        for line in ui_rx.try_iter() {
            dirty = true;
            push_log(&mut log, line);
        }
        if !rt.begin_frame() {
            continue;
        }

        let size = session.terminal.size()?;
        let [main, bottom, status] = Layout::vertical([
            Constraint::Percentage(62),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .areas(Rect::from(size));
        let [panel_col, config_area] =
            Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                .areas(main);
        let [log_area, svc_area] =
            Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                .areas(bottom);
        let (panel_area, readout_area) = if halfblocks {
            let [p, r] =
                Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(panel_col);
            (p, Some(r))
        } else {
            (panel_col, None)
        };
        let panel_block = Block::default()
            .borders(Borders::ALL)
            .title(" Device panel (live) ")
            .border_style(if router.focused().is_some() {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            });
        let inner = panel_block.inner(panel_area);
        pane.set_area(inner);
        let changed = pane.update();
        let host_changed = std::mem::take(&mut dirty);
        if !(changed || host_changed) {
            continue;
        }

        let p = pane.component();
        let readout = format!(
            " {:.1}°C  target {:.1}°C  {}{}  fan {:.0}%  {:.2} kW",
            p.get_temperature(),
            p.get_setpoint(),
            if p.get_heating() {
                "heating"
            } else {
                "standby"
            },
            if p.get_eco() { " eco" } else { "" },
            p.get_fan(),
            p.get_power_kw()
        );
        let hint = if router.focused().is_some() {
            "panel focused: Ctrl+G leaves"
        } else if editing.is_some() {
            "editing: Enter applies, Esc cancels"
        } else {
            "click the panel to use it | Tab switch | Enter edit | q quit"
        };
        session.terminal.draw(|f| {
            f.render_widget(panel_block, panel_area);
            f.render_stateful_widget(SlintWidget, inner, &mut pane);
            if let Some(r) = readout_area {
                f.render_widget(
                    Paragraph::new(readout.clone()).style(Style::default().fg(Color::Yellow)),
                    r,
                );
            }

            let items: Vec<ListItem> = config
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let value = match &editing {
                        Some(buf) if config_state.selected() == Some(i) => format!("{buf}▏"),
                        _ => e.value.clone(),
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{:<10}", e.key), Style::default().fg(Color::Gray)),
                        Span::raw(value),
                    ]))
                })
                .collect();
            let border = |on: bool| {
                if on {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                }
            };
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Config ")
                        .border_style(border(focus == Focus::Config && router.focused().is_none())),
                )
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            f.render_stateful_widget(list, config_area, &mut config_state);

            let height = log_area.height.saturating_sub(2) as usize;
            let end = log.len().saturating_sub(log_scroll.min(log.len()));
            let start = end.saturating_sub(height);
            let lines: Vec<Line> = log
                .range(start..end)
                .map(|l| Line::from(l.as_str()))
                .collect();
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Log ")
                        .border_style(border(focus == Focus::Log && router.focused().is_none())),
                ),
                log_area,
            );

            let rows = services.iter().map(|(n, cpu, mem)| {
                Row::new(vec![n.clone(), format!("{cpu:4.1}"), format!("{mem:3} MB")])
            });
            f.render_widget(
                Table::new(
                    rows,
                    [
                        Constraint::Min(12),
                        Constraint::Length(6),
                        Constraint::Length(7),
                    ],
                )
                .header(
                    Row::new(vec!["SERVICE", "CPU%", "MEM"])
                        .style(Style::default().add_modifier(Modifier::BOLD)),
                )
                .block(Block::default().borders(Borders::ALL).title(" Services ")),
                svc_area,
            );
            f.render_widget(
                Paragraph::new(format!(" {:?} | {hint}", pane.protocol_type()))
                    .style(Style::default().fg(Color::Gray)),
                status,
            );
        })?;
        rt.write_pending_output(session.terminal.backend_mut())?;
    })();
    drop(pane); // queues deletion of its kitty image
    rt.write_pending_output(session.terminal.backend_mut())?;
    session.restore()?;
    res
}

/// Pushes a config value into the panel, as the device would after a config change.
fn apply_config(
    pane: &ratatui_slint::SlintPane<DevicePanel>,
    key: &str,
    value: &str,
    log: &mut VecDeque<String>,
) {
    let p = pane.component();
    let ok = match key {
        "setpoint" => value
            .parse::<f32>()
            .ok()
            .filter(|v| (15.0..=30.0).contains(v))
            .map(|v| p.set_setpoint(v))
            .is_some(),
        "eco" => value.parse::<bool>().ok().map(|v| p.set_eco(v)).is_some(),
        "fan" => value
            .parse::<f32>()
            .ok()
            .filter(|v| (0.0..=100.0).contains(v))
            .map(|v| p.set_fan(v))
            .is_some(),
        _ => true,
    };
    let line = if ok {
        format!("{} config: {key} = {value}", timestamp())
    } else {
        format!("{} config: rejected {key} = {value}", timestamp())
    };
    log.push_back(line);
}
