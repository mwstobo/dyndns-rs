FROM rust:1.85 as builder
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y libssl3 ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=builder target/release/dyndns-rs /usr/local/bin/dyndns-rs
CMD ["dyndns-rs"]
