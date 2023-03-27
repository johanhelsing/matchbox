# Signaling Server

A lightweight signaling server with some opinions, to work with all [examples](../examples/).

## Next rooms

This signaling server supports a rudimentary form of matchmaking. By appending `?next=3` to the room id, the next three players to join will be connected, and then the next three players will be connected separately to the first three.

You can also use the room id for scoping what kind of players you want to match. i.e.: `wss://match.example.com/awesome_game_v1.1.0_pvp?next=2`

## Run

```sh
cargo run
```
