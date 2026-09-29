use super::*;

#[test]
fn url_direct_stream_without_token() {
    let url = direct_stream_url("http://h:8096", "item1", "src1", None, None);
    assert_eq!(
        url,
        "http://h:8096/Videos/item1/stream?static=true&MediaSourceId=src1"
    );
}

#[test]
fn url_puts_apikey_when_no_header() {
    let url = direct_stream_url("http://h:8096", "item1", "src1", None, Some("tok"));
    assert!(url.contains("ApiKey=tok"));
    assert!(url.contains("static=true"));
}

#[test]
fn redact_api_key_hides_a_trailing_token() {
    assert_eq!(
        redact_api_key("http://s/Videos/i/stream?static=true&ApiKey=sekrit"),
        "http://s/Videos/i/stream?static=true&ApiKey=<redacted>"
    );
}

#[test]
fn redact_api_key_keeps_later_parameters() {
    assert_eq!(
        redact_api_key("http://s/v?ApiKey=sekrit&LiveStreamId=xyz"),
        "http://s/v?ApiKey=<redacted>&LiveStreamId=xyz"
    );
}

#[test]
fn redact_api_key_leaves_a_tokenless_url_alone() {
    let url = "http://s/Videos/i/stream?static=true";
    assert_eq!(redact_api_key(url), url);
}

#[test]
fn direct_stream_url_encodes_a_live_stream_id_with_base64_padding() {
    let url = direct_stream_url("http://s", "item", "src", Some("ab+cd=="), None);
    assert!(url.contains("LiveStreamId=ab%2Bcd%3D%3D"), "{url}");
}

/// A raw `&` in a value used to start a new query parameter.
#[test]
fn direct_stream_url_values_cannot_inject_parameters() {
    let url = direct_stream_url("http://s", "item", "a&Foo=1", None, None);
    assert!(url.contains("MediaSourceId=a%26Foo%3D1"), "{url}");
    assert!(!url.contains("&Foo=1"), "{url}");
}

#[test]
fn image_url_asks_for_a_bounded_jpeg() {
    let url = image_url("http://h:8096/", "item1");
    assert_eq!(
        url,
        "http://h:8096/Items/item1/Images/Primary?maxWidth=600&maxHeight=600&format=Jpg&quality=85"
    );
}

#[test]
fn unreserved_characters_pass_through() {
    assert_eq!(encode_query_value("a-Z_0.9~"), "a-Z_0.9~");
}

/// The case that motivated this: a LiveStreamId carrying base64 padding.
#[test]
fn plus_and_equals_are_escaped() {
    assert_eq!(encode_query_value("ab+cd=="), "ab%2Bcd%3D%3D");
}

#[test]
fn separators_cannot_inject_extra_parameters() {
    assert_eq!(encode_query_value("x&Foo=1"), "x%26Foo%3D1");
}

#[test]
fn non_ascii_is_percent_encoded_per_utf8_byte() {
    assert_eq!(encode_query_value("é"), "%C3%A9");
}
