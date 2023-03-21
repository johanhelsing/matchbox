use crate::signaling_server::error::SignalingError;

/// Errors that can happen when using a signaling server.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurring during the signalling loop.
    #[error("An unrecoverable error in the signalling loop")]
    Signaling(#[from] SignalingError),

    /// An error occurring from hyper
    #[error("Hyper error")]
    Hyper(#[from] hyper::Error),
}
