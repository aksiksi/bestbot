#!/bin/bash

# Start geckodriver
geckodriver &

# Start the bot
RUST_LOG=debug bestbot --headless /config/config.toml

