# Development image of the Rust backend: toolchain, sqlx-cli, watchexec and
# cargo-deny.
# Used by the `backend` (one-off commands: build, lint, test, migrate) and
# `api` (server with live reload) services of deploy/compose.dev.yaml.
# Nothing in it is shipped: production images come with L01-US06.

# CI pulls it from ghcr.io, tagged with this file's hash, instead of
# compiling the tools on every run (`just ci-dev-image`).

# Keep in sync with backend/rust-toolchain.toml.
FROM rust:1.99.1-trixie
LABEL org.opencontainers.image.source="https://github.com/argoleon/MyLittleBigProjectPlanner" \
      org.opencontainers.image.description="Development toolchain of My Little Big Project Planner (not shipped)"

RUN rustup component add rustfmt clippy \
    && apt-get update \
    && apt-get install -y --no-install-recommends procps \
    && rm -rf /var/lib/apt/lists/*

# Tools are installed in /usr/local/cargo/bin, outside the CARGO_HOME volume.
# sqlx-cli 0.9 ships no lock file, so --locked does not apply to it.
RUN cargo install sqlx-cli@0.9.0 --no-default-features --features postgres \
    && cargo install --locked watchexec-cli \
    && cargo install --locked cargo-deny@0.20.2 \
    && rm -rf /usr/local/cargo/registry /usr/local/cargo/git

# Commands run with the host user's uid (see the justfile), so files created
# in the bind-mounted repository belong to the developer. These directories
# are mounted as named volumes; Docker copies their permissions on first use.
ENV CARGO_HOME=/cargo \
    CARGO_TARGET_DIR=/cargo-target \
    HOME=/home/dev \
    PATH=/usr/local/cargo/bin:$PATH
RUN mkdir -p /cargo /cargo-target /home/dev \
    && chmod 1777 /cargo /cargo-target /home/dev

WORKDIR /workspace/backend
