/// An error that can occur with WebRTC signalling.
#[derive(Debug, thiserror::Error)]
pub enum SignallingError {
    #[cfg(not(target_arch = "wasm32"))]
    #[error("Failed to connect to signalling server")]
    Unreachable(#[from] async_tungstenite::tungstenite::Error),
    #[cfg(target_arch = "wasm32")]
    #[error("Failed to connect to signalling server")]
    Unreachable(#[from] ws_stream_wasm::WsErr),
    #[cfg(target_arch = "wasm32")]
    #[error("The stream is exhausted")]
    StreamExhausted,
}
