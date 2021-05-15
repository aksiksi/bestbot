use std::collections::VecDeque;
use std::iter::FromIterator;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use fantoccini::{Locator, elements::Element};
use regex::Regex;
use rusty_money::{Money, iso};
use tokio::time::sleep;

use crate::config::{Address, Config, PaymentInfo};
use crate::gmail::GmailClient;

static CART_URL: &str = "https://www.bestbuy.com/cart";
static SIGN_IN_URL: &str = "https://www.bestbuy.com/identity/global/signin";
static EMAIL_CODE_PAT: &str = r#"<span.+>(\d+)</span>"#;

#[derive(Clone, Copy, Debug)]
enum BotClientState {
    Started,
    SignedIn,
    CartUpdated,
    NotInStock,
    Purchased,
}

#[derive(Clone)]
struct BotClient {
    client: fantoccini::Client,
    gmail_client: Arc<GmailClient>,
    username: String,
    payment_info: PaymentInfo,
    shipping_address: Address,
    dry_run: bool,
    state: BotClientState,
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
    const SHOPPING_CART_COUNT_SEL: &'static str = r#"div.shop-cart-icon div.dot"#;
    const VERIFICATION_CODE_SEL: &'static str = r#"input#verificationCode"#;
    const VERIFICATION_CODE_FORM: &'static str = r#"form.cia-form"#;
    const CHECKOUT_PAGE_READY_SEL: &'static str = r#"h1.fulfillment__page-title"#;
    const CHECKOUT_PAGE_SHIPPING_SEL: &'static str = r#"div.streamlined__shipping"#;
    const CHECKOUT_PAGE_CONTINUE_SEL: &'static str = r#"div.button--continue > button"#;
    const SHIPPING_ADDRESS_FIRST_NAME_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.firstName']"#;
    const SHIPPING_ADDRESS_LAST_NAME_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.lastName']"#;
    const SHIPPING_ADDRESS_STREET_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.street']"#;
    const SHIPPING_ADDRESS_CITY_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.city']"#;
    const SHIPPING_ADDRESS_STATE_SEL: &'static str = r#"input[id='consolidatedAddresses.ui_address_2.state']"#;
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
           gmail_client: GmailClient,
           username: String,
           payment_info: PaymentInfo,
           shipping_address: Address,
           dry_run: bool) -> Self {
        Self {
            client,
            gmail_client: Arc::new(gmail_client),
            username,
            payment_info,
            shipping_address,
            dry_run,
            state: BotClientState::Started,
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

    /// Open the cart page
    async fn open_cart(&mut self) -> Result<()> {
        self.client.goto(CART_URL).await?;
        self.client.wait_for_find(Locator::Css(Self::CART_READY_TEXT_SEL)).await?;
        Ok(())
    }

    /// Clear everything in the cart
    async fn clear_cart(&mut self) -> Result<()> {
        // Check if there are any items in the cart
        if !self.is_element_present(Self::SHOPPING_CART_COUNT_SEL).await? {
            return Ok(());
        }

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
    async fn sign_in(&mut self, username: &str, password: &str) -> Result<BotClientState> {
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

        // Clear the cart after logging in
        self.clear_cart().await?;

        Ok(BotClientState::SignedIn)
    }

    /// Figure out if we have a modal. If we do, close it.
    async fn close_modal(&mut self) -> Result<()> {
        if self.is_element_present(".close-modal-x").await? {
            let btn = self.client
                .find(Locator::Css(".close-modal-x"))
                .await?;
            btn.click().await?;
            println!("Closed modal");
        }

        Ok(())
    }

    /// Check if a product is in stock. If yes, add it to the cart.
    async fn check_product(&mut self, product_url: &str) -> Result<BotClientState> {
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

        // If the product is sold out, stop here
        let is_sold_out = add_to_cart_btn.text().await?.to_lowercase() == "sold out";
        if is_sold_out {
            println!("Currently sold out...");
            return Ok(BotClientState::NotInStock);
        }

        // Add this product to the cart
        add_to_cart_btn.click().await?;

        // Wait for cart modal to pop up and close it
        sleep(Duration::from_millis(1000)).await;
        self.close_modal().await?;

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

        Ok(code)
    }

    /// Check if we have a verification code on the page. If we do, go through
    /// the verification flow.
    async fn verify_code(&mut self) -> Result<()> {
        let verify_required = self.is_element_present(Self::VERIFICATION_CODE_SEL).await?;
        if !verify_required {
            return Ok(());
        }

        let form = self.client
            .form(Locator::Css(Self::VERIFICATION_CODE_FORM))
            .await?;
        let mut input = self.find_element(Self::VERIFICATION_CODE_SEL).await?;

        // Get the verifcation code from Gmail
        let code = self.get_email_code().await?;
        input.send_keys(&code).await?;

        println!("Code: {}", code);

        // Submit the form
        form.submit().await?;
        self.client.wait_for_navigation(None).await?;

        Ok(())
    }

    /// Handles the fulfillment page (first step in checkout).
    async fn fulfillment(&mut self) -> Result<()> {
        // Wait for page to load
        self.client.wait_for_find(Locator::Css(Self::CHECKOUT_PAGE_READY_SEL)).await?;

        let shippping_info_required =
            self.is_element_present(Self::CHECKOUT_PAGE_SHIPPING_SEL).await?;

        if shippping_info_required {
            self.client.wait_for_find(Locator::Css(Self::SHIPPING_ADDRESS_FIRST_NAME_SEL)).await?;

            let mut first_name_input = self.find_element(Self::SHIPPING_ADDRESS_FIRST_NAME_SEL).await?;
            let mut last_name_input = self.find_element(Self::SHIPPING_ADDRESS_LAST_NAME_SEL).await?;
            let mut address_input = self.find_element(Self::SHIPPING_ADDRESS_STREET_SEL).await?;
            let mut city_input = self.find_element(Self::SHIPPING_ADDRESS_CITY_SEL).await?;
            let state_input = self.find_element(Self::SHIPPING_ADDRESS_STATE_SEL).await?;
            let mut zip_input = self.find_element(Self::SHIPPING_ADDRESS_ZIP_SEL).await?;
            let save_input = self.find_element(Self::SHIPPING_ADDRESS_SAVE_SEL).await?;

            first_name_input.send_keys(&self.shipping_address.first_name).await?;
            last_name_input.send_keys(&self.shipping_address.last_name).await?;
            address_input.send_keys(&self.shipping_address.street).await?;
            city_input.send_keys(&self.shipping_address.city).await?;
            state_input.select_by_value(&self.shipping_address.state).await?;
            zip_input.send_keys(&self.shipping_address.zip_code).await?;
            save_input.click().await?;
        }

        // Move to the payment page
        let continue_btn = self.find_element(Self::CHECKOUT_PAGE_CONTINUE_SEL).await?;
        continue_btn.click().await?;
        self.client.wait_for_navigation(None).await?;

        Ok(())
    }

    /// Handles the payment page (second step in checkout).
    async fn payment(&mut self) -> Result<()> {
        // Wait for payment page to load
        self.client.wait_for_find(Locator::Css(Self::PAYMENT_CC_INPUT_SEL)).await?;

        // Input the CC number first to get other elements to appear
        let mut cc_input = self.find_element(Self::PAYMENT_CC_INPUT_SEL).await?;
        cc_input.send_keys(&self.payment_info.card_number).await?;
        sleep(Duration::from_millis(100)).await;

        // Input remaining CC info
        let exp_month_input = self.find_element(Self::PAYMENT_EXP_MONTH_SEL).await?;
        let exp_year_input = self.find_element(Self::PAYMENT_EXP_YEAR_SEL).await?;
        let mut cvv_input = self.find_element(Self::PAYMENT_CVV_SEL).await?;
        let save_card_input = self.find_element(Self::PAYMENT_SAVE_CARD_SEL).await?;

        exp_month_input.select_by_value(&self.payment_info.exp_month).await?;
        exp_year_input.select_by_value(&self.payment_info.exp_year).await?;
        cvv_input.send_keys(&self.payment_info.cvv.to_string()).await?;
        save_card_input.click().await?;

        // Input billing address
        let mut first_name_input = self.find_element(Self::PAYMENT_ADDRESS_FIRST_NAME_SEL).await?;
        let mut last_name_input = self.find_element(Self::PAYMENT_ADDRESS_LAST_NAME_SEL).await?;
        let mut address_input = self.find_element(Self::PAYMENT_ADDRESS_STREET_SEL).await?;
        let mut city_input = self.find_element(Self::PAYMENT_ADDRESS_CITY_SEL).await?;
        let state_input = self.find_element(Self::PAYMENT_ADDRESS_STATE_SEL).await?;
        let mut zip_input = self.find_element(Self::PAYMENT_ADDRESS_ZIP_SEL).await?;

        first_name_input.send_keys(&self.payment_info.billing_address.first_name).await?;
        last_name_input.send_keys(&self.payment_info.billing_address.last_name).await?;
        address_input.send_keys(&self.payment_info.billing_address.street).await?;
        city_input.send_keys(&self.payment_info.billing_address.city).await?;
        state_input.select_by_value(&self.payment_info.billing_address.state).await?;
        zip_input.send_keys(&self.payment_info.billing_address.zip_code).await?;

        // Place the order!
        if !self.dry_run {
            let order_btn = self.find_element(Self::PAYMENT_PLACE_ORDER_SEL).await?;
            order_btn.click().await?;
            self.client.wait_for_navigation(None).await?;
        }

        Ok(())
    }

    /// Purchase whatever is in the cart.
    async fn checkout(&mut self) -> Result<BotClientState> {
        self.open_cart().await?;
        self.client.wait_for_find(Locator::Css(Self::CART_CHECKOUT_BTN_SEL)).await?;

        let checkout_btn_locator = Locator::Css(Self::CART_CHECKOUT_BTN_SEL);
        let checkout_btn = self.client.find(checkout_btn_locator).await?;

        // Start the checkout
        checkout_btn.click().await?;
        self.client.wait_for_navigation(None).await?;

        // Check for verification code and input if needed
        self.verify_code().await?;

        self.fulfillment().await?;
        self.payment().await?;

        Ok(BotClientState::Purchased)
    }

    /// Run the client to completion.
    async fn run(&mut self, product_url: &str, username: &str, password: &str, checkout: bool) -> Result<BotClientState> {
        loop {
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
pub struct BestBuyBot {
    interval: Duration,
    username: String,
    password: String,
    hostname: String,
    working_dir: String,
    product_urls: VecDeque<String>,
    payment_info: PaymentInfo,
    shipping_address: Address,
    dry_run: bool,
}

impl BestBuyBot {
    pub fn new(config: Config, dry_run: bool) -> Self {
        let login_info = config.login_info.unwrap();
        let username = login_info.username;
        let password = login_info.password;
        let hostname = config.hostname.unwrap_or_else(|| "http://localhost:4444".to_string());
        let interval = Duration::from_secs(config.interval.unwrap_or(20));
        let product_urls = config.products.into_iter();
        let working_dir = config.working_dir.unwrap_or_else(|| String::new());
        let payment_info = config.payment_info;
        let shipping_address = config.shipping_address.unwrap();

        Self {
            interval,
            username,
            password,
            hostname,
            working_dir,
            product_urls: VecDeque::from_iter(product_urls),
            payment_info,
            shipping_address,
            dry_run,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        // Setup the Gmail API client
        let app_secret_name = format!("{}-secret.json", self.username);
        let token_persist_name = format!("{}-token.json", self.username);
        let app_secret_path = PathBuf::new().join(&self.working_dir).join(app_secret_name);
        let token_persist_path = PathBuf::new().join(&self.working_dir).join(token_persist_name);
        let gmail_client = GmailClient::new(&app_secret_path, &token_persist_path).await?;

        // Setup the Webdriver client
        let client = fantoccini::ClientBuilder::native()
            .connect(&self.hostname)
            .await?;

        // Create a BestBuy bot client
        let mut client = BotClient::new(
            client,
            gmail_client,
            self.username.clone(),
            self.payment_info.clone(),
            self.shipping_address.clone(),
            self.dry_run,
        );

        while self.product_urls.len() > 0 {
            let num_urls = self.product_urls.len();

            // Check each of the products in the list.
            //
            // If a product is out of stock, it is put back on the queue.
            for _ in 0..num_urls {
                if let Some(product_url) = self.product_urls.pop_front() {
                    match client.run(&product_url, &self.username, &self.password, true).await? {
                        BotClientState::Purchased => (),
                        _ => self.product_urls.push_back(product_url),
                    };
                }
            }

            sleep(self.interval).await;
        }

        Ok(())
    }
}
