use std::collections::VecDeque;
use std::iter::FromIterator;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use fantoccini::{cookies::Cookie, Locator, elements::Element};
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rusty_money::{Money, iso};
use serde_json::Value as Json;
use tokio::time::sleep;

use crate::{common::BotClientState, twilio::TwilioClient};
use crate::config::{Address, Config, PaymentInfo};
use crate::gmail::GmailClient;

static CART_URL: &str = "https://www.bestbuy.com/cart";
static SIGN_IN_URL: &str = "https://www.bestbuy.com/identity/global/signin";
static EMAIL_CODE_PAT: &str = r#"<span.+>(\d+)</span>"#;

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
            "ut" | "bm_sz" => true,
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

        let default_headers: HeaderMap =
            [("Origin", Self::BASE_URL), ("Referer", Self::BASE_URL)]
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

    /// Add a single item to the cart
    async fn add_to_cart(&self, sku: &str) -> Result<Json> {
        let endpoint = format!("{}/cart/api/v1/addToCart", Self::BASE_URL);
        let json = serde_json::json!(
            {
                "items": [
                    {"skuId": sku},
                ]
            }
        );

        let resp: Json = self.client
            .post(&endpoint)
            .json(&json)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        log::debug!("{:?}", resp);

        Ok(resp)
    }

    #[allow(dead_code)]
    async fn remove_cart_item(&self, item_id: &str) -> Result<()> {
        let endpoint = format!("{}/cart/item/{}", Self::BASE_URL, item_id);
        self.client
            .delete(&endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(())
    }

    async fn get_cart(&self) -> Result<Json> {
        let endpoint = format!("{}/cart/json", Self::BASE_URL);
        let resp: Json = self.client
            .get(&endpoint)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        log::debug!("{:?}", resp);

        Ok(resp)
    }
}

#[derive(Clone)]
struct WebdriverBot<'c, 'g> {
    client: fantoccini::Client,
    gmail_client: &'g GmailClient,
    api_client: Option<BestBuyApi>,
    username: &'c str,
    payment: &'c PaymentInfo,
    shipping: &'c Address,
    dry_run: bool,
    state: BotClientState,
}

