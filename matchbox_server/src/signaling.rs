use crate::error::{ClientRequestError, ServerError};
use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    response::IntoResponse,
    Error,
};
use futures::{lock::Mutex, stream::SplitSink, StreamExt};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{error, info, warn};

pub mod matchbox {
    use serde::{Deserialize, Serialize};

    pub type PeerId = String;

    /// Requests go from peer to signalling server
    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    pub enum PeerRequest<S> {
        Signal { receiver: PeerId, data: S },
        KeepAlive,
    }

    /// Events go from signalling server to peer
    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    pub enum PeerEvent<S> {
        IdAssigned(PeerId),
        NewPeer(PeerId),
        PeerLeft(PeerId),
        Signal { sender: PeerId, data: S },
    }
}
use matchbox::*;

type PeerRequest = matchbox::PeerRequest<serde_json::Value>;
type PeerEvent = matchbox::PeerEvent<serde_json::Value>;

#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone)]
pub(crate) struct Peer {
    pub uuid: PeerId,
    pub room: RequestedRoom,
    pub sender: tokio::sync::mpsc::UnboundedSender<std::result::Result<Message, Error>>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct ServerState {
    clients: HashMap<PeerId, Peer>,
    rooms: HashMap<RequestedRoom, HashSet<PeerId>>,
}

impl ServerState {
    /// Add a peer, returning the peers already in room
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

    /// Remove a peer from the state if it existed, returning the peer removed.
    #[must_use]
    fn remove_peer(&mut self, peer_id: &PeerId) -> Option<Peer> {
        let peer = self.clients.remove(peer_id);

        if let Some(ref peer) = peer {
            // Best effort to remove peer from their room
            _ = self
                .rooms
                .get_mut(&peer.room)
                .map(|room| room.remove(peer_id));
        }
        peer
    }

    /// Send a message to a peer without blocking.
    fn try_send(&self, id: &PeerId, message: Message) -> Result<(), ServerError> {
        let peer = self.clients.get(id);
        let peer = match peer {
            Some(peer) => peer,
            None => {
                return Err(ServerError::UnknownPeer);
            }
        };

        peer.sender.send(Ok(message)).map_err(ServerError::from)
    }
}

/// The handler for the HTTP request to upgrade to WebSockets.
/// This is the last point where we can extract TCP/IP metadata such as IP address of the client.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    room_id: Option<Path<RoomId>>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<Mutex<ServerState>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    info!("`{addr}` connected.");

    let room_id = room_id.map(|path| path.0).unwrap_or_default();
    let next = params
        .get("next")
        .and_then(|next| next.parse::<usize>().ok());

    // Finalize the upgrade process by returning upgrade callback to client
    ws.on_upgrade(move |websocket| handle_ws(websocket, state, RequestedRoom { id: room_id, next }))
}

fn parse_request(request: Result<Message, Error>) -> Result<PeerRequest, ClientRequestError> {
    let request = request?;

    let request: PeerRequest = match request {
        Message::Text(text) => serde_json::from_str(&text)?,
        Message::Close(_) => return Err(ClientRequestError::Close),
        _ => return Err(ClientRequestError::UnsupportedType),
    };

    Ok(request)
}

fn spawn_sender_task(
    sender: SplitSink<WebSocket, Message>,
) -> mpsc::UnboundedSender<Result<Message, Error>> {
    let (client_sender, receiver) = mpsc::unbounded_channel();
    tokio::task::spawn(UnboundedReceiverStream::new(receiver).forward(sender));
    client_sender
}

