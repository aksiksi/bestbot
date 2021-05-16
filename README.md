# bestbot

A simple Bestbuy bot written in Rust.  Work in progress.

## Quickstart

1. Install Google Chrome
2. Download the matching version of [`chromedriver`](https://chromedriver.chromium.org/downloads)
3. Run `chromedriver` on port 4444: `chromedriver --port=4444`
4. Run the bot

## Docker

Steps to follow:

1. Enable IPv6 support in Docker: https://docs.docker.com/config/daemon/ipv6/
2. Build the image: `docker build -t bestbot:v1 .`
3. Place your `config.toml`, `gmail-api-secret.json`, and Gmail token in the current dir
4. Run: `docker run --env RUST_LOG=debug -v "$(pwd):/config" bestbot:v1`

## Design

TBD