impl<'c, 'g> WebdriverBot<'c, 'g> {
    const USERNAME_SEL: &'static str = r#"#fld-e"#;
    const PASSWORD_SEL: &'static str = r#"#fld-p1"#;
    const SUBMIT_SEL: &'static str = r#"div.cia-form__controls > button"#;
    const PRODUCT_PRICE_SEL: &'static str = r#"div.priceView-customer-price > span"#;
    const PRODUCT_TITLE_SEL: &'static str = r#"div.sku-title"#;
    const PRODUCT_SKU_SEL: &'static str = r#"div.sku span.product-data-value"#;
    const CART_READY_TEXT_SEL: &'static str = r#"h2.order-summary__heading"#;
    const ADD_TO_CART_BTN_SEL: &'static str = r#"div.fulfillment-add-to-cart-button button"#;
    const REMOVE_CART_LINK_SEL: &'static str = r#"a.cart-item__remove"#;
    const CART_CHECKOUT_BTN_SEL: &'static str = r#"div.checkout-buttons__checkout > button"#;
    const VERIFICATION_CODE_SEL: &'static str = r#"input#verificationCode"#;
    const VERIFICATION_CODE_FORM: &'static str = r#"form.cia-form"#;
    const CHECKOUT_PAGE_READY_SEL: &'static str = r#"h1.fulfillment__page-title"#;
    const CHECKOUT_PAGE_SHIPPING_SEL: &'static str = r#"div.streamlined__shipping"#;
    const CHECKOUT_PAGE_CONTINUE_SEL: &'static str = r#"div.button--continue > button"#;
    const CHECKOUT_PAGE_NEW_ADDRESS_SEL: &'static str = r#"button.saved-addresses__add-new-link"#;
    const SHIPPING_ADDRESS_FIRST_NAME_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.firstName']"#;
    const SHIPPING_ADDRESS_LAST_NAME_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.lastName']"#;
    const SHIPPING_ADDRESS_STREET_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.street']"#;
    const SHIPPING_ADDRESS_CITY_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.city']"#;
    const SHIPPING_ADDRESS_STATE_SEL: &'static str = r#"select[id='consolidatedAddresses.ui_address_2.state']"#;
    const SHIPPING_ADDRESS_ZIP_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.zipcode']"#;
    const SHIPPING_ADDRESS_SAVE_SEL: &'static str = r#"input[id='save-for-billing-address-ui_address_2']"#;
    const PAYMENT_CC_INPUT_SEL: &'static str = r#"input#optimized-cc-card-number"#;
    const PAYMENT_EXP_MONTH_SEL: &'static str = r#"label#credit-card-expiration-month select"#;
    const PAYMENT_EXP_YEAR_SEL: &'static str = r#"label#credit-card-expiration-year select"#;
    const PAYMENT_CVV_SEL: &'static str = r#"input#credit-card-cvv"#;
    const PAYMENT_SAVE_CARD_SEL: &'static str = r#"input#save-card-checkbox"#;
    const PAYMENT_ADDRESS_FIRST_NAME_SEL: &'static str = r#"input[id='payment.billingAddress.firstName']"#;
    const PAYMENT_ADDRESS_LAST_NAME_SEL: &'static str = r#"input[id='payment.billingAddress.lastName']"#;
    const PAYMENT_ADDRESS_STREET_SEL: &'static str = r#"input[id='payment.billingAddress.street']"#;
    const PAYMENT_ADDRESS_CITY_SEL: &'static str = r#"input[id='payment.billingAddress.city']"#;
    const PAYMENT_ADDRESS_STATE_SEL: &'static str = r#"select[id='payment.billingAddress.state']"#;
    const PAYMENT_ADDRESS_ZIP_SEL: &'static str = r#"input[id='payment.billingAddress.zipcode']"#;
    const PAYMENT_PLACE_ORDER_SEL: &'static str = r#"div.button--place-order > button"#;

    fn new(client: fantoccini::Client,
           gmail_client: &'g GmailClient,
           username: &'c str,
           payment: &'c PaymentInfo,
           shipping: &'c Address,
           dry_run: bool) -> Self {
        Self {
            client,
            gmail_client,
            api_client: None,
            username,
            payment,
            shipping,
            dry_run,
            state: BotClientState::Started,
        }
    }

