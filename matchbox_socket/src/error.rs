pub use crate::webrtc_socket::error::SignalingError;

/// Errors that can happen when using Matchbox sockets.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurring if the connection fails to establish. Perhaps check your connection or
    /// try again.
    #[error("The connection failed to establish. Check your connection and try again.")]
    ConnectionFailed(SignalingError),
    /// Disconnected from the signaling server
    #[error("The signaling server connection was severed.")]
    Disconnected(SignalingError),
}
