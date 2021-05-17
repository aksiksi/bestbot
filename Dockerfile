FROM rust:1.52.1

SHELL ["/bin/bash", "-c"]
ENV TZ="America/New_York"

WORKDIR /var/bestbot
COPY . .

# Install dependencies
RUN apt-get update && \
    apt-get install -y git wget curl unzip && \
    apt-get install -y firefox-esr && \
    rm -rf /var/lib/apt/lists/*

# Download geckodriver
RUN wget https://github.com/mozilla/geckodriver/releases/download/v0.29.1/geckodriver-v0.29.1-linux64.tar.gz
RUN tar -xvzf geckodriver-v0.29.1-linux64.tar.gz
RUN mv geckodriver /usr/local/bin/
RUN chmod +x /usr/local/bin/geckodriver

# Build
RUN cargo install --path .
RUN cargo clean

# Prepare the config volume
RUN mkdir /config
VOLUME /config

# Copy the entrypoint script
COPY docker/entrypoint.sh /

RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]

