#!/bin/bash

# Start geckodriver
geckodriver &

# Start the bot
RUST_LOG=bestbot=debug bestbot --headless /config/config.toml
