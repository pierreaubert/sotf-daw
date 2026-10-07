//! Contract tests for the exact shared CLAP/VST3 state-return implementation.

// Compile the private vendor helper directly so its deterministic scheduling
// tests remain part of the normal wrapper gate without exposing a public API.
#[path = "../../../../../../sotf-3rdparties/nih-plug/src/wrapper/gui_state_return.rs"]
mod return_path;

use crossbeam::channel::bounded;
use return_path::GuiStateReturn;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct State {
    drops: Arc<AtomicUsize>,
    owner: std::thread::ThreadId,
}

impl Drop for State {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.owner);
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn cold_return_finishes_before_gui_receives_and_preserves_ownership() {
    let exchange = Arc::new(GuiStateReturn::new());
    let drops = Arc::new(AtomicUsize::new(0));
    let state = Box::new(State {
        drops: Arc::clone(&drops),
        owner: std::thread::current().id(),
    });
    let gui = exchange.lock_gui_exchange();
    let (finished, completion) = std::sync::mpsc::channel();
    let audio_exchange = Arc::clone(&exchange);
    let audio = std::thread::spawn(move || {
        assert_no_alloc::assert_no_alloc(|| audio_exchange.return_from_audio(state));
        finished.send(()).unwrap();
    });
    let completed = completion.recv_timeout(Duration::from_secs(2));
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    // Receive even on failure so a regression to a rendezvous channel cannot
    // strand the test's audio thread indefinitely.
    let returned = exchange.receive_on_gui();
    audio.join().unwrap();
    completed.expect("audio must finish while the GUI has not called receive");
    drop(returned);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(gui);
}

#[test]
fn concurrent_control_callers_keep_each_response_with_its_request() {
    const CALLERS: usize = 8;
    let exchange = Arc::new(GuiStateReturn::new());
    let (sender, receiver) = bounded(0);
    let audio_exchange = Arc::clone(&exchange);
    let audio = std::thread::spawn(move || {
        for _ in 0..CALLERS {
            let state = receiver.recv().unwrap();
            assert_no_alloc::assert_no_alloc(|| audio_exchange.return_from_audio(state));
        }
    });
    let controls: Vec<_> = (0..CALLERS)
        .map(|value| {
            let exchange = Arc::clone(&exchange);
            let sender = sender.clone();
            std::thread::spawn(move || {
                let _gui = exchange.lock_gui_exchange();
                sender.send(value).unwrap();
                assert_eq!(exchange.receive_on_gui(), value);
            })
        })
        .collect();
    for control in controls {
        control.join().unwrap();
    }
    audio.join().unwrap();
}

#[test]
fn unaccepted_request_timeout_cannot_leave_a_stale_response() {
    let exchange = Arc::new(GuiStateReturn::new());
    let (sender, receiver) = bounded(0);
    let retired = Arc::new(AtomicUsize::new(0));
    {
        let _gui = exchange.lock_gui_exchange();
        let state = Box::new(State {
            drops: Arc::clone(&retired),
            owner: std::thread::current().id(),
        });
        // No audio receiver exists yet, so this request cannot be accepted.
        let state = match sender.send_timeout(state, Duration::from_millis(1)) {
            Err(crossbeam::channel::SendTimeoutError::Timeout(state)) => state,
            _ => panic!("an unreceived rendezvous request must time out"),
        };
        assert_eq!(retired.load(Ordering::SeqCst), 0);
        drop(state);
    }
    assert_eq!(retired.load(Ordering::SeqCst), 1);

    let accepted = Arc::new(AtomicUsize::new(0));
    let _gui = exchange.lock_gui_exchange();
    let audio_exchange = Arc::clone(&exchange);
    let audio = std::thread::spawn(move || {
        let state = receiver.recv().unwrap();
        assert_no_alloc::assert_no_alloc(|| audio_exchange.return_from_audio(state));
    });
    sender
        .send(Box::new(State {
            drops: Arc::clone(&accepted),
            owner: std::thread::current().id(),
        }))
        .unwrap();
    let response = exchange.receive_on_gui();
    assert!(Arc::ptr_eq(&response.drops, &accepted));
    audio.join().unwrap();
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
    drop(response);
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}
