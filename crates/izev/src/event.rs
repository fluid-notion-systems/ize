//! Event handling with crossterm integration

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers};

/// Application events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Quit the application
    Quit,
    /// A character key was pressed
    Key(char),
    /// Up arrow
    Up,
    /// Down arrow
    Down,
    /// Left arrow
    Left,
    /// Right arrow
    Right,
    /// Tab key
    Tab,
    /// Backspace key
    Backspace,
    /// Enter key
    Enter,
    /// Escape key
    Escape,
    /// Page up
    PageUp,
    /// Page down
    PageDown,
    /// Home key
    Home,
    /// End key
    End,
    /// Alt+P (jump to projects)
    AltP,
    /// Alt+C (jump to channels)
    AltC,
    /// Terminal resize event
    Resize(u16, u16),
    /// No event (tick)
    Tick,
}

/// Poll timeout for events
const POLL_TIMEOUT: Duration = Duration::from_millis(100);

/// Poll for the next event
pub fn poll_event() -> Result<Option<Event>> {
    if event::poll(POLL_TIMEOUT)? {
        let event = event::read()?;
        Ok(Some(convert_event(event)))
    } else {
        Ok(Some(Event::Tick))
    }
}

/// Convert crossterm event to our Event type
fn convert_event(event: event::Event) -> Event {
    match event {
        event::Event::Key(key_event) => convert_key_event(key_event),
        event::Event::Resize(width, height) => Event::Resize(width, height),
        _ => Event::Tick,
    }
}

/// Convert a key event to our Event type
fn convert_key_event(key: KeyEvent) -> Event {
    // Handle Ctrl+C, Ctrl+Q
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('q') => return Event::Quit,
            _ => {}
        }
    }

    // Handle Alt+P and Alt+C
    if key.modifiers.contains(KeyModifiers::ALT) {
        match key.code {
            KeyCode::Char('p') => return Event::AltP,
            KeyCode::Char('c') => return Event::AltC,
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char(c) => Event::Key(c),
        KeyCode::Up => Event::Up,
        KeyCode::Down => Event::Down,
        KeyCode::Left => Event::Left,
        KeyCode::Right => Event::Right,
        KeyCode::Tab => Event::Tab,
        KeyCode::Backspace => Event::Backspace,
        KeyCode::Enter => Event::Enter,
        KeyCode::Esc => Event::Escape,
        KeyCode::PageUp => Event::PageUp,
        KeyCode::PageDown => Event::PageDown,
        KeyCode::Home => Event::Home,
        KeyCode::End => Event::End,
        _ => Event::Tick,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_equality() {
        assert_eq!(Event::Quit, Event::Quit);
        assert_eq!(Event::Key('a'), Event::Key('a'));
        assert_ne!(Event::Key('a'), Event::Key('b'));
    }
}
