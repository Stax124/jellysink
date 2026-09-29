use super::auth::Api;
use super::browse::with_legacy_fallback;
use super::url::encode_query_value;
use color_eyre::eyre::Result;

impl Api {
    pub async fn set_played(&self, item_id: &str, played: bool) -> Result<()> {
        tracing::debug!(item_id, played, "set played");
        let user_id = encode_query_value(&self.user_id);
        let modern = format!("/UserPlayedItems/{item_id}?userId={user_id}");
        let legacy = format!("/Users/{user_id}/PlayedItems/{item_id}");
        with_legacy_fallback(
            self.send_played(&modern, played),
            self.send_played(&legacy, played),
        )
        .await
    }

    async fn send_played(&self, path: &str, played: bool) -> Result<()> {
        if played {
            self.post(path).await
        } else {
            self.delete(path).await
        }
    }
}
