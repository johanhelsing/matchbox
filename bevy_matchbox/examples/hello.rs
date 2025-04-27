//! Sends messages periodically to all connected peers (or host if connected in
//! a client server topology).

use bevy::{prelude::*, time::common_conditions::on_timer};
use bevy_matchbox::prelude::*;
use core::time::Duration;

const CHANNEL_ID: usize = 0;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, start_socket)
        .add_systems(Update, receive_messages)
        .add_systems(
            Update,
            send_message.run_if(on_timer(Duration::from_secs(5))),
        )
        .run();
}

fn start_socket(mut commands: Commands) {
    let socket = MatchboxSocket::new_reliable("ws://localhost:3536/hello");
    commands.insert_resource(socket);
}

fn send_message(mut socket: ResMut<MatchboxSocket>) {
    let peers: Vec<_> = socket.connected_peers().collect();

    for peer in peers {
        let message = "Hello";
        info!("Sending message: {message:?} to {peer}");
        socket
            .channel_mut(CHANNEL_ID)
            .send(message.as_bytes().into(), peer);
    }
}

fn receive_messages(mut socket: ResMut<MatchboxSocket>) {
    for (peer, state) in socket.update_peers() {
        info!("{peer}: {state:?}");
    }

    for (_id, message) in socket.channel_mut(CHANNEL_ID).receive() {
        match std::str::from_utf8(&message) {
            Ok(message) => info!("Received message: {message:?}"),
            Err(e) => error!("Failed to convert message to string: {e}"),
        }
    }
}
