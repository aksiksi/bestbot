use std::path::PathBuf;

use anyhow::Result;
use structopt::StructOpt;

mod bestbuy;
mod common;
mod config;
mod discord;
mod gmail;
mod twilio;

use bestbuy::BestBuyBot;
use discord::DiscordWebhook;
use gmail::GmailClient;
use twilio::TwilioClient;

#[derive(StructOpt)]
struct Args {
    config_file: PathBuf,
    #[structopt(long)]
    dry_run: bool,
    #[structopt(long)]
    headless: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let args = Args::from_args();
    let config = config::Config::load(args.config_file)?;

    let gmail_client = GmailClient::from_config(&config).await?;
    let twilio_client = TwilioClient::from_config(&config)?;
    let discord_client = DiscordWebhook::from_config(&config);

    let mut bot = BestBuyBot::new(
        &config,
        gmail_client.as_ref(),
        twilio_client.as_ref(),
        discord_client.as_ref()
    );

    bot.start(args.dry_run, args.headless).await?;

    Ok(())
}
