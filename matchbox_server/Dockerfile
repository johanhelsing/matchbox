# Signaling server as a docker image
#
# to build, run `docker build -f matchbox_server/Dockerfile` from root of the
# repository

FROM rust:1.73 as builder

WORKDIR /usr/src/matchbox_server/

# cache dependency compilation in a low layer to speed up subsequent builds and
# save disk space on builders
COPY README.md /usr/src/README.md
COPY matchbox_server/Cargo.toml /usr/src/matchbox_server/Cargo.toml
COPY matchbox_protocol /usr/src/matchbox_protocol
COPY matchbox_server /usr/src/matchbox_server
COPY matchbox_signaling /usr/src/matchbox_signaling

RUN cargo build --release

FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y libssl1.1
RUN rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/matchbox_server/target/release/matchbox_server /usr/local/bin/matchbox_server
CMD ["matchbox_server"]
