use crate::APP_NAME;
use crate::usage_err;
use color_eyre::eyre::WrapErr;

pub fn normalize_server_url(input: &str) -> color_eyre::Result<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(usage_err("server URL is empty"));
    }

    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        let first = trimmed.split('/').next().unwrap_or_default();
        if matches!(
            first.to_ascii_lowercase().as_str(),
            "http" | "https" | "http:" | "https:"
        ) {
            return Err(usage_err(
                "scheme is missing '//' — expected e.g. 'http://host:8096'",
            ));
        }
        format!("http://{trimmed}")
    };

    let mut url = reqwest::Url::parse(&with_scheme)
        .wrap_err_with(|| format!("invalid server URL {input:?}"))?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(usage_err(format!(
            "unsupported scheme {:?} — expected 'http' or 'https'",
            url.scheme()
        )));
    }

    url.set_username("")
        .and_then(|()| url.set_password(None))
        .map_err(|()| usage_err("server URL has no host"))?;
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
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
