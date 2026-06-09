//! A single async event stream the app loop can await on.
//!
//! Three sources are merged onto one channel:
//!   * a blocking thread reading crossterm input (keyboard + mouse),
//!   * a periodic tick for animations / time-based refreshes,
//!   * async Asana API results pushed back from spawned tasks.

use std::time::Duration;

use ratatui::crossterm::event::{self, Event as CrosstermEvent};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::asana::AsanaUpdate;

/// Anything the main loop might need to react to.
pub enum Event {
    /// Periodic tick (time-based work, animations).
    Tick,
    /// A raw terminal input event (key or mouse).
    Crossterm(CrosstermEvent),
    /// A result coming back from the Asana client.
    Asana(AsanaUpdate),
}

pub struct EventBus {
    rx: UnboundedReceiver<Event>,
    /// Clone this to let async tasks (e.g. API calls) push events back.
    pub tx: UnboundedSender<Event>,
}

impl EventBus {
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = unbounded_channel();

        // Input thread: blocking crossterm reads, forwarded onto the channel.
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                loop {
                    match event::poll(Duration::from_millis(100)) {
                        Ok(true) => match event::read() {
                            Ok(ev) => {
                                if tx.send(Event::Crossterm(ev)).is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        },
                        Ok(false) => {}
                        Err(_) => break,
                    }
                }
            });
        }

        // Tick task on the tokio runtime.
        {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(tick_rate);
                loop {
                    interval.tick().await;
                    if tx.send(Event::Tick).is_err() {
                        break;
                    }
                }
            });
        }

        Self { rx, tx }
    }

    /// Await the next event from any source.
    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}
