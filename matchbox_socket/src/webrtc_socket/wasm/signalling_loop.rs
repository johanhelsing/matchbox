use crate::webrtc_socket::messages::*;
use futures::{SinkExt, StreamExt};
use futures_util::select;
use log::{debug, error};
use ws_stream_wasm::{WsMessage, WsMeta};

pub async fn signalling_loop(
    room_url: String,
    mut requests_receiver: futures_channel::mpsc::UnboundedReceiver<PeerRequest>,
    events_sender: futures_channel::mpsc::UnboundedSender<PeerEvent>,
) {
    let (_ws, wsio) = WsMeta::connect(&room_url, None)
        .await
        .expect("failed to connect to signalling server");

    let mut wsio = wsio.fuse();

    loop {
        select! {
            request = requests_receiver.next() => {
                let request = serde_json::to_string(&request).expect("serializing request");
                debug!("-> {}", request);
                wsio.send(WsMessage::Text(request)).await.expect("request send error");
            }

            message = wsio.next() => {
                match message {
                    Some(WsMessage::Text(message)) => {
                        debug!("{}", message);
                        let event: PeerEvent = serde_json::from_str(&message)
                            .unwrap_or_else(|_| panic!("couldn't parse peer event {}", message));
                        events_sender.unbounded_send(event).unwrap();
                    },
                    Some(WsMessage::Binary(_)) => {
                        error!("Received binary data from signal server (expected text). Ignoring.");
                    },
                    None => {
                        error!("Disconnected from signalling server!");
                        break;
                    }
                }
            }

            complete => break
        }
    }
}
