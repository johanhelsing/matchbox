use futures::{lock::Mutex, stream::SplitSink, StreamExt};
use log::{error, info, warn};
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    sync::Arc,
};
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
        KeepAlive,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RoomId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RequestedRoom {
    id: RoomId,
    next: Option<usize>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct QueryParam {
    next: Option<usize>,
}

pub(crate) struct Peer {
    pub uuid: PeerId,
    pub room: RequestedRoom,
    pub sender: tokio::sync::mpsc::UnboundedSender<std::result::Result<Message, warp::Error>>,
}

#[derive(Default)]
pub(crate) struct State {
    clients: HashMap<PeerId, Peer>,
    rooms: HashMap<RequestedRoom, HashSet<PeerId>>,
}

impl State {
    /// Returns peers already in room
    fn add_peer(&mut self, peer: Peer) -> Vec<PeerId> {
        let peer_id = peer.uuid.clone();
        let room = peer.room.clone();
        self.clients.insert(peer.uuid.clone(), peer);
        let peers = self.rooms.entry(room.clone()).or_default();

        let ret = peers.iter().cloned().collect();
        match room.next {
            None => {
                peers.insert(peer_id);
                ret
            }
            Some(num_players) => {
                if peers.len() == num_players - 1 {
                    peers.clear(); // the room is complete, we can forget about it now
                } else {
                    peers.insert(peer_id);
                }
                ret
            }
        }
    }

    fn remove_peer(&mut self, peer_id: &PeerId) {
        let peer = self
            .clients
            .remove(peer_id)
            .expect("Couldn't find uuid to remove");

        let room_peers = self.rooms.get_mut(&peer.room);

        if let Some(room_peers) = room_peers {
            room_peers.remove(peer_id);
        }
    }

    fn try_send(&self, id: &PeerId, message: Message) {
        let peer = self.clients.get(id);
        let peer = match peer {
            Some(peer) => peer,
            None => {
                error!("Unknown peer {:?}", id);
                return;
            }
        };
        if let Err(e) = peer.sender.send(Ok(message)) {
            error!("Error sending message {:?}", e);
        }
    }
}

fn parse_room_id(id: String) -> RoomId {
    RoomId(id)
}

pub(crate) fn ws_filter(
    state: Arc<Mutex<State>>,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::ws()
        .and(warp::any())
        .and(warp::path::param().map(parse_room_id))
        .and(warp::query::<QueryParam>().map(parse_room_next))
        .and(with_state(state))
        .and_then(ws_handler)
}

fn parse_room_next(p: QueryParam) -> Option<usize> {
    p.next
}

fn with_state(
    state: Arc<Mutex<State>>,
) -> impl Filter<Extract = (Arc<Mutex<State>>,), Error = Infallible> + Clone {
    warp::any().map(move || state.clone())
}

pub(crate) async fn ws_handler(
    ws: warp::ws::Ws,
    room_id: RoomId,
    next: Option<usize>,
    state: Arc<Mutex<State>>,
) -> std::result::Result<impl Reply, Rejection> {
    Ok(ws.on_upgrade(move |websocket| {
        handle_ws(websocket, state, RequestedRoom { id: room_id, next })
    }))
}

#[derive(Debug, thiserror::Error)]
enum RequestError {
    #[error("Warp error")]
    Warp(#[from] warp::Error),
    #[error("Not text error")]
    NotText,
    #[error("Message is close")]
    Close,
    #[error("Json error")]
    Json(#[from] serde_json::Error),
}

fn parse_request(request: Result<Message, Error>) -> Result<PeerRequest, RequestError> {
    let request = request?;

    if request.is_close() {
        return Err(RequestError::Close);
    }

    if !request.is_text() {
        return Err(RequestError::NotText);
    }

    let request = request.to_str().map_err(|_| RequestError::NotText)?;

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

async fn handle_ws(websocket: WebSocket, state: Arc<Mutex<State>>, requested_room: RequestedRoom) {
    let (ws_sender, mut ws_receiver) = websocket.split();
    let sender = spawn_sender_task(ws_sender);
    let mut peer_uuid = None;

    while let Some(request) = ws_receiver.next().await {
        let request = match parse_request(request) {
            Ok(request) => request,
            Err(RequestError::Warp(e)) => {
                error!("Warp error while receiving request: {:?}", e);
                // Most likely a ConnectionReset or similar.
                // just give up on this peer.
                break;
            }
            Err(RequestError::Close) => {
                info!("Received websocket close from {peer_uuid:?}");
                break;
            }
            Err(e) => {
                error!("Error untangling request: {:?}", e);
                continue;
            }
        };

        info!("{:?} <- {:?}", peer_uuid, request);

        match request {
            PeerRequest::Uuid(id) => {
                if peer_uuid.is_some() {
                    error!("client set uuid more than once");
                    continue;
                }
                peer_uuid = Some(id.clone());

                let mut state = state.lock().await;
                let peers = state.add_peer(Peer {
                    uuid: id.clone(),
                    sender: sender.clone(),
                    room: requested_room.clone(),
                });

                let event = Message::text(
                    serde_json::to_string(&PeerEvent::NewPeer(id.clone()))
                        .expect("error serializing message"),
                );

                for peer_id in peers {
                    // Tell everyone about this new peer
                    info!("{:?} -> {:?}", peer_id, event.to_str().unwrap());
                    state.try_send(&peer_id, event.clone());
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
                if let Some(peer) = state.clients.get(&receiver) {
                    if let Err(e) = peer.sender.send(Ok(event)) {
                        error!("error sending: {:?}", e);
                    }
                } else {
                    warn!("peer not found ({receiver}), ignoring signal");
                }
            }
            PeerRequest::KeepAlive => {}
        }
    }

    info!("Removing peer: {:?}", peer_uuid);
    if let Some(uuid) = peer_uuid {
        let mut state = state.lock().await;
        state.remove_peer(&uuid);
    }
}

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use futures::pin_mut;
    use tokio::{select, time};
    use warp::{test::WsClient, ws::Message, Filter, Rejection, Reply};

    use crate::signaling::{parse_room_id, parse_room_next, PeerEvent, QueryParam, RoomId};

    fn api() -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
        super::ws_filter(Default::default())
    }

    #[tokio::test]
    async fn ws_connect() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        warp::test::ws()
            .path("/room_a")
            .handshake(api)
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn new_peer() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        let mut client_a = warp::test::ws()
            .path("/room_a")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let mut client_b = warp::test::ws()
            .path("/room_a")
            .handshake(api)
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

        let mut client_a = warp::test::ws()
            .path("/room_a")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let mut client_b = warp::test::ws()
            .path("/room_a")
            .handshake(api)
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

    async fn recv_peer_event(client: &mut WsClient) -> PeerEvent {
        let message = client.recv().await;
        serde_json::from_str(message.unwrap().to_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn match_pairs() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        let mut client_a = warp::test::ws()
            .path("/room_name?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let mut client_b = warp::test::ws()
            .path("/room_name?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_b
            .send(Message::text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        let mut client_c = warp::test::ws()
            .path("/room_name?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_c
            .send(Message::text(r#"{"Uuid": "uuid-c"}"#.to_string()))
            .await;

        let mut client_d = warp::test::ws()
            .path("/room_name?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_d
            .send(Message::text(r#"{"Uuid": "uuid-d"}"#.to_string()))
            .await;

        // Clients should be matched in pairs as they arrive, i.e. a + b and c + d
        let new_peer_b = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_c).await;

        assert_eq!(new_peer_b, PeerEvent::NewPeer("uuid-b".to_string()));
        assert_eq!(new_peer_d, PeerEvent::NewPeer("uuid-d".to_string()));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.recv() => panic!("unexpected message"),
            _ = client_b.recv() => panic!("unexpected message"),
            _ = client_c.recv() => panic!("unexpected message"),
            _ = client_d.recv() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
    #[tokio::test]
    async fn match_pair_and_other_alone_room_without_next() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        let mut client_a = warp::test::ws()
            .path("/room_name?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let mut client_b = warp::test::ws()
            .path("/room_name")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_b
            .send(Message::text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        let mut client_c = warp::test::ws()
            .path("/room_name?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_c
            .send(Message::text(r#"{"Uuid": "uuid-c"}"#.to_string()))
            .await;

        // Clients should be matched in pairs as they arrive, i.e. a + b and c + d
        let new_peer_c = recv_peer_event(&mut client_a).await;

        assert_eq!(new_peer_c, PeerEvent::NewPeer("uuid-c".to_string()));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.recv() => panic!("unexpected message"),
            _ = client_b.recv() => panic!("unexpected message"),
            _ = client_c.recv() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }

    #[tokio::test]
    async fn match_different_id_same_next() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        let mut client_a = warp::test::ws()
            .path("/scope_1?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        let mut client_b = warp::test::ws()
            .path("/scope_2?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        let mut client_c = warp::test::ws()
            .path("/scope_1?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        let mut client_d = warp::test::ws()
            .path("/scope_2?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;
        client_c
            .send(Message::text(r#"{"Uuid": "uuid-c"}"#.to_string()))
            .await;
        client_b
            .send(Message::text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        client_d
            .send(Message::text(r#"{"Uuid": "uuid-d"}"#.to_string()))
            .await;

        // Clients should be matched in pairs as they arrive, i.e. a + c and b + d
        let new_peer_c = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_b).await;

        assert_eq!(new_peer_c, PeerEvent::NewPeer("uuid-c".to_string()));
        assert_eq!(new_peer_d, PeerEvent::NewPeer("uuid-d".to_string()));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.recv() => panic!("unexpected message"),
            _ = client_b.recv() => panic!("unexpected message"),
            _ = client_c.recv() => panic!("unexpected message"),
            _ = client_d.recv() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
    #[tokio::test]
    async fn match_same_id_different_next() {
        let _ = pretty_env_logger::try_init();
        let api = api();

        let mut client_a = warp::test::ws()
            .path("/scope_1?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        let mut client_b = warp::test::ws()
            .path("/scope_1?next=3")
            .handshake(api.clone())
            .await
            .expect("handshake");

        let mut client_c = warp::test::ws()
            .path("/scope_1?next=2")
            .handshake(api.clone())
            .await
            .expect("handshake");

        let mut client_d = warp::test::ws()
            .path("/scope_1?next=3")
            .handshake(api.clone())
            .await
            .expect("handshake");

        let mut client_e = warp::test::ws()
            .path("/scope_1?next=3")
            .handshake(api.clone())
            .await
            .expect("handshake");

        client_a
            .send(Message::text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;
        client_c
            .send(Message::text(r#"{"Uuid": "uuid-c"}"#.to_string()))
            .await;
        client_b
            .send(Message::text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        client_d
            .send(Message::text(r#"{"Uuid": "uuid-d"}"#.to_string()))
            .await;

        client_e
            .send(Message::text(r#"{"Uuid": "uuid-e"}"#.to_string()))
            .await;

        // Clients should be matched in pairs as they arrive, i.e. a + c and (b + d ; b + e ; d + e)
        let new_peer_c = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_b).await;
        let new_peer_e = recv_peer_event(&mut client_b).await;
        assert_eq!(new_peer_e, PeerEvent::NewPeer("uuid-e".to_string()));
        let new_peer_e = recv_peer_event(&mut client_d).await;

        assert_eq!(new_peer_c, PeerEvent::NewPeer("uuid-c".to_string()));
        assert_eq!(new_peer_d, PeerEvent::NewPeer("uuid-d".to_string()));
        assert_eq!(new_peer_d, PeerEvent::NewPeer("uuid-d".to_string()));
        assert_eq!(new_peer_e, PeerEvent::NewPeer("uuid-e".to_string()));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.recv() => panic!("unexpected message"),
            _ = client_b.recv() => panic!("unexpected message"),
            _ = client_c.recv() => panic!("unexpected message"),
            _ = client_d.recv() => panic!("unexpected message"),
            _ = client_e.recv() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }

    #[test]
    fn requested_room() {
        assert_eq!(
            parse_room_id("room_name".into()),
            RoomId("room_name".to_string())
        );
    }
    #[test]
    fn requested_scope() {
        assert_eq!(parse_room_next(QueryParam { next: Some(3) }), Some(3));
        assert_eq!(parse_room_next(QueryParam { next: None }), None);
    }
}