    fn api_client(&self) -> &BestBuyApi {
        self.api_client.as_ref().unwrap()
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

    /// Open the cart page
    async fn open_cart(&mut self) -> Result<()> {
        log::debug!("Opening cart...");
        self.client.goto(CART_URL).await?;
        self.client.wait_for_find(Locator::Css(Self::CART_READY_TEXT_SEL)).await?;
        Ok(())
    }

    /// Clear everything in the cart
    async fn clear_cart(&mut self) -> Result<()> {
        self.open_cart().await?;

        log::debug!("Clearing cart...");

        // Find all of the remove buttons on the cart page
        let remove_btns =
            self.client.find_all(Locator::Css(Self::REMOVE_CART_LINK_SEL)).await?;

        for btn in remove_btns.into_iter() {
            sleep(Duration::from_millis(1000)).await;
            btn.click().await?;
        }

        log::debug!("Cart cleared");

        Ok(())
    }

    /// Sign in to BestBuy
    async fn sign_in(&mut self, username: &str, password: &str) -> Result<BotClientState> {
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

        // TODO: remember me

        // Submit the login form and wait for the new page to load
        submit.click().await?;
        self.client.wait_for_navigation(None).await?;

        log::info!("Signed in successfully");

        // Clear the cart after signing in
        self.clear_cart().await?;

        // Get the authentication cookies
        // TODO: Need to determine how to get valid value of _abck
        let cookies = self.client.get_all_cookies().await?;

        // Build a new API client using the cookies
        self.api_client = Some(BestBuyApi::from_cookies(&cookies)?);

        Ok(BotClientState::SignedIn)
    }

    /// Check if a product is in stock. If yes, add it to the cart.
    async fn check_product(&mut self, product_url: &str) -> Result<BotClientState> {
        log::debug!("Checking product");

        self.client.goto(product_url).await?;

        self.client.wait_for_find(Locator::Css(Self::ADD_TO_CART_BTN_SEL)).await?;
        self.client.wait_for_find(Locator::Css(Self::PRODUCT_PRICE_SEL)).await?;

        let mut price_elem = self.find_element(Self::PRODUCT_PRICE_SEL).await?;
        let price = price_elem
            .prop("innerText")
            .await?
            // Sane default price
            .unwrap_or_else(|| "9999999".to_string());
        let price = Money::from_str(&price.replace("$", ""), iso::USD)?;

        let mut product_title_elem = self.find_element(Self::PRODUCT_TITLE_SEL).await?;
        let product_title = product_title_elem
            .prop("innerText")
            .await?
            .unwrap_or_else(|| "Unknown".to_string());

        let mut sku_label_elem = self.find_element(Self::PRODUCT_SKU_SEL).await?;
        let sku = sku_label_elem
            .prop("innerText")
            .await?
            .expect("Could not extract SKU ID");

        log::info!("Product: {}, SKU: {}, Price: {}", product_title, sku, price);

        let mut add_to_cart_btn = self.client
            .find(Locator::Css(Self::ADD_TO_CART_BTN_SEL))
            .await?;

        // If the product is sold out, stop here
        let is_sold_out = add_to_cart_btn.text().await?.to_lowercase() == "sold out";
        if is_sold_out {
            log::info!("Currently sold out...");
            return Ok(BotClientState::NotInStock);
        } else {
            log::info!("Adding to cart...");
        }

        self.api_client().get_cart().await?;
        self.api_client().add_to_cart(&sku).await?;

        Ok(BotClientState::CartUpdated)
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

    /// Handles the fulfillment page (first step in checkout).
    async fn fulfillment(&mut self) -> Result<()> {
        log::debug!("Starting fulfillment flow...");

        // Wait for page to load
        self.client.wait_for_find(Locator::Css(Self::CHECKOUT_PAGE_READY_SEL)).await?;

        let shippping_info_required =
            self.is_element_present(Self::CHECKOUT_PAGE_SHIPPING_SEL).await?;

        if shippping_info_required {
            log::info!("Entering shipping info...");

            if self.is_element_present(Self::CHECKOUT_PAGE_NEW_ADDRESS_SEL).await? {
                log::debug!("Adding a new address");
                let new_address_btn = self.find_element(Self::CHECKOUT_PAGE_NEW_ADDRESS_SEL).await?;
                new_address_btn.click().await?;
            }

            // Wait for the address inputs to appear
            self.client.wait_for_find(Locator::Css(Self::SHIPPING_ADDRESS_FIRST_NAME_SEL)).await?;
            self.client.wait_for_find(Locator::Css(Self::SHIPPING_ADDRESS_SAVE_SEL)).await?;

            let mut first_name_input = self.find_element(Self::SHIPPING_ADDRESS_FIRST_NAME_SEL).await?;
            let mut last_name_input = self.find_element(Self::SHIPPING_ADDRESS_LAST_NAME_SEL).await?;
            let mut address_input = self.find_element(Self::SHIPPING_ADDRESS_STREET_SEL).await?;
            let mut city_input = self.find_element(Self::SHIPPING_ADDRESS_CITY_SEL).await?;
            let state_input = self.find_element(Self::SHIPPING_ADDRESS_STATE_SEL).await?;
            let mut zip_input = self.find_element(Self::SHIPPING_ADDRESS_ZIP_SEL).await?;
            let save_input = self.find_element(Self::SHIPPING_ADDRESS_SAVE_SEL).await?;

            first_name_input.send_keys(&self.shipping.first_name).await?;
            last_name_input.send_keys(&self.shipping.last_name).await?;
            address_input.send_keys(&self.shipping.street).await?;
            city_input.send_keys(&self.shipping.city).await?;
            state_input.select_by_value(&self.shipping.state).await?;
            zip_input.send_keys(&self.shipping.zip_code).await?;
            save_input.click().await?;
        }

        // Move to the payment page
        let continue_btn = self.find_element(Self::CHECKOUT_PAGE_CONTINUE_SEL).await?;
        continue_btn.click().await?;
        self.client.wait_for_navigation(None).await?;

        log::debug!("Fulfillment flow completed");

        Ok(())
    }

    /// Handles the payment page (second step in checkout).
    async fn payment(&mut self) -> Result<()> {
        log::debug!("Starting payment flow...");

        // Wait for payment page to load
        self.client.wait_for_find(Locator::Css(Self::PAYMENT_CC_INPUT_SEL)).await?;

        // Input the CC number first to get other elements to appear
        let mut cc_input = self.find_element(Self::PAYMENT_CC_INPUT_SEL).await?;
        cc_input.send_keys(&self.payment.card_number).await?;
        sleep(Duration::from_millis(100)).await;

        // Input remaining CC info
        let exp_month_input = self.find_element(Self::PAYMENT_EXP_MONTH_SEL).await?;
        let exp_year_input = self.find_element(Self::PAYMENT_EXP_YEAR_SEL).await?;
        let mut cvv_input = self.find_element(Self::PAYMENT_CVV_SEL).await?;
        let save_card_input = self.find_element(Self::PAYMENT_SAVE_CARD_SEL).await?;

        exp_month_input.select_by_value(&self.payment.exp_month).await?;
        exp_year_input.select_by_value(&self.payment.exp_year).await?;
        cvv_input.send_keys(&self.payment.cvv.to_string()).await?;
        save_card_input.click().await?;

        // Input billing address
        let mut first_name_input = self.find_element(Self::PAYMENT_ADDRESS_FIRST_NAME_SEL).await?;
        let mut last_name_input = self.find_element(Self::PAYMENT_ADDRESS_LAST_NAME_SEL).await?;
        let mut address_input = self.find_element(Self::PAYMENT_ADDRESS_STREET_SEL).await?;
        let mut city_input = self.find_element(Self::PAYMENT_ADDRESS_CITY_SEL).await?;
        let state_input = self.find_element(Self::PAYMENT_ADDRESS_STATE_SEL).await?;
        let mut zip_input = self.find_element(Self::PAYMENT_ADDRESS_ZIP_SEL).await?;

        first_name_input.send_keys(&self.payment.billing.first_name).await?;
        last_name_input.send_keys(&self.payment.billing.last_name).await?;
        address_input.send_keys(&self.payment.billing.street).await?;
        city_input.send_keys(&self.payment.billing.city).await?;
        state_input.select_by_value(&self.payment.billing.state).await?;
        zip_input.send_keys(&self.payment.billing.zip_code).await?;

        if self.dry_run {
            log::info!("Dry run; stopping here");
        } else {
            // Place the order!
            let order_btn = self.find_element(Self::PAYMENT_PLACE_ORDER_SEL).await?;
            order_btn.click().await?;
            self.client.wait_for_navigation(None).await?;
            log::info!("Order placed!");

            sleep(Duration::from_secs(10)).await;
        }

        Ok(())
    }

    /// Purchase whatever is in the cart.
    async fn checkout(&mut self) -> Result<BotClientState> {
        log::debug!("Starting checkout flow...");

        self.open_cart().await?;
        self.client.wait_for_find(Locator::Css(Self::CART_CHECKOUT_BTN_SEL)).await?;

        let checkout_btn_locator = Locator::Css(Self::CART_CHECKOUT_BTN_SEL);

        // Start the checkout
        // Note that we may need to try this a few times
        loop {
            let checkout_btn = self.client.find(checkout_btn_locator).await?;
            if let Ok(_) = checkout_btn.click().await {
                break;
            }
        }

        self.client.wait_for_navigation(None).await?;

        self.fulfillment().await?;
        self.payment().await?;

        log::debug!("Checkout flow completed");

        Ok(BotClientState::Purchased)
    }

    /// Run the client to completion.
    async fn run(&mut self, product_url: &str, username: &str, password: &str, checkout: bool) -> Result<BotClientState> {
        loop {
            // Prior to executing a step, check if we hit the email verification
            self.verify_code().await?;

            // Figure out what to do next based on current state
            match self.state {
                BotClientState::Started => self.state = self.sign_in(username, password).await?,
                BotClientState::SignedIn => self.state = self.check_product(product_url).await?,
                BotClientState::CartUpdated => {
                    if checkout {
                        self.state = self.checkout().await?;
                    } else {
                        break;
                    }
                }
                BotClientState::NotInStock | BotClientState::Purchased => break,
            }
        }

        let state = self.state;

        // Put the client back in the signed in state
        self.state = BotClientState::SignedIn;

        Ok(state)
    }
}

/// A single instance of a BestBuy bot.
///
/// Each bot checks the given list of products on every tick and adds
/// all available to the cart before checking out.
pub struct BestBuyBot<'c, 'g, 't> {
    product_urls: VecDeque<String>,
    gmail_client: &'g GmailClient,
    twilio_client: Option<&'t TwilioClient>,
    config: &'c Config,
}

impl<'c, 'g, 't> BestBuyBot<'c, 'g, 't> {
    pub fn new(config: &'c Config,
               gmail_client: &'g GmailClient,
               twilio_client: Option<&'t TwilioClient>) -> Self {
        let product_urls = VecDeque::from_iter(config.general.products.to_owned().into_iter());

        Self {
            config,
            product_urls,
            gmail_client,
            twilio_client,
        }
    }

