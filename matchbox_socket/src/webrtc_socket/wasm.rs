pub mod message_loop;

use super::error::SignallingError;
use super::Signaller;
use async_trait::async_trait;
use futures::{stream::Fuse, SinkExt, StreamExt};
use ws_stream_wasm::{WsMessage, WsMeta, WsStream};

pub(crate) struct WasmSignaller {
    websocket_stream: Fuse<WsStream>,
}

#[async_trait(?Send)]
impl Signaller for WasmSignaller {
    async fn new(room_url: &str) -> Result<Self, SignallingError> {
        Ok(Self {
            websocket_stream: WsMeta::connect(room_url, None)
                .await
                .map_err(SignallingError::from)?
                .1
                .fuse(),
        })
    }

    async fn send(&mut self, request: String) -> Result<(), SignallingError> {
        self.websocket_stream
            .send(WsMessage::Text(request))
            .await
            .map_err(SignallingError::from)
    }

    async fn next_message(&mut self) -> Result<String, SignallingError> {
        match self.websocket_stream.next().await {
            Some(WsMessage::Text(message)) => Ok(message),
            Some(_) => Err(SignallingError::UnknownFormat),
            None => Err(SignallingError::StreamExhausted),
        }
    }
}
