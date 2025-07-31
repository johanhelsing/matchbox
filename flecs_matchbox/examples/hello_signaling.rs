//! An example of running just a signaling server as a headless flecs app.
//!
//! Run with `cargo run --example hello_signaling --features signaling`.

use flecs_ecs::prelude::*;
use flecs_matchbox::prelude::*;
use log::info;
use matchbox_signaling::SignalingServer;
use std::net::{Ipv4Addr, SocketAddrV4};

fn main() {
    env_logger::init();
    info!("Starting flecs_matchbox signaling server example");

    let mut world = World::new();

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

    loop {
        world.progress();
    }
}
