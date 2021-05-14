use fantoccini::{ClientBuilder, Locator};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to webdriver instance that is listening on port 4444
    let mut client = ClientBuilder::native()
        .connect("http://localhost:4444")
        .await?;

    // client.goto("https://www.bestbuy.com/site/sony-playstation-5-console/6426149.p?skuId=6426149").await?;
    client.goto("https://www.bestbuy.com/site/seagate-game-drive-for-playstation-2tb-external-usb-3-0-portable-hard-drive-black-black/6309234.p?skuId=6309234").await?;

    // This sleep is just used to make the browser's actions visible.
    sleep(Duration::from_millis(1000)).await;

    // Get current item price
    let price = client
        .find(Locator::Css(
            r#"div.priceView-customer-price>span"#,
        ))
        .await?
        .text()
        .await?;

    let mut add_to_cart_btn = client
        .find(Locator::Css(
            r#"div.fulfillment-add-to-cart-button button"#,
        ))
        .await?;

    let is_sold_out = add_to_cart_btn.text().await? == "Sold Out";

    if is_sold_out {
        println!("Currently sold out...");
    } else {
        add_to_cart_btn.click().await?;
        sleep(Duration::from_millis(1000)).await;
    }

    client.close().await?;

    Ok(())
}
