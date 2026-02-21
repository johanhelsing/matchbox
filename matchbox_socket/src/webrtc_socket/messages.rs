//! Messages involved in the "signaling" process.
//!
//! These messages involve the signaling server and are used to establish the direct peer to peer
//! connections.
//! The direct peer to peer messages do not use these types: they use [`crate::Packet`].

use serde::{Deserialize, Serialize};

/// Events which go from signaling server to peer.
pub type PeerEvent = matchbox_protocol::PeerEvent<PeerSignal>;

/// Requests which go from peer to signaling server.
pub type PeerRequest = matchbox_protocol::PeerRequest<PeerSignal>;

/// Signals which go from peer to peer via the signaling server.
///
/// Wrapped by [`PeerRequest::Signal`] on the way to the signaling server,
/// and [`PeerEvent::Signal`] on the from the signalizing server the other peer.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum PeerSignal {
    /// [Ice (Interactive Connectivity Establishment) Candidate](https://en.wikipedia.org/wiki/Interactive_Connectivity_Establishment).
    // JSON encoding of an [`RTCIceCandidate`](https://developer.mozilla.org/en-US/docs/Web/API/RTCIceCandidate).
    //
    // Note that while [`RTCIceCandidate`](https://developer.mozilla.org/en-US/docs/Web/API/RTCIceCandidate/RTCIceCandidate)'s
    // constructor accepts a string for legacy purposes, that is not how this is being used: this
    // is instead a
    // [JSON encoding of the of the `RTCIceCandidate`](https://developer.mozilla.org/en-US/docs/Web/API/RTCIceCandidate/toJSON).
    //
    // When PeerSignal itself is JSON serialized, that results in JSON data as a string inside of
    // another JSON string: this is inefficient (but negligibly so) and reduces type safety.
    // It may be preferable refactor the abstraction implemented by the native and wasm sockets to
    // have a with a more specific type instead of this to deduplicate their encoding logic
    // and help ensure they are well aligned and can interoperate.
    IceCandidate(String),
    /// Offer a handshake.
    ///
    /// The contained string is the [`sdp`](https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection/createOffer#sdp).
    Offer(String),
    /// Answer accepting a handshake.
    ///
    /// The contained string is the [`sdp`](https://developer.mozilla.org/en-US/docs/Web/API/RTCSessionDescription/sdp).
    Answer(String),
}
