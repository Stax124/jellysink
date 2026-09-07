pub(crate) mod auth;
pub(crate) mod playback;
pub(crate) mod profile;
pub(crate) mod session;
pub(crate) mod url;

/// Percent-encodes a value for a query string (RFC 3986 unreserved set). Ids
/// are usually GUIDs, but `LiveStreamId` can contain `+` and `=`.
pub(crate) fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
