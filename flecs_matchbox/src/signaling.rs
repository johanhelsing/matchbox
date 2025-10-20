//! A wrapper for `matchbox_signaling::SignalingServer` for use in Flecs.
//! This module is only available on native targets with the `signaling` feature enabled.

use async_compat::CompatExt;
use flecs_ecs::prelude::*;
use log::{error, info};
use matchbox_signaling::{
    Error, SignalingCallbacks, SignalingServer, SignalingServerBuilder, SignalingState,
};
use std::thread::JoinHandle;

/// A [`SignalingServer`] wrapped for use as a Flecs singleton component.
/// When this component is added, it will spawn a background thread to run the server.
#[derive(Component)]
#[allow(dead_code)] // task is kept alive to not drop it
pub struct FlecsMatchboxServer(JoinHandle<Result<(), Error>>);

impl<Topology, Cb, S> From<SignalingServerBuilder<Topology, Cb, S>> for FlecsMatchboxServer
where
    Topology: matchbox_signaling::topologies::SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    fn from(builder: SignalingServerBuilder<Topology, Cb, S>) -> Self {
        Self::from(builder.build())
    }
}

impl From<SignalingServer> for FlecsMatchboxServer {
    fn from(server: SignalingServer) -> Self {
        let task = std::thread::spawn(move || {
            info!("Signaling server started");
            let result = futures_lite::future::block_on(server.serve().compat());
            if let Err(e) = &result {
                error!("Signaling server ended with error: {e:?}");
            }
            info!("Signaling server finished");
            result
        });
        FlecsMatchboxServer(task)
    }
}

/// An extension trait for `flecs_ecs::prelude::World` to simplify starting a signaling server.
pub trait StartServerExt<
    Topology: matchbox_signaling::topologies::SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
>
{
    /// Starts a [`FlecsMatchboxServer`] and adds it as a singleton.
    fn start_server(&mut self, builder: SignalingServerBuilder<Topology, Cb, S>);
}

impl<Topology, Cb, S> StartServerExt<Topology, Cb, S> for World
where
    Topology: matchbox_signaling::topologies::SignalingTopology<Cb, S> + Send + 'static,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    fn start_server(&mut self, builder: SignalingServerBuilder<Topology, Cb, S>) {
        let server = FlecsMatchboxServer::from(builder);
        self.set(server);
    }
}

/// An extension trait for `flecs_ecs::prelude::World` to simplify stopping a signaling server.
pub trait StopServerExt {
    /// Removes the [`FlecsMatchboxServer`] singleton, stopping the server.
    fn stop_server(&mut self);
}

impl StopServerExt for World {
    fn stop_server(&mut self) {
        self.remove::<FlecsMatchboxServer>();
    }
}
