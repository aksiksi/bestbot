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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::from_args();
    let config = config::Config::load(args.config_file)?;
    let mut bot = BestBuyBot::new(config, args.dry_run);

    bot.start().await?;

    Ok(())
}
