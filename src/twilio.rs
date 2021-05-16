use anyhow::Result;

use crate::config::Config;

pub struct TwilioClient {
    sid: String,
    auth_token: String,
    client: reqwest::Client,
}

impl TwilioClient {
    const BASE_URL: &'static str = "https://api.twilio.com/2010-04-01/Accounts";

    pub fn new(sid: String, auth_token: String) -> Result<Self> {
        let client = reqwest::ClientBuilder::default().build()?;

        Ok(Self {
            sid,
            auth_token,
            client,
        })
    }

    pub fn from_config(config: &Config) -> Result<Option<Self>> {
        if config.twilio.is_none() {
            Ok(None)
        } else {
            let sid = config.twilio.as_ref().unwrap().sid.clone();
            let auth_token = config.twilio.as_ref().unwrap().auth_token.clone();
            let client = Self::new(sid, auth_token)?;
            Ok(Some(client))
        }
    }

    pub async fn send_message(&self, from: &str, to: &str, body: &str) -> Result<()> {
        let url = format!("{}/{}/Messages.json", Self::BASE_URL, self.sid);

        self.client
            .post(&url)
            .form(&[("Body", body), ("To", to), ("From", from)])
            .basic_auth(&self.sid, Some(&self.auth_token))
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_twilio_client() {
        let sid = std::env::var("TWILIO_SID").unwrap();
        let auth_token = std::env::var("TWILIO_AUTH_TOKEN").unwrap();
        let from_number = std::env::var("TWILIO_FROM_NUMBER").unwrap();
        let to_number = std::env::var("TWILIO_TO_NUMBER").unwrap();

        let client = TwilioClient::new(sid, auth_token).unwrap();

        client.send_message(&from_number, &to_number, "Test passed!").await.unwrap();
    }
}
