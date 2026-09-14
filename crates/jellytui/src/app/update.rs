use super::*;

impl App {
    pub(super) fn check_for_update(&self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match jellysink_core::update::check(env!("CARGO_BIN_NAME")).await {
                Ok(Some(version)) => {
                    tracing::info!(%version, "update available");
                    let _ = tx.send(Msg::UpdateAvailable(version));
                }
                Ok(None) => tracing::debug!("already up to date"),
                Err(e) => tracing::warn!("update check failed: {e:#}"),
            }
        });
    }

    pub(super) fn start_update(&mut self) {
        if self.update_offer.is_some() {
            self.quit = Some(Exit::Update);
        } else {
            self.message = format!("jellytui {VERSION} is up to date");
        }
    }
}

#[cfg(test)]
#[path = "update_test.rs"]
mod tests;
