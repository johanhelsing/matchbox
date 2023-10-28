use crate::{
    signaling_server::builder::SignalingServerBuilder,
    topologies::{
        client_server::{ClientServer, ClientServerCallbacks, ClientServerState},
        full_mesh::{FullMesh, FullMeshCallbacks, FullMeshState},
    },
};
use axum::{extract::connect_info::IntoMakeServiceWithConnectInfo, Router};
use hyper::{server::conn::AddrIncoming, Server};
use std::net::SocketAddr;

/// Contains the interface end of a signaling server
#[derive(Debug)]
pub struct SignalingServer {
    /// The socket configured for this server
    pub(crate) requested_addr: SocketAddr,

    /// Low-level info for how to build an axum server
    pub(crate) info: IntoMakeServiceWithConnectInfo<Router, SocketAddr>,

    /// Low-level info for how to build an axum server
    pub(crate) server:
        Option<Server<AddrIncoming, IntoMakeServiceWithConnectInfo<Router, SocketAddr>>>,
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
    ///
    /// The server needs to [`bind`] first
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.server.as_ref().map(|s| s.local_addr())
    }

    /// Binds the server to a socket
    ///
    /// Optional: Will happen automatically on [`serve`]
    pub fn bind(&mut self) -> Result<SocketAddr, crate::Error> {
        let server = axum::Server::try_bind(&self.requested_addr)
            .map_err(crate::Error::Bind)?
            .serve(self.info.clone());

        let addr = server.local_addr();
        self.server = Some(server);
        Ok(addr)
    }

    /// Serve the signaling server
    ///
    /// Will bind if not already bound
    pub async fn serve(mut self) -> Result<(), crate::Error> {
        if self.server.is_none() {
            self.bind()?;
            assert!(self.server.is_some());
        }

        match self.server.expect("no server, this is a bug").await {
            Ok(()) => Ok(()),
            Err(e) => Err(crate::Error::from(e)),
        }
    }
}
