//! The log pane: what `crate::logs` captured, scrolled to where `App` says.

use super::{ACCENT, DIM, WARN};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tracing::Level;

pub(super) fn render(app: &App, frame: &mut Frame, area: Rect) {
    let (lines, _) = app.log_window(area.height);
    if lines.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                " nothing logged yet · RUST_LOG=jellytui=debug for a line per keypress",
                Style::default().fg(DIM),
            )),
            area,
        );
        return;
    }
    let rows: Vec<Line> = lines
        .iter()
        .map(|line| {
            Line::from(vec![
                Span::styled(
                    format!(" {:>8.3} ", line.elapsed.as_secs_f64()),
                    Style::default().fg(DIM),
                ),
                Span::styled(
                    format!("{:<5} ", level_name(line.level)),
                    level_style(line.level),
                ),
                Span::styled(format!("{} ", line.target), Style::default().fg(DIM)),
                Span::raw(line.message.clone()),
            ])
        })
        .collect();
    // No `wrap`: an unwrapped `Paragraph` clips at the right edge, which is
    // what keeps the scroll arithmetic in whole lines.
    frame.render_widget(Paragraph::new(rows), area);
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARN",
        Level::INFO => "INFO",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "TRACE",
    }
}

fn level_style(level: Level) -> Style {
    let colour = match level {
        Level::ERROR => Color::Red,
        Level::WARN => WARN,
        Level::INFO => ACCENT,
        Level::DEBUG | Level::TRACE => DIM,
    };
    Style::default().fg(colour)
}
