use axum::extract::ws::Message;
use tokio::sync::mpsc::error::SendError;

/// An error derived from a client's request.
#[derive(Debug, thiserror::Error)]
pub enum ClientRequestError {
    /// An error originating from Axum
    #[error("Axum error: {0}")]
    Axum(#[from] axum::Error),

    /// The socket is closed
    #[error("Socket is closed.")]
    Close,

    /// Message received was not JSON
    #[error("Json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Unsupported message type (not JSON)
    #[error("Unsupported message type: {0:?}")]
    UnsupportedType(Message),
}

/// An error in server logic.
#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    /// The recipient peer is unknown
    #[error("Unknown recipient peer")]
    UnknownPeer,

    /// The message was undeliverable (socket may be closed or a future was dropped prematurely)
    #[error("Undeliverable message: {0}")]
    Undeliverable(#[from] SendError<Result<Message, axum::Error>>),
}
