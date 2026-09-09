//! The three screen bodies: the Home shelves, a browse level and search
//! results.

use super::{ACCENT, DIM};
use crate::app::{App, HomePane};
use crate::view::{grid, rail};
use jellysink_core::jellyfin::model::Item;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};

/// The two Home shelves, one above the other. Each is a single row of tiles,
/// which is all half the body has the height for.
pub(super) fn shelves(body: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(body)
}

pub(crate) fn shelf_rect(body: Rect, pane: HomePane) -> Rect {
    let [resume, next_up] = shelves(body);
    match pane {
        HomePane::Resume => resume,
        HomePane::NextUp => next_up,
    }
}

pub(super) fn render_home(app: &App, frame: &mut Frame, area: Rect) {
    for (pane, rect) in HomePane::ALL.into_iter().zip(shelves(area)) {
        let shelf = app.shelf(pane);
        grid::render(
            frame,
            rect,
            Line::from(format!(" {} ", pane.title())),
            grid::View {
                items: &shelf.items,
                selected: shelf.selected,
                offset: shelf.offset,
                rows: grid::SHELF_ROWS,
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
            grid::View {
                items: &level.items,
                selected: level.selected,
                offset: level.offset,
                rows: grid::TARGET_ROWS,
                focused: true,
            },
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

pub(super) fn render_search(app: &App, frame: &mut Frame, area: Rect) {
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
    if let Some(fraction) = item.watched_fraction().filter(|_| !item.played()) {
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
