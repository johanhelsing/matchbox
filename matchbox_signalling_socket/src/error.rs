use crate::signalling_socket::error::ServerError;

/// Errors that can happen when using Matchbox.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurring during the signalling loop.
    #[error("An unrecoverable error in the signalling loop")]
    Server(#[from] ServerError),

    /// An error occurring from hyper
    #[error("Hyper error")]
    Hyper(#[from] hyper::Error),
}
