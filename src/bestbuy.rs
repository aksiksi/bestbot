use std::time::Duration;

use anyhow::Result;
use fantoccini::Locator;
use futures::{TryStreamExt, stream::FuturesUnordered};
use rusty_money::{Money, iso};
use tokio::time::sleep;

static SIGN_IN_URL: &str = "https://www.bestbuy.com/identity/global/signin";

#[derive(Clone, Copy, Debug)]
enum ClientState {
    None,
    Started,
    SignedIn,
    CartUpdated,
    NotInStock,
    Purchased,
    Errored,
}

struct BotClient {
    username: String,
    password: String,
    product_url: String,
    hostname: String,
    client: Option<fantoccini::Client>,
    state: ClientState,
}

impl BotClient {
    const USERNAME_SEL: &'static str = r#"#fld-e"#;
    const PASSWORD_SEL: &'static str = r#"#fld-p1"#;
    const SUBMIT_SEL: &'static str = r#"div.cia-form__controls > button"#;
    const PRODUCT_PRICE_SEL: &'static str = r#"div.priceView-customer-price > span"#;
    const ADD_TO_CART_BTN_SEL: &'static str = r#"div.fulfillment-add-to-cart-button button"#;

    /// Creates a new browser client.
    fn new(username: String, password: String, product_url: String, hostname: Option<&str>) -> Self {
        let hostname = hostname.unwrap_or("http://localhost:4444").to_string();
        Self {
            username,
            password,
            product_url,
            hostname,
            client: None,
            state: ClientState::None,
        }
    }

    async fn init(&mut self) -> Result<ClientState> {
        let client = fantoccini::ClientBuilder::native()
            .connect(&self.hostname)
            .await?;
        self.client = Some(client);
        Ok(ClientState::Started)
    }

    /// Sign in to BestBuy
    async fn sign_in(&mut self) -> Result<ClientState> {
        let client = self.client.as_mut().unwrap();

        client.goto(SIGN_IN_URL).await?;

        client.wait_for_find(Locator::Css(Self::USERNAME_SEL)).await?;
        client.wait_for_find(Locator::Css(Self::PASSWORD_SEL)).await?;
        client.wait_for_find(Locator::Css(Self::SUBMIT_SEL)).await?;

        let mut username = client.find(
            Locator::Css(Self::USERNAME_SEL)
        ).await?;
        let mut password = client.find(
            Locator::Css(Self::PASSWORD_SEL)
        ).await?;
        let submit = client.find(
            Locator::Css(Self::SUBMIT_SEL)
        ).await?;

        username.send_keys(&self.username).await?;
        password.send_keys(&self.password).await?;

        // Submit the login form and wait for the new page to load
        submit.click().await?;
        client.wait_for_navigation(None).await?;

        Ok(ClientState::SignedIn)
    }

    /// Check if a product is in stock. If yes, add it to the cart.
    async fn check_product(&mut self) -> Result<ClientState> {
        let client = self.client.as_mut().unwrap();

        client.goto(&self.product_url).await?;

        client.wait_for_find(Locator::Css(Self::ADD_TO_CART_BTN_SEL)).await?;
        client.wait_for_find(Locator::Css(Self::PRODUCT_PRICE_SEL)).await?;

        let mut price_elem = client
            .find(Locator::Css(Self::PRODUCT_PRICE_SEL))
            .await?;
        let price = price_elem
            .prop("innerText")
            .await?
            // Sane default price
            .unwrap_or_else(|| "9999999".to_string());

        let price = Money::from_str(&price.replace("$", ""), iso::USD)?;
        println!("{}", price);

        let mut add_to_cart_btn = client
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
        let close_modal_btn = client
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

    /// Purchase whatever is in the cart.
    async fn purchase(&mut self) -> Result<ClientState> {
        Ok(ClientState::Purchased)
    }

    /// Run the client to completion.
    async fn run(&mut self) -> Result<ClientState> {
        loop {
            match self.state {
                ClientState::None => self.state = self.init().await?,
                ClientState::Started => self.state = self.sign_in().await?,
                ClientState::SignedIn => self.state = self.check_product().await?,
                ClientState::CartUpdated => self.state = self.purchase().await?,
                ClientState::Errored | ClientState::NotInStock | ClientState::Purchased => break,
            }
        }

        if let Some(client) = self.client.as_mut() {
            client.close().await?;
        }

        self.client = None;

        Ok(self.state)
    }
}

pub struct BestBuyBot {
    interval: Duration,
    num_clients: usize,
    product_urls: Vec<String>,
}

impl BestBuyBot {
    pub fn new(interval: Duration, num_clients: Option<usize>) -> Self {
        let num_clients = if let Some(n) = num_clients {
            assert!(n > 0);
            n
        } else {
            4
        };

        Self {
            interval,
            num_clients,
            product_urls: vec![],
        }
    }

    pub fn add_product(&mut self, product_id: String) {
        self.product_urls.push(product_id);
    }

    pub fn start(&mut self) -> Result<()> {
        let username = match std::env::var("BESTBOT_USERNAME") {
            Ok(u) => u,
            Err(_) => panic!("BESTBOT_USERNAME env variable not set"),
        };
        let password = match std::env::var("BESTBOT_PASSWORD") {
            Ok(u) => u,
            Err(_) => panic!("BESTBOT_PASSWORD env variable not set"),
        };

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let num_clients = std::cmp::min(self.num_clients, self.product_urls.len());

        let mut products = self.product_urls.iter().cycle();

        loop {
            // Build a list of product URLs to check
            let mut product_urls = Vec::new();
            for p in &mut products {
                product_urls.push(p.clone());
                if product_urls.len() == num_clients {
                    break;
                }
            }

            // Create a new client for each product
            let mut clients: Vec<_> = product_urls
                .into_iter()
                .map(|product_url: String| {
                    BotClient::new(username.clone(), password.clone(), product_url, None)
                })
                .collect();

            // Wait for all clients to terminate
            rt.block_on(async move {
                let tasks = clients
                    .iter_mut()
                    .map(|client| {
                        client.run()
                    })
                    .collect::<FuturesUnordered<_>>();

                let results: Result<Vec<ClientState>> = tasks.into_stream().try_collect().await;

                dbg!(results.unwrap());
            });

            std::thread::sleep(self.interval);
        }
    }
}
