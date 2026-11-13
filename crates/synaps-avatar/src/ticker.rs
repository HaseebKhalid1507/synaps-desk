//! One wake-up thread for the whole view: it sleeps until the next frame/typing/fade
//! deadline and pokes the main loop. Idle means this thread is parked, nothing else runs.
//! (A plain thread is one hop; GPUI's timer goes timer thread → worker → main loop.)

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Instant;

pub struct Ticker {
    tx: mpsc::Sender<Option<Instant>>,
}

impl Ticker {
    /// Returns the ticker and the receiver the main loop awaits.
    pub fn start() -> (Ticker, async_channel::Receiver<()>) {
        let (tx, rx) = mpsc::channel::<Option<Instant>>();
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("avatar-ticker".into())
            .spawn(move || {
                let mut deadline: Option<Instant> = None;
                loop {
                    let msg = match deadline {
                        Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match msg {
                        Ok(d) => deadline = d,
                        Err(RecvTimeoutError::Timeout) => {
                            deadline = None;
                            // Full means a wake-up is already pending; that one will do.
                            if let Err(async_channel::TrySendError::Closed(_)) = wake_tx.try_send(()) {
                                return;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .expect("spawn ticker thread");
        (Ticker { tx }, wake_rx)
    }

    /// Wake the main loop at `at` (replacing any earlier request), or never.
    pub fn set(&self, at: Option<Instant>) {
        self.tx.send(at).ok();
    }
}
