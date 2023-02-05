pub mod message_loop;

use super::error::SignallingError;
use crate::webrtc_socket::Signaller;
use async_trait::async_trait;
use async_tungstenite::{
    async_std::{connect_async, ConnectStream},
    tungstenite::Message,
    WebSocketStream,
};
use futures::{SinkExt, StreamExt};

pub(crate) struct NativeSignaller {
    websocket_stream: WebSocketStream<ConnectStream>,
}

#[async_trait]
impl Signaller for NativeSignaller {
    async fn new(room_url: &str) -> Result<Self, SignallingError> {
        Ok(Self {
            websocket_stream: connect_async(room_url)
                .await
                .map_err(SignallingError::from)?
                .0,
        })
    }

    async fn send(&mut self, request: String) -> Result<(), SignallingError> {
        self.websocket_stream
            .send(Message::Text(request))
            .await
            .map_err(SignallingError::from)
    }

    async fn next_message(&mut self) -> Result<String, SignallingError> {
        match self.websocket_stream.next().await {
            Some(Ok(Message::Text(message))) => Ok(message),
            Some(Ok(_)) => Err(SignallingError::UnknownFormat),
            Some(Err(err)) => Err(SignallingError::from(err)),
            None => Err(SignallingError::StreamExhausted),
        }
    }
}
