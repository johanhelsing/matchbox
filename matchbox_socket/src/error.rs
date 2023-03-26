use crate::webrtc_socket::error::SignalingError;

/// Errors that can happen when using Matchbox.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurring during the signaling loop.
    #[error("An error in the signaling loop")]
    Signaling(#[from] SignalingError),
}
