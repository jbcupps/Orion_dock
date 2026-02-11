# Dev image for Orion Docker-first workflow.
# Source is bind-mounted at /app (see docker-compose.yml).

ARG RUST_VERSION=1.75

FROM rust:${RUST_VERSION}-bookworm
RUN apt-get update -y && apt-get install -y --no-install-recommends \
    build-essential \
    libssl-dev \
    pkg-config \
    ca-certificates \
    curl \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt clippy

WORKDIR /app
ENV CARGO_TARGET_DIR=/app/target
CMD ["bash"]
