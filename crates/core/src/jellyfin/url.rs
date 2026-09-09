//! Building — and redacting — Jellyfin URLs.
use super::encode_query_value;

/// Stream URLs carry the access token whenever the Authorization header is not
/// in play, and those URLs end up in `Debug` output and color-eyre captures.
pub fn redact_api_key(url: &str) -> String {
    let Some(at) = url.find("ApiKey=") else {
        return url.to_string();
    };
    let value_start = at + "ApiKey=".len();
    let value_end = url[value_start..]
        .find('&')
        .map_or(url.len(), |i| value_start + i);
    format!("{}<redacted>{}", &url[..value_start], &url[value_end..])
}

pub fn direct_stream_url(
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

/// The item's primary image (cover art / thumbnail), for MPRIS `mpris:artUrl`.
/// Carries the token in the query string like [`direct_stream_url`] — this
/// URL is handed to a desktop widget to fetch itself, not read server-side by
/// jellysink, so it is redacted by [`redact_api_key`] wherever it is logged.
pub fn image_url(server: &str, item_id: &str, token: &str) -> String {
    let server = server.trim_end_matches('/');
    format!(
        "{server}/Items/{item_id}/Images/Primary?ApiKey={}",
        encode_query_value(token)
    )
}

#[cfg(test)]
#[path = "url_test.rs"]
mod tests;
