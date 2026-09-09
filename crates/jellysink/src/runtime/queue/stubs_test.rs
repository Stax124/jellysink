use super::*;

#[test]
fn a_stub_row_with_a_known_title_never_carries_the_token_in_the_title() {
    let (title, url) = playlist_stub_entry("http://s", "e1", Some("Show - s1e01"), Some("tok"));
    assert_eq!(title, "Show - s1e01");
    assert!(url.contains("ApiKey=tok"));
}

/// The leak this guards: the fallback used to be the playable URL, so an
/// episode missing from the series listing showed the access token in mpv's
/// playlist selector and wrote it to the user's watch_later files.
#[test]
fn a_stub_row_without_a_title_falls_back_to_a_tokenless_url() {
    let (title, url) = playlist_stub_entry("http://s", "e1", None, Some("tok"));
    assert!(!title.contains("ApiKey"), "title leaked the token: {title}");
    assert!(title.contains("e1"), "title should still identify the row");
    assert!(
        url.contains("ApiKey=tok"),
        "the playable url still needs auth"
    );
}

#[test]
fn no_token_goes_on_the_url_when_mpv_carries_the_auth_header() {
    let (_, url) = playlist_stub_entry("http://s", "e1", Some("t"), None);
    assert!(!url.contains("ApiKey"), "{url}");
}
