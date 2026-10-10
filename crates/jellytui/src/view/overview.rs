//! Overviews are markdown with basic HTML, as the web client renders them. This
//! renders the HTML and markdown's whitespace rule; markdown syntax shows as written.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

const ENTITIES: [(&str, char); 13] = [
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{a0}'),
    ("mdash", '—'),
    ("ndash", '–'),
    ("hellip", '…'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("ldquo", '“'),
    ("rdquo", '”'),
];

pub(super) fn lines(overview: &str, base: Style) -> Vec<Line<'static>> {
    let mut builder = Builder::new(base);
    let mut rest = overview;
    while let Some(next) = rest.chars().next() {
        let consumed = if next.is_whitespace() {
            let run = rest.len() - rest.trim_start().len();
            builder.whitespace(rest[..run].matches('\n').count());
            run
        } else if let Some((name, closing, length)) = tag(rest) {
            builder.tag(&name, closing);
            length
        } else if let Some((decoded, length)) = entity(rest) {
            builder.text(decoded);
            length
        } else {
            builder.text(next);
            next.len_utf8()
        };
        rest = &rest[consumed..];
    }
    builder.finish()
}

/// `<name …>` or `</name>` as its lowercased name, whether it closes, and its length.
fn tag(text: &str) -> Option<(String, bool, usize)> {
    let inner = text.strip_prefix('<')?;
    let end = inner.find('>')?;
    let (closing, body) = match inner[..end].strip_prefix('/') {
        Some(body) => (true, body),
        None => (false, &inner[..end]),
    };
    let name: String = body
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    name.starts_with(|first: char| first.is_ascii_alphabetic())
        .then(|| (name.to_ascii_lowercase(), closing, end + 2))
}

/// `&name;` or `&#…;` as the character it stands for, and its length.
fn entity(text: &str) -> Option<(char, usize)> {
    let body = text.strip_prefix('&')?;
    let end = body.find(';')?;
    let name = &body[..end];
    let decoded = match name.strip_prefix('#') {
        Some(number) => match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => number.parse().ok(),
        }
        .and_then(char::from_u32),
        None => ENTITIES
            .iter()
            .find(|(entity, _)| *entity == name)
            .map(|(_, decoded)| *decoded),
    }?;
    Some((decoded, end + 2))
}

struct Builder {
    base: Style,
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    text: String,
    italic: u16,
    bold: u16,
    /// Whitespace waiting for more text on the same line, which is the only place it shows.
    space: bool,
}

impl Builder {
    fn new(base: Style) -> Self {
        Self {
            base,
            lines: Vec::new(),
            spans: Vec::new(),
            text: String::new(),
            italic: 0,
            bold: 0,
            space: false,
        }
    }

    fn text(&mut self, next: char) {
        if std::mem::take(&mut self.space) && !(self.text.is_empty() && self.spans.is_empty()) {
            self.text.push(' ');
        }
        self.text.push(next);
    }

    fn whitespace(&mut self, newlines: usize) {
        if newlines >= 2 {
            self.paragraph();
        } else {
            self.space = true;
        }
    }

    fn tag(&mut self, name: &str, closing: bool) {
        let step = |depth: u16| {
            if closing {
                depth.saturating_sub(1)
            } else {
                depth.saturating_add(1)
            }
        };
        match name {
            "br" => self.end_line(),
            "p" => self.paragraph(),
            "i" | "em" => {
                self.flush();
                self.italic = step(self.italic);
            }
            "b" | "strong" => {
                self.flush();
                self.bold = step(self.bold);
            }
            _ => {}
        }
    }

    fn flush(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let mut style = self.base;
        if self.italic > 0 {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.bold > 0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        self.spans
            .push(Span::styled(std::mem::take(&mut self.text), style));
    }

    fn end_line(&mut self) {
        self.flush();
        self.lines.push(Line::from(std::mem::take(&mut self.spans)));
        self.space = false;
    }

    fn paragraph(&mut self) {
        self.end_line();
        self.lines.push(Line::default());
    }

    /// Breaks pile up wherever tags and blank lines meet, so the blank lines are
    /// collapsed to one and trimmed from both ends.
    fn finish(mut self) -> Vec<Line<'static>> {
        self.end_line();
        let mut lines: Vec<Line<'static>> = Vec::new();
        for line in self.lines {
            let blank = line.spans.is_empty();
            if blank && lines.last().is_none_or(|last| last.spans.is_empty()) {
                continue;
            }
            lines.push(line);
        }
        if lines.last().is_some_and(|last| last.spans.is_empty()) {
            lines.pop();
        }
        lines
    }
}

#[cfg(test)]
#[path = "overview_test.rs"]
mod tests;
