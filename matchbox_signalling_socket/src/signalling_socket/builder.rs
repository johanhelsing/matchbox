use crate::{
    signalling_socket::topologies::{ClientServer, FullMesh},
    SignallingServer,
};
use std::{marker::PhantomData, net::SocketAddr};

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
    /// Creates a new builder for a [`SignallingServer`].
    pub fn new(socket_addr: impl Into<SocketAddr>) -> Self {
        Self {
            socket_addr: socket_addr.into(),
            _pd: PhantomData,
        }
    }

    /// Changes the topology of the [`SignallingServer`] to full-mesh.
    pub fn full_mesh_topology(self) -> SignallingServerBuilder<FullMesh> {
        // TODO: When #![feature(type_changing_struct_update)] is stable, just do
        // TODO: - SignallingServerBuilder { ..self }
        SignallingServerBuilder {
            socket_addr: self.socket_addr,
            _pd: PhantomData,
        }
    }

    /// Changes the topology of the [`SignallingServer`] to client-server.
    pub fn client_server_topology(self) -> SignallingServerBuilder<ClientServer> {
        // TODO: When #![feature(type_changing_struct_update)] is stable, just do
        // TODO: - SignallingServerBuilder { ..self }
        SignallingServerBuilder {
            socket_addr: self.socket_addr,
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
