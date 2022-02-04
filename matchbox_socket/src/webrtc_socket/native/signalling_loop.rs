use async_tungstenite::{async_std::connect_async, tungstenite::Message};
use futures::{pin_mut, FutureExt, SinkExt, StreamExt};
use futures_util::select;
use log::{debug, warn};

use crate::webrtc_socket::messages::{PeerEvent, PeerRequest};

pub async fn signalling_loop(
    room_url: String,
    mut requests_receiver: futures_channel::mpsc::UnboundedReceiver<PeerRequest>,
    events_sender: futures_channel::mpsc::UnboundedSender<PeerEvent>,
) {
    debug!("Signalling loop started");
    let (mut wsio, _response) = connect_async(&room_url)
        .await
        .expect("failed to connect to signalling server");

    loop {
        let next_request = requests_receiver.next().fuse();
        let next_websocket_message = wsio.next().fuse();

        pin_mut!(next_request, next_websocket_message);

        select! {
            request = next_request => {
                let request = serde_json::to_string(&request).expect("serializing request");
                debug!("-> {}", request);
                wsio.send(Message::Text(request)).await.expect("request send error");
            }

            message = next_websocket_message => {
                match message {
                    Some(Ok(Message::Text(message))) => {
                        debug!("{}", message);
                        let event: PeerEvent = serde_json::from_str(&message)
                            .unwrap_or_else(|err| panic!("couldn't parse peer event: {}.\nEvent: {}", err, message));
                        events_sender.unbounded_send(event).unwrap();
                    },
                    Some(Ok(message)) => {
                        warn!("ignoring unexpected non-text message from signalling server: {:?}", message)
                    },
                    Some(Err(e)) => {
                        // TODO: propagate errors or recover
                        panic!("WebSocket error {:?}", e)
                    },
                    None => {} // Disconnected from signalling server
                };
            }

            complete => break
        }
    }
}
