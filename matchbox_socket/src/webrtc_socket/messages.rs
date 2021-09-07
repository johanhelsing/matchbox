use serde::{Deserialize, Serialize};

/// Events go from signalling server to peer
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerEvent {
    NewPeer(String),
    Signal { sender: String, data: PeerSignal },
}

// TODO: move back into lib
/// Requests go from peer to signalling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerRequest {
    Uuid(String),
    Signal { receiver: String, data: PeerSignal },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerSignal {
    IceCandidate(String),
    Offer(String),
    Answer(String),
}
