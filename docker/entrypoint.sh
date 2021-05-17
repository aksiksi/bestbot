#!/bin/bash

# Start geckodriver
geckodriver &

# Start the bot
bestbot --headless /config/config.toml

