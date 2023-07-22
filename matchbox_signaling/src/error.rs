use std::io;

use crate::signaling_server::error::SignalingError;

/// Errors that can occur in the lifetime of a signaling server.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurring during the signaling loop.
    #[error("An unrecoverable error in the signaling loop")]
    Signaling(#[from] SignalingError),

    /// An error occurring from hyper
    #[error("IO Error")]
    Io(#[from] io::Error),
}
