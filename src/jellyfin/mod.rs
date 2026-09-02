pub(crate) mod auth;
pub(crate) mod playback;
pub(crate) mod profile;
pub(crate) mod session;
pub(crate) mod url;

/// Percent-encodes a value for use in a query string (RFC 3986 unreserved set).
///
/// Query parameters used to be appended raw. Item and user ids are server
/// GUIDs and survive that, but `LiveStreamId` is not a GUID for LiveTV or
/// transcode-managed sources and can legitimately contain `+` and `=` — pasted
/// in unencoded, the server sees a truncated or wrong id.
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
mod tests {
    use super::*;

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
}
