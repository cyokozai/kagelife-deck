# kagelife-deck のビルド・テスト用の image。
# ローカルに Rust を入れずに、この image の中で cargo を回す。
FROM rust:1-slim-bookworm

# elgato-streamdeck（hidapi）が libudev を使う。/usr/bin/time は資源の計測に使う
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libudev-dev time \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt

WORKDIR /src
