ENV TZ="America/New_York"

# Layer that just builds the image
FROM rust:1.52.1 as builder
WORKDIR /var/bestbot
COPY . .
RUN cargo install --path .

# Layer that contains the final binary
FROM debian:buster-slim

# Install dependencies
RUN apt-get update && \
    apt-get install -y extra-runtime-dependencies && \
    apt-get install -y git wget curl unzip && \
    rm -rf /var/lib/apt/lists/*

# Copy binary from previous layer
COPY --from=builder /usr/local/cargo/bin/bestbot /usr/local/bin/

# Install latest Google Chrome
# This will likely fail due to missing deps
RUN wget https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb && dpkg -i google-chrome-stable_current_amd64.deb; exit 0

# Install missing Chrome deps
# NOTE: Needs to be non-interactive to disable the tzdata config prompt
RUN DEBIAN_FRONTEND="noninteractive" apt-get install -fy

# Install latest Chromedriver
RUN a=$(uname -m) && \
    mkdir /tmp/chromedriver/ && \
    wget -O /tmp/chromedriver/LATEST_RELEASE http://chromedriver.storage.googleapis.com/LATEST_RELEASE && \
    if [ $a == i686 ]; then b=32; elif [ $a == x86_64 ]; then b=64; fi && \
    latest=$(cat /tmp/chromedriver/LATEST_RELEASE) && \
    wget -O /tmp/chromedriver/chromedriver.zip 'http://chromedriver.storage.googleapis.com/'$latest'/chromedriver_linux'$b'.zip' && \
    sudo unzip /tmp/chromedriver/chromedriver.zip chromedriver -d /usr/local/bin/ && \
    rm -rf /tmp/chromedriver

# Prepare the config volume
RUN mkdir /config
VOLUME /config

# Copy the start script
COPY docker/start.sh /

ENTRYPOINT ["/start.sh"]
