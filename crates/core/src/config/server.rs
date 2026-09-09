use crate::APP_NAME;
use crate::usage_err;
use color_eyre::eyre::WrapErr;

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

    if !matches!(url.scheme(), "http" | "https") {
        return Err(usage_err(format!(
            "unsupported scheme {:?} — expected 'http' or 'https'",
            url.scheme()
        )));
    }

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
