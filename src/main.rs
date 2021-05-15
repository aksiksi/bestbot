use std::path::PathBuf;

use anyhow::Result;
use structopt::StructOpt;

mod bestbuy;
mod config;
mod gmail;

use bestbuy::BestBuyBot;

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
    let mut bot = BestBuyBot::new(config);

    bot.start(args.dry_run, args.headless).await?;

    Ok(())
}
