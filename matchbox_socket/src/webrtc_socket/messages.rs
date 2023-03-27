use serde::{Deserialize, Serialize};

/// Events go from signaling server to peer
pub type PeerEvent = matchbox_protocol::PeerEvent<PeerSignal>;

/// Requests go from peer to signaling server
pub type PeerRequest = matchbox_protocol::PeerRequest<PeerSignal>;

/// Signals go from peer to peer via the signaling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerSignal {
    IceCandidate(String),
    Offer(String),
    Answer(String),
}
