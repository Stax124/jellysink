use super::*;

#[test]
fn parse_log_filter_accepts_a_level() {
    let filter = parse_log_filter("debug").unwrap();
    let rendered = filter.to_string();
    assert!(rendered.contains("debug"), "expected debug in {rendered:?}");
}

#[test]
fn parse_log_filter_accepts_target_directives() {
    let filter = parse_log_filter("jellysink=trace,warn").unwrap();
    let rendered = filter.to_string();
    assert!(
        rendered.contains("jellysink"),
        "expected target in {rendered:?}"
    );
}

#[test]
fn validate_log_level_accepts_levels_and_target_filters() {
    for spec in [
        "info",
        "TRACE",
        "off",
        "jellysink=debug,warn",
        "jellysink=trace",
    ] {
        validate_log_level(spec).unwrap_or_else(|e| panic!("{spec:?} should be valid: {e}"));
    }
}

/// `Targets` reads a bare word as a target name, so this parses — and then
/// silences every jellysink log.
#[test]
fn validate_log_level_rejects_a_bare_word_that_is_not_a_level() {
    assert!(parse_log_filter("banana").is_ok(), "premise of the test");
    assert!(validate_log_level("banana").is_err());
}

#[test]
fn validate_log_level_rejects_an_unknown_level_in_a_target_filter() {
    assert!(validate_log_level("jellysink=banana").is_err());
}
