use super::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState};

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn press(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn letters_are_commands_while_browsing_and_text_while_searching() {
    assert_eq!(map(press('j'), false), Some(Intent::Down));
    assert_eq!(map(press('j'), true), Some(Intent::Type('j')));
    // 'q' would otherwise quit mid-query.
    assert_eq!(map(press('q'), false), Some(Intent::Quit));
    assert_eq!(map(press('q'), true), Some(Intent::Type('q')));
}

#[test]
fn navigation_keys_keep_working_inside_the_search_box() {
    assert_eq!(
        map(key(KeyCode::Down, KeyModifiers::NONE), true),
        Some(Intent::Down)
    );
    assert_eq!(
        map(key(KeyCode::Enter, KeyModifiers::NONE), true),
        Some(Intent::Enter)
    );
    assert_eq!(
        map(key(KeyCode::Esc, KeyModifiers::NONE), true),
        Some(Intent::Back)
    );
    assert_eq!(
        map(key(KeyCode::Backspace, KeyModifiers::NONE), true),
        Some(Intent::Backspace)
    );
}

#[test]
fn arrows_navigate_bare_and_seek_with_shift() {
    assert_eq!(
        map(key(KeyCode::Left, KeyModifiers::NONE), false),
        Some(Intent::Back)
    );
    assert_eq!(
        map(key(KeyCode::Right, KeyModifiers::NONE), false),
        Some(Intent::Enter)
    );
    assert_eq!(
        map(key(KeyCode::Left, KeyModifiers::SHIFT), false),
        Some(Intent::SeekBy(-10))
    );
    assert_eq!(
        map(key(KeyCode::Right, KeyModifiers::SHIFT), true),
        Some(Intent::SeekBy(10)),
        "seeking must survive the search box, where arrows are still navigation"
    );
}

#[test]
fn ctrl_c_quits_from_anywhere_and_other_ctrl_chords_do_not_type() {
    assert_eq!(
        map(key(KeyCode::Char('c'), KeyModifiers::CONTROL), true),
        Some(Intent::Quit)
    );
    assert_eq!(
        map(key(KeyCode::Char('a'), KeyModifiers::CONTROL), true),
        None
    );
}

#[test]
fn an_unbound_key_is_ignored_rather_than_mapped_to_something_else() {
    assert_eq!(map(key(KeyCode::F(5), KeyModifiers::NONE), false), None);
    assert_eq!(map(press('z'), false), None);
}
