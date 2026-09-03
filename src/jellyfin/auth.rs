use crate::config::{Credentials, device_name, normalize_server_url};
use crate::usage_err;
use crate::{CLIENT_NAME, VERSION};
use color_eyre::eyre::WrapErr;
use serde::Deserialize;
use serde_json::json;
use std::fmt;

/// The server rejected our access token.
///
/// Typed so the reconnect loop can recognise it without substring-matching a
/// formatted error chain. That chain carries the request URL, so matching on
/// `"401"` also fired for a server on port 401 or an item id containing `401`.
#[derive(Debug)]
pub(crate) struct AuthExpired;

impl fmt::Display for AuthExpired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("server returned 401; run `jellysink login` again")
    }
}

impl std::error::Error for AuthExpired {}

/// Whether `err` was caused by an expired token, at any depth. `Report`'s own
/// `downcast_ref` only inspects the outermost error, so a caller adding
/// `wrap_err` context would hide it.
pub(crate) fn is_auth_expired(err: &color_eyre::Report) -> bool {
    err.chain().any(|cause| cause.is::<AuthExpired>())
}

pub(crate) fn authorization_header(device: &str, device_id: &str, token: Option<&str>) -> String {
    let device = sanitize_token_field(device);
    let mut header = format!(
        r#"MediaBrowser Client="{CLIENT_NAME}", Device="{device}", DeviceId="{device_id}", Version="{VERSION}""#
    );
    if let Some(token) = token {
        header.push_str(&format!(r#", Token="{token}""#));
    }
    header
}

fn sanitize_token_field(s: &str) -> String {
    s.replace(['"', '\\'], "")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthResponse {
    access_token: String,
    user: AuthUser,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthUser {
    id: String,
    name: String,
}

#[derive(Clone)]
pub(crate) struct Api {
    pub(crate) http: reqwest::Client,
    pub(crate) server: String,
    pub(crate) token: String,
    pub(crate) device_id: String,
    pub(crate) device_name: String,
    pub(crate) user_id: String,
    /// Precomputed: it is the same for the process lifetime, and every request
    /// needs it — including a progress report once a second.
    auth_header: String,
}

impl fmt::Debug for Api {
    /// Hand-written so `token` cannot reach a log line or a color-eyre capture.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("server", &self.server)
            .field("token", &"<redacted>")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

impl Api {
    pub(crate) fn from_credentials(creds: &Credentials) -> color_eyre::Result<Self> {
        let device_name = device_name();
        Ok(Self {
            http: http_client()?,
            server: creds.server.trim_end_matches('/').to_string(),
            auth_header: authorization_header(
                &device_name,
                &creds.device_id,
                Some(&creds.access_token),
            ),
            token: creds.access_token.clone(),
            device_id: creds.device_id.clone(),
            device_name,
            user_id: creds.user_id.clone(),
        })
    }

    pub(crate) fn auth_header(&self) -> &str {
        &self.auth_header
    }

    pub(crate) fn mpv_auth_header_field(&self) -> String {
        format!("Authorization: {}", self.auth_header())
    }

    /// Attaches auth, sends, and turns a 401 into [`AuthExpired`]. The single
    /// place that decides what a 401 means.
    async fn send(
        &self,
        req: reqwest::RequestBuilder,
        method: &str,
        url: &str,
    ) -> color_eyre::Result<reqwest::Response> {
        let resp = req
            .header("Authorization", self.auth_header())
            .send()
            .await
            .wrap_err_with(|| format!("{method} {url}"))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AuthExpired.into());
        }
        Ok(resp)
    }

    pub(crate) async fn get(&self, path: &str) -> color_eyre::Result<reqwest::Response> {
        let url = format!("{}{path}", self.server);
        self.send(self.http.get(&url), "GET", &url).await
    }

    pub(crate) async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> color_eyre::Result<reqwest::Response> {
        let url = format!("{}{path}", self.server);
        self.send(self.http.post(&url).json(body), "POST", &url)
            .await
    }
}

/// The one place an HTTP client is built. `login` cannot go through `Api`,
/// which needs credentials it does not have yet.
fn http_client() -> color_eyre::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("{CLIENT_NAME}/{VERSION}"))
        .build()
        .wrap_err("building HTTP client")
}

pub(crate) async fn login(
    server: &str,
    username: &str,
    password: &str,
    device_id: &str,
) -> color_eyre::Result<Credentials> {
    let server = normalize_server_url(server)?;
    let device = device_name();
    let http = http_client()?;

    let url = format!("{server}/Users/AuthenticateByName");
    let resp = http
        .post(&url)
        .header(
            "Authorization",
            authorization_header(&device, device_id, None),
        )
        .json(&json!({
            "Username": username,
            "Pw": password,
        }))
        .send()
        .await
        .wrap_err("connecting to the Jellyfin server")?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(usage_err("login failed: wrong username or password"));
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(usage_err(format!("login failed ({status}): {body}")));
    }

    let parsed: AuthResponse = resp.json().await.wrap_err("decoding login response")?;
    Ok(Credentials {
        server,
        username: parsed.user.name,
        user_id: parsed.user.id,
        access_token: parsed.access_token,
        device_id: device_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
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
}
