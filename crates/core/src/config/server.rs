use crate::APP_NAME;
use crate::usage_err;
use color_eyre::eyre::WrapErr;

/// Bare host → `http://host:8096`. Existing scheme/port/path are kept.
/// Trailing slashes are stripped.
pub fn normalize_server_url(input: &str) -> color_eyre::Result<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(usage_err("server URL is empty"));
    }

    if !trimmed.contains("://") {
        let first = trimmed.split('/').next().unwrap_or_default();
        if matches!(
            first.to_ascii_lowercase().as_str(),
            "http" | "https" | "http:" | "https:"
        ) {
            return Err(usage_err(
                "scheme is missing '//' — expected e.g. 'http://host:8096'",
            ));
        }
    }

    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };

    let url = reqwest::Url::parse(&with_scheme)
        .wrap_err_with(|| format!("invalid server URL {input:?}"))?;

    let host = url
        .host_str()
        .ok_or_else(|| usage_err("server URL has no host"))?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    let port_part = match url.port() {
        Some(p) => format!(":{p}"),
        None if explicit_port(trimmed) => {
            // `Url::port()` hides 80/443, but the user wrote it on purpose.
            match url.port_or_known_default() {
                Some(p) => format!(":{p}"),
                None => String::new(),
            }
        }
        None if url.scheme() == "http" => ":8096".to_string(),
        None => String::new(),
    };

    let path = url.path().trim_end_matches('/');
    let path = if path.is_empty() || path == "/" {
        String::new()
    } else {
        path.to_string()
    };

    Ok(format!("{}://{}{}{}", url.scheme(), host, port_part, path))
}

fn explicit_port(input: &str) -> bool {
    let rest = match input.split_once("://") {
        Some((_, r)) => r,
        None => input,
    };
    if let Some(end) = rest.find(']') {
        return rest[end + 1..].starts_with(':');
    }
    let hostport = rest.split('/').next().unwrap_or(rest);
    hostport.contains(':')
}

pub fn device_name() -> String {
    let name = rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned();
    if name.is_empty() {
        APP_NAME.to_string()
    } else {
        name
    }
}

#[cfg(test)]
#[path = "server_test.rs"]
mod tests;
