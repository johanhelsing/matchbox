use crate::webrtc_socket::{error::SignallingError, Signaller};
use async_trait::async_trait;
use async_tungstenite::{
    async_std::{connect_async, ConnectStream},
    tungstenite::Message,
    WebSocketStream,
};
use futures::{SinkExt, StreamExt};
use log::warn;
pub mod message_loop;

pub(crate) struct NativeSignaller {
    websocket_stream: WebSocketStream<ConnectStream>,
}

#[async_trait]
impl Signaller for NativeSignaller {
    async fn new(mut attempts: Option<u16>, room_url: &str) -> Result<Self, SignallingError> {
        let websocket_stream = 'signalling: loop {
            match connect_async(room_url).await.map_err(SignallingError::from) {
                Ok((wss, _)) => break wss,
                Err(e) => {
                    if let Some(attempts) = attempts.as_mut() {
                        if *attempts <= 1 {
                            return Err(SignallingError::NoMoreAttempts(Box::new(e)));
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

#[cfg(test)]
mod tests {
    use crate::{
        webrtc_socket::error::SignallingError, ChannelConfig, Error, RtcIceServerConfig,
        WebRtcSocket, WebRtcSocketConfig,
    };
    use futures::{pin_mut, select, FutureExt};
    use std::time::Duration;
    use tokio::time;

    #[tokio::test]
    async fn test_signalling_attempts_exhausted() {
        let (_socket, loop_fut) = WebRtcSocket::new_with_config(WebRtcSocketConfig {
            room_url: "wss://example.invalid/".to_string(),
            attempts: Some(3),
            ice_server: RtcIceServerConfig::default(),
            channels: vec![ChannelConfig::unreliable()],
        });

        let timeout = time::sleep(Duration::from_millis(100)).fuse();
        pin_mut!(timeout);

        let loop_fut = loop_fut.fuse();
        pin_mut!(loop_fut);

        select! {
            result = loop_fut => {
                assert!(result.is_err());
                assert!(matches!(result.err().unwrap(), Error::Signalling(SignallingError::NoMoreAttempts(_))))
            },
            _ = &mut timeout => panic!("should fail, not timeout")
        }
    }
}
