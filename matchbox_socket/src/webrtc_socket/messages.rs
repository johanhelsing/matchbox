use serde::{Deserialize, Serialize};

pub(crate) type PeerId = String;

/// Events go from signalling server to peer
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerEvent {
    NewPeer(PeerId),
    PeerLeft(PeerId),
    Signal { sender: PeerId, data: PeerSignal },
}

// TODO: move back into lib
/// Requests go from peer to signalling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerRequest {
    Uuid(PeerId),
    Signal {
        receiver: PeerId,
        data: PeerSignal,
    },
    /// A routine packet to prevent idle websocket disconnection
    KeepAlive,
}

/// Signals go from peer to peer via the signalling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerSignal {
    IceCandidate(String),
    Offer(String),
    Answer(String),
}
