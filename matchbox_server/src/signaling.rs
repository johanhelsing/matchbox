use std::convert::Infallible;

use futures::{stream::SplitSink, StreamExt};
use log::{error, info};
// use matchbox::*;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::{
    ws::{Message, WebSocket},
    Error, Filter, Rejection, Reply,
};

mod matchbox {
    use serde::{Deserialize, Serialize};

    /// Requests go from peer to signalling server
    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    pub enum PeerRequest<S> {
        Uuid(String),
        Signal { receiver: String, data: S },
    }

    /// Events go from signalling server to peer
    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    pub enum PeerEvent<S> {
        NewPeer(String),
        Signal { sender: String, data: S },
    }
}

type PeerRequest = matchbox::PeerRequest<serde_json::Value>;
type PeerEvent = matchbox::PeerEvent<serde_json::Value>;

use crate::{Clients, Peer};

pub fn ws_filter(clients: Clients) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::ws()
        .and(warp::any())
        .and(warp::path::param())
        .and(with_clients(clients.clone()))
        .and_then(ws_handler)
}

fn with_clients(clients: Clients) -> impl Filter<Extract = (Clients,), Error = Infallible> + Clone {
    warp::any().map(move || clients.clone())
}

pub async fn ws_handler(
    ws: warp::ws::Ws,
    _path: String,
    clients: Clients,
) -> std::result::Result<impl Reply, Rejection> {
    Ok(ws.on_upgrade(move |websocket| handle_ws(websocket, clients)))
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

async fn handle_ws(websocket: WebSocket, clients: Clients) {
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
                let mut clients = clients.lock().await;
                clients.insert(
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

                for peer in clients.iter().filter(|peer| peer.1.uuid != uuid) {
                    // Tell everyone about this new peer
                    info!("{:?} -> {:?}", peer.1.uuid, event.to_str().unwrap());
                    if let Err(e) = peer.1.sender.as_ref().unwrap().send(Ok(event.clone())) {
                        error!("error sending: {:?}", e);
                    }
                }
            }
            PeerRequest::Signal { receiver, data } => {
                let clients = clients.lock().await;
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
                if let Err(e) = clients
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
        let mut clients = clients.lock().await;
        clients.remove(&uuid).expect("Couldn't find uuid to remove");
    }
}

#[cfg(test)]
mod tests {
    use warp::ws::Message;

    use crate::{new_clients, signaling::PeerEvent};

    #[tokio::test]
    async fn ws_connect() {
        let _ = pretty_env_logger::try_init();

        let route = super::ws_filter(new_clients());
        // let req = warp::test::ws().path("/echo");
        warp::test::ws()
            .path("/room_a")
            .handshake(route)
            // .handshake(ws_echo())
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn new_peer() {
        let _ = pretty_env_logger::try_init();

        let route = super::ws_filter(new_clients());

        // let req = warp::test::ws().path("/echo");
        let mut client_a = warp::test::ws()
            .path("/room_a")
            .handshake(route.clone())
            // .handshake(ws_echo())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let mut client_b = warp::test::ws()
            .path("/room_a")
            .handshake(route)
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

        let route = super::ws_filter(new_clients());

        // let req = warp::test::ws().path("/echo");
        let mut client_a = warp::test::ws()
            .path("/room_a")
            .handshake(route.clone())
            // .handshake(ws_echo())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let mut client_b = warp::test::ws()
            .path("/room_a")
            .handshake(route)
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
                data: "123".to_string(),
                sender: "uuid-a".to_string(),
            }
        );
    }
}
