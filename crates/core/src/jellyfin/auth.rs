use crate::config::{Credentials, device_name, normalize_server_url};
use crate::usage_err;
use crate::{CLIENT_NAME, VERSION};
use color_eyre::eyre::WrapErr;
use serde::Deserialize;
use serde_json::json;
use std::fmt;

/// The server rejected our access token. Typed so the reconnect loop does not
/// have to look for `"401"` in an error chain that also carries the URL.
#[derive(Debug)]
pub struct AuthExpired;

impl fmt::Display for AuthExpired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("server returned 401; run `jellysink login` again")
    }
}

impl std::error::Error for AuthExpired {}

/// Whether `err` was caused by an expired token at any depth; `downcast_ref`
/// would only see the outermost error.
pub fn is_auth_expired(err: &color_eyre::Report) -> bool {
    err.chain().any(|cause| cause.is::<AuthExpired>())
}

pub fn authorization_header(device: &str, device_id: &str, token: Option<&str>) -> String {
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
pub struct Api {
    pub http: reqwest::Client,
    pub server: String,
    pub token: String,
    pub device_id: String,
    pub device_name: String,
    pub user_id: String,
    /// Precomputed: constant for the process, and needed once a second.
    auth_header: String,
}

impl fmt::Debug for Api {
    /// Hand-written so `token` cannot reach a log line.
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
    pub fn from_credentials(creds: &Credentials) -> color_eyre::Result<Self> {
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

    pub fn auth_header(&self) -> &str {
        &self.auth_header
    }

    pub fn mpv_auth_header_field(&self) -> String {
        format!("Authorization: {}", self.auth_header())
    }

    /// Attaches auth, sends, and turns a 401 into [`AuthExpired`].
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

    pub async fn get(&self, path: &str) -> color_eyre::Result<reqwest::Response> {
        let url = format!("{}{path}", self.server);
        self.send(self.http.get(&url), "GET", &url).await
    }

    pub async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> color_eyre::Result<reqwest::Response> {
        let url = format!("{}{path}", self.server);
        self.send(self.http.post(&url).json(body), "POST", &url)
            .await
    }

    /// A POST whose parameters all live in the query string.
    pub async fn post(&self, path: &str) -> color_eyre::Result<reqwest::Response> {
        let url = format!("{}{path}", self.server);
        self.send(self.http.post(&url), "POST", &url).await
    }

    pub async fn get_json(&self, path: &str) -> color_eyre::Result<serde_json::Value> {
        let resp = self
            .get(path)
            .await?
            .error_for_status()
            .wrap_err_with(|| format!("GET {path}"))?;
        resp.json().await.wrap_err("decoding JSON")
    }
}

/// The one place an HTTP client is built; `login` has no `Api` to go through.
fn http_client() -> color_eyre::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("{CLIENT_NAME}/{VERSION}"))
        .build()
        .wrap_err("building HTTP client")
}

pub async fn login(
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
#[path = "auth_test.rs"]
mod tests;
