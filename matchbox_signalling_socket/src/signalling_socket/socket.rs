use super::topologies::{ClientServer, FullMesh};
use matchbox_protocol::PeerId;
use std::{collections::HashSet, marker::PhantomData};

/// Contains the interface end of a signalling server
#[derive(Debug, Default)]
pub struct SignallingServer<Topology = FullMesh> {
    peers: HashSet<PeerId>,
    host: Option<PeerId>,
    _pd: PhantomData<Topology>,
}

/// Common methods
impl<Topology> SignallingServer<Topology> {
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
