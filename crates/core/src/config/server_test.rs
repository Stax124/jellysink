use super::*;

#[test]
fn bare_host_gets_http_and_no_port() {
    assert_eq!(
        normalize_server_url("192.168.1.10").unwrap(),
        "http://192.168.1.10"
    );
}

#[test]
fn written_port_is_kept() {
    assert_eq!(
        normalize_server_url("192.168.1.10:8096").unwrap(),
        "http://192.168.1.10:8096"
    );
    assert_eq!(
        normalize_server_url("https://media.local:8920").unwrap(),
        "https://media.local:8920"
    );
}

#[test]
fn default_port_is_left_implicit() {
    assert_eq!(
        normalize_server_url("http://media.local:80").unwrap(),
        "http://media.local"
    );
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

#[test]
fn non_http_scheme_is_rejected() {
    assert!(normalize_server_url("ftp://media.local").is_err());
}
