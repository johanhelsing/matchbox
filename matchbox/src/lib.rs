use serde::{Deserialize, Serialize};

/// Requests go from peer to signalling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerRequest<S> {
    Uuid(String),
    Signal { receiver: String, data: S },
}

/// Events go from signalling server to peer
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerEvent<S> {
    NewPeer(String),
    Signal { sender: String, data: S },
}
