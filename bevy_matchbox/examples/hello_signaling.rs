//! Runs a signaling server with server/client topology as a headless bevy
//! application.

use bevy::{app::ScheduleRunnerPlugin, log::LogPlugin, prelude::*, utils::Duration};
use bevy_matchbox::{matchbox_signaling::SignalingServer, prelude::*};
use std::net::{Ipv4Addr, SocketAddrV4};

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f32(
                1. / 120., // be nice to the CPU
            ))),
            LogPlugin::default(),
        ))
        .add_systems(Startup, (start_signaling_server).chain())
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
