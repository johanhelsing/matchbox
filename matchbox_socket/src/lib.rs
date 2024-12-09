#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod error;
#[cfg(feature = "ggrs")]
mod ggrs_socket;
mod webrtc_socket;

pub use error::Error;
pub use matchbox_protocol::PeerId;
pub use webrtc_socket::{
    error::ChannelError, ChannelConfig, MessageLoopFuture, Packet, PeerState, RtcIceServerConfig,
    WebRtcChannel, WebRtcSocket, WebRtcSocketBuilder,
};
#[cfg(feature = "ggrs")]
pub use ggrs_socket::GGRS_CHANNEL_ID;
