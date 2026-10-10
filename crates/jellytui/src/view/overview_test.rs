use super::*;
use ratatui::style::Color;

const BASE: Style = Style::new().fg(Color::DarkGray);

fn texts(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

#[test]
fn an_anilist_overview_keeps_its_breaks_and_styles_and_loses_its_tags() {
    let lines = lines(
        "<b>Rimuru</b> and his allies return to Tempest.\n<br><br>\n(Source: Crunchyroll)\
         <br><br><i>Note: Episode 24.9 is a recap.</i><br>",
        BASE,
    );
    assert_eq!(
        texts(&lines),
        [
            "Rimuru and his allies return to Tempest.",
            "",
            "(Source: Crunchyroll)",
            "",
            "Note: Episode 24.9 is a recap.",
        ]
    );
    assert_eq!(lines[0].spans[0].style, BASE.add_modifier(Modifier::BOLD));
    assert_eq!(lines[0].spans[1].style, BASE);
    assert_eq!(lines[4].spans[0].style, BASE.add_modifier(Modifier::ITALIC));
}

#[test]
fn entities_are_decoded_and_what_is_not_markup_is_left_as_written() {
    let lines = lines(
        "<a href=\"https://anidb.net/ch1\">Tom</a> &amp; Jerry&#x2019;s &mdash; 3 < 5 &copy;",
        BASE,
    );
    assert_eq!(texts(&lines), ["Tom & Jerry’s — 3 < 5 &copy;"]);
}

#[test]
fn only_a_blank_line_or_a_tag_breaks_a_line_as_in_markdown() {
    let plain = "First paragraph,\nwrapped by the provider.\n\n  Second paragraph.\n";
    assert_eq!(
        texts(&lines(plain, BASE)),
        [
            "First paragraph, wrapped by the provider.",
            "",
            "Second paragraph."
        ]
    );
    let paragraphs = "<p>First.</p>\n<P>Second.</P><p></p>";
    assert_eq!(texts(&lines(paragraphs, BASE)), ["First.", "", "Second."]);
}
