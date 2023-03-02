use crate::webrtc_socket::{error::SignallingError, Signaller};
use async_trait::async_trait;
use futures::{stream::Fuse, SinkExt, StreamExt};
use log::warn;
use ws_stream_wasm::{WsMessage, WsMeta, WsStream};
pub mod message_loop;
pub(crate) struct WasmSignaller {
    websocket_stream: Fuse<WsStream>,
}

#[async_trait(?Send)]
impl Signaller for WasmSignaller {
    async fn new(mut attempts: Option<u16>, room_url: &str) -> Result<Self, SignallingError> {
        let websocket_stream = 'signalling: loop {
            match WsMeta::connect(room_url, None)
                .await
                .map_err(SignallingError::from)
            {
                Ok((_, wss)) => break wss.fuse(),
                Err(e) => {
                    if let Some(attempts) = attempts.as_mut() {
                        if *attempts <= 1 {
                            return Err(SignallingError::ConnectionFailed(Box::new(e)));
                        } else {
                            *attempts -= 1;
                            warn!("connection to signalling server failed, {attempts} attempt(s) remain");
                        }
                    } else {
                        continue 'signalling;
                    }
                }
            };
        };
        Ok(Self { websocket_stream })
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
