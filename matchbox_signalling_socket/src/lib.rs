#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod error;
mod signalling_socket;

pub use error::Error;
pub use signalling_socket::socket::SignallingServer;
