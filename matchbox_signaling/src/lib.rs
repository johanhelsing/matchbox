#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod error;
mod signaling_server;
/// Network topologies to be created by the [`SignalingServer`]
pub mod topologies;

pub use error::Error;
pub use signaling_server::{
    builder::SignalingServerBuilder, callbacks::Callback, server::SignalingServer,
    SignalingCallbacks, SignalingState,
};
