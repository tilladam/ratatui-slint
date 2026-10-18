//! Executed by scripts/smoke.sh inside a pseudo-terminal, never against the user's terminal.
use ratatui_slint::{
    Runtime,
    terminal::{TerminalOptions, TerminalSession},
};

#[test]
#[ignore = "requires scripts/fake_terminal.py"]
fn session_ownership_and_reader_teardown() {
    let mut rt = Runtime::new().unwrap();
    let mut old = TerminalSession::init(TerminalOptions::default()).unwrap();
    rt.spawn_terminal_reader().unwrap();
    assert!(TerminalSession::init(TerminalOptions::default()).is_err());
    assert!(crossterm::terminal::is_raw_mode_enabled().unwrap());
    old.restore().unwrap();
    old.restore().unwrap();
    assert!(!crossterm::terminal::is_raw_mode_enabled().unwrap());
    let mut new = TerminalSession::init(TerminalOptions::default()).unwrap();
    drop(old);
    assert!(
        crossterm::terminal::is_raw_mode_enabled().unwrap(),
        "old Drop restored new session"
    );
    // A caught worker panic must not restore modes before the host handles it.
    std::thread::spawn(|| {
        let _ = std::panic::catch_unwind(|| panic!("caught worker panic"));
    })
    .join()
    .unwrap();
    assert!(crossterm::terminal::is_raw_mode_enabled().unwrap());
    rt.spawn_terminal_reader().unwrap();
    new.finish(&mut rt).unwrap();
    new.finish(&mut rt).unwrap();
    assert!(!crossterm::terminal::is_raw_mode_enabled().unwrap());
}
