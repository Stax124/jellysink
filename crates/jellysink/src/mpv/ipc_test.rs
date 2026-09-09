use super::*;

#[test]
fn encode_is_one_json_line() {
    let line = encode_command(3, &[json!("get_property"), json!("pause")]);
    assert!(line.ends_with('\n'));
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["request_id"], 3);
    assert_eq!(v["command"][0], "get_property");
}

#[test]
fn time_pos_parses_integer_and_float_seconds() {
    assert_eq!(json_as_seconds(&json!(12)), Some(12.0));
    assert_eq!(json_as_seconds(&json!(12.5)), Some(12.5));
}

#[test]
fn time_pos_rejects_a_non_number_instead_of_zero() {
    assert_eq!(json_as_seconds(&json!(null)), None);
    assert_eq!(json_as_seconds(&json!("unavailable")), None);
}

#[test]
fn parse_reply_and_event() {
    let r = parse_ipc_line(r#"{"error":"success","data":12.5,"request_id":1}"#).unwrap();
    assert_eq!(
        r,
        IpcMessage::Reply {
            request_id: 1,
            error: "success".into(),
            data: json!(12.5),
        }
    );
    let e = parse_ipc_line(r#"{"event":"end-file","reason":"eof"}"#).unwrap();
    assert_eq!(
        e,
        IpcMessage::Event {
            name: "end-file".into(),
            reason: Some("eof".into()),
        }
    );
}

#[test]
fn a_property_change_parses_to_the_property_name() {
    assert_eq!(
        parse_ipc_line(r#"{"event":"property-change","id":1,"name":"sid","data":3}"#).unwrap(),
        IpcMessage::PropertyChange {
            property: "sid".into()
        }
    );
}

#[test]
fn property_coercions_accept_the_expected_shapes() {
    assert_eq!(as_i64_property("playlist-pos", &json!(3)).unwrap(), 3);
    // mpv reports -1 for playlist-pos while idle; that is a real answer.
    assert_eq!(as_i64_property("playlist-pos", &json!(-1)).unwrap(), -1);
    assert_eq!(as_f64_property("volume", &json!(62.5)).unwrap(), 62.5);
    assert!(as_bool_property("pause", &json!(true)).unwrap());
}

/// The regression: a null or wrong-typed answer used to become 0 / 100.0 /
/// false, and `playlist_eof` then picked an episode from it.

#[test]
fn property_coercions_reject_a_missing_or_wrong_typed_answer() {
    for v in [json!(null), json!("3"), json!({})] {
        assert!(
            as_i64_property("playlist-pos", &v).is_err(),
            "{v} should not coerce to an integer"
        );
    }
    assert!(as_f64_property("volume", &json!(null)).is_err());
    assert!(as_bool_property("pause", &json!(null)).is_err());
    assert!(as_bool_property("pause", &json!(1)).is_err());
}

#[test]
fn a_coercion_error_names_the_property_and_what_arrived() {
    let err = as_i64_property("playlist-count", &json!("nope")).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("playlist-count"), "{msg}");
    assert!(msg.contains("nope"), "{msg}");
}