    /// Try to send a notification SMS when an item is purchased.
    async fn send_message(&self, message: &str) -> Result<()> {
        if self.twilio_client.is_none() {
            return Ok(());
        }

        let twilio_client = self.twilio_client.unwrap();
        let twilio_config = self.config.twilio.as_ref().unwrap();

        twilio_client.send_message(
            &twilio_config.from_number,
            &twilio_config.to_number,
            message
        ).await?;

        log::info!("Sent notification SMS successfully");

        Ok(())
    }

    pub async fn start(&mut self, dry_run: bool, headless: bool) -> Result<()> {
        let username = self.config.login.as_ref().unwrap().username.as_str();
        let password = self.config.login.as_ref().unwrap().password.as_str();
        let hostname = self.config.general.hostname.as_deref();
        let payment = &self.config.payment;
        let shipping = self.config.shipping.as_ref().unwrap();
        let interval = Duration::from_secs(self.config.general.interval.unwrap_or(20));

        // Connect to Webdriver client
        let client = crate::common::new_webdriver_client(headless, hostname).await?;

        // Create a Webdriver bot for BestBuy
        let mut client = WebdriverBot::new(
            client,
            self.gmail_client,
            username,
            payment,
            shipping,
            dry_run,
        );

        while self.product_urls.len() > 0 {
            let num_urls = self.product_urls.len();

            // Check each of the products in the queue.
            //
            // If a product is out of stock, it is put back on the queue.
            for _ in 0..num_urls {
                if let Some(product_url) = self.product_urls.pop_front() {
                    match client.run(&product_url, username, password, true).await? {
                        BotClientState::Purchased => {
                            let message = format!("Purchased: {}", product_url);
                            self.send_message(&message).await?;
                        }
                        _ => self.product_urls.push_back(product_url),
                    };
                }
            }

            log::debug!("Sleeping for {:?}", interval);

            sleep(interval).await;
        }

        Ok(())
    }
}
