//! An example of running a client and an embedded signaling server in the same process.
//! This is useful for local testing or for "listen server" style games.
//!
//! Run with `cargo run --example hello_host --features signaling`.

use flecs_ecs::prelude::*;
use flecs_matchbox::prelude::*;
use log::{error, info};
use matchbox_signaling::SignalingServer;
use std::net::{Ipv4Addr, SocketAddrV4};

const CHANNEL_ID: usize = 0;

fn main() {
    env_logger::init();
    info!("Starting flecs_matchbox hello_host example");

    let mut world = World::new();

    // Start the server
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 3536);
    world.start_server(
        SignalingServer::client_server_builder(addr)
            .on_connection_request(|connection| {
                info!("Connecting: {connection:?}");
                Ok(true) // Allow all connections
            })
            .on_id_assignment(|(socket, id)| info!("{socket} received {id}"))
            .on_host_connected(|id| info!("Host joined: {id}"))
            .on_host_disconnected(|id| info!("Host left: {id}"))
            .on_client_connected(|id| info!("Client joined: {id}"))
            .on_client_disconnected(|id| info!("Client left: {id}"))
            .cors()
            .trace(),
    );

    // Start the client
    let _ = world.entity_named("Socket").set({
        info!("Opening socket...");
        let room_url = "ws://localhost:3536/hello_flecs";
        FlecsMatchboxSocket::new_reliable(room_url)
    });

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
    info!("(Host) Sending message to connected peers: {peers:?}");
    for peer in peers {
        let packet = "hello from your host!".as_bytes().to_vec();
        socket.channel_mut(CHANNEL_ID).send(packet.into(), peer);
    }
}

fn handle_connections_and_messages(socket: &mut FlecsMatchboxSocket) {
    // Check for new connections
    for (peer, state) in socket.update_peers() {
        info!("(Host) Peer {peer} changed state to {state:?}");
    }

    // Handle incoming messages
    for (peer, message) in socket.channel_mut(CHANNEL_ID).receive() {
        match std::str::from_utf8(&message) {
            Ok(text) => info!("(Host)Received message from {peer}: {text}"),
            Err(e) => error!("Failed to decode message from {peer}: {e}"),
        }
    }
}