/// One of these handlers is spawned for every web socket.
async fn handle_ws(
    websocket: WebSocket,
    state: Arc<Mutex<ServerState>>,
    requested_room: RequestedRoom,
) {
    let (ws_sender, mut ws_receiver) = websocket.split();
    let sender = spawn_sender_task(ws_sender);

    let peer_uuid = uuid::Uuid::new_v4().to_string();

    {
        let mut add_peer_state = state.lock().await;

        let peers = add_peer_state.add_peer(Peer {
            uuid: peer_uuid.clone(),
            sender: sender.clone(),
            room: requested_room.clone(),
        });

        let event_text = serde_json::to_string(&PeerEvent::IdAssigned(peer_uuid.clone()))
            .expect("error serializing message");
        let event = Message::Text(event_text.clone());

        if let Err(e) = add_peer_state.try_send(&peer_uuid, event) {
            error!("error sending to {peer_uuid}: {e:?}");
        } else {
            info!("{:?} -> {:?}", peer_uuid, event_text);
        };

        let event_text = serde_json::to_string(&PeerEvent::NewPeer(peer_uuid.clone()))
            .expect("error serializing message");
        let event = Message::Text(event_text.clone());

        for peer_id in peers {
            // Tell everyone about this new peer
            if let Err(e) = add_peer_state.try_send(&peer_id, event.clone()) {
                error!("error sending to {peer_id}: {e:?}");
            } else {
                info!("{:?} -> {:?}", peer_id, event_text);
            }
        }
    }

    // The state machine for the data channel established for this websocket.
    while let Some(request) = ws_receiver.next().await {
        let request = match parse_request(request) {
            Ok(request) => request,
            Err(ClientRequestError::Axum(e)) => {
                // Most likely a ConnectionReset or similar.
                error!("Axum error while receiving request: {:?}", e);
                warn!("Severing connection with {peer_uuid}");
                break; // give up on this peer.
            }
            Err(ClientRequestError::Close) => {
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
            PeerRequest::Signal { receiver, data } => {
                let event = Message::Text(
                    serde_json::to_string(&PeerEvent::Signal {
                        sender: peer_uuid.clone(),
                        data,
                    })
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
            PeerRequest::KeepAlive => {
                // Do nothing. KeepAlive packets are used to protect against users' browsers
                // disconnecting idle websocket connections.
            }
        }
    }

    // Peer disconnected or otherwise ended communication.
    info!("Removing peer: {:?}", peer_uuid);
    let mut state = state.lock().await;
    if let Some(removed_peer) = state.remove_peer(&peer_uuid) {
        let room = removed_peer.room;
        let peers = state
            .rooms
            .get(&room)
            .map(|room_peers| {
                room_peers
                    .iter()
                    .filter(|peer_id| *peer_id != &peer_uuid)
                    .collect::<Vec<&String>>()
            })
            .unwrap_or_default();
        // Tell each connected peer about the disconnected peer.
        let event = Message::Text(
            serde_json::to_string(&PeerEvent::PeerLeft(removed_peer.uuid))
                .expect("error serializing message"),
        );
        for peer_id in peers {
            match state.try_send(peer_id, event.clone()) {
                Ok(()) => info!("Sent peer remove to: {:?}", peer_id),
                Err(e) => error!("Failure sending peer remove: {e:?}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{signaling::PeerEvent, ws_handler, PeerId, ServerState};
    use axum::{routing::get, Router};
    use futures::{lock::Mutex, pin_mut, SinkExt, StreamExt};
    use std::{
        net::{Ipv4Addr, SocketAddr},
        sync::Arc,
        time::Duration,
    };
    use tokio::{net::TcpStream, select, time};
    use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

    fn app() -> Router {
        Router::new()
            .route("/:room_id", get(ws_handler))
            .with_state(Arc::new(Mutex::new(ServerState::default())))
    }

    // Helper to take the next PeerEvent from a stream
    async fn recv_peer_event(client: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> PeerEvent {
        let message: Message = client
            .next()
            .await
            .expect("some message")
            .expect("socket message");
        serde_json::from_str(&message.to_string()).expect("json peer event")
    }

    fn get_peer_id(peer_event: PeerEvent) -> PeerId {
        if let PeerEvent::IdAssigned(id) = peer_event {
            id
        } else {
            panic!("Peer_event was not IdAssigned: {peer_event:?}");
        }
    }

    #[tokio::test]
    async fn ws_connect() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn uuid_assigned() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let id_assigned_event = recv_peer_event(&mut client).await;

        assert!(matches!(id_assigned_event, PeerEvent::IdAssigned(..)));
    }

    #[tokio::test]
    async fn new_peer() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let new_peer_event = recv_peer_event(&mut client_a).await;

        assert_eq!(new_peer_event, PeerEvent::NewPeer(b_uuid));
    }

    #[tokio::test]
    async fn disconnect_peer() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        // Ensure Peer B was received
        let new_peer_event = recv_peer_event(&mut client_a).await;
        assert_eq!(new_peer_event, PeerEvent::NewPeer(b_uuid.clone()));

        // Disconnect Peer B
        _ = client_b.close(None).await;
        let peer_left_event = recv_peer_event(&mut client_a).await;

        assert_eq!(peer_left_event, PeerEvent::PeerLeft(b_uuid));
    }

    #[tokio::test]
    async fn signal() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let new_peer_event = recv_peer_event(&mut client_a).await;
        let peer_uuid = match new_peer_event {
            PeerEvent::NewPeer(peer) => peer,
            _ => panic!("unexpected event"),
        };

        _ = client_a
            .send(Message::text(format!(
                "{{\"Signal\": {{\"receiver\": \"{peer_uuid}\", \"data\": \"123\" }}}}"
            )))
            .await;

        let signal_event = recv_peer_event(&mut client_b).await;
        assert_eq!(
            signal_event,
            PeerEvent::Signal {
                data: serde_json::Value::String("123".to_string()),
                sender: a_uuid,
            }
        );
    }

    #[tokio::test]
    async fn match_pairs() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let _c_uuid = get_peer_id(recv_peer_event(&mut client_c).await);

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let d_uuid = get_peer_id(recv_peer_event(&mut client_d).await);

        // Clients should be matched in pairs as they arrive, i.e. a + b and c + d
        let new_peer_b = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_c).await;

        assert_eq!(new_peer_b, PeerEvent::NewPeer(b_uuid));
        assert_eq!(new_peer_d, PeerEvent::NewPeer(d_uuid));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = client_d.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
    #[tokio::test]
    async fn match_pair_and_other_alone_room_without_next() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let c_uuid = get_peer_id(recv_peer_event(&mut client_c).await);

        // Clients should be matched in pairs as they arrive, i.e. a + b and c + d
        let new_peer_c = recv_peer_event(&mut client_a).await;

        assert_eq!(new_peer_c, PeerEvent::NewPeer(c_uuid));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }

    #[tokio::test]
    async fn match_different_id_same_next() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_2?next=2"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let c_uuid = get_peer_id(recv_peer_event(&mut client_c).await);

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_2?next=2"))
                .await
                .unwrap();

        let d_uuid = get_peer_id(recv_peer_event(&mut client_d).await);

        // Clients should be matched in pairs as they arrive, i.e. a + c and b + d
        let new_peer_c = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_b).await;

        assert_eq!(new_peer_c, PeerEvent::NewPeer(c_uuid));
        assert_eq!(new_peer_d, PeerEvent::NewPeer(d_uuid));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = client_d.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
    #[tokio::test]
    async fn match_same_id_different_next() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let c_uuid = get_peer_id(recv_peer_event(&mut client_c).await);

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        let d_uuid = get_peer_id(recv_peer_event(&mut client_d).await);

        let (mut client_e, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        let e_uuid = get_peer_id(recv_peer_event(&mut client_e).await);

        // Clients should be matched in pairs as they arrive, i.e. a + c and (b + d ; b + e ; d + e)
        let new_peer_c = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_b).await;
        let new_peer_e = recv_peer_event(&mut client_b).await;
        assert_eq!(new_peer_e, PeerEvent::NewPeer(e_uuid.clone()));
        let new_peer_e = recv_peer_event(&mut client_d).await;

        assert_eq!(new_peer_c, PeerEvent::NewPeer(c_uuid));
        assert_eq!(new_peer_d, PeerEvent::NewPeer(d_uuid.clone()));
        assert_eq!(new_peer_d, PeerEvent::NewPeer(d_uuid));
        assert_eq!(new_peer_e, PeerEvent::NewPeer(e_uuid));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = client_d.next() => panic!("unexpected message"),
            _ = client_e.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
}
