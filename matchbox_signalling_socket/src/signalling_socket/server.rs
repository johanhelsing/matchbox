use super::{
    builder::SignallingServerBuilder,
    topologies::{ClientServer, FullMesh},
};
use matchbox_protocol::PeerId;
use std::{collections::HashSet, marker::PhantomData, net::SocketAddr};

/// Contains the interface end of a signalling server
#[derive(Debug, Default)]
pub struct SignallingServer<Topology = FullMesh> {
    peers: HashSet<PeerId>,
    host: Option<PeerId>,
    _pd: PhantomData<Topology>,
}

/// Common methods
impl<Topology> SignallingServer<Topology> {
    pub fn builder(socket_addr: impl Into<SocketAddr>) -> SignallingServerBuilder<Topology> {
        SignallingServerBuilder::new(socket_addr)
    }

    pub fn peers(&self) -> &HashSet<PeerId> {
        &self.peers
    }
}

/// Client-Server only methods
impl SignallingServer<ClientServer> {
    pub fn is_host_connected(&self) -> bool {
        self.host.is_some()
    }

    pub fn host(&self) -> Option<&PeerId> {
        self.host.as_ref()
    }
}
