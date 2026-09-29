use super::*;
use crate::cli::BIN_NAME;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UpdateCheck {
    Pending,
    Failed,
    Current,
    Available(String),
}

impl App {
    pub(super) fn check_for_update(&self) {
        self.spawn_msg(async move {
            let update = match jellysink_core::update::check(BIN_NAME).await {
                Ok(Some(version)) => {
                    tracing::info!(%version, "update available");
                    UpdateCheck::Available(version)
                }
                Ok(None) => {
                    tracing::debug!("already up to date");
                    UpdateCheck::Current
                }
                Err(e) => {
                    tracing::warn!("update check failed: {e:#}");
                    UpdateCheck::Failed
                }
            };
            Some(Msg::UpdateChecked(update))
        });
    }

    pub(super) fn start_update(&mut self) {
        self.message = match &self.update {
            UpdateCheck::Available(_) => {
                self.quit = Some(Exit::Update);
                return;
            }
            UpdateCheck::Current => format!("jellytui {VERSION} is up to date"),
            UpdateCheck::Pending => "still checking for an update".to_string(),
            UpdateCheck::Failed => "the update check failed — see L".to_string(),
        };
    }
}

#[cfg(test)]
#[path = "update_test.rs"]
mod tests;
