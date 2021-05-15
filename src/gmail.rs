use std::path::{Path, PathBuf};

use anyhow::Result;
use google_gmail1::{Gmail, api::Message};
use hyper::Client;
use hyper_rustls::HttpsConnector;
use yup_oauth2::InstalledFlowAuthenticator;

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
