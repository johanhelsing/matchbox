use crate::{
    signaling_server::builder::SignalingServerBuilder,
    topologies::{
        client_server::{ClientServer, ClientServerCallbacks, ClientServerState},
        full_mesh::{FullMesh, FullMeshCallbacks, FullMeshState},
    },
};
use axum_server::Handle;
use futures::Future;
use std::{io, net::SocketAddr, pin::Pin};

/// Contains the interface end of a signaling server
pub struct SignalingServer {
    /// The handle used for server introspection
    pub(crate) handle: Handle,

    /// The low-level axum server
    pub(crate) server: Pin<Box<dyn Future<Output = io::Result<()>> + Send>>,
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

    /// Returns a clone to a server handle for introspection.
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Serve the signaling server
    pub async fn serve(self) -> Result<(), crate::Error> {
        // TODO: Shouldn't this return Result<!, crate::Error>?
        match self.server.await {
            Ok(()) => Ok(()),
            Err(e) => Err(crate::Error::from(e)),
        }
    }
}
