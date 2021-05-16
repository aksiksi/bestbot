#!/bin/bash

# Start Chromdriver
chromedriver --port=4444 &

# Start the bot
bestbot --headless /config/config.toml

