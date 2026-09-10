//! Where the log pane is scrolled to. The buffer itself is `crate::logs`.

use super::*;

impl App {
    /// The lines to draw and whether the view is still following the tail.
    /// Anchored by sequence number, so eviction under a pinned view slides the
    /// window rather than jumping it.
    pub(crate) fn log_window(&self, height: u16) -> (Vec<LogLine>, bool) {
        let height = usize::from(height);
        let (first_seq, len) = self.logs.extent();
        let tail = len.saturating_sub(height);
        let Some(anchor) = self.log_anchor else {
            return (self.logs.window(tail, height), true);
        };
        let start = usize::try_from(anchor.saturating_sub(first_seq))
            .unwrap_or(usize::MAX)
            .min(tail);
        (self.logs.window(start, height), start == tail)
    }

    pub(crate) fn scroll_logs(&mut self, delta: isize) {
        let height = usize::from(self.body_area().height);
        let (first_seq, len) = self.logs.extent();
        let tail = len.saturating_sub(height);
        let start = self
            .log_anchor
            .map_or(tail, |anchor| {
                usize::try_from(anchor.saturating_sub(first_seq)).unwrap_or(usize::MAX)
            })
            .min(tail);
        let moved = start.saturating_add_signed(delta).min(tail);
        // Reaching the bottom resumes following: a view pinned exactly at the
        // tail would otherwise stop moving as soon as the next line arrived.
        self.log_anchor = (moved < tail).then(|| first_seq + moved as u64);
    }

    pub(crate) fn logs_to_top(&mut self) {
        let (first_seq, _) = self.logs.extent();
        self.log_anchor = Some(first_seq);
    }

    pub(crate) fn logs_to_bottom(&mut self) {
        self.log_anchor = None;
    }

    pub(crate) fn clear_logs(&mut self) {
        self.logs.clear();
        self.log_anchor = None;
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
            Intent::Top => self.logs_to_top(),
            Intent::Bottom => self.logs_to_bottom(),
            Intent::ClearLogs => self.clear_logs(),
            Intent::Back => self.toggle_logs(),
            _ => return false,
        }
        true
    }

    /// `L` is a toggle, so it has to remember what it covered up.
    pub(crate) fn toggle_logs(&mut self) {
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
