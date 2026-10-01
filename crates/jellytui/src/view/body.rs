//! The three screen bodies: the Home shelves, a browse level and search
//! results.

use super::{ACCENT, DIM, SELECTED, panel};
use crate::app::{App, HomePane};
use crate::nav::{self, Mode, Rows};
use crate::view::{grid, rail, synopsis};
use jellysink_core::jellyfin::model::Item;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

/// The two Home shelves, one above the other and indexed by `HomePane`. Each
/// is a single row of tiles, which is all half the body has the height for.
pub(crate) fn shelves(body: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(body)
}

pub(super) fn render_home(app: &App, frame: &mut Frame, area: Rect) {
    for (pane, rect) in HomePane::ALL.into_iter().zip(shelves(area)) {
        grid::render(
            frame,
            rect,
            Line::from(format!(" {} ", pane.title())),
            grid::View {
                rows: app.shelf(pane),
                metrics: app.shelf_metrics(pane),
                focused: app.home_pane == pane,
            },
            &app.covers,
        );
    }
}

pub(super) fn render_browse(app: &App, frame: &mut Frame, area: Rect) {
    let Some(level) = app.stack.last() else {
        return;
    };
    let trail = app
        .stack
        .iter()
        .map(|level| level.title.as_str())
        .collect::<Vec<_>>()
        .join(" › ");
    let title = if level.rows.loading {
        format!("{trail} — loading…")
    } else {
        trail
    };
    if let Some(metrics) = app.grid_metrics() {
        let (area, band) = match nav::mode(&level.rows.items) {
            Mode::Episodes => {
                let [grid, band] = synopsis::split(area);
                (grid, Some(band))
            }
            Mode::Grid | Mode::List => (area, None),
        };
        grid::render(
            frame,
            area,
            Line::from(format!(" {title} ")),
            grid::View {
                rows: &level.rows,
                metrics: Some(metrics),
                focused: true,
            },
            &app.covers,
        );
        if let (Some(band), Some(item)) = (band, level.rows.selected_item()) {
            synopsis::render(frame, band, item);
        }
        return;
    }
    let area = rail::beside(frame, area, level.rows.selected_item(), &app.covers);
    render_list(frame, area, &title, &level.rows);
}

pub(super) fn render_search(app: &App, frame: &mut Frame, area: Rect) {
    let area = rail::beside(frame, area, app.results.selected_item(), &app.covers);
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
    render_list(frame, results, &title, &app.results);
}

fn render_list(frame: &mut Frame, area: Rect, title: &str, rows: &Rows) {
    let block = panel(format!(" {title} ")).border_style(Style::default().fg(ACCENT));

    if rows.items.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("nothing here", Style::default().fg(DIM))).block(block),
            area,
        );
        return;
    }

    let list = List::new(rows.items.iter().map(row))
        .block(block)
        .highlight_style(SELECTED);
    let mut state = ListState::default();
    state.select(Some(rows.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

pub(super) fn row(item: &Item) -> ListItem<'static> {
    let mut spans = vec![
        Span::raw(if item.played() { "✓ " } else { "  " }),
        Span::raw(item.label()),
    ];
    if let Some(percent) = watched_percent(item) {
        spans.push(Span::styled(percent, Style::default().fg(ACCENT)));
    }
    if let Some(sublabel) = item.sublabel() {
        spans.push(Span::styled(
            format!("  · {sublabel}"),
            Style::default().fg(DIM),
        ));
    }
    ListItem::new(Line::from(spans))
}

/// A finished item is ticked rather than given a percentage.
fn watched_percent(item: &Item) -> Option<String> {
    let fraction = item.watched_fraction().filter(|_| !item.played())?;
    Some(format!("  {}%", (fraction * 100.0).round() as u32))
}
