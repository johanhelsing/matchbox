use crate::signaling_server::error::SignalingError;

/// Errors that can occur in the lifetime of a signaling server.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurring during the signaling loop.
    #[error("An unrecoverable error in the signaling loop: {0}")]
    Signaling(#[from] SignalingError),

    /// An error occurring from hyper
    #[error("Hyper error: {0}")]
    Hyper(#[from] hyper::Error),

    /// Couldn't bind to socket
    #[error("Bind error: {0}")]
    Bind(std::io::Error),

    /// Error on serve
    #[error("Serve error: {0}")]
    Serve(std::io::Error),
}
