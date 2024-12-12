use async_compat::CompatExt;
use bevy::{
    ecs::world::Command,
    prelude::{Commands, Resource},
    tasks::{IoTaskPool, Task},
};
pub use matchbox_signaling;
use matchbox_signaling::{
    topologies::{
        client_server::{ClientServer, ClientServerCallbacks, ClientServerState},
        full_mesh::{FullMesh, FullMeshCallbacks, FullMeshState},
        SignalingTopology,
    },
    Error, SignalingCallbacks, SignalingServer, SignalingServerBuilder, SignalingState,
};
use std::net::SocketAddr;

/// A [`SignalingServer`] as a [`Resource`].
///
/// As a [`Resource`], with [`Commands`]
/// ```
/// use std::net::Ipv4Addr;
/// use bevy_matchbox::{
///     prelude::*,
///     matchbox_signaling::topologies::full_mesh::{FullMesh, FullMeshState}
/// };
/// use bevy::prelude::*;
///
/// fn start_server_system(mut commands: Commands) {
///     let builder = SignalingServerBuilder::new(
///         (Ipv4Addr::UNSPECIFIED, 3536),
///         FullMesh,
///         FullMeshState::default(),
///     );
///     commands.start_server(builder);
/// }
///
/// fn stop_server_system(mut commands: Commands) {
///     commands.stop_server();
/// }
/// ```
///
/// As a [`Resource`], directly
/// ```
/// use std::net::Ipv4Addr;
/// use bevy_matchbox::{
///     prelude::*,
///     matchbox_signaling::topologies::full_mesh::{FullMesh, FullMeshState}
/// };
/// use bevy::prelude::*;
///
/// fn start_server_system(mut commands: Commands) {
///     let server: MatchboxServer = SignalingServerBuilder::new(
///         (Ipv4Addr::UNSPECIFIED, 3536),
///         FullMesh,
///         FullMeshState::default(),
///     ).into();
///
///     commands.insert_resource(MatchboxServer::from(server));
/// }
///
/// fn stop_server_system(mut commands: Commands) {
///     commands.remove_resource::<MatchboxServer>();
/// }
/// ```
#[derive(Debug, Resource)]
#[allow(dead_code)] // we take ownership of the task to not drop it
pub struct MatchboxServer(Task<Result<(), Error>>);

impl<Topology, Cb, S> From<SignalingServerBuilder<Topology, Cb, S>> for MatchboxServer
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    fn from(value: SignalingServerBuilder<Topology, Cb, S>) -> Self {
        MatchboxServer::from(value.build())
    }
}

impl From<SignalingServer> for MatchboxServer {
    fn from(server: SignalingServer) -> Self {
        let task_pool = IoTaskPool::get();
        let task = task_pool.spawn(server.serve().compat());
        MatchboxServer(task)
    }
}

struct StartServer<Topology, Cb, S>(SignalingServerBuilder<Topology, Cb, S>)
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState;

impl<Topology, Cb, S> Command for StartServer<Topology, Cb, S>
where
    Topology: SignalingTopology<Cb, S> + Send + 'static,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    fn apply(self, world: &mut bevy::prelude::World) {
        world.insert_resource(MatchboxServer::from(self.0))
    }
}

/// A [`Commands`] extension used to start a [`MatchboxServer`].
pub trait StartServerExt<
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
>
{
    /// Starts a [`MatchboxServer`] and allocates it as a resource.
    fn start_server(&mut self, builder: SignalingServerBuilder<Topology, Cb, S>);
}

impl<Topology, Cb, S> StartServerExt<Topology, Cb, S> for Commands<'_, '_>
where
    Topology: SignalingTopology<Cb, S> + Send + 'static,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    fn start_server(&mut self, builder: SignalingServerBuilder<Topology, Cb, S>) {
        self.add(StartServer(builder))
    }
}

struct StopServer;

impl Command for StopServer {
    fn apply(self, world: &mut bevy::prelude::World) {
        world.remove_resource::<MatchboxServer>();
    }
}

/// A [`Commands`] extension used to stop a [`MatchboxServer`].
pub trait StopServerExt {
    /// Delete the [`MatchboxServer`] resource.
    fn stop_server(&mut self);
}

impl StopServerExt for Commands<'_, '_> {
    fn stop_server(&mut self) {
        self.add(StopServer)
    }
}

impl MatchboxServer {
    /// Creates a new builder for a [`SignalingServer`] with full-mesh topology.
    pub fn full_mesh_builder(
        socket_addr: impl Into<SocketAddr>,
    ) -> SignalingServerBuilder<FullMesh, FullMeshCallbacks, FullMeshState> {
        SignalingServer::full_mesh_builder(socket_addr)
    }

    /// Creates a new builder for a [`SignalingServer`] with client-server topology.
    pub fn client_server_builder(
        socket_addr: impl Into<SocketAddr>,
    ) -> SignalingServerBuilder<ClientServer, ClientServerCallbacks, ClientServerState> {
        SignalingServer::client_server_builder(socket_addr)
    }
}

#[cfg(test)]
mod tests {
    use crate::matchbox_signaling::topologies::client_server::{ClientServer, ClientServerState};
    use crate::prelude::*;
    use bevy::prelude::*;
    use std::net::Ipv4Addr;

    fn start_signaling(mut commands: Commands) {
        let server: MatchboxServer = SignalingServerBuilder::new(
            (Ipv4Addr::UNSPECIFIED, 3536),
            ClientServer,
            ClientServerState::default(),
        )
        .into();

        commands.insert_resource(server);
    }

    #[test]
    // https://github.com/johanhelsing/matchbox/issues/350
    fn start_signaling_without_panics() {
        let mut app = App::new();

        app.add_plugins(MinimalPlugins)
            .add_systems(Startup, start_signaling);

        app.update();
    }
}
