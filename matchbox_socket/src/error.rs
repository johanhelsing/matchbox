use crate::webrtc_socket::error::SignallingError;

/// Errors that can happen when using Matchbox.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurring during the signalling loop.
    #[error("An error in the signalling loop")]
    Signalling(#[from] SignallingError),
}
