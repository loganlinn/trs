//! Event handling: poll crossterm events with timeout.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};

use super::app::{App, Message};

/// Poll for a crossterm event and convert to an app Message.
pub fn handle_event(app: &mut App) -> Result<Option<Message>> {
    if event::poll(Duration::from_millis(50))? {
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => Ok(app.handle_key(key)),
            _ => Ok(None),
        }
    } else {
        Ok(None)
    }
}
