use crate::webrtc_socket::error::SignalingError;

/// Errors that can happen when using Matchbox sockets.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurring if the connection fails to establish. Perhaps check your connection or try again.
    #[error("The connection failed to establish. Check your connection and try again.")]
    ConnectionFailed {
        /// The source of the connection failure
        source: SignalingError,
    },
    /// Unexpected fatal error ocurred with messaging. Please file an issue or triage.
    #[error("An unexpected error ocurred at runtime with messaging: {source}")]
    Runtime {
        /// The source of the connection failure
        source: SignalingError,
    },
    /// Kicked by the server or disconnected
    #[error("The signaling server connection was severed.")]
    Disconnected {
        /// The source of the connection failure
        source: SignalingError,
    },
}
