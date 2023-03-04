# Simple Example

Shows how to use `matchbox_socket` in a simple example.

## Instructions

- Run the matchbox-provided [`matchbox_server`](../../matchbox_server/) ([help](../../matchbox_server/README.md)), or run your own on `ws://localhost:3536/`.
- Run the demo
  - [on Native](#run-on-native)
  - [on WASM](#run-on-wasm)

## Run on Native

```sh
cargo run
```

## Run on WASM

### Prerequisites

Install the `wasm32-unknown-unknown` target

```sh
rustup target install wasm32-unknown-unknown
```

Install a lightweight web server

```sh
cargo install wasm-server-runner
```

### Serve

```sh
cargo run --target wasm32-unknown-unknown
```

### Run

- Use a web browser and navigate to <http://127.0.0.1:1334>
- Open the console to see execution logs
