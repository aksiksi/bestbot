FROM rust:1.52.1

SHELL ["/bin/bash", "-c"]
ENV TZ="America/New_York"

WORKDIR /var/bestbot
COPY . .

# Install dependencies
RUN apt-get update && \
    apt-get install -y git wget curl unzip && \
    apt-get install -y libnss3-dev libgdk-pixbuf2.0-dev libgtk-3-dev libxss-dev && \
    rm -rf /var/lib/apt/lists/*

# Install latest Google Chrome
# This will likely fail due to missing deps
RUN wget -q -O - https://dl-ssl.google.com/linux/linux_signing_key.pub | apt-key add -
RUN sh -c 'echo "deb [arch=amd64] http://dl.google.com/linux/chrome/deb/ stable main" >> /etc/apt/sources.list.d/google-chrome.list'
RUN apt-get -y update && \
    apt-get install -y google-chrome-stable

# Install latest Chromedriver
RUN a=$(uname -m) && \
    mkdir /tmp/chromedriver/ && \
    wget -O /tmp/chromedriver/LATEST_RELEASE http://chromedriver.storage.googleapis.com/LATEST_RELEASE && \
    if [ $a == i686 ]; then b=32; elif [ $a == x86_64 ]; then b=64; fi && \
    latest=$(cat /tmp/chromedriver/LATEST_RELEASE) && \
    wget -O /tmp/chromedriver/chromedriver.zip 'http://chromedriver.storage.googleapis.com/'$latest'/chromedriver_linux'$b'.zip' && \
    unzip /tmp/chromedriver/chromedriver.zip chromedriver -d /usr/local/bin/ && \
    rm -rf /tmp/chromedriver

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

