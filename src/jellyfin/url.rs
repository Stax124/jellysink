//! Building — and redacting — Jellyfin URLs.
use super::encode_query_value;

/// Replaces the value of an `ApiKey=` query parameter with a placeholder.
///
/// Stream URLs carry the access token whenever the Authorization header is not
/// in play, and those URLs end up in `Debug` output and color-eyre captures.
pub(crate) fn redact_api_key(url: &str) -> String {
    let Some(at) = url.find("ApiKey=") else {
        return url.to_string();
    };
    let value_start = at + "ApiKey=".len();
    let value_end = url[value_start..]
        .find('&')
        .map_or(url.len(), |i| value_start + i);
    format!("{}<redacted>{}", &url[..value_start], &url[value_end..])
}

pub(crate) fn direct_stream_url(
    server: &str,
    item_id: &str,
    media_source_id: &str,
    live_stream_id: Option<&str>,
    token: Option<&str>,
) -> String {
    let server = server.trim_end_matches('/');
    let mut url = format!(
        "{server}/Videos/{item_id}/stream?static=true&MediaSourceId={}",
        encode_query_value(media_source_id)
    );
    if let Some(live) = live_stream_id {
        url.push_str("&LiveStreamId=");
        url.push_str(&encode_query_value(live));
    }
    if let Some(token) = token {
        url.push_str("&ApiKey=");
        url.push_str(&encode_query_value(token));
    }
    url
}

#[cfg(test)]
mod tests {
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
}
