//! Rendering: the terminal setup that has to be undone on the way out, the
//! layout every screen shares, and the chrome around the body.

pub(super) mod body;
pub(super) mod grid;
mod logs;
pub(super) mod playing;
pub(super) mod rail;

use crate::app::{App, Daemon, Screen, UpdateCheck};
use body::{render_browse, render_home, render_search};
use color_eyre::eyre::{Result, WrapErr};
use jellysink_core::ticks::format_hms;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, LineGauge, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::borrow::Cow;

pub(super) const ACCENT: Color = Color::Cyan;
pub(super) const DIM: Color = Color::DarkGray;
/// Complaints only. In `ACCENT` they would read as another piece of chrome.
pub(super) const WARN: Color = Color::Yellow;
const OK: Color = Color::Green;
const BAD: Color = Color::Red;
/// The cursor's row in a list.
const SELECTED: Style = Style::new()
    .fg(ACCENT)
    .add_modifier(Modifier::REVERSED.union(Modifier::BOLD));

/// `try_init` also installs a panic hook that restores the terminal first.
pub(super) fn enter() -> Result<DefaultTerminal> {
    ratatui::try_init().wrap_err("entering the terminal's alternate screen")
}

fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
}

/// The four horizontal bands of the screen. `App` needs the body to work out
/// which covers are on screen, so the split is not inline in [`render`].
pub(super) struct Panes {
    pub(super) header: Rect,
    pub(super) body: Rect,
    pub(super) footer: Rect,
    pub(super) hint: Rect,
}

pub(super) fn panes(area: Rect) -> Panes {
    let [header, body, footer, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area);
    Panes {
        header,
        body,
        footer,
        hint,
    }
}

pub(super) fn render(app: &App, frame: &mut Frame) {
    let panes = panes(frame.area());

    render_header(app, frame, panes.header);
    match app.screen {
        Screen::Home => render_home(app, frame, panes.body),
        Screen::Browse => render_browse(app, frame, panes.body),
        Screen::Search => render_search(app, frame, panes.body),
        Screen::Playing => playing::render(app, frame, panes.body),
        Screen::Logs => logs::render(app, frame, panes.body),
    }
    render_now_playing(app, frame, panes.footer);
    render_hint(app, frame, panes.hint);
}

/// One highlighted-when-active tab. The key is a span of its own so that it
/// alone is bold — it is what the label is there to be reached by.
fn segment(key: &str, label: &str, active: bool) -> [Span<'static>; 2] {
    let style = if active {
        Style::default().fg(Color::Black).bg(ACCENT)
    } else {
        Style::default().fg(DIM)
    };
    [
        Span::styled(format!(" {key}"), style.add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {label} "), style),
    ]
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let mut spans = vec![Span::styled(
        " jellytui ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    for (key, label, active) in [
        ("1", "Home", app.screen == Screen::Home),
        ("2", "Libraries", app.screen == Screen::Browse),
        ("3", "Playing", app.screen == Screen::Playing),
        ("/", "Search", app.screen == Screen::Search),
    ] {
        spans.extend(segment(key, label, active));
    }
    // Not a tab one browses to: it names itself only while it is up.
    if app.screen == Screen::Logs {
        spans.extend(segment("L", "Logs", true));
    }
    let tabs = Line::from(spans);
    let offer = update_offer(app);
    let status = daemon_status(app);
    // The message rides here rather than on the hint row, where it would cost
    // the bindings their space.
    let room = area
        .width
        .saturating_sub(width_of(&tabs) + width_of(&offer) + width_of(&status));
    let wanted = width_of(&Line::from(app.message.as_str()));
    let message = Line::from(Span::styled(
        to_width(&app.message, room.min(wanted)),
        Style::default().fg(WARN),
    ));
    let [tabs_area, message_area, offer_area, status_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width_of(&message)),
        Constraint::Length(width_of(&offer)),
        Constraint::Length(width_of(&status)),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(tabs), tabs_area);
    frame.render_widget(Paragraph::new(message), message_area);
    frame.render_widget(Paragraph::new(offer), offer_area);
    frame.render_widget(Paragraph::new(status), status_area);
}

/// Names the key, because the hint rows are full and cannot.
fn update_offer(app: &App) -> Line<'static> {
    match &app.update {
        UpdateCheck::Available(version) => Line::from(Span::styled(
            format!(" ↑{version} u "),
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )),
        UpdateCheck::Pending | UpdateCheck::Failed | UpdateCheck::Current => Line::default(),
    }
}

fn daemon_status(app: &App) -> Line<'static> {
    let colour = match app.daemon {
        Daemon::Unknown => DIM,
        Daemon::Absent => BAD,
        Daemon::Connected(_) => OK,
    };
    Line::from(vec![
        // The leading space is the gutter that keeps a full-width message off
        // the dot.
        Span::styled(" ●", Style::default().fg(colour)),
        Span::styled(" jellysink ", Style::default().fg(DIM)),
    ])
}

fn render_now_playing(app: &App, frame: &mut Frame, area: Rect) {
    let [status, track] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let Some(now_playing) = app.now_playing() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {}", idle_text(&app.daemon)),
                Style::default().fg(DIM),
            )),
            status,
        );
        return;
    };

    let state = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            if now_playing.is_paused { "⏸" } else { "▶" },
            Style::default().fg(ACCENT),
        ),
        Span::raw("  "),
        Span::styled(
            now_playing.title.as_str(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]);
    let meta = Line::from(vec![
        Span::styled("vol ", Style::default().fg(DIM)),
        Span::raw(now_playing.volume.to_string()),
        Span::styled(
            if now_playing.is_muted { " muted" } else { "" },
            Style::default().fg(DIM),
        ),
        Span::styled("  ·  queue ", Style::default().fg(DIM)),
        Span::raw(format!(
            "{}/{} ",
            now_playing.queue_index + 1,
            now_playing.queue_len
        )),
    ]);
    let [state_area, meta_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(width_of(&meta))])
            .areas(status);
    frame.render_widget(Paragraph::new(state), state_area);
    frame.render_widget(Paragraph::new(meta), meta_area);

    let position = now_playing.position_ticks;
    // An item the server gives no duration for gets an empty bar, not a full one.
    let total = now_playing.run_time_ticks.unwrap_or(0);
    let ratio = if total > 0 {
        (position as f64 / total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let elapsed = Line::from(Span::styled(
        format!("  {} / {} ", format_hms(position), format_hms(total)),
        Style::default().fg(DIM),
    ));
    let [bar_area, elapsed_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(width_of(&elapsed))])
            .areas(track);
    frame.render_widget(
        LineGauge::default()
            .ratio(ratio)
            // The default label is a percentage, and it is drawn *before* the
            // track: an empty one leaves the single leading space we want.
            .label("")
            .filled_style(Style::default().fg(ACCENT))
            .unfilled_style(Style::default().fg(DIM))
            .filled_symbol(symbols::line::THICK_HORIZONTAL)
            .unfilled_symbol(symbols::line::THICK_HORIZONTAL),
        bar_area,
    );
    frame.render_widget(Paragraph::new(elapsed), elapsed_area);
}

/// What stands in for the playing item when there is none.
fn idle_text(daemon: &Daemon) -> &'static str {
    match daemon {
        Daemon::Unknown => "checking for jellysink…",
        Daemon::Absent => "jellysink not connected",
        Daemon::Connected(_) => "nothing playing",
    }
}

fn width_of(line: &Line) -> u16 {
    u16::try_from(line.width()).unwrap_or(u16::MAX)
}

fn render_hint(app: &App, frame: &mut Frame, area: Rect) {
    // `q` is a character in the search box, so the quit key differs there.
    let keys: Cow<str> = match app.screen {
        // A shelf is one row, so up and down move between the two of them.
        Screen::Home => "←/→ move · ↑/↓ shelf · Enter play · t watched · r reload · q quit".into(),
        Screen::Playing => {
            "↑/↓ episode · Enter play · Esc back · t watched · r reload · q quit".into()
        }
        Screen::Search => {
            "type to search · ↑/↓ move · Enter play · Esc leave search · ^C quit".into()
        }
        Screen::Logs => format!(
            "↑/↓ scroll · g/G top/bottom · c clear · L/Esc back · q quit · {}",
            if app.logs_following() {
                "following"
            } else {
                "paused"
            }
        )
        .into(),
        // In a grid every arrow moves, so back and open need naming.
        _ if app.grid_metrics().is_some() => {
            "↑/↓/←/→ move · Enter open · Esc back · t watched · r reload · q quit".into()
        }
        _ => "↑/↓ move · Enter play · Esc back · t watched · r reload · q quit".into(),
    };
    frame.render_widget(
        Paragraph::new(Span::styled(keys, Style::default().fg(DIM))),
        area,
    );
}

/// Exactly `width` columns of text, measured in display width: elided if it
/// overruns, padded if it falls short.
pub(super) fn to_width(text: &str, width: u16) -> String {
    let width = usize::from(width);
    let columns = |text: &str| Span::raw(text).width();
    let mut fitted = String::new();
    if columns(text) <= width {
        fitted.push_str(text);
    } else if width > 0 {
        let mut used = 0;
        for grapheme in Span::raw(text).styled_graphemes(Style::default()) {
            // One column is kept back for the ellipsis.
            used += columns(grapheme.symbol);
            if used >= width {
                break;
            }
            fitted.push_str(grapheme.symbol);
        }
        fitted.push('…');
    }
    let short = width.saturating_sub(columns(&fitted));
    fitted.push_str(&" ".repeat(short));
    fitted
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
