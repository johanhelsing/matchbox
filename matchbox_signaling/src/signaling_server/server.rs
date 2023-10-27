use crate::{
    signaling_server::builder::SignalingServerBuilder,
    topologies::{
        client_server::{ClientServer, ClientServerCallbacks, ClientServerState},
        full_mesh::{FullMesh, FullMeshCallbacks, FullMeshState},
    },
};
use axum::{extract::connect_info::IntoMakeServiceWithConnectInfo, Router};
use std::net::SocketAddr;

/// Contains the interface end of a signaling server
#[derive(Debug)]
pub struct SignalingServer {
    /// The socket address bound for this server
    pub(crate) socket_addr: SocketAddr,

    /// Low-level info for how to build an axum server
    pub(crate) info: IntoMakeServiceWithConnectInfo<Router, SocketAddr>,
}

/// Common methods
impl SignalingServer {
    /// Creates a new builder for a [`SignalingServer`] with full-mesh topology.
    pub fn full_mesh_builder(
        socket_addr: impl Into<SocketAddr>,
    ) -> SignalingServerBuilder<FullMesh, FullMeshCallbacks, FullMeshState> {
        SignalingServerBuilder::new(socket_addr, FullMesh, FullMeshState::default())
    }

    /// Creates a new builder for a [`SignalingServer`] with client-server topology.
    pub fn client_server_builder(
        socket_addr: impl Into<SocketAddr>,
    ) -> SignalingServerBuilder<ClientServer, ClientServerCallbacks, ClientServerState> {
        SignalingServerBuilder::new(socket_addr, ClientServer, ClientServerState::default())
    }

    /// Returns the local address this server is bound to
    pub fn local_addr(&self) -> SocketAddr {
        self.socket_addr
    }

    /// Serve the signaling server
    pub async fn serve(self) -> Result<(), crate::Error> {
        // TODO: Shouldn't this return Result<!, crate::Error>?
        // todo: use try_serve?
        let server = axum::Server::bind(&self.socket_addr).serve(self.info);

        match server.await {
            Ok(()) => Ok(()),
            Err(e) => Err(crate::Error::from(e)),
        }
    }
}
