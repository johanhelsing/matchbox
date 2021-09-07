FROM rust:1.54 as builder
WORKDIR /usr/src/matchbox_server
COPY Cargo.toml .
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && cargo build --release

COPY . .
RUN touch src/main.rs
RUN cargo build --release

FROM debian:buster-slim
RUN apt-get update && apt-get install -y libssl1.1 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/matchbox_server/target/release/matchbox_server /usr/local/bin/matchbox_server
#COPY --from=builder /usr/src/matchbox_server/target/debug/matchbox_server /usr/local/bin/matchbox_server
CMD ["matchbox_server"]
