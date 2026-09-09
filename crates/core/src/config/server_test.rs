use super::*;

#[test]
fn bare_host_gets_http_and_8096() {
    assert_eq!(
        normalize_server_url("192.168.1.10").unwrap(),
        "http://192.168.1.10:8096"
    );
}

#[test]
fn explicit_port_80_is_kept() {
    assert_eq!(
        normalize_server_url("http://media.local:80").unwrap(),
        "http://media.local:80"
    );
}

#[test]
fn https_without_port_is_not_given_8096() {
    assert_eq!(
        normalize_server_url("https://jellyfin.example").unwrap(),
        "https://jellyfin.example"
    );
}

#[test]
fn subpath_is_kept() {
    assert_eq!(
        normalize_server_url("http://host:8096/jellyfin/").unwrap(),
        "http://host:8096/jellyfin"
    );
}

#[test]
fn scheme_typo_without_slashes_is_rejected() {
    assert!(normalize_server_url("http//").is_err());
    assert!(normalize_server_url("http/media.local").is_err());
    assert!(normalize_server_url("https:").is_err());
    assert!(normalize_server_url("http").is_err());
}
