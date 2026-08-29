use crate::config::{Credentials, device_name, normalize_server_url};
use crate::usage_err;
use crate::{CLIENT_NAME, VERSION};
use color_eyre::eyre::{WrapErr, eyre};
use serde::Deserialize;
use serde_json::json;

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

#[derive(Debug, Clone)]
pub struct Api {
    pub http: reqwest::Client,
    pub server: String,
    pub token: String,
    pub device_id: String,
    pub device_name: String,
    pub user_id: String,
    pub username: String,
}

impl Api {
    pub fn from_credentials(creds: &Credentials) -> color_eyre::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("{CLIENT_NAME}/{VERSION}"))
            .build()
            .wrap_err("building HTTP client")?;
        Ok(Self {
            http,
            server: creds.server.trim_end_matches('/').to_string(),
            token: creds.access_token.clone(),
            device_id: creds.device_id.clone(),
            device_name: device_name(),
            user_id: creds.user_id.clone(),
            username: creds.username.clone(),
        })
    }

    pub fn auth_header(&self) -> String {
        authorization_header(&self.device_name, &self.device_id, Some(&self.token))
    }

    pub fn mpv_auth_header_field(&self) -> String {
        format!("Authorization: {}", self.auth_header())
    }

    pub async fn get(&self, path: &str) -> color_eyre::Result<reqwest::Response> {
        let url = format!("{}{path}", self.server);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .wrap_err_with(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(eyre!("server returned 401; run `jellysink login` again"));
        }
        Ok(resp)
    }

    pub async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> color_eyre::Result<reqwest::Response> {
        let url = format!("{}{path}", self.server);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(body)
            .send()
            .await
            .wrap_err_with(|| format!("POST {url}"))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(eyre!("server returned 401; run `jellysink login` again"));
        }
        Ok(resp)
    }
}

pub async fn login(
    server: &str,
    username: &str,
    password: &str,
    device_id: &str,
) -> color_eyre::Result<Credentials> {
    let server = normalize_server_url(server)?;
    let device = device_name();
    let http = reqwest::Client::builder()
        .user_agent(format!("{CLIENT_NAME}/{VERSION}"))
        .build()
        .wrap_err("building HTTP client")?;

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
}
