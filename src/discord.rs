use anyhow::Result;

use crate::config::Config;

#[derive(Debug)]
pub struct DiscordWebhook {
    client: reqwest::Client,
    webhook_url: String,
}

impl DiscordWebhook {
    pub fn from_config(config: &Config) -> Option<Self> {
        if config.discord.is_none() {
            return None;
        }

        let webhook_url = config.discord.as_ref().unwrap().webhook_url.to_string();

        Some(Self {
            client: reqwest::Client::new(),
            webhook_url,
        })
    }

    pub async fn trigger(&self, message: &str) -> Result<()> {
        let json = serde_json::json!({ "content": message });

        self.client
            .post(&self.webhook_url)
            .json(&json)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }
}
