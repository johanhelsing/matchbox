#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

#[cfg(feature = "ggrs-socket")]
mod ggrs_socket;
mod webrtc_socket;

pub use webrtc_socket::{ChannelConfig, RtcIceServerConfig, WebRtcSocket, WebRtcSocketConfig};
