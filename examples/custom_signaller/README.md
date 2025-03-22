# Simple Example

Shows how to use `matchbox_socket` in a simple example.

## Instructions

- No need for matchbox server; signaling will be done through Iroh P2P Network
- Run the demo
  - [on Native](#run-on-native)
  - [on WASM](#run-on-wasm)

## Run on Native


First node:
```sh
cargo run
```

Then read from terminal the Iroh ID to pass to subsequent nodes:

Second node:

```sh
cargo run -- "000DEADBEEF....FFF"
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
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' cargo run --target wasm32-unknown-unknown
```

### Run

- Use a web browser and navigate to <http://127.0.0.1:1334>
- Open the console to see the Iroh ID, this can be used from native node
