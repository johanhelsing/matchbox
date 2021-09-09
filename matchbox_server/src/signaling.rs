use futures::{lock::Mutex, stream::SplitSink, StreamExt};
use log::{error, info};
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::{
    ws::{Message, WebSocket},
    Error, Filter, Rejection, Reply,
};

pub mod matchbox {
    use serde::{Deserialize, Serialize};

    pub type PeerId = String;

    /// Requests go from peer to signalling server
    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    pub enum PeerRequest<S> {
        Uuid(PeerId),
        Signal { receiver: PeerId, data: S },
    }

    /// Events go from signalling server to peer
    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    pub enum PeerEvent<S> {
        NewPeer(PeerId),
        Signal { sender: PeerId, data: S },
    }
}
use matchbox::*;

type PeerRequest = matchbox::PeerRequest<serde_json::Value>;
type PeerEvent = matchbox::PeerEvent<serde_json::Value>;

pub struct Peer {
    pub uuid: PeerId,
    pub sender:
        Option<tokio::sync::mpsc::UnboundedSender<std::result::Result<Message, warp::Error>>>,
}

#[derive(Default)]
pub(crate) struct State {
    clients: HashMap<PeerId, Peer>,
}

pub(crate) fn ws_filter(
    state: Arc<Mutex<State>>,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::ws()
        .and(warp::any())
        .and(warp::path::param())
        .and(with_state(state.clone()))
        .and_then(ws_handler)
}

fn with_state(
    state: Arc<Mutex<State>>,
) -> impl Filter<Extract = (Arc<Mutex<State>>,), Error = Infallible> + Clone {
    warp::any().map(move || state.clone())
}

pub(crate) async fn ws_handler(
    ws: warp::ws::Ws,
    _path: String,
    state: Arc<Mutex<State>>,
) -> std::result::Result<impl Reply, Rejection> {
    Ok(ws.on_upgrade(move |websocket| handle_ws(websocket, state)))
}

#[derive(Debug, thiserror::Error)]
enum RequestError {
    #[error("Warp error")]
    WarpError(#[from] warp::Error),
    #[error("Text error")]
    TextError,
    #[error("Json error")]
    JsonError(#[from] serde_json::Error),
}

fn parse_request(request: Result<Message, Error>) -> Result<PeerRequest, RequestError> {
    let request = request?;

    if !request.is_text() {
        return Err(RequestError::TextError);
    }

    let request = request.to_str().map_err(|_| RequestError::TextError)?;

    let request: PeerRequest = serde_json::from_str(request)?;

    Ok(request)
}

fn spawn_sender_task(
    sender: SplitSink<WebSocket, Message>,
) -> mpsc::UnboundedSender<std::result::Result<Message, warp::Error>> {
    let (client_sender, receiver) = mpsc::unbounded_channel();
    tokio::task::spawn(UnboundedReceiverStream::new(receiver).forward(sender));
    client_sender
}

async fn handle_ws(websocket: WebSocket, state: Arc<Mutex<State>>) {
    let (ws_sender, mut ws_receiver) = websocket.split();
    let sender = spawn_sender_task(ws_sender);
    let mut peer_uuid = None;

    while let Some(request) = ws_receiver.next().await {
        let request = match parse_request(request) {
            Ok(request) => request,
            Err(e) => {
                error!("Error untangling request: {:?}", e);
                continue;
            }
        };

        info!("{:?} <- {:?}", peer_uuid, request);

        match request {
            PeerRequest::Uuid(uuid) => {
                if peer_uuid.is_some() {
                    error!("client set uuid more than once");
                    continue;
                }
                peer_uuid = Some(uuid.clone());

                let mut state = state.lock().await;
                state.clients.insert(
                    uuid.clone(),
                    Peer {
                        uuid: uuid.clone(),
                        sender: Some(sender.clone()),
                    },
                );

                let event = Message::text(
                    serde_json::to_string(&PeerEvent::NewPeer(uuid.clone()))
                        .expect("error serializing message"),
                );

                for peer in state.clients.iter().filter(|peer| peer.1.uuid != uuid) {
                    // Tell everyone about this new peer
                    info!("{:?} -> {:?}", peer.1.uuid, event.to_str().unwrap());
                    if let Err(e) = peer.1.sender.as_ref().unwrap().send(Ok(event.clone())) {
                        error!("error sending: {:?}", e);
                    }
                }
            }
            PeerRequest::Signal { receiver, data } => {
                let sender = match peer_uuid.clone() {
                    Some(sender) => sender,
                    None => {
                        error!("client is trying signal before sending uuid");
                        continue;
                    }
                };
                let event = Message::text(
                    serde_json::to_string(&PeerEvent::Signal { sender, data })
                        .expect("error serializing message"),
                );
                let state = state.lock().await;
                if let Err(e) = state
                    .clients
                    .get(&receiver)
                    .unwrap()
                    .sender
                    .as_ref()
                    .unwrap()
                    .send(Ok(event))
                {
                    error!("error sending: {:?}", e);
                }
            }
        }
    }

    if let Some(uuid) = peer_uuid {
        let mut state = state.lock().await;
        state
            .clients
            .remove(&uuid)
            .expect("Couldn't find uuid to remove");
    }
}

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use futures::pin_mut;
    use tokio::{select, time};
    use warp::{test::WsClient, ws::Message, Filter, Rejection, Reply};

    use crate::signaling::PeerEvent;

    fn api() -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
        super::ws_filter(Default::default())
    }

    #[tokio::test]
    async fn ws_connect() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        // let req = warp::test::ws().path("/echo");
        warp::test::ws()
            .path("/room_a")
            .handshake(api)
            // .handshake(ws_echo())
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn new_peer() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        // let req = warp::test::ws().path("/echo");
        let mut client_a = warp::test::ws()
            .path("/room_a")
            .handshake(api.clone())
            // .handshake(ws_echo())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let mut client_b = warp::test::ws()
            .path("/room_a")
            .handshake(api)
            // .handshake(ws_echo())
            .await
            .expect("handshake");

        client_b
            .send(Message::text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        let a_msg = client_a.recv().await;
        let new_peer_event: PeerEvent =
            serde_json::from_str(a_msg.unwrap().to_str().unwrap()).unwrap();

        assert_eq!(new_peer_event, PeerEvent::NewPeer("uuid-b".to_string()));
    }

    #[tokio::test]
    async fn signal() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        // let req = warp::test::ws().path("/echo");
        let mut client_a = warp::test::ws()
            .path("/room_a")
            .handshake(api.clone())
            // .handshake(ws_echo())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let mut client_b = warp::test::ws()
            .path("/room_a")
            .handshake(api)
            // .handshake(ws_echo())
            .await
            .expect("handshake");

        client_b
            .send(Message::text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        let a_msg = client_a.recv().await;
        let new_peer_event: PeerEvent =
            serde_json::from_str(a_msg.unwrap().to_str().unwrap()).unwrap();

        let peer_uuid = match new_peer_event {
            PeerEvent::NewPeer(peer) => peer,
            _ => panic!("unexpected event"),
        };

        client_a
            .send(Message::text(format!(
                "{{\"Signal\": {{\"receiver\": \"{}\", \"data\": \"123\" }}}}",
                peer_uuid
            )))
            .await;

        let b_msg = client_b.recv().await;
        let signal_event: PeerEvent =
            serde_json::from_str(b_msg.unwrap().to_str().unwrap()).unwrap();

        assert_eq!(
            signal_event,
            PeerEvent::Signal {
                data: serde_json::Value::String("123".to_string()),
                sender: "uuid-a".to_string(),
            }
        );
    }
}
