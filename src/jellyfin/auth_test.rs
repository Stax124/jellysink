use super::*;

#[test]
fn header_without_token() {
    let h = authorization_header("box", "abc", None);
    assert!(h.starts_with("MediaBrowser Client=\"jellysink\""));
    assert!(h.contains("Device=\"box\""));
    assert!(h.contains("DeviceId=\"abc\""));
    assert!(!h.contains("Token="));
}

#[test]
fn header_with_token() {
    let h = authorization_header("box", "abc", Some("sekrit"));
    assert!(h.contains("Token=\"sekrit\""));
}

#[test]
fn quotes_stripped_from_device_name() {
    let h = authorization_header(r#"weird"name"#, "id", None);
    assert!(!h.contains(r#"Device="weird"name""#));
    assert!(h.contains("Device=\"weirdname\""));
}

#[test]
fn auth_expired_is_recognised_through_added_context() {
    let err = color_eyre::Report::new(AuthExpired)
        .wrap_err("PlaybackInfo")
        .wrap_err("starting the current item");
    assert!(is_auth_expired(&err));
}

/// The bug the typed error replaces: the old check was
/// `format!("{e:#}").contains("401")`, and the chain carries the URL.
#[test]
fn an_unrelated_error_mentioning_401_is_not_an_auth_failure() {
    let err = color_eyre::eyre::eyre!("GET http://media.example:401/Items/4013");
    assert!(format!("{err:#}").contains("401"), "premise of the test");
    assert!(!is_auth_expired(&err));
}

#[test]
fn api_debug_never_prints_the_token() {
    let api = Api::from_credentials(&Credentials {
        server: "http://s".into(),
        username: "u".into(),
        user_id: "uid".into(),
        access_token: "sekrit".into(),
        device_id: "d".into(),
    })
    .unwrap();
    let rendered = format!("{api:?}");
    assert!(!rendered.contains("sekrit"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
}

#[test]
fn the_auth_header_is_computed_once_and_matches_the_free_function() {
    let creds = Credentials {
        server: "http://s/".into(),
        username: "u".into(),
        user_id: "uid".into(),
        access_token: "sekrit".into(),
        device_id: "dev".into(),
    };
    let api = Api::from_credentials(&creds).unwrap();
    assert_eq!(
        api.auth_header(),
        authorization_header(&api.device_name, "dev", Some("sekrit"))
    );
    assert_eq!(api.server, "http://s", "trailing slash is trimmed");
    assert!(api.mpv_auth_header_field().starts_with("Authorization: "));
}
