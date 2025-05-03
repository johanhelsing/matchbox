use crate::{
    signaling_server::builder::SignalingServerBuilder,
    topologies::{
        client_server::{ClientServer, ClientServerCallbacks, ClientServerState},
        full_mesh::{FullMesh, FullMeshCallbacks, FullMeshState},
    },
};
use axum::{Router, extract::connect_info::IntoMakeServiceWithConnectInfo};
use std::net::{SocketAddr, TcpListener};
use tokio::net as tokio;

/// Contains the interface end of a signaling server
#[derive(Debug)]
pub struct SignalingServer {
    /// The socket configured for this server
    pub(crate) requested_addr: SocketAddr,

    /// Low-level info for how to build an axum server
    pub(crate) info: IntoMakeServiceWithConnectInfo<Router, SocketAddr>,

    pub(crate) listener: Option<TcpListener>,
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
        self.listener.as_ref().map(|l| l.local_addr().unwrap())
    }

    /// Binds the server to a socket
    ///
    /// Optional: Will happen automatically on [`serve`]
    pub fn bind(&mut self) -> Result<SocketAddr, crate::Error> {
        let listener = TcpListener::bind(self.requested_addr).map_err(crate::Error::Bind)?;
        listener.set_nonblocking(true).map_err(crate::Error::Bind)?;
        let addr = listener.local_addr().unwrap();
        self.listener = Some(listener);
        Ok(addr)
    }

    /// Serve the signaling server
    ///
    /// Will bind if not already bound
    pub async fn serve(mut self) -> Result<(), crate::Error> {
        match self.listener {
            Some(_) => (),
            None => _ = self.bind()?,
        };
        let listener =
            tokio::TcpListener::from_std(self.listener.expect("No listener, this is a bug!"))
                .map_err(crate::Error::Bind)?;
        axum::serve(listener, self.info)
            .await
            .map_err(crate::Error::Serve)
    }
}
