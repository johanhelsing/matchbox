use axum::extract::ws::Message;
use tokio::sync::mpsc::error::SendError;

/// An error derived from a client's request.
#[derive(Debug, thiserror::Error)]
pub enum ClientRequestError {
    #[error("Axum error")]
    Axum(#[from] axum::Error),
    #[error("Message is close")]
    Close,
    #[error("Json error")]
    Json(#[from] serde_json::Error),
    #[error("Unsupported message type")]
    UnsupportedType,
}

/// An error in server logic.
#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    #[error("Unknown recipient peer")]
    UnknownPeer,
    #[error("Undeliverable message")]
    Undeliverable(#[from] SendError<Result<Message, axum::Error>>),
}
