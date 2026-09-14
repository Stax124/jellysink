use super::*;
use serde_json::json;

#[test]
fn a_number_coerces_from_whichever_json_shape_it_arrived_in() {
    assert_eq!(coerce_i64(&json!(-12)), Some(-12));
    assert_eq!(coerce_i64(&json!(12.9)), Some(12));
    assert_eq!(coerce_i64(&json!("9000000000")), Some(9_000_000_000));
    assert_eq!(coerce_u64(&json!(60)), Some(60));
    assert_eq!(coerce_u64(&json!(60.0)), Some(60));
    assert_eq!(coerce_f64(&json!(12)), Some(12.0));
    assert_eq!(coerce_f64(&json!(12.5)), Some(12.5));
}

/// The regression: a null or wrong-typed mpv answer became 0, and
/// `playlist_eof` then picked an episode from it.
#[test]
fn a_non_number_is_none_rather_than_zero() {
    assert_eq!(coerce_f64(&json!(null)), None);
    assert_eq!(coerce_f64(&json!("unavailable")), None);
    assert_eq!(coerce_i64(&json!("nope")), None);
    assert_eq!(coerce_u64(&json!({})), None);
}

#[test]
fn a_list_field_holding_one_id_arrives_unwrapped() {
    assert_eq!(string_list(&json!("abc")), ["abc"]);
    assert_eq!(string_list(&json!(["a", 1, "b"])), ["a", "b"]);
    assert!(string_list(&json!(null)).is_empty());
}
