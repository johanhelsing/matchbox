//! Runs both signaling with server/client topology and runs the host in the same process
//!
//! Sends messages periodically to all connected clients.
//!
//! Note: When building a signaling server make sure you depend on
//! `bevy_matchbox` with the `signaling` feature enabled.
//!
//! ```toml
//! bevy_matchbox = { version = "0.x", features = ["signaling"] }
//! ```

use bevy::{
    app::ScheduleRunnerPlugin, log::LogPlugin, prelude::*, time::common_conditions::on_timer,
};
use bevy_matchbox::{matchbox_signaling::SignalingServer, prelude::*};
use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddrV4};

fn main() {
    App::new()
        // .add_plugins(DefaultPlugins)
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f32(
                1. / 120., // be nice to the CPU
            ))),
            LogPlugin::default(),
        ))
        .add_systems(Startup, (start_signaling_server, start_host_socket).chain())
        .add_systems(Update, receive_messages)
        .add_systems(
            Update,
            send_message.run_if(on_timer(Duration::from_secs(5))),
        )
        .run();
}

fn start_signaling_server(mut commands: Commands) {
    info!("Starting signaling server");
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 3536);
    let signaling_server = MatchboxServer::from(
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
            .trace()
            .build(),
    );
    commands.insert_resource(signaling_server);
}

fn start_host_socket(mut commands: Commands) {
    let socket = MatchboxSocket::new_reliable("ws://localhost:3536/hello");
    commands.insert_resource(socket);
}

fn send_message(mut socket: ResMut<MatchboxSocket>) {
    let peers: Vec<_> = socket.connected_peers().collect();

    for peer in peers {
        let message = "Hello, I'm the host";
        info!("Sending message: {message:?} to {peer}");
        socket.channel_mut(0).send(message.as_bytes().into(), peer);
    }
}

fn receive_messages(mut socket: ResMut<MatchboxSocket>) {
    for (peer, state) in socket.update_peers() {
        info!("{peer}: {state:?}");
    }

    for (_id, message) in socket.channel_mut(0).receive() {
        match std::str::from_utf8(&message) {
            Ok(message) => info!("Received message: {message:?}"),
            Err(e) => error!("Failed to convert message to string: {e}"),
        }
    }
}
