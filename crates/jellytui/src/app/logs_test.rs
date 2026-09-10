use super::*;
use crate::test_support::app_with_logs;
use ratatui::layout::Size;

/// Tall enough that `view::panes` leaves a body of `rows` lines.
fn app_showing(rows: u16, lines: usize) -> App {
    let logs = LogBuffer::new();
    for n in 0..lines {
        logs.push_line(format!("line {n}"));
    }
    let mut app = app_with_logs(logs);
    app.viewport = Size::new(80, rows + 4);
    app.apply(Intent::Logs);
    app
}

fn messages(app: &App, rows: u16) -> Vec<String> {
    app.log_window(rows)
        .0
        .iter()
        .map(|line| line.message.clone())
        .collect()
}

#[test]
fn the_pane_opens_on_the_newest_lines_and_returns_to_the_screen_it_covered() {
    let mut app = app_showing(3, 10);
    assert_eq!(messages(&app, 3), ["line 7", "line 8", "line 9"]);

    app.apply(Intent::Logs);
    app.apply(Intent::Playing);
    app.apply(Intent::Logs);
    assert_eq!(app.screen, Screen::Logs);
    app.apply(Intent::Logs);
    assert_eq!(app.screen, Screen::Playing);
}

#[test]
fn scrolling_up_pins_the_view_and_new_lines_do_not_move_it() {
    let mut app = app_showing(3, 10);
    app.apply(Intent::Up);
    app.apply(Intent::Up);
    assert_eq!(messages(&app, 3), ["line 5", "line 6", "line 7"]);

    app.logs.push_line("line 10".into());
    assert_eq!(messages(&app, 3), ["line 5", "line 6", "line 7"]);

    app.apply(Intent::Bottom);
    assert_eq!(messages(&app, 3), ["line 8", "line 9", "line 10"]);
}

#[test]
fn a_pinned_view_slides_with_eviction_rather_than_showing_dropped_lines() {
    let mut app = app_showing(3, 10);
    app.apply(Intent::Top);
    assert_eq!(messages(&app, 3), ["line 0", "line 1", "line 2"]);

    for n in 10..crate::logs::CAPACITY + 4 {
        app.logs.push_line(format!("line {n}"));
    }
    // `line 0`..`line 3` are gone; the anchor lands on the oldest line left
    // instead of four lines further down.
    assert_eq!(messages(&app, 3), ["line 4", "line 5", "line 6"]);
}

#[test]
fn clearing_empties_the_pane_and_resumes_following() {
    let mut app = app_showing(3, 10);
    app.apply(Intent::Up);
    app.apply(Intent::ClearLogs);
    assert!(messages(&app, 3).is_empty());

    app.logs.push_line("after".into());
    assert_eq!(messages(&app, 3), ["after"]);
}
