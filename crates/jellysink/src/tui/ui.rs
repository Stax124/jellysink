//! Rendering, and the terminal setup that has to be undone on the way out.

use super::app::{App, HomePane, Screen};
use super::grid;
use super::playing;
use super::rail;
use color_eyre::eyre::Result;
use jellysink_core::jellyfin::model::Item;
use jellysink_core::ticks::format_hms;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, LineGauge, List, ListItem, ListState, Paragraph,
};
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

/// One highlighted-when-active label, shared by the header's tabs and the
/// pair of pane names in the Home grid's title.
fn segment(label: &str, active: bool) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        if active {
            Style::default().fg(Color::Black).bg(ACCENT)
        } else {
            Style::default().fg(DIM)
        },
    )
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let tabs = Line::from(vec![
        Span::styled(" jellytui ", Style::default().add_modifier(Modifier::BOLD)),
        segment("1 Home", app.screen == Screen::Home),
        segment("2 Libraries", app.screen == Screen::Browse),
        segment("3 Playing", app.screen == Screen::Playing),
        segment("/ Search", app.screen == Screen::Search),
    ]);
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

/// One grid at a time, with Tab swapping which list feeds it — two stacked
/// grids would leave each a single row of tiles.
fn render_home(app: &App, frame: &mut Frame, area: Rect) {
    let title = Line::from(vec![
        segment("Continue Watching", app.home_pane == HomePane::Resume),
        segment("Next Up", app.home_pane == HomePane::NextUp),
    ]);
    grid::render(
        frame,
        area,
        title,
        app.home_rows(),
        app.home_selected,
        app.grid_offset(),
        &app.covers,
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
    if app.grid_metrics().is_some() {
        grid::render(
            frame,
            area,
            Line::from(format!(" {title} ")),
            &level.items,
            level.selected,
            level.offset,
            &app.covers,
        );
        return;
    }
    let (area, rail_area) = rail::split(area);
    render_list(frame, area, &title, &level.items, level.selected, true);
    if let Some(rail_area) = rail_area {
        rail::render(
            frame,
            rail_area,
            level.items.get(level.selected),
            &app.covers,
        );
    }
}

fn render_search(app: &App, frame: &mut Frame, area: Rect) {
    let (area, rail_area) = rail::split(area);
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
    if let Some(rail_area) = rail_area {
        rail::render(
            frame,
            rail_area,
            app.results.items.get(app.results.selected),
            &app.covers,
        );
    }
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

pub(super) fn row(item: &Item) -> ListItem<'static> {
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
#[path = "ui_test.rs"]
mod tests;
