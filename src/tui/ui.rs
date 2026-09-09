//! Rendering, and the terminal setup that has to be undone on the way out.

use super::app::{App, HomePane, Screen};
use crate::jellyfin::model::Item;
use crate::ticks::format_hms;
use color_eyre::eyre::Result;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

/// Enters the alternate screen and makes sure a panic cannot leave the user
/// in it — color_eyre's hook prints over a raw-mode terminal otherwise.
pub(super) fn enter() -> Result<DefaultTerminal> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave();
        previous(info);
    }));
    Ok(ratatui::try_init()?)
}

pub(super) fn leave() {
    ratatui::restore();
}

pub(super) fn render(app: &App, frame: &mut Frame) {
    let [header, body, footer, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_header(app, frame, header);
    match app.screen {
        Screen::Home => render_home(app, frame, body),
        Screen::Browse => render_browse(app, frame, body),
        Screen::Search => render_search(app, frame, body),
    }
    render_now_playing(app, frame, footer);
    render_hint(app, frame, hint);
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let tab = |label: &'static str, active: bool| {
        Span::styled(
            format!(" {label} "),
            if active {
                Style::default().fg(Color::Black).bg(ACCENT)
            } else {
                Style::default().fg(DIM)
            },
        )
    };
    let line = Line::from(vec![
        Span::styled(" jellytui ", Style::default().add_modifier(Modifier::BOLD)),
        tab("1 Home", app.screen == Screen::Home),
        tab("2 Libraries", app.screen == Screen::Browse),
        tab("/ Search", app.screen == Screen::Search),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_home(app: &App, frame: &mut Frame, area: Rect) {
    let [left, right] = Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);
    let focused = app.home_pane;
    render_list(
        frame,
        left,
        "Continue Watching",
        &app.resume,
        app.selected(),
        focused == HomePane::Resume,
    );
    render_list(
        frame,
        right,
        "Next Up",
        &app.next_up,
        app.selected(),
        focused == HomePane::NextUp,
    );
}

fn render_browse(app: &App, frame: &mut Frame, area: Rect) {
    let Some(level) = app.stack.last() else {
        return;
    };
    let trail = app
        .stack
        .iter()
        .map(|level| level.title.as_str())
        .collect::<Vec<_>>()
        .join(" › ");
    let title = if level.loading {
        format!("{trail} — loading…")
    } else {
        trail
    };
    render_list(frame, area, &title, &level.items, level.selected, true);
}

fn render_search(app: &App, frame: &mut Frame, area: Rect) {
    let [input, results] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    let search_box = Block::default().borders(Borders::ALL).title(" Search ");
    frame.render_widget(
        Paragraph::new(format!("{}▏", app.query)).block(search_box),
        input,
    );
    let title = if app.results.loading {
        "Results — loading…".to_string()
    } else {
        format!("Results ({})", app.results.items.len())
    };
    render_list(
        frame,
        results,
        &title,
        &app.results.items,
        app.results.selected,
        true,
    );
}

fn render_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: &[Item],
    selected: usize,
    focused: bool,
) {
    let border = Style::default().fg(if focused { ACCENT } else { DIM });
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(format!(" {title} "))
        .border_type(BorderType::Rounded);

    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("nothing here", Style::default().fg(DIM))).block(block),
            area,
        );
        return;
    }

    let rows: Vec<ListItem> = items.iter().map(row).collect();
    let list = List::new(rows).block(block).highlight_style(
        Style::default()
            .fg(ACCENT)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(focused.then_some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn row(item: &Item) -> ListItem<'static> {
    let mut spans = vec![
        Span::raw(if item.played() { "✓ " } else { "  " }),
        Span::raw(item.label()),
    ];
    if let Some(fraction) = item.watched_fraction() {
        spans.push(Span::styled(
            format!("  {}%", (fraction * 100.0).round() as u32),
            Style::default().fg(ACCENT),
        ));
    }
    if let Some(sublabel) = item.sublabel() {
        spans.push(Span::styled(
            format!("  · {sublabel}"),
            Style::default().fg(DIM),
        ));
    }
    ListItem::new(Line::from(spans))
}

fn render_now_playing(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Now Playing ");
    let Some(now_playing) = app.now_playing() else {
        let message = match (app.player_polled, app.player.is_some()) {
            (false, _) => "checking for jellysink…",
            (true, true) => "nothing playing",
            (true, false) => "jellysink not connected",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(message, Style::default().fg(DIM))).block(block),
            area,
        );
        return;
    };

    let position = now_playing.position_ticks;
    // The duration arrives a moment after the item does; until then the bar
    // stays empty rather than jumping.
    let total = app.total_ticks().unwrap_or(0);
    let ratio = if total > 0 {
        (position as f64 / total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let label = format!(
        "{} {}   {} / {}   vol {}{}   [{}/{}]",
        if now_playing.is_paused { "⏸" } else { "▶" },
        now_playing.title,
        format_hms(position),
        format_hms(total),
        now_playing.volume,
        if now_playing.is_muted { " (muted)" } else { "" },
        now_playing.queue_index + 1,
        now_playing.queue_len,
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [text, bar] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(label), text);
    frame.render_widget(
        Gauge::default()
            .ratio(ratio)
            .label("")
            .gauge_style(Style::default().fg(ACCENT)),
        bar,
    );
}

fn render_hint(app: &App, frame: &mut Frame, area: Rect) {
    // `q` is a character in the search box, so the quit key differs there.
    let keys = match app.screen {
        Screen::Search => {
            "type to search · ↑/↓ move · Enter play · Esc leave search · ⇧←/⇧→ seek · ^C quit"
        }
        _ => {
            "j/k move · Enter play · Esc back · / search · space pause · ⇧←/⇧→ seek · n/p track · +/- vol · m mute · f full · q quit"
        }
    };
    let text = if app.message.is_empty() {
        keys
    } else {
        &app.message
    };
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(DIM))),
        area,
    );
}

#[cfg(test)]
#[path = "ui_test.rs"]
mod tests;
