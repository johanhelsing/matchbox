FROM rust:1.66 as builder
WORKDIR /usr/src/signalling_server
COPY Cargo.toml .
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && cargo build --release

COPY . .
RUN touch src/main.rs
RUN cargo build --release

FROM debian:buster-slim
RUN apt-get update && apt-get install -y libssl1.1 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/signalling_server/target/release/signalling_server /usr/local/bin/signalling_server
#COPY --from=builder /usr/src/signalling_server/target/debug/signalling_server /usr/local/bin/signalling_server
CMD ["signalling_server"]
