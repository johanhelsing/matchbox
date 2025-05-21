# [![Matchbox](https://raw.githubusercontent.com/johanhelsing/matchbox/main/images/matchbox_logo.png)](https://github.com/johanhelsing/matchbox)

[![crates.io](https://img.shields.io/crates/v/matchbox_socket.svg)](https://crates.io/crates/matchbox_socket)
![MIT/Apache 2.0](https://img.shields.io/badge/license-MIT%2FApache-blue.svg)
[![crates.io](https://img.shields.io/crates/d/matchbox_socket.svg)](https://crates.io/crates/matchbox_socket)
[![docs.rs](https://img.shields.io/docsrs/matchbox_socket)](https://docs.rs/matchbox_socket)

Painless peer-to-peer WebRTC networking for rust's native and wasm applications.

The goal of the Matchbox project is to enable udp-like, unordered, unreliable p2p connections in web browsers or native to facilitate low-latency multiplayer games.

Matchbox supports both unreliable and reliable data channels, with configurable ordering guarantees and variable packet retransmits.

- [Tutorial for usage with Bevy and GGRS](https://johanhelsing.studio/posts/extreme-bevy)

The Matchbox project contains:

- [matchbox_socket](https://github.com/johanhelsing/matchbox/tree/main/matchbox_socket): A socket abstraction for Wasm or Native, with:
  - `ggrs`: A feature providing a [ggrs](https://github.com/gschup/ggrs) compatible socket.
- [matchbox_signaling](https://github.com/johanhelsing/matchbox/tree/main/matchbox_signaling): A signaling server library, with ready to use examples
- [matchbox_server](https://github.com/johanhelsing/matchbox/tree/main/matchbox_server): A ready to use full-mesh signalling server
- [bevy_matchbox](https://github.com/johanhelsing/matchbox/tree/main/bevy_matchbox): A `matchbox_socket` integration for the [Bevy](https://bevyengine.org/) game engine

  | bevy  | bevy_matchbox |
  | ----- | ------------- |
  | 0.16  | 0.12, main    |
  | 0.15  | 0.11          |
  | 0.14  | 0.10          |
  | 0.13  | 0.9           |
  | 0.12  | 0.8           |
  | 0.11  | 0.7           |
  | 0.10  | 0.6           |
  | < 0.9 | Unsupported   |

## Examples

- [simple](examples/simple): A simple communication loop using matchbox_socket
- [bevy_ggrs](examples/bevy_ggrs): An example browser game, using `bevy` and `bevy_ggrs`
  - Live 2-player demo: <https://helsing.studio/box_game/>
  - Live 4-player demo: <https://helsing.studio/box_game/?players=4>

## How it works

![Connection](https://raw.githubusercontent.com/johanhelsing/matchbox/main/images/connection.excalidraw.svg)

WebRTC allows direct connections between peers, but in order to establish those connections, some kind of signaling service is needed. `matchbox_server` is such a service. Once the connections are established, however, data will flow directly between peers, and no traffic will go through the signaling server.

The signaling service needs to run somewhere all clients can reach it over http or https connections. In production, this usually means the public internet.

When a client wants to join a p2p (mesh) network, it connects to the signaling service. The signaling server then notifies the peers that have already connected about the new peer (sends a `NewPeer` event).

Peers then negotiate a connection through the signaling server. The initiator sends an "offer" and the recipient responds with an "answer." Once peers have enough information relayed, a [RTCPeerConnection](https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection) is established for each peer, which comes with one or more data channels.

All of this, however, is hidden from rust application code. All you will need to do on the client side, is:

- Create a new socket, and give it a signaling server url
- `.await` the message loop future that processes new messages.
  - If you are using [Bevy](https://bevyengine.org), this is done automatically by `bevy_matchbox` (see the [`bevy_ggrs`](examples/bevy_ggrs/) example).
  - Otherwise, if you are using WASM, `wasm-bindgen-futures` can help (see the [`simple`](examples/simple/) example).
  - Alternatively, the future can be polled manually, i.e. once per frame.

You can hook into the lifecycle of your socket through the socket's API, such as connection state changes. Similarly, you can send packets to peers using the socket through a simple, non-blocking method.

## Showcase

Projects using Matchbox:

- [NES Bundler](https://github.com/tedsteen/nes-bundler) - Transform your NES game into a single executable targeting your favorite OS!
- [Cargo Space](https://helsing.studio/cargospace) (in development) - A coop 2D space game about building and flying a ship together
- [Extreme Bevy](https://helsing.studio/extreme) - Simple 2-player arcade shooter
- [Matchbox demo](https://helsing.studio/box_game/)
- [A Janitors Nightmare](https://gorktheork.itch.io/bevy-jam-1-submission) - 2-player jam game
- [Lavagna](https://github.com/alepez/lavagna) - collaborative blackboard for online meetings

## Contributing

PRs welcome!

If you have questions or suggestions, feel free to make an [issue](https://github.com/johanhelsing/matchbox/issues). There's also a [Discord channel](https://discord.gg/ye9UDNvqQD) if you want to get in touch.

## Thanks

- A huge thanks to Ernest Wong for his [Dango Tribute experiment](https://github.com/ErnWong/dango-tribute)! `matchbox_socket` is heavily inspired its wasm-bindgen server_socket and Matchbox would probably not exist without it.

## License

All code in this repository dual-licensed under either:

- [MIT License](LICENSE-MIT) or <http://opensource.org/licenses/MIT>
- [Apache License, Version 2.0](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
