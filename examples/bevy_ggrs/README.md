# Bevy + GGRS

Shows how to use `matchbox_socket` with `bevy` and `ggrs`, to create a simple working browser "game" (if moving cubes around on a plane can be called a game).

## Live Demo
There is a live version here (move the cube with WASD):

- 2-Player: https://helsing.studio/box_game/
- 3-Player: https://helsing.studio/box_game/?players=3
- N-player: Edit the link above.

When enough players have joined, you should see a couple of boxes, one of which
you can move around using the `WASD` keys.

You can open the browser console to get some rough idea about what's happening
(or not happening if that's the unfortunate case).

# Instructions
- Run the matchbox-provided [`signalling_server`](../../signalling_server/) ([help](../../signalling_server/README.md)), or run your own on `ws://localhost:3536/`.
- Run the demo (enough clients must connect before the game stats)
  - [on Native](#run-on-native)
  - [on WASM](#run-on-wasm)

## Run on Native
```
cargo run [--matchbox ws://127.0.0.1:3536] [--players 2] [--room <name>]
```

## Run on WASM
### Prerequisites
Install the `wasm32-unknown-unknown` target
```
rustup target install wasm32-unknown-unknown
```

Install a lightweight web server
```
cargo install wasm-server-runner
```
### Serve
```
cargo run --target wasm32-unknown-unknown
```
### Run
- Use a web browser and navigate to <http://127.0.0.1:1334/?players=2>
- Open the console to see execution logs