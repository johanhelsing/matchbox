# Matchbox Demo

Shows how to use `matchbox_socket` with `bevy` and `ggrs`, to create a simple
working browser "game" (if moving cubes around on a plane can be called a game).

There is a live version here (move the cube with WASD):

- 2-Player: https://helsing.studio/box_game/
- 3-Player: https://helsing.studio/box_game/?players=3
- N-player: Edit the link above.

## Prerequisites

```
rustup target install wasm32-unknown-unknown
```

```
cargo install wasm-server-runner
```

## Build and serve

```
cargo run --target wasm32-unknown-unknown
```

then point your browser to http://127.0.0.1:1334/

Note: you also need to run a Matchbox signalling server at http://127.0.0.1:3536