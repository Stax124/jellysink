//! Rendering: the terminal setup that has to be undone on the way out, the
//! layout every screen shares, and the chrome around the body.

pub(super) mod body;
pub(super) mod grid;
pub(super) mod playing;
pub(super) mod rail;

use crate::app::{App, Screen};
use body::{render_browse, render_home, render_search};
use color_eyre::eyre::Result;
use jellysink_core::ticks::format_hms;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{LineGauge, Paragraph};
use ratatui::{DefaultTerminal, Frame};

pub(super) const ACCENT: Color = Color::Cyan;
pub(super) const DIM: Color = Color::DarkGray;
/// Complaints only. In `ACCENT` they would read as another piece of chrome.
pub(super) const WARN: Color = Color::Yellow;
const OK: Color = Color::Green;
const BAD: Color = Color::Red;

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

/// The four horizontal bands of the screen. `App` needs the body to work out
/// which covers are on screen, so the split lives here rather than inline in
/// [`render`].
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
    let tabs = Line::from(spans);
    let status = daemon_status(app);
    // The message rides here rather than on the hint row: the bindings are
    // worth more than any complaint, and a complaint has to fit what is left
    // between the tabs and the dot.
    let room = area
        .width
        .saturating_sub(width_of(&tabs) + width_of(&status));
    let wanted = u16::try_from(app.message.chars().count()).unwrap_or(u16::MAX);
    let message = Line::from(Span::styled(
        to_width(&app.message, room.min(wanted)),
        Style::default().fg(WARN),
    ));
    let [tabs_area, message_area, status_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width_of(&message)),
        Constraint::Length(width_of(&status)),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(tabs), tabs_area);
    frame.render_widget(Paragraph::new(message), message_area);
    frame.render_widget(Paragraph::new(status), status_area);
}

/// Whether the daemon answered its status socket, which is the one thing the
/// whole frontend depends on. Before the first poll the answer is not yet
/// known, and saying "absent" then would be a lie for the first second.
fn daemon_status(app: &App) -> Line<'static> {
    let colour = match (app.player_polled, app.player.is_some()) {
        (false, _) => DIM,
        (true, true) => OK,
        (true, false) => BAD,
    };
    Line::from(vec![
        // The leading space is the gutter that keeps a full-width message off
        // the dot; it is invisible when there is no message.
        Span::styled(" ●", Style::default().fg(colour)),
        Span::styled(" jellysink ", Style::default().fg(DIM)),
    ])
}

fn render_now_playing(app: &App, frame: &mut Frame, area: Rect) {
    let [status, track] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let Some(now_playing) = app.now_playing() else {
        let message = match (app.player_polled, app.player.is_some()) {
            (false, _) => " checking for jellysink…",
            (true, true) => " nothing playing",
            (true, false) => " jellysink not connected",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(message, Style::default().fg(DIM))),
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
    // The duration arrives a moment after the item does; until then the bar
    // stays empty rather than jumping.
    let total = app.total_ticks().unwrap_or(0);
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

fn width_of(line: &Line) -> u16 {
    u16::try_from(line.width()).unwrap_or(u16::MAX)
}

fn render_hint(app: &App, frame: &mut Frame, area: Rect) {
    // `q` is a character in the search box, so the quit key differs there.
    let keys = match app.screen {
        // A shelf is one row, so up and down move between the two of them.
        Screen::Home => {
            "h/l move · j/k shelf · Enter play · / search · space pause · ⇧←/⇧→ seek · +/- vol · q quit"
        }
        Screen::Playing => {
            "j/k episode · Enter play · Esc back · space pause · ⇧←/⇧→ seek · n/p track · +/- vol · m mute · q quit"
        }
        Screen::Search => {
            "type to search · ↑/↓ move · Enter play · Esc leave search · ⇧←/⇧→ seek · ^C quit"
        }
        // In a grid every arrow moves, so back and open need naming.
        _ if app.grid_metrics().is_some() => {
            "h/j/k/l move · Enter open · Esc back · / search · space pause · ⇧←/⇧→ seek · +/- vol · q quit"
        }
        _ => {
            "j/k move · Enter play · Esc back · / search · space pause · ⇧←/⇧→ seek · n/p track · +/- vol · m mute · f full · q quit"
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled(keys, Style::default().fg(DIM))),
        area,
    );
}

/// Exactly `width` columns of text: elided if it overruns, padded if it falls
/// short, for the places that draw into a box of a fixed width.
pub(super) fn to_width(text: &str, width: u16) -> String {
    let width = usize::from(width);
    let mut fitted: String = text.chars().take(width).collect();
    if fitted.chars().count() < text.chars().count() {
        fitted.pop();
        fitted.push('…');
    }
    let short = width.saturating_sub(Line::from(fitted.as_str()).width());
    fitted.push_str(&" ".repeat(short));
    fitted
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
