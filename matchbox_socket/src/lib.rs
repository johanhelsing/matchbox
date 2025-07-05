#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod error;
#[cfg(feature = "ggrs")]
mod ggrs_socket;
mod webrtc_socket;

pub use async_trait;
pub use error::{Error, SignalingError};
pub use matchbox_protocol::PeerId;
pub use webrtc_socket::{
    ChannelConfig, MessageLoopFuture, Packet, PeerEvent, PeerRequest, PeerSignal, PeerState,
    RtcIceServerConfig, Signaller, SignallerBuilder, WebRtcChannel, WebRtcSocket,
    WebRtcSocketBuilder, error::ChannelError, PeerBuffered
};
