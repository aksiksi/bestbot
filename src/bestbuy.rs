#![allow(non_snake_case)]
use std::collections::VecDeque;
use std::iter::FromIterator;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use fantoccini::{cookies::Cookie, Locator, elements::Element};
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde_json::Value as Json;
use tokio::time::sleep;

use crate::{common::BotClientState, discord::DiscordWebhook, twilio::TwilioClient};
use crate::config::Config;
use crate::gmail::GmailClient;

static SIGN_IN_URL: &str = "https://www.bestbuy.com/identity/global/signin";
static EMAIL_CODE_PAT: &str = r#"<span.+>(\d+)</span>"#;

#[derive(Debug, Deserialize)]
struct FulfillmentStore {
    storeId: String,
    storeName: String,
    storeAddress: String,
    storeCity: String,
    storeState: String,
    storeZipCode: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "typeCode")]
enum CartFulfillment {
    #[serde(rename = "SHIPPING")]
    Shipping {
        zipcode: String,
        minDate: u64,
        daysTillFulfillment: u32,
        maxDate: u64,
        price: String, // "FREE" for free shipping
        selected: bool,
        isPreOrder: bool,
    },
    #[serde(rename = "IN_STORE_PICKUP")]
    InStorePickup {
        daysTillPickup: String, // As a number
        pickupDate: String, // "Tue, May 25"
        pickUpToday: bool,
        isCurbsideAvailable: bool,
        selected: bool,
        store: FulfillmentStore,
    },
}

#[derive(Debug, Deserialize)]
struct CartItemPrice {
    linePrice: String,
    regularPrice: String,
}

#[derive(Debug, Deserialize)]
enum CartItemType {
    #[serde(rename = "HARDGOOD")]
    HardGood,
}

#[derive(Debug, Deserialize)]
struct CartItem {
    skuId: String,
    shortLabel: String,
    imageUrl: String,
    itemUrl: String,
    fulfillments: Vec<CartFulfillment>,
    typeCode: CartItemType,
    price: CartItemPrice,
}

#[derive(Debug, Deserialize)]
struct CartLineItem {
    id: String,
    quantity: u32,
    quantityLimit: u32,
    item: CartItem,
    digital: bool,
}

#[derive(Debug, Deserialize)]
struct CartSummary {
    productTotal: String,
    orderTotal: String,
}

#[derive(Debug, Deserialize)]
struct Cart {
    id: String,
    cartItemCount: String, // As a number
    subtotalAmount: String, // As a number
    lineItems: Vec<CartLineItem>,
    fulfillments: Vec<CartFulfillment>,
    orderSummary: CartSummary,
    paypalWalletEnabled: bool,
    creditCardInProfile: bool,
}

#[derive(Debug, Deserialize)]
struct ItemPriceInfo {
    regularPrice: f64,
    currentPrice: f64,
    customerPrice: f64,
}

#[derive(Debug, Deserialize)]
struct ItemInfo {
    sku: String,
    name: String,
    url: String,
    price: ItemPriceInfo,
    image_url: String,
    description: String,
}

#[derive(Clone, Debug)]
struct BestBuyApi {
    client: reqwest::Client,
}

impl BestBuyApi {
    const BASE_URL: &'static str = "https://www.bestbuy.com";
    const USER_AGENT: &'static str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:88.0) Gecko/20100101 Firefox/88.0";

    fn is_auth_cookie(name: &str) -> bool {
        let name = name.to_lowercase();
        match name.as_str() {
            // As far as I can tell, BestBuy uses these three auth cookies
            "ut" | "bm_sz" | "at" => true,
            _ => false,
        }
    }

    /// Build an API client from a list of cookies.
    fn from_cookies(cookies: &[Cookie]) -> Result<Self> {
        // Build a cookie jar for use with the HTTP client
        let cookie_jar = reqwest::cookie::Jar::default();
        let url: reqwest::Url = Self::BASE_URL.parse().unwrap();
        for cookie in cookies {
            if Self::is_auth_cookie(cookie.name()) {
                let encoded = cookie.encoded().to_string();
                cookie_jar.add_cookie_str(&encoded, &url);
            }
        }

        // Default headers for every request
        let default_headers: HeaderMap =
            [
                ("Origin", Self::BASE_URL),
                ("Referer", Self::BASE_URL),
                ("Accept-Language", "en-US"),
            ]
            .iter()
            .map(|(name, value)| {
                (HeaderName::from_str(name).unwrap(), HeaderValue::from_str(value).unwrap())
            })
            .collect();

        // The BestBuy API only accepts HTTP/2 requests and relies on ALPN to handle
        // negotiating the protocol. As a result, we need to use the `rustls-tls`
        // backend for `reqwest` and explicitly enable it when building the client.
        let client = reqwest::Client::builder()
            .user_agent(Self::USER_AGENT)
            .default_headers(default_headers)
            .timeout(std::time::Duration::from_secs(10))
            .cookie_provider(std::sync::Arc::new(cookie_jar))
            .https_only(true)
            .use_rustls_tls() // Needed for ALPN (HTTP -> HTTP2 upgrade)
            .build()?;

        Ok(Self {
            client
        })
    }

