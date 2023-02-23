use crate::webrtc_socket::{error::SignallingError, messages::*};
use futures::{SinkExt, StreamExt};
use futures_util::select;
use log::{debug, error};
use ws_stream_wasm::{WsMessage, WsMeta};

pub async fn signalling_loop(
    room_url: String,
    mut requests_receiver: futures_channel::mpsc::UnboundedReceiver<PeerRequest>,
    events_sender: futures_channel::mpsc::UnboundedSender<PeerEvent>,
) -> Result<(), SignallingError> {
    let (_ws, wsio) = WsMeta::connect(&room_url, None)
        .await
        .map_err(SignallingError::from)?;

    let mut wsio = wsio.fuse();

    loop {
        select! {
            request = requests_receiver.next() => {
                let request = serde_json::to_string(&request).expect("serializing request");
                debug!("-> {}", request);
                wsio.send(WsMessage::Text(request)).await.map_err(SignallingError::from)?;
            }

            message = wsio.next() => {
                match message {
                    Some(WsMessage::Text(message)) => {
                        debug!("{}", message);
                        let event: PeerEvent = serde_json::from_str(&message)
                            .unwrap_or_else(|_| panic!("couldn't parse peer event {message}"));
                        events_sender.unbounded_send(event).map_err(SignallingError::from)?;
                    },
                    Some(WsMessage::Binary(_)) => {
                        error!("Received binary data from signal server (expected text). Ignoring.");
                    },
                    None => {
                        error!("Disconnected from signalling server!");
                        break Err(SignallingError::StreamExhausted)
                    }
                }
            }

            complete => break Ok(())
        }
    }
}
