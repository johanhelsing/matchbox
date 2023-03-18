use axum::extract::ws::Message;
use matchbox_protocol::PeerId;
use std::collections::HashMap;

use super::error::SignalingError;

/// A wrapper for storage in the signaling server state
#[derive(Debug, Clone)]
pub(crate) struct Peer {
    pub uuid: PeerId,
    pub sender: tokio::sync::mpsc::UnboundedSender<std::result::Result<Message, axum::Error>>,
}

/// Contains the signaling server state
#[derive(Default, Debug, Clone)]
pub struct SignalingState {
    pub(crate) peers: HashMap<PeerId, Peer>,
    pub(crate) host: Option<PeerId>,
}

impl SignalingState {
    /// Add a peer
    pub(crate) fn add_peer(&mut self, peer: Peer) {
        let peer_id = peer.uuid;
        self.peers.insert(peer.uuid, peer);
    }

    /// Remove a peer from the state if it existed, returning the peer removed.
    #[must_use]
    pub(crate) fn remove_peer(&mut self, peer_id: &PeerId) -> Option<Peer> {
        self.peers.remove(peer_id)
    }

    /// Send a message to a peer without blocking.
    pub(crate) fn try_send(&self, id: PeerId, message: Message) -> Result<(), SignalingError> {
        let peer = self.peers.get(&id);
        let peer = match peer {
            Some(peer) => peer,
            None => {
                return Err(SignalingError::UnknownPeer);
            }
        };

        peer.sender.send(Ok(message)).map_err(SignalingError::from)
    }
}