    /// Get pricing info for a given SKU.
    async fn get_item_price(&self, sku: &str) -> Result<ItemPriceInfo> {
        let endpoint = format!("{}/pricing/v1/price/item", Self::BASE_URL);

        let info: ItemPriceInfo = self.client
            .get(endpoint)
            .header("X-CLIENT-ID", "lib-price-browser")
            .query(&[
                ("skuId", sku),
                ("catalog", "bby"),
                ("context", "product-carousel-v2"),
                ("includeOpenboxPrice", "false"),
                ("includeExpirationTimeStamp", "true"),
                ("salesChannel", "LargeView"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(info)
    }

    /// Get relevant info for a given item, including its price
    async fn get_item_info(&self, sku: &str) -> Result<ItemInfo> {
        let endpoint = format!("{}/api/tcfb/model.json", Self::BASE_URL);

        let price = self.get_item_price(sku).await?;

        // Query: item name, item URL, item image URL, and item description
        let paths = format!(r#"[
            ["shop", "magellan", "v2", "product", "skus", {sku}, "names", "short"],
            ["shop", "magellan", "v1", "sites", "skuId", {sku}, "sites", "bbypres", "relativePdpUrl"],
            ["shop", "magellan", "v2", "product", "skus", {sku}, "images", "0"],
            ["shop", "magellan", "v2", "product", "skus", {sku}, "descriptions", "long"]
        ]"#, sku=sku);

        let json: Json = self.client
            .get(endpoint)
            .query(&[
                ("method", "get"),
                ("paths", &paths)
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let name =
            json["jsonGraph"]["shop"]["magellan"]["v2"]["product"]["skus"][sku]["names"]["short"]["value"].as_str().unwrap().to_string();
        let relative_url =
            json["jsonGraph"]["shop"]["magellan"]["v1"]["sites"]["skuId"][sku]["sites"]["bbypres"]["relativePdpUrl"]["value"].as_str().unwrap();
        let image_url =
            json["jsonGraph"]["shop"]["magellan"]["v2"]["product"]["skus"][sku]["images"]["0"]["value"]["href"].as_str().unwrap().to_string();
        let description =
            json["jsonGraph"]["shop"]["magellan"]["v2"]["product"]["skus"][sku]["descriptions"]["long"]["value"].as_str().unwrap().to_string();

        let url = format!("{}{}", Self::BASE_URL, relative_url);

        let item_info = ItemInfo {
            sku: sku.to_string(),
            name,
            url,
            price,
            image_url,
            description,
        };

        Ok(item_info)
    }

    /// Checks if a product is in stock by fetching the "add to cart"
    /// button HTML component.
    async fn is_in_stock(&self, sku: &str) -> Result<bool> {
        let endpoint = format!(
            "{}/site/canopy/component/fulfillment/add-to-cart-button/v1",
            Self::BASE_URL
        );

        let resp = self.client
            .get(endpoint)
            .query(&[("skuId", sku)])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let in_stock = resp.contains("Add to Cart");

        log::debug!("{} is in stock: {}", sku, in_stock);

        Ok(in_stock)
    }

    async fn get_cart_count(&self) -> Result<u32> {
        let endpoint = format!("{}/basket/v1/basketCount", Self::BASE_URL);

        #[derive(Deserialize)]
        struct CartCount {
            count: u32,
        }

        let resp: CartCount = self.client
            .get(endpoint)
            .header("X-CLIENT-ID", "browse")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let count = resp.count;

        log::debug!("Cart has {} items", count);

        Ok(count)
    }

    /// Add a single item to the cart
    #[allow(dead_code)]
    async fn add_to_cart(&self, sku: &str) -> Result<()> {
        let endpoint = format!("{}/cart/api/v1/addToCart", Self::BASE_URL);
        let json = serde_json::json!(
            {
                "items": [
                    {"skuId": sku},
                ]
            }
        );

        self.client
            .post(&endpoint)
            .json(&json)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(())
    }

    async fn get_cart(&self) -> Result<Cart> {
        let endpoint = format!("{}/cart/json", Self::BASE_URL);
        let resp: Json = self.client
            .get(&endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        // TODO: Error handling
        let cart_json = resp.as_object().unwrap().get("cart").unwrap().to_owned();
        let cart: Cart = serde_json::from_value(cart_json)?;

        log::trace!("{:?}", cart);

        Ok(cart)
    }

    #[allow(dead_code)]
    async fn remove_from_cart(&self, item_id: &str) -> Result<()> {
        let endpoint = format!("{}/cart/item/{}", Self::BASE_URL, item_id);
        self.client
            .delete(&endpoint)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Modify an existing cart item
    #[allow(dead_code)]
    async fn modify_cart_item(&self, item_id: &str, quantity: Option<u32>) -> Result<()> {
        if quantity.is_none() {
            return Ok(());
        }

        let endpoint = format!("{}/cart/item/{}", Self::BASE_URL, item_id);
        let mut json = serde_json::json!({});

        if let Some(quantity) = quantity {
            json["quantity"] = serde_json::json!(quantity);
        }

        self.client
            .put(&endpoint)
            .json(&json)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    async fn clear_cart(&self) -> Result<()> {
        let cart = self.get_cart().await?;

        for line_item in &cart.lineItems {
            self.remove_from_cart(&line_item.id).await?;
        }

        log::debug!("Cleared the cart");

        Ok(())
    }
}

#[derive(Clone)]
struct WebdriverBot<'c, 'g> {
    client: fantoccini::Client,
    gmail_client: &'g GmailClient,
    config: &'c Config,
}

impl<'c, 'g> WebdriverBot<'c, 'g> {
    const USERNAME_SEL: &'static str = r#"#fld-e"#;
    const PASSWORD_SEL: &'static str = r#"#fld-p1"#;
    const SUBMIT_SEL: &'static str = r#"div.cia-form__controls > button"#;
    const VERIFICATION_CODE_SEL: &'static str = r#"input#verificationCode"#;
    const VERIFICATION_CODE_FORM: &'static str = r#"form.cia-form"#;

    fn new(client: fantoccini::Client,
           gmail_client: &'g GmailClient,
           config: &'c Config) -> Self {
        Self {
            client,
            gmail_client,
            config,
        }
    }

    async fn find_element(&mut self, selector: &str) -> Result<Element> {
        let elem = self.client
            .find(Locator::Css(selector))
            .await?;
        Ok(elem)
    }

    async fn is_element_present(&mut self, selector: &str) -> Result<bool> {
        let matches = self.client
            .find_all(Locator::Css(selector))
            .await?;
        Ok(matches.len() > 0)
    }

    /// Get latest email code using Gmail API
    async fn get_email_code(&self) -> Result<String> {
        let username = &self.config.bestbuy.as_ref().unwrap().username;

        let messages = self.gmail_client
            .list_messages(&username, "BestBuy", None)
            .await?;
        let latest_message = messages[0].id.as_ref().unwrap();

        let body = self.gmail_client.get_message_body(&username, latest_message).await?;
        let code_pat = Regex::new(EMAIL_CODE_PAT)?;
        let code = code_pat.captures(&body).unwrap().get(1).unwrap().as_str().to_owned();

        log::info!("Email code: {}", code);

        Ok(code)
    }

    /// Check if we have a verification code on the page. If we do, go through
    /// the verification flow.
    async fn verify_code(&mut self) -> Result<()> {
        let verify_required = self.is_element_present(Self::VERIFICATION_CODE_SEL).await?;
        if !verify_required {
            return Ok(());
        }

        log::info!("Email verification required");

        let form = self.client
            .form(Locator::Css(Self::VERIFICATION_CODE_FORM))
            .await?;
        let mut input = self.find_element(Self::VERIFICATION_CODE_SEL).await?;

        // Get the verifcation code from Gmail
        let code = self.get_email_code().await?;
        input.send_keys(&code).await?;

        // Submit the form
        form.submit().await?;
        self.client.wait_for_navigation(None).await?;

        Ok(())
    }

    /// Sign in to BestBuy and return the list of cookies
    async fn sign_in(&mut self) -> Result<Vec<Cookie<'_>>> {
        let username = &self.config.bestbuy.as_ref().unwrap().username;
        let password = &self.config.bestbuy.as_ref().unwrap().password;

        log::debug!("Signing in...");

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

        // Check if we need to verify
        self.verify_code().await?;

        log::info!("Signed in successfully");

        // Get the authentication cookies and return them
        let cookies = self.client.get_all_cookies().await?;

        Ok(cookies)
    }
}

/// A single instance of a BestBuy bot.
///
/// Each bot checks the given list of products on every tick and adds
/// all available to the cart before checking out.
pub struct BestBuyBot<'c, 'g, 't> {
    skus: VecDeque<String>,
    gmail_client: &'g GmailClient,
    api_client: Option<BestBuyApi>,
    config: &'c Config,
    twilio_client: Option<&'t TwilioClient>,
    discord_webhook: Option<&'t DiscordWebhook>,
    state: BotClientState,
}

impl<'c, 'g, 't> BestBuyBot<'c, 'g, 't> {
    pub fn new(config: &'c Config,
               gmail_client: &'g GmailClient,
               twilio_client: Option<&'t TwilioClient>,
               discord_webhook: Option<&'t DiscordWebhook>) -> Self {
        let bestbuy = config.bestbuy.as_ref().expect("BestBuy config is not present!");
        let skus = VecDeque::from_iter(bestbuy.skus.to_owned().into_iter());

        assert!(skus.len() == 0, "No BestBuy SKUs specified");

        Self {
            config,
            skus,
            gmail_client,
            api_client: None,
            twilio_client,
            discord_webhook,
            state: BotClientState::Started,
        }
    }

    fn api_client(&self) -> &BestBuyApi {
        self.api_client.as_ref().unwrap()
    }

    /// Try to send a notification when an item is purchased.
    async fn send_message(&self, message: &str) -> Result<()> {
        if self.twilio_client.is_none() {
            return Ok(());
        }

        if let Some(twilio_client) = &self.twilio_client {
            let twilio_config = self.config.twilio.as_ref().unwrap();

            twilio_client.send_message(
                &twilio_config.from_number,
                &twilio_config.to_number,
                message
            ).await?;

            log::info!("Sent notification SMS successfully");
        }

        if let Some(discord_webhook) = &self.discord_webhook {
            discord_webhook.trigger(message).await?;
            log::info!("Triggered Discord webhook successfully");
        }

        Ok(())
    }

    /// Run the client to completion for a given product.
    async fn run(&mut self, sku: &str, _dry_run: bool) -> Result<BotClientState> {
        let api_client = self.api_client.as_ref().unwrap();

        let mut state: BotClientState = self.state;

        loop {
            // Figure out what to do next based on current state
            match self.state {
                BotClientState::SignedIn => {
                    state = if api_client.is_in_stock(sku).await? {
                        BotClientState::InStock
                    } else {
                        BotClientState::NotInStock
                    };
                }
                BotClientState::NotInStock | BotClientState::InStock => break,
                _ => unreachable!("Invalid state"),
            }

            self.state = state;
        }

        // Put the client back in the initial signed in state
        self.state = BotClientState::SignedIn;

        Ok(state)
    }

    pub async fn start(&mut self, dry_run: bool, headless: bool) -> Result<()> {
        let hostname = self.config.general.hostname.as_deref();
        let interval = Duration::from_secs(self.config.general.interval.unwrap_or(20));

        // Connect to the Webdriver client
        let client = crate::common::new_webdriver_client(headless, hostname).await?;

        // Create a Webdriver bot for BestBuy
        let mut client = WebdriverBot::new(
            client,
            self.gmail_client,
            self.config,
        );

        // Use the WebDriver bot to sign in to BestBuy
        // Then, feed the resulting cookies to the API client
        let cookies = client.sign_in().await?;
        let api_client = BestBuyApi::from_cookies(&cookies)?;
        self.api_client = Some(api_client);
        self.state = BotClientState::SignedIn;

        // Clear the cart
        if self.api_client().get_cart_count().await? > 0 {
            self.api_client().clear_cart().await?;
        }

        while self.skus.len() > 0 {
            let num_products = self.skus.len();

            // Check each of the products in the queue.
            //
            // If a product is out of stock, it is put back on the queue.
            for _ in 0..num_products {
                if let Some(sku) = self.skus.pop_front() {
                    // Get item info
                    let item_info = self.api_client().get_item_info(&sku).await?;
                    let (name, price) = (&item_info.name, item_info.price.currentPrice);
                    log::info!("Name: \"{}\", Price: ${}", name, price);

                    match self.run(&sku, dry_run).await? {
                        BotClientState::InStock => {
                            let message = format!("In Stock: {} for ${}", name, price);
                            self.send_message(&message).await?;
                        }
                        BotClientState::Purchased => {
                            let message = format!("Purchased: {} for ${}", name, price);
                            self.send_message(&message).await?;
                        }
                        _ => self.skus.push_back(sku),
                    };
                }
            }

            log::debug!("Sleeping for {:?}", interval);

            sleep(interval).await;
        }

        Ok(())
    }
}
