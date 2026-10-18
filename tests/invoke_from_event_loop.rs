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
    pane_lifecycle(&mut rt);
}

// One test owns the process-wide proxy, so all lifecycle cases run sequentially above.
fn pane_lifecycle(rt: &mut Runtime) {
    use common::*;
    use ratatui::layout::Rect;
    use ratatui_slint::SlintWidget;
    use slint::ComponentHandle;

    fn drain(rt: &mut Runtime) {
        for _ in 0..10 {
            assert!(!matches!(
                rt.next_event_capped(Some(Duration::from_millis(2))),
                LoopEvent::Quit
            ));
        }
    }
    fn explicit_quit(rt: &mut Runtime) {
        slint::quit_event_loop().unwrap();
        for _ in 0..20 {
            if matches!(
                rt.next_event_capped(Some(Duration::from_millis(2))),
                LoopEvent::Quit
            ) {
                return;
            }
        }
        panic!("explicit quit was suppressed");
    }
    explicit_quit(rt); // no windows
    let mut a = rt.create_pane(&halfblocks(), Still::new).unwrap();
    a.set_area(Rect::new(0, 0, 10, 3));
    assert!(a.update());
    test_terminal(10, 3)
        .draw(|f| f.render_stateful_widget(SlintWidget, f.area(), &mut a))
        .unwrap();
    explicit_quit(rt); // one window
    let b = rt.create_pane(&halfblocks(), Still::new).unwrap();
    explicit_quit(rt); // several windows
    b.component().hide().unwrap();
    drop(b);
    drain(rt);
    drop(a);
    drain(rt);
    assert_eq!(rt.live_windows(), 0);

    // Direct hide and a callback that disposes the last pane use context proxies too.
    let a = rt.create_pane(&halfblocks(), Still::new).unwrap();
    a.component().hide().unwrap();
    drain(rt);
    drop(a);
    let a = rt.create_pane(&halfblocks(), Still::new).unwrap();
    let completed = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = completed.clone();
    slint::spawn_local(async move {
        drop(a);
        flag.set(true);
    })
    .unwrap();
    drain(rt);
    assert!(
        completed.get(),
        "context proxy failed to invoke spawn_local"
    );
    assert_eq!(rt.live_windows(), 0);
    let failed = rt.create_pane(&halfblocks(), || {
        let c = Still::new().unwrap();
        c.show().unwrap();
        c.hide().unwrap();
        Err::<Still, _>("factory failed after creating a helper")
    });
    assert!(failed.is_err());
    drain(rt);
    let _again = rt.create_pane(&halfblocks(), Still::new).unwrap();
    explicit_quit(rt);
}
