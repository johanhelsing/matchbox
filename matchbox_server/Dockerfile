# Signalling server as a docker image
FROM rust:1.66 as builder

COPY ../matchbox_protocol /usr/src/matchbox_protocol/
COPY . /usr/src/matchbox_server/

WORKDIR /usr/src/matchbox_server/
RUN cargo build --release

FROM debian:buster-slim
RUN apt-get update && apt-get install -y libssl1.1
RUN rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/matchbox_server/target/release/matchbox_server /usr/local/bin/matchbox_server
CMD ["matchbox_server"]
