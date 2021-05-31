use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Twilio {
    pub sid: String,
    pub auth_token: String,
    pub from_number: String,
    pub to_number: String,
}

#[derive(Deserialize)]
pub struct Discord {
    pub webhook_url: String,
}

#[derive(Deserialize)]
pub struct BestBuy {
    pub skus: Vec<String>,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct General {
    pub interval: Option<u64>,
    pub hostname: Option<String>,
    pub working_dir: Option<String>,
    pub gmail_user: Option<String>,
}

#[derive(Deserialize)]
pub struct Config {
    pub general: General,
    pub bestbuy: Option<BestBuy>,
    pub twilio: Option<Twilio>,
    pub discord: Option<Discord>,
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config_file = std::fs::read_to_string(path)?;
        let parsed: Config = toml::from_str(&config_file)?;
        Ok(parsed)
    }
}
