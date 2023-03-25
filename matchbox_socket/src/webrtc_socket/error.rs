use crate::webrtc_socket::messages::PeerEvent;
use cfg_if::cfg_if;
use futures_channel::mpsc::TrySendError;

#[derive(Debug, thiserror::Error)]
pub enum GetChannelError {
    #[error("This channel was never created")]
    NotFound,
    #[error("This channel has already been taken and is no longer on the socket")]
    Taken,
}

/// An error that can occur with WebRTC signalling.
#[derive(Debug, thiserror::Error)]
pub enum SignallingError {
    // Common
    #[error("failed to send event to signalling server")]
    Undeliverable(#[from] TrySendError<PeerEvent>),
    #[error("The stream is exhausted")]
    StreamExhausted,
    #[error("Message received in unknown format")]
    UnknownFormat,
    #[error("failed to establish initial connection")]
    ConnectionFailed(#[from] Box<SignallingError>),

    // Native
    #[cfg(not(target_arch = "wasm32"))]
    #[error("socket failure communicating with signalling server")]
    Socket(#[from] async_tungstenite::tungstenite::Error),

    // WASM
    #[cfg(target_arch = "wasm32")]
    #[error("socket failure communicating with signalling server")]
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

        // The below is just to wrap Result<JsValue, JsValue> into something sensible-ish

        pub trait JsErrorExt<T> {
            fn efix(self) -> Result<T, JsError>;
        }

        impl<T> JsErrorExt<T> for Result<T, JsValue> {
            fn efix(self) -> Result<T, JsError> {
                self.map_err(JsError)
            }
        }

        #[derive(Debug)]
        pub struct JsError(JsValue);

        impl std::error::Error for JsError {}

        impl std::fmt::Display for JsError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:?}", self.0)
            }
        }
    }
}
