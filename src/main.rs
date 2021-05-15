use std::time::Duration;

use anyhow::Result;

mod bestbuy;
mod gmail;

use bestbuy::BestBuyBot;

#[tokio::main]
async fn main() -> Result<()> {
    let mut bot = BestBuyBot::new(Duration::from_secs(10), None);
    bot.add_product("https://www.bestbuy.com/site/sony-playstation-5-console/6426149.p?skuId=6426149".to_string());
    bot.add_product("https://www.bestbuy.com/site/macbook-air-13-3-laptop-apple-m1-chip-8gb-memory-256gb-ssd-latest-model-gold/6418599.p?skuId=6418599".to_string());
    bot.start().await?;
    Ok(())
}
