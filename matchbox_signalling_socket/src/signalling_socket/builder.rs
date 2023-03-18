use crate::SignallingServer;
use std::{marker::PhantomData, net::SocketAddr};

use super::topologies::{ClientServer, FullMesh};

/// Builder for [`SignallingServer`]s.
///
/// Begin with [`SignallingServerBuilder::new`] and add parameters before calling
/// [`SignallingServerBuilder::build`] to produce the desired [`SignallingServer`].
#[derive(Debug, Clone)]
pub struct SignallingServerBuilder<Topology> {
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,

    _pd: PhantomData<Topology>,
}

impl<Topology> SignallingServerBuilder<Topology> {
    /// Creates a new builder for a [`SignallingServer`] with full-mesh topology.
    pub fn new_full_mesh(socket_addr: impl Into<SocketAddr>) -> Self {
        Self {
            socket_addr: socket_addr.into(),
            _pd: PhantomData,
        }
    }

    /// Creates a new builder for a [`SignallingServer`] with client-server topology.
    pub fn new_client_server(socket_addr: impl Into<SocketAddr>) -> Self {
        Self {
            socket_addr: socket_addr.into(),
            _pd: PhantomData,
        }
    }
}

impl SignallingServerBuilder<FullMesh> {
    /// Create a [`SignallingServer`] with full-mesh topology.
    pub fn build_full_mesh(&self) -> SignallingServer<FullMesh> {
        todo!()
    }
}

impl SignallingServerBuilder<ClientServer> {
    /// Create a [`SignallingServer`] with client-server topology.
    pub fn build_client_server(&self) -> SignallingServer<FullMesh> {
        todo!()
    }
}
