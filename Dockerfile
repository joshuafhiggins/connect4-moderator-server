FROM rust:1.91.1 AS build

RUN rustup target add x86_64-unknown-linux-musl && \
    apt update && \
    apt install -y musl-tools musl-dev && \
    update-ca-certificates

COPY ./src ./src
COPY ./Cargo.lock .
COPY ./Cargo.toml .

RUN cargo build --target x86_64-unknown-linux-musl --release

ENTRYPOINT ["./target/x86_64-unknown-linux-musl/release/connect4-moderator-server"]
