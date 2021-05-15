use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use fantoccini::Locator;
use regex::Regex;
use rusty_money::{Money, iso};
use tokio::time::sleep;

use crate::gmail::GmailClient;

static CART_URL: &str = "https://www.bestbuy.com/cart";
static SIGN_IN_URL: &str = "https://www.bestbuy.com/identity/global/signin";
static EMAIL_CODE_PAT: &str = r#"<span.+>(\d+)</span>"#;

#[derive(Clone, Copy, Debug)]
enum ClientState {
    Started,
    SignedIn,
    CartUpdated,
    NotInStock,
    Purchased,
    Errored,
}

#[derive(Clone)]
struct BotClient {
    client: fantoccini::Client,
    gmail_client: Arc<GmailClient>,
    username: String,
    state: ClientState,
}

impl BotClient {
    const USERNAME_SEL: &'static str = r#"#fld-e"#;
    const PASSWORD_SEL: &'static str = r#"#fld-p1"#;
    const SUBMIT_SEL: &'static str = r#"div.cia-form__controls > button"#;
    const PRODUCT_PRICE_SEL: &'static str = r#"div.priceView-customer-price > span"#;
    const CART_READY_TEXT_SEL: &'static str = r#"h2.order-summary__heading"#;
    const ADD_TO_CART_BTN_SEL: &'static str = r#"div.fulfillment-add-to-cart-button button"#;
    const REMOVE_CART_LINK_SEL: &'static str = r#"a.cart-item__remove"#;
    const CART_CHECKOUT_BTN_SEL: &'static str = r#"div.checkout-buttons__checkout > button"#;
    const CART_CHECKOUT_PP_BTN_SEL: &'static str = r#"div.checkout-buttons__container > button.checkout-buttons__paypal"#;

    fn new(client: fantoccini::Client, gmail_client: GmailClient, username: String) -> Self {
        Self {
            client,
            gmail_client: Arc::new(gmail_client),
            username,
            state: ClientState::Started,
        }
    }

    /// Open the cart page
    async fn open_cart(&mut self) -> Result<()> {
        self.client.goto(CART_URL).await?;
        self.client.wait_for_find(Locator::Css(Self::CART_READY_TEXT_SEL)).await?;
        Ok(())
    }

    /// Clear everything in the cart
    async fn clear_cart(&mut self) -> Result<()> {
        self.open_cart().await?;

        // Find all of the remove buttons on the cart page
        let remove_btns =
            self.client.find_all(Locator::Css(Self::REMOVE_CART_LINK_SEL)).await?;

        for btn in remove_btns.into_iter() {
            btn.click().await?;
            sleep(Duration::from_millis(1000)).await;
        }

        Ok(())
    }

    /// Sign in to BestBuy
    async fn sign_in(&mut self, username: &str, password: &str) -> Result<ClientState> {
        self.client.goto(SIGN_IN_URL).await?;

        self.client.wait_for_find(Locator::Css(Self::USERNAME_SEL)).await?;
        self.client.wait_for_find(Locator::Css(Self::PASSWORD_SEL)).await?;
        self.client.wait_for_find(Locator::Css(Self::SUBMIT_SEL)).await?;

        let mut username_input = self.client.find(
            Locator::Css(Self::USERNAME_SEL)
        ).await?;
        let mut password_input = self.client.find(
            Locator::Css(Self::PASSWORD_SEL)
        ).await?;
        let submit = self.client.find(
            Locator::Css(Self::SUBMIT_SEL)
        ).await?;

        username_input.send_keys(username).await?;
        password_input.send_keys(password).await?;

        // Submit the login form and wait for the new page to load
        submit.click().await?;
        self.client.wait_for_navigation(None).await?;

        // Clear the cart
        self.clear_cart().await?;

        Ok(ClientState::SignedIn)
    }

    /// Check if a product is in stock. If yes, add it to the cart.
    async fn check_product(&mut self, product_url: &str) -> Result<ClientState> {
        self.client.goto(product_url).await?;

        self.client.wait_for_find(Locator::Css(Self::ADD_TO_CART_BTN_SEL)).await?;
        self.client.wait_for_find(Locator::Css(Self::PRODUCT_PRICE_SEL)).await?;

        let mut price_elem = self.client
            .find(Locator::Css(Self::PRODUCT_PRICE_SEL))
            .await?;
        let price = price_elem
            .prop("innerText")
            .await?
            // Sane default price
            .unwrap_or_else(|| "9999999".to_string());

        let price = Money::from_str(&price.replace("$", ""), iso::USD)?;
        println!("{}", price);

        let mut add_to_cart_btn = self.client
            .find(Locator::Css(Self::ADD_TO_CART_BTN_SEL))
            .await?;

        let is_sold_out = add_to_cart_btn.text().await? == "Sold Out";
        if is_sold_out {
            println!("Currently sold out...");
            return Ok(ClientState::NotInStock);
        }

        add_to_cart_btn.click().await?;

        // Wait for cart modal to pop up
        sleep(Duration::from_millis(1000)).await;

        // Figure out if we have a modal. If we do, close it.
        let close_modal_btn = self.client
            .find(Locator::Css(".close-modal-x"))
            .await;
        let close_modal_btn = if close_modal_btn.is_err() {
            match close_modal_btn {
                Err(fantoccini::error::CmdError::NoSuchElement(_)) => None,
                _ => return Ok(close_modal_btn.map(|_| ClientState::Errored)?),
            }
        } else {
            Some(close_modal_btn.unwrap())
        };

        if let Some(btn) = close_modal_btn {
            btn.click().await?;
            println!("Closed modal");
        }

        Ok(ClientState::CartUpdated)
    }

