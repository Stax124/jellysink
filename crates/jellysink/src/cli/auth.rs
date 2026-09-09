use color_eyre::eyre::WrapErr;
use dialoguer::{Input, Password, theme::ColorfulTheme};
use jellysink_core::config::{Credentials, Paths, normalize_server_url};
use jellysink_core::jellyfin::auth::login;
use uuid::Uuid;

pub(crate) async fn cmd_login(paths: &Paths) -> color_eyre::Result<()> {
    paths.ensure()?;
    let theme = ColorfulTheme::default();

    let server = Input::<String>::with_theme(&theme)
        .with_prompt("Server URL")
        .default("http://localhost:8096".to_string())
        .validate_with(|input: &String| normalize_server_url(input).map(|_| ()))
        .interact_text()
        .wrap_err("reading server URL")?;
    let server = normalize_server_url(&server)?;

    let username = Input::<String>::with_theme(&theme)
        .with_prompt("Username")
        .validate_with(|input: &String| {
            if input.trim().is_empty() {
                Err("username cannot be empty")
            } else {
                Ok(())
            }
        })
        .interact_text()
        .wrap_err("reading username")?;

    let password = Password::with_theme(&theme)
        .with_prompt("Password")
        .allow_empty_password(true)
        .interact()
        .wrap_err("reading password")?;

    let existing = Credentials::load(paths)?;
    let device_id = existing
        .map(|c| c.device_id)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let creds = login(&server, &username, &password, &device_id).await?;
    creds.save(paths)?;
    println!("Logged in as {} on {}", creds.username, creds.server);
    Ok(())
}

pub(crate) fn cmd_logout(paths: &Paths) -> color_eyre::Result<()> {
    Credentials::remove(paths)?;
    println!("Logged out.");
    Ok(())
}
