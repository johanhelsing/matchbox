use cfg_if::cfg_if;
use serde::{Deserialize, Serialize};

/// The format for a peer signature given by the signalling server
pub type PeerId = String;

/// Requests go from peer to signalling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerRequest<S> {
    Signal { receiver: PeerId, data: S },
    KeepAlive,
}

/// Events go from signalling server to peer
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerEvent<S> {
    IdAssigned(PeerId),
    NewPeer(PeerId),
    PeerLeft(PeerId),
    Signal { sender: PeerId, data: S },
}

cfg_if! {
    if #[cfg(feature = "json")] {
        pub type JsonPeerRequest = PeerRequest<serde_json::Value>;
        pub type JsonPeerEvent = PeerEvent<serde_json::Value>;
    }
}
