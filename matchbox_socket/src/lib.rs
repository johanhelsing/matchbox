#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod error;
#[cfg(feature = "ggrs-socket")]
mod ggrs_socket;
mod webrtc_socket;

pub use error::Error;
pub use webrtc_socket::{
    ChannelConfig, MessageLoopFuture, Packet, PeerState, RtcIceServerConfig, WebRtcSocket,
    WebRtcSocketConfig,
};
