//! `slint::invoke_from_event_loop` from another thread wakes the loop and runs on the UI thread.
//!
//! Its own test binary: Slint keeps the event-loop proxy in a process-wide slot, so only the first
//! runtime in a process receives these closures.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ratatui_slint::{LoopEvent, Runtime};

#[test]
fn invoke_from_event_loop_wakes_the_loop() {
    let mut rt = Runtime::new().unwrap();
    assert!(rt.receives_invoke_from_event_loop());
    let ui_thread = std::thread::current().id();
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        slint::invoke_from_event_loop(move || {
            assert_eq!(std::thread::current().id(), ui_thread);
            flag.store(true, Ordering::SeqCst);
        })
        .unwrap();
    });
    let _ = rt.begin_frame(); // clear the initial pending frame so the loop blocks
    assert_eq!(rt.timeout(), None);
    let start = Instant::now();
    let event = rt.next_event(); // blocks with no timeout
    assert!(matches!(event, LoopEvent::Wake), "{event:?}");
    assert!(ran.load(Ordering::SeqCst));
    assert!(start.elapsed() < Duration::from_secs(2));

    let _ = rt.begin_frame();
    std::thread::spawn(|| slint::quit_event_loop().unwrap());
    assert!(matches!(rt.next_event(), LoopEvent::Quit));
}
