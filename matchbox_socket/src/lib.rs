#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

#[cfg(feature = "ggrs-socket")]
mod ggrs_socket;
mod webrtc_socket;

pub use webrtc_socket::{RtcDataChannelConfig, RtcIceServerConfig, WebRtcSocket, WebRtcSocketConfig};
