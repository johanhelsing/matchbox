/// An error that can occur with WebRTC signalling.
#[derive(Debug, thiserror::Error)]
pub enum SignallingError {
    // Common
    #[error("failed to send event to signalling server")]
    Undeliverable(#[from] futures_channel::mpsc::TrySendError<super::messages::PeerEvent>),
    #[error("The stream is exhausted")]
    StreamExhausted,

    // Native
    #[cfg(not(target_arch = "wasm32"))]
    #[error("socket failure communicating with signalling server")]
    Socket(#[from] async_tungstenite::tungstenite::Error),

    // WASM
    #[cfg(target_arch = "wasm32")]
    #[error("socket failure communicating with signalling server")]
    Socket(#[from] ws_stream_wasm::WsErr),
}
