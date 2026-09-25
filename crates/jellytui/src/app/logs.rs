//! Where the log pane is scrolled to. The buffer itself is `crate::logs`.

use super::*;

impl App {
    /// The lines to draw. Anchored by sequence number, so eviction slides
    /// rather than jumps the view.
    pub(crate) fn log_window(&self, height: u16) -> Vec<LogLine> {
        let height = usize::from(height);
        let (_, start, _) = self.log_start(height);
        self.logs.window(start, height)
    }

    pub(crate) fn logs_following(&self) -> bool {
        self.log_anchor.is_none()
    }

    /// `(first_seq, start, tail)`: the top line's sequence base, its index, and
    /// the furthest down it can go.
    fn log_start(&self, height: usize) -> (u64, usize, usize) {
        let (first_seq, len) = self.logs.extent();
        let tail = len.saturating_sub(height);
        let start = self
            .log_anchor
            .map_or(tail, |anchor| {
                usize::try_from(anchor.saturating_sub(first_seq)).unwrap_or(usize::MAX)
            })
            .min(tail);
        (first_seq, start, tail)
    }

    fn scroll_logs(&mut self, delta: isize) {
        let (first_seq, start, tail) = self.log_start(usize::from(self.body_area().height));
        let moved = start.saturating_add_signed(delta).min(tail);
        // Reaching the bottom resumes following: a view pinned exactly at the
        // tail would otherwise stop moving as soon as the next line arrived.
        self.log_anchor = (moved < tail).then(|| first_seq + moved as u64);
    }

    /// Whether the log pane claimed this intent. Movement means scrolling
    /// here, and Esc leaves — the rest falls through to [`App::apply`].
    pub(super) fn scroll_in_logs(&mut self, intent: &Intent) -> bool {
        let page = self.body_area().height.saturating_sub(1).max(1) as isize;
        match intent {
            Intent::Up => self.scroll_logs(-1),
            Intent::Down => self.scroll_logs(1),
            Intent::PageUp => self.scroll_logs(-page),
            Intent::PageDown => self.scroll_logs(page),
            Intent::Top => self.log_anchor = Some(self.logs.extent().0),
            Intent::Bottom => self.log_anchor = None,
            Intent::ClearLogs => {
                self.logs.clear();
                self.log_anchor = None;
            }
            Intent::Back => self.toggle_logs(),
            _ => return false,
        }
        true
    }

    /// `L` is a toggle, so it has to remember what it covered up.
    pub(super) fn toggle_logs(&mut self) {
        self.screen = if self.screen == Screen::Logs {
            self.screen_before_logs
        } else {
            self.screen_before_logs = self.screen;
            Screen::Logs
        };
    }
}

#[cfg(test)]
#[path = "logs_test.rs"]
mod tests;
