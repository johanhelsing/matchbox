#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![deny(unsafe_code)]
mod authentication;
mod error;
mod signaling_server;
mod topologies;

pub use error::Error;
pub use signaling_server::{
    builder::SignalingServerBuilder, callbacks::Callback, server::SignalingServer,
    SignalingCallbacks, SignalingState,
};