    /// Get latest email code using Gmail API
    async fn get_email_code(&self) -> Result<String> {
        let messages = self.gmail_client
            .list_messages(&self.username, "BestBuy", None)
            .await?;
        let latest_message = messages[0].id.as_ref().unwrap();

        let body = self.gmail_client.get_message_body(&self.username, latest_message).await?;
        let code_pat = Regex::new(EMAIL_CODE_PAT)?;
        let code = code_pat.captures(&body).unwrap().get(1).unwrap().as_str().to_owned();

        Ok(code)
    }

    /// Purchase whatever is in the cart.
    async fn checkout(&mut self, paypal: bool) -> Result<ClientState> {
        self.open_cart().await?;
        self.client.wait_for_find(Locator::Css(Self::CART_CHECKOUT_BTN_SEL)).await?;

        let checkout_btn_locator = if !paypal {
            Locator::Css(Self::CART_CHECKOUT_BTN_SEL)
        } else {
            Locator::Css(Self::CART_CHECKOUT_PP_BTN_SEL)
        };

        let checkout_btn = self.client.find(checkout_btn_locator).await?;

        checkout_btn.click().await?;
        self.client.wait_for_navigation(None).await?;

        let code = self.get_email_code().await?;
        println!("Code: {}", code);

        Ok(ClientState::Purchased)
    }

    /// Run the client to completion.
    async fn run(&mut self, product_url: &str, username: &str, password: &str, checkout: bool) -> Result<ClientState> {
        loop {
            match self.state {
                ClientState::Started => self.state = self.sign_in(username, password).await?,
                ClientState::SignedIn => self.state = self.check_product(product_url).await?,
                ClientState::CartUpdated => {
                    if checkout {
                        self.state = self.checkout(false).await?;
                    } else {
                        break;
                    }
                }
                ClientState::Errored | ClientState::NotInStock | ClientState::Purchased => break,
            }
        }

        let state = self.state;

        self.state = ClientState::SignedIn;

        Ok(state)
    }
}

/// A single instance of a BestBuy bot.
///
/// Each bot checks the given list of products on every tick and adds
/// all available to the cart before checking out.
pub struct BestBuyBot {
    interval: Duration,
    username: String,
    password: String,
    hostname: String,
    product_urls: VecDeque<String>,
}

impl BestBuyBot {
    pub fn new(interval: Duration, hostname: Option<&str>) -> Self {
        let username = match std::env::var("BESTBOT_USERNAME") {
            Ok(u) => u,
            Err(_) => panic!("BESTBOT_USERNAME env variable not set"),
        };
        let password = match std::env::var("BESTBOT_PASSWORD") {
            Ok(u) => u,
            Err(_) => panic!("BESTBOT_PASSWORD env variable not set"),
        };
        let hostname = hostname.unwrap_or("http://localhost:4444").to_string();

        Self {
            interval,
            username,
            password,
            hostname,
            product_urls: VecDeque::new(),
        }
    }

    pub fn add_product(&mut self, product_url: String) {
        self.product_urls.push_back(product_url);
    }

    pub async fn start(&mut self) -> Result<()> {
        let app_secret_path = format!("{}-secret.json", self.username);
        let token_persist_path = format!("{}-token.json", self.username);
        let gmail_client = GmailClient::new(&app_secret_path, &token_persist_path).await?;

        let client = fantoccini::ClientBuilder::native()
            .connect(&self.hostname)
            .await?;

        let mut client = BotClient::new(client, gmail_client, self.username.clone());

        while self.product_urls.len() > 0 {
            let num_urls = self.product_urls.len();
            let mut cart_updated = false;

            for _ in 0..num_urls {
                if let Some(product_url) = self.product_urls.pop_front() {
                    match client.run(&product_url, &self.username, &self.password, false).await? {
                        ClientState::CartUpdated => cart_updated = true,
                        _ => self.product_urls.push_back(product_url),
                    };
                }
            }

            if cart_updated {
                // Checkout now
                println!("Checking out...");
                client.checkout(false).await?;
            }

            sleep(self.interval).await;
        }

        Ok(())
    }
}
