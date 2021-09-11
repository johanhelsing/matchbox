# Matchbox Demo

Shows how to use `matchbox_socket` with `bevy` and `ggrs`, to create a simple
working browser "game" (if moving cubes around on a plane can be called a game).

There is a live version here (move the cube with WASD):

- 2-Player: https://s3.johanhelsing.studio/dump/box_game/index.html
- 3-Player: https://s3.johanhelsing.studio/dump/box_game/index.html?players=3
- N-player: Edit the link above.

## Prerequisites

```
cargo install cargo-make
```

## Build and serve

```
cargo make serve
```

then point your browser to http://127.0.0.1:4000/

Note: you also need to run a Matchbox signalling server at http://127.0.0.1:3536

<!-- ![Screenshot](https://mrk.sed.pl/bevy-showcase/assets/bevy_webgl2_app_template.png?v=3) -->
