use serde::{Deserialize, Serialize};

/// The format for a peer signature given by the signalling server
pub type PeerId = String;

/// Events go from signalling server to peer
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerEvent {
    IdAssigned(PeerId),
    NewPeer(PeerId),
    PeerLeft(PeerId),
    Signal { sender: PeerId, data: PeerSignal },
}

// TODO: move back into lib
/// Requests go from peer to signalling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerRequest {
    Signal { receiver: PeerId, data: PeerSignal },
    KeepAlive,
}

/// Signals go from peer to peer via the signalling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerSignal {
    IceCandidate(String),
    Offer(String),
    Answer(String),
}
