use crate::webrtc_socket::{
    error::SignallingError,
    messages::{PeerEvent, PeerRequest},
};
use async_tungstenite::{async_std::connect_async, tungstenite::Message};
use futures::{pin_mut, FutureExt, SinkExt, StreamExt};
use futures_util::select;
use log::{debug, error, warn};

pub async fn signalling_loop(
    room_url: String,
    mut requests_receiver: futures_channel::mpsc::UnboundedReceiver<PeerRequest>,
    events_sender: futures_channel::mpsc::UnboundedSender<PeerEvent>,
) -> Result<(), SignallingError> {
    debug!("Signalling loop started");
    let (mut wsio, _response) = connect_async(&room_url)
        .await
        .map_err(SignallingError::from)?;

    loop {
        let next_request = requests_receiver.next().fuse();
        let next_websocket_message = wsio.next().fuse();

        pin_mut!(next_request, next_websocket_message);

        select! {
            request = next_request => {
                let request = serde_json::to_string(&request).expect("serializing request");
                debug!("-> {}", request);
                wsio.send(Message::Text(request)).await.map_err(SignallingError::from)?;
            }

            message = next_websocket_message => {
                match message {
                    Some(Ok(Message::Text(message))) => {
                        debug!("{}", message);
                        let event: PeerEvent = serde_json::from_str(&message)
                            .unwrap_or_else(|err| panic!("couldn't parse peer event: {err}.\nEvent: {message}"));
                        events_sender.unbounded_send(event).map_err(SignallingError::from)?;
                    },
                    Some(Ok(message)) => {
                        warn!("ignoring unexpected non-text message from signalling server: {:?}", message)
                    },
                    Some(Err(e)) => {
                        break Err(SignallingError::from(e))
                    },
                    None => {
                        error!("Disconnected from signalling server!");
                        break Err(SignallingError::StreamExhausted)
                    }
                };
            }

            complete => break Ok(())
        }
    }
}
