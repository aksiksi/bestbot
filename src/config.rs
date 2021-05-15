use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Login {
    pub username: String,
    pub password: String,
}

#[derive(Clone, Deserialize)]
pub struct Address {
    pub first_name: String,
    pub last_name: String,
    pub street: String,
    pub city: String,
    pub state: String,
    pub zip_code: String,
}

#[derive(Clone, Deserialize)]
pub struct PaymentInfo {
    pub card_number: String,
    pub exp_month: String,
    pub exp_year: String,
    pub cvv: u32,
    pub billing_address: Address,
}

#[derive(Deserialize)]
pub struct Config {
    pub interval: Option<u64>,
    pub hostname: Option<String>,
    pub working_dir: Option<String>,
    pub products: Vec<String>,
    pub login_info: Option<Login>,
    pub payment_info: PaymentInfo,
    pub shipping_address: Option<Address>,
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config_file = std::fs::read_to_string(path)?;
        let mut parsed: Config = toml::from_str(&config_file)?;

        assert!(parsed.products.len() > 0, "No products specified!");

        if parsed.login_info.is_none() {
            let username = match std::env::var("BESTBOT_USERNAME") {
                Ok(u) => u,
                Err(_) => panic!("BESTBOT_USERNAME env variable not set"),
            };
            let password = match std::env::var("BESTBOT_PASSWORD") {
                Ok(u) => u,
                Err(_) => panic!("BESTBOT_PASSWORD env variable not set"),
            };

            let login_info = Login {
                username,
                password,
            };

            parsed.login_info = Some(login_info);
        }

        if parsed.shipping_address.is_none() {
            parsed.shipping_address = Some(parsed.payment_info.billing_address.clone());
        }

        Ok(parsed)
    }
}
