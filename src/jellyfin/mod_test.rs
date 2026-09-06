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
