//! Nonblocking return of restored state to its owning GUI thread.

use crossbeam::queue::ArrayQueue;
use parking_lot::{Mutex, MutexGuard};

pub(crate) struct GuiStateReturn<T> {
    gui_exchange: Mutex<()>,
    response: ArrayQueue<T>,
}

impl<T> GuiStateReturn<T> {
    pub(crate) fn new() -> Self {
        // One serialized request may await a response. Prepare that response's
        // storage now so the audio thread never waits for the GUI to receive it.
        Self {
            gui_exchange: Mutex::new(()),
            response: ArrayQueue::new(1),
        }
    }

    /// Serialize control callers through receipt and destruction of their state.
    pub(crate) fn lock_gui_exchange(&self) -> MutexGuard<'_, ()> {
        self.gui_exchange.lock()
    }

    /// Return one accepted request without allocating, blocking, or dropping it.
    pub(crate) fn return_from_audio(&self, state: T) {
        // The GUI holds gui_exchange from before request submission until after
        // response receipt. Therefore only one request can be accepted, and its
        // response slot is empty. The queue belongs to this live wrapper,
        // so disconnection is impossible. A failure is a programming bug,
        // not backpressure that the audio callback could wait for or discard.
        if self.response.push(state).is_err() {
            unreachable!("a serialized GUI state exchange has one empty response slot");
        }
    }

    /// Consume the accepted request's response on the control thread.
    pub(crate) fn receive_on_gui(&self) -> T {
        // Once the request rendezvous succeeds there is no timeout/cancellation:
        // this caller retains the exchange lock and wrapper until audio replies.
        // An unaccepted request timeout retains its state and needs no response.
        // GUI-only polling avoids a channel receiver-waker mutex on the audio
        // return path. It adds one polling interval plus scheduler delay to GUI
        // acknowledgement, without delaying audio publication.
        loop {
            if let Some(state) = self.response.pop() {
                return state;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
