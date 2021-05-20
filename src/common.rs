use anyhow::Result;

#[derive(Clone, Copy, Debug)]
pub enum BotClientState {
    Started,
    SignedIn,
    CartUpdated,
    NotInStock,
    Purchased,
}

/// Creates a new Webdriver client
pub async fn new_webdriver_client(headless: bool, hostname: Option<&str>) -> Result<fantoccini::Client> {
    let hostname = hostname.unwrap_or("http://localhost:4444");

    let mut client = fantoccini::ClientBuilder::native();

    if headless {
        let mut caps = serde_json::map::Map::new();

        let chrome_args = serde_json::json!({
            "args": [
                "--no-sandbox",
                "--headless",
                "--no-proxy-server",
                "--proxy-server='direct://'",
                "--proxy-bypass-list=*",
                "--window-size=1920,1080",
                "--start-maximized",
                "--ignore-certificate-errors",
                "--disable-extensions",
                "--blink-settings=imagesEnabled=false",
            ]
        });

        // https://developer.mozilla.org/en-US/docs/Web/WebDriver/Capabilities/firefoxOptions
        let firefox_args = serde_json::json!({
            "args": [
                "-headless",
            ]
        });

        caps.insert("goog:chromeOptions".to_string(), chrome_args);
        caps.insert("moz:firefoxOptions".to_string(), firefox_args);

        client.capabilities(caps);
    }

    let mut client = client.connect(hostname).await?;

    log::debug!("Connected to WebDriver - session ID: {}", client.session_id().await?.unwrap());

    Ok(client)
}
