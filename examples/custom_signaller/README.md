# Custom signaller example

Shows how to use `matchbox_socket` with a custom signaller implementation; In
this case, the iroh p2p network.

## Instructions

- No need for matchbox server; signaling will be done through Iroh P2P Network
- extra dependencies: clang
  - Windows: <https://github.com/llvm/llvm-project/releases>, `LLVM-20.1.1-win64.exe`, check "add to path for all users"
  - Linux (Ubuntu/debian-based): `sudo apt install clang`
  - MacOS: `brew install llvm`
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
- Open the console to see the Iroh ID and room join URL
- In a second browser tab, open the room join URL. It should look like this: <http://127.0.0.1:1334#000DEADBEEF....FFF>
