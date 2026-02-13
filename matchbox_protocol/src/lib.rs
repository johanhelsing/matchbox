use cfg_if::cfg_if;
use derive_more::{Display, From};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The format for a peer signature given by the signaling server
#[derive(
    Debug, Display, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, From, Hash, PartialOrd, Ord,
)]
pub struct PeerId(pub Uuid);

/// Requests go from peer to signaling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerRequest<S> {
    Signal { receiver: PeerId, data: S },
    KeepAlive,
}

/// Events go from signaling server to peer
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerEvent<S> {
    /// Sent by the server to the connecting peer, immediately after connection
    /// before any other events.
    /// Includes the PeerId that that the receiver should consider to be theirs.
    IdAssigned(PeerId),
    /// Includes the PeerId for the connecting remote peer.
    NewPeer(PeerId),
    PeerLeft(PeerId),
    Signal {
        sender: PeerId,
        data: S,
    },
}

cfg_if! {
    if #[cfg(feature = "json")] {
        pub type JsonPeerRequest = PeerRequest<serde_json::Value>;
        pub type JsonPeerEvent = PeerEvent<serde_json::Value>;
        use std::fmt;


        impl fmt::Display for JsonPeerRequest {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", serde_json::to_string(self).map_err(|_| fmt::Error)?)
            }
        }
        impl std::str::FromStr for JsonPeerRequest {
            type Err = serde_json::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                serde_json::from_str(s)
            }
        }

        impl fmt::Display for JsonPeerEvent {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", serde_json::to_string(self).map_err(|_| fmt::Error)?)
            }
        }
        impl std::str::FromStr for JsonPeerEvent {
            type Err = serde_json::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                serde_json::from_str(s)
            }
        }
    }
}
