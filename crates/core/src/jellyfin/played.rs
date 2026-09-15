use super::auth::Api;
use super::encode_query_value;
use color_eyre::eyre::{Result, WrapErr};

impl Api {
    pub async fn set_played(&self, item_id: &str, played: bool) -> Result<()> {
        tracing::debug!(item_id, played, "set played");
        let user_id = encode_query_value(&self.user_id);
        let modern = format!("/UserPlayedItems/{item_id}?userId={user_id}");
        let err = match self.send_played(&modern, played).await {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };
        tracing::debug!(%err, "played toggle failed; trying legacy endpoint");
        let legacy = format!("/Users/{}/PlayedItems/{item_id}", self.user_id);
        self.send_played(&legacy, played)
            .await
            .wrap_err_with(|| format!("{err:#}"))
    }

    /// `Api::send` leaves the status to its caller, so the check belongs here.
    async fn send_played(&self, path: &str, played: bool) -> Result<()> {
        let (method, response) = if played {
            ("POST", self.post(path).await?)
        } else {
            ("DELETE", self.delete(path).await?)
        };
        response
            .error_for_status()
            .wrap_err_with(|| format!("{method} {path}"))?;
        Ok(())
    }
}
