use crate::webrtc_socket::messages::PeerEvent;
use cfg_if::cfg_if;
use futures_channel::mpsc::TrySendError;

/// An error that can occur when getting a socket's channel through
/// `get_channel`, `take_channel` or `try_update_peers`.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// Can occur if trying to get a channel with an Id that was not added while building the
    /// socket
    #[error("This channel was never created")]
    NotFound,
    /// The channel has already been taken and is no longer on the socket
    #[error("This channel has already been taken and is no longer on the socket")]
    Taken,
    /// Channel might have been opened but later closed, or never opened in the first place.
    /// The latter can for example occur when an one calls `try_update_peers` on a socket that was
    /// given an invalid room URL.
    #[error("This channel is closed.")]
    Closed,
}

/// An error that can occur with WebRTC signaling.
#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    // Common
    #[error("failed to send event to signaling server")]
    Undeliverable(#[from] TrySendError<PeerEvent>),
    #[error("The stream is exhausted")]
    StreamExhausted,
    #[error("Message received in unknown format")]
    UnknownFormat,
    #[error("failed to establish initial connection")]
    ConnectionFailed(#[from] Box<SignalingError>),

    // Native
    #[cfg(not(target_arch = "wasm32"))]
    #[error("socket failure communicating with signaling server")]
    Socket(#[from] async_tungstenite::tungstenite::Error),

    // WASM
    #[cfg(target_arch = "wasm32")]
    #[error("socket failure communicating with signaling server")]
    Socket(#[from] ws_stream_wasm::WsErr),
}

/// An error that can occur with WebRTC messaging.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, thiserror::Error)]
#[error("failed to send message to peer")]
pub(crate) struct MessagingError(#[from] futures_channel::mpsc::TrySendError<crate::Packet>);

#[cfg(target_arch = "wasm32")]
#[derive(Debug, thiserror::Error)]
#[error("failed to send message to peer")]
pub(crate) struct MessagingError(#[from] JsError);

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        use wasm_bindgen::{JsValue};
        use derive_more::Display;

        // The below is just to wrap Result<JsValue, JsValue> into something sensible-ish

        pub trait JsErrorExt<T> {
            fn efix(self) -> Result<T, JsError>;
        }

        impl<T> JsErrorExt<T> for Result<T, JsValue> {
            fn efix(self) -> Result<T, JsError> {
                self.map_err(JsError)
            }
        }

        #[derive(Debug, Display)]
        #[display(fmt = "{_0:?}")]
        pub struct JsError(JsValue);

        impl std::error::Error for JsError {}
    }
}
