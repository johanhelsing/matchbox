use crate::{
    signaling_server::builder::SignalingServerBuilder,
    topologies::{ClientServer, ClientServerState, FullMesh, FullMeshState},
};
use axum::{extract::connect_info::IntoMakeServiceWithConnectInfo, Router, Server};
use hyper::server::conn::AddrIncoming;
use std::net::SocketAddr;

pub trait SignalingState: Clone + Send + Sync + 'static {}

/// Contains the interface end of a signaling server
#[derive(Debug)]
pub struct SignalingServer {
    /// The socket address bound for this server
    pub(crate) socket_addr: SocketAddr,

    /// The low-level axum server
    pub(crate) server: Server<AddrIncoming, IntoMakeServiceWithConnectInfo<Router, SocketAddr>>,
}

/// Common methods
impl SignalingServer {
    /// Creates a new builder for a [`SignalingServer`] with full-mesh topology.
    pub fn full_mesh_builder(
        socket_addr: impl Into<SocketAddr>,
    ) -> SignalingServerBuilder<FullMesh, FullMeshState> {
        SignalingServerBuilder::new(socket_addr, FullMesh, FullMeshState::default())
    }

    /// Creates a new builder for a [`SignalingServer`] with client-server topology.
    pub fn client_server_builder(
        socket_addr: impl Into<SocketAddr>,
    ) -> SignalingServerBuilder<ClientServer, ClientServerState> {
        SignalingServerBuilder::new(socket_addr, ClientServer, ClientServerState::default())
    }

    /// Returns the local address this server is bound to
    pub fn local_addr(&self) -> SocketAddr {
        self.socket_addr
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
