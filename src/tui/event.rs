//! Event handling: channel-based event handler with dedicated thread.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent, KeyEventKind};

/// Terminal events.
pub enum Event {
    /// Periodic tick for debounced operations.
    Tick,
    /// Key press.
    Key(crossterm::event::KeyEvent),
    /// Mouse event.
    Mouse(crossterm::event::MouseEvent),
    /// Terminal resize.
    #[allow(dead_code)]
    Resize(u16, u16),
}

/// Polls crossterm events on a background thread, delivering them over a channel.
pub struct EventHandler {
    rx: mpsc::Receiver<Event>,
}

impl EventHandler {
    /// Spawn a new event handler. `tick_rate` controls idle tick frequency.
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || loop {
            if event::poll(tick_rate).unwrap_or(false) {
                match event::read() {
                    Ok(CrosstermEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                        if tx.send(Event::Key(key)).is_err() {
                            break;
                        }
                    }
                    Ok(CrosstermEvent::Mouse(mouse)) => {
                        if tx.send(Event::Mouse(mouse)).is_err() {
                            break;
                        }
                    }
                    Ok(CrosstermEvent::Resize(w, h)) => {
                        if tx.send(Event::Resize(w, h)).is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            } else if tx.send(Event::Tick).is_err() {
                break;
            }
        });

        Self { rx }
    }

    /// Block until the next event is available.
    pub fn next(&self) -> Result<Event, mpsc::RecvError> {
        self.rx.recv()
    }
}
