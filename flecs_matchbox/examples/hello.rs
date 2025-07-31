//! A simple example of using flecs_matchbox.
//!
//! Run with `cargo run --example hello`.
//! You will need to have a `matchbox_server` running.

use flecs_ecs::prelude::*;
use flecs_matchbox::prelude::*;
use log::{error, info};

const CHANNEL_ID: usize = 0;

fn main() {
    env_logger::init();

    info!("Starting flecs_matchbox hello example");

    let world = World::new();

    let _ = world.entity_named("Socket").set({
        info!("Opening socket...");
        let room_url = "ws://localhost:3536/hello_flecs";
        FlecsMatchboxSocket::new_reliable(room_url)
    });

    // Add systems to run during the main loop
    world
        .system::<&mut FlecsMatchboxSocket>()
        .each(handle_connections_and_messages);

    world
        .system::<&mut FlecsMatchboxSocket>()
        .set_interval(5.0)
        .each(send_message);

    // Run the world
    loop {
        world.progress();
    }
}

fn send_message(socket: &mut FlecsMatchboxSocket) {
    let peers: Vec<_> = socket.connected_peers().collect();
    info!("Sending message to connected peers: {peers:?}");
    for peer in peers {
        let packet = "hello from flecs!".as_bytes().to_vec();
        socket.channel_mut(CHANNEL_ID).send(packet.into(), peer);
    }
}

fn handle_connections_and_messages(socket: &mut FlecsMatchboxSocket) {
    // Check for new connections
    for (peer, state) in socket.update_peers() {
        match state {
            PeerState::Connected => info!("Peer Connected: {peer}"),
            PeerState::Disconnected => info!("Peer Disconnected: {peer}"),
        }
    }

    // Handle incoming messages
    for (peer, message) in socket.channel_mut(CHANNEL_ID).receive() {
        match std::str::from_utf8(&message) {
            Ok(text) => info!("Received message from {peer}: {text}"),
            Err(e) => error!("Failed to decode message from {peer}: {e}"),
        }
    }
}
