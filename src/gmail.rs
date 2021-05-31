use std::path::{Path, PathBuf};

use anyhow::Result;
use google_gmail1::{Gmail, api::Message};
use hyper::Client;
use hyper_rustls::HttpsConnector;
use yup_oauth2::InstalledFlowAuthenticator;

use crate::config;

pub struct GmailClient {
    client: google_gmail1::Gmail,
}

impl GmailClient {
    pub async fn new<P: AsRef<Path>, Q: Into<PathBuf>>(app_secret_path: P, token_persist_path: Q) -> Result<Self> {
        let secret = yup_oauth2::read_application_secret(app_secret_path).await?;

        let auth =
            InstalledFlowAuthenticator::builder(
                secret,
                yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
            )
            .persist_tokens_to_disk(token_persist_path)
            .build()
            .await?;

        let client = Gmail::new(Client::builder().build(HttpsConnector::with_native_roots()), auth);

        Ok(Self {
            client,
        })
    }

    /// Constructs a GmailClient from a Config.
    pub async fn from_config(config: &config::Config) -> Result<Option<Self>> {
        let default_working_dir = "".to_string();

        if config.general.gmail_user.is_none() {
            return Ok(None);
        }

        let working_dir = config.general.working_dir.as_ref().unwrap_or(&default_working_dir);
        let username = &config.general.gmail_user.as_ref().unwrap();

        let app_secret_name = "gmail-api-secret.json";
        let token_persist_name = format!("{}-token.json", username);

        let app_secret_path = PathBuf::new().join(working_dir).join(app_secret_name);
        let token_persist_path = PathBuf::new().join(working_dir).join(token_persist_name);

        let gmail_client = GmailClient::new(&app_secret_path, &token_persist_path).await?;

        Ok(Some(gmail_client))
    }

    /// List the first `limit` messages that match the given query.
    pub async fn list_messages(&self, user_id: &str, query: &str, limit: Option<u32>) -> Result<Vec<Message>> {
        let (_, response) = self.client
            .users()
            .messages_list(user_id)
            .add_scope(google_gmail1::api::Scope::Readonly)
            .q(query)
            .max_results(limit.unwrap_or(20))
            .include_spam_trash(false)
            .doit()
            .await?;

        let messages = response.messages.ok_or_else(|| anyhow::format_err!("No messages found"))?;

        Ok(messages)
    }

    /// Get the full content for a single Gmail message.
    pub async fn get_message(&self, user_id: &str, message_id: &str, format: &str) -> Result<Message> {
        let (_, message) = self.client
            .users()
            .messages_get(user_id, message_id)
            .add_scope(google_gmail1::api::Scope::Readonly)
            .format(format)
            .doit()
            .await?;
        Ok(message)
    }

    pub async fn get_message_body(&self, user_id: &str, message_id: &str) -> Result<String> {
        let message = self.get_message(user_id, message_id, "RAW").await?;
        let raw = message.raw.as_ref().unwrap();
        let config = base64::Config::new(base64::CharacterSet::UrlSafe, false);
        let decoded = base64::decode_config(raw, config)?;
        let body = String::from_utf8(decoded)?;
        Ok(body)
    }
}
