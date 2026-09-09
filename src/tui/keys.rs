//! Key bindings, as a pure mapping so the bindings can be tested without a
//! terminal.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Intent {
    Quit,
    Home,
    Libraries,
    Playing,
    StartSearch,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    /// Movement along a grid's second axis, and nothing at all in a list.
    /// Which of the two a screen is stays `App`'s business, not this mapping's.
    Left,
    Right,
    NextPane,
    Enter,
    Back,
    Refresh,
    PlayPause,
    Stop,
    Next,
    Previous,
    SeekBy(i64),
    VolumeBy(i64),
    ToggleMute,
    ToggleFullscreen,
    Type(char),
    Backspace,
}

const SEEK_SECONDS: i64 = 10;
const VOLUME_STEP: i64 = 5;

/// While the search box has focus, printable keys are text — only the keys
/// that cannot be part of a query still act as commands.
pub(super) fn map(key: KeyEvent, typing: bool) -> Option<Intent> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Intent::Quit),
            KeyCode::Char('u') if typing => Some(Intent::Back),
            _ => None,
        };
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        match key.code {
            KeyCode::Left => return Some(Intent::SeekBy(-SEEK_SECONDS)),
            KeyCode::Right => return Some(Intent::SeekBy(SEEK_SECONDS)),
            _ => {}
        }
    }
    match key.code {
        KeyCode::Up => Some(Intent::Up),
        KeyCode::Down => Some(Intent::Down),
        KeyCode::PageUp => Some(Intent::PageUp),
        KeyCode::PageDown => Some(Intent::PageDown),
        KeyCode::Enter => Some(Intent::Enter),
        KeyCode::Esc => Some(Intent::Back),
        KeyCode::Tab => Some(Intent::NextPane),
        KeyCode::Backspace if typing => Some(Intent::Backspace),
        KeyCode::Left if !typing => Some(Intent::Left),
        KeyCode::Right if !typing => Some(Intent::Right),
        KeyCode::Char(c) if typing => Some(Intent::Type(c)),
        KeyCode::Char(c) => command(c),
        _ => None,
    }
}

fn command(c: char) -> Option<Intent> {
    match c {
        'q' => Some(Intent::Quit),
        '1' => Some(Intent::Home),
        '2' => Some(Intent::Libraries),
        '3' => Some(Intent::Playing),
        '/' => Some(Intent::StartSearch),
        'j' => Some(Intent::Down),
        'k' => Some(Intent::Up),
        'g' => Some(Intent::Top),
        'G' => Some(Intent::Bottom),
        'l' => Some(Intent::Right),
        'h' => Some(Intent::Left),
        'r' => Some(Intent::Refresh),
        ' ' => Some(Intent::PlayPause),
        's' => Some(Intent::Stop),
        'n' => Some(Intent::Next),
        'p' => Some(Intent::Previous),
        'm' => Some(Intent::ToggleMute),
        'f' => Some(Intent::ToggleFullscreen),
        '+' | '=' => Some(Intent::VolumeBy(VOLUME_STEP)),
        '-' => Some(Intent::VolumeBy(-VOLUME_STEP)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "keys_test.rs"]
mod tests;
