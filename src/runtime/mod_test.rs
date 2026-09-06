use super::*;

#[test]
fn a_quick_failure_keeps_the_grown_backoff() {
    let grown = Duration::from_secs(8);
    assert_eq!(
        reconnect_delay(grown, Duration::from_secs(2), false),
        grown,
        "a session that died immediately should keep backing off"
    );
}

#[test]
fn a_healthy_session_resets_the_backoff() {
    assert_eq!(
        reconnect_delay(BACKOFF_MAX, SESSION_HEALTHY_AFTER, false),
        BACKOFF_MIN,
        "a session that stayed up should reconnect promptly"
    );
}

#[test]
fn an_expired_token_backs_off_to_the_maximum_however_long_the_session_ran() {
    assert_eq!(
        reconnect_delay(BACKOFF_MIN, Duration::from_secs(3600), true),
        BACKOFF_MAX,
        "retrying a rejected token fast is pointless"
    );
}
