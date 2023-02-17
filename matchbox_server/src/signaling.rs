use crate::error::{ClientRequestError, ServerError};
use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Error;
use futures::{lock::Mutex, stream::SplitSink, StreamExt};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{error, info, warn};

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

#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SessionId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RequestedSession {
    id: SessionId,
    next: Option<usize>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct QueryParam {
    next: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Peer {
    pub uuid: PeerId,
    pub session: RequestedSession,
    pub sender: tokio::sync::mpsc::UnboundedSender<std::result::Result<Message, Error>>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct ServerState {
    clients: HashMap<PeerId, Peer>,
    sessions: HashMap<RequestedSession, HashSet<PeerId>>,
}

impl ServerState {
    /// Add a peer, returning the peers already in the session
    fn add_peer(&mut self, peer: Peer) -> Vec<PeerId> {
        let peer_id = peer.uuid.clone();
        let session = peer.session.clone();
        self.clients.insert(peer.uuid.clone(), peer);
        let peers = self.sessions.entry(session.clone()).or_default();

        let ret = peers.iter().cloned().collect();
        match session.next {
            None => {
                peers.insert(peer_id);
                ret
            }
            Some(num_players) => {
                if peers.len() == num_players - 1 {
                    peers.clear(); // the session is complete, we can forget about it now
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
            // Best effort to remove peer from server state
            _ = self
                .sessions
                .get_mut(&peer.session)
                .map(|session| session.remove(peer_id));
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
    session_id: Option<Path<SessionId>>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<Mutex<ServerState>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    info!("`{addr}` connected.");

    let session_id = session_id.map(|path| path.0).unwrap_or_default();
    let next = params
        .get("next")
        .and_then(|next| next.parse::<usize>().ok());

    // Finalize the upgrade process by returning upgrade callback to client
    ws.on_upgrade(move |websocket| {
        handle_ws(
            websocket,
            state,
            RequestedSession {
                id: session_id,
                next,
            },
        )
    })
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
    requested_session: RequestedSession,
) {
    let (ws_sender, mut ws_receiver) = websocket.split();
    let sender = spawn_sender_task(ws_sender);
    let mut peer_uuid = None;

    // The state machine for the data channel established for this websocket.
    while let Some(request) = ws_receiver.next().await {
        let request = match parse_request(request) {
            Ok(request) => request,
            Err(ClientRequestError::Axum(e)) => {
                // Most likely a ConnectionReset or similar.
                error!("Axum error while receiving request: {:?}", e);
                if let Some(ref peer_uuid) = peer_uuid {
                    warn!("Severing connection with {peer_uuid}")
                }
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
            PeerRequest::Uuid(id) => {
                if peer_uuid.is_some() {
                    error!("client set uuid more than once");
                    continue;
                }

                peer_uuid.replace(id.clone());
                let mut state = state.lock().await;
                let peers = state.add_peer(Peer {
                    uuid: id.clone(),
                    sender: sender.clone(),
                    session: requested_session.clone(),
                });

                let event_text = serde_json::to_string(&PeerEvent::NewPeer(id.clone()))
                    .expect("error serializing message");
                let event = Message::Text(event_text.clone());

                for peer_id in peers {
                    // Tell everyone about this new peer
                    if let Err(e) = state.try_send(&peer_id, event.clone()) {
                        error!("error sending to {peer_id}: {e:?}");
                    } else {
                        info!("{:?} -> {:?}", peer_id, event_text);
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
                let event = Message::Text(
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
        if state.remove_peer(&uuid).is_none() {
            error!("couldn't remove {uuid}, not found");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::signaling::PeerEvent;
    use axum::routing::get;
    use axum::Router;
    use futures::lock::Mutex;
    use futures::{pin_mut, SinkExt, StreamExt};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::{net::TcpStream, select, time};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

    use super::{ws_handler, ServerState};

    fn app() -> Router {
        Router::new()
            .route("/:session_id", get(ws_handler))
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

    #[tokio::test]
    async fn ws_connect() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        tokio_tungstenite::connect_async(format!("ws://{addr}/session_a"))
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn new_peer() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_a"))
                .await
                .unwrap();

        _ = client_a
            .send(Message::Text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_a"))
                .await
                .unwrap();

        _ = client_b
            .send(Message::Text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        let new_peer_event = recv_peer_event(&mut client_a).await;

        assert_eq!(new_peer_event, PeerEvent::NewPeer("uuid-b".to_string()));
    }

    #[tokio::test]
    async fn signal() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_a"))
                .await
                .unwrap();

        _ = client_a
            .send(Message::Text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_a"))
                .await
                .unwrap();

        _ = client_b
            .send(Message::Text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

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
                sender: "uuid-a".to_string(),
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
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_name?next=2"))
                .await
                .unwrap();
        _ = client_a
            .send(Message::Text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_name?next=2"))
                .await
                .unwrap();
        _ = client_b
            .send(Message::Text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_name?next=2"))
                .await
                .unwrap();
        _ = client_c
            .send(Message::Text(r#"{"Uuid": "uuid-c"}"#.to_string()))
            .await;

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_name?next=2"))
                .await
                .unwrap();
        _ = client_d
            .send(Message::Text(r#"{"Uuid": "uuid-d"}"#.to_string()))
            .await;

        // Clients should be matched in pairs as they arrive, i.e. a + b and c + d
        let new_peer_b = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_c).await;

        assert_eq!(new_peer_b, PeerEvent::NewPeer("uuid-b".to_string()));
        assert_eq!(new_peer_d, PeerEvent::NewPeer("uuid-d".to_string()));

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
    async fn match_pair_and_other_alone_session_without_next() {
        let server = axum::Server::bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .serve(app().into_make_service_with_connect_info::<SocketAddr>());
        let addr = server.local_addr();
        tokio::spawn(server);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_name?next=2"))
                .await
                .unwrap();
        _ = client_a
            .send(Message::Text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_name"))
                .await
                .unwrap();
        _ = client_b
            .send(Message::Text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/session_name?next=2"))
                .await
                .unwrap();
        _ = client_c
            .send(Message::Text(r#"{"Uuid": "uuid-c"}"#.to_string()))
            .await;
        // Clients should be matched in pairs as they arrive, i.e. a + b and c + d
        let new_peer_c = recv_peer_event(&mut client_a).await;

        assert_eq!(new_peer_c, PeerEvent::NewPeer("uuid-c".to_string()));

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

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_2?next=2"))
                .await
                .unwrap();

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_2?next=2"))
                .await
                .unwrap();

        _ = client_a
            .send(Message::Text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;
        _ = client_c
            .send(Message::Text(r#"{"Uuid": "uuid-c"}"#.to_string()))
            .await;
        _ = client_b
            .send(Message::Text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;
        _ = client_d
            .send(Message::Text(r#"{"Uuid": "uuid-d"}"#.to_string()))
            .await;

        // Clients should be matched in pairs as they arrive, i.e. a + c and b + d
        let new_peer_c = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_b).await;

        assert_eq!(new_peer_c, PeerEvent::NewPeer("uuid-c".to_string()));
        assert_eq!(new_peer_d, PeerEvent::NewPeer("uuid-d".to_string()));

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

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        let (mut client_e, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        _ = client_a
            .send(Message::Text(r#"{"Uuid": "uuid-a"}"#.to_string()))
            .await;
        _ = client_c
            .send(Message::Text(r#"{"Uuid": "uuid-c"}"#.to_string()))
            .await;
        _ = client_b
            .send(Message::Text(r#"{"Uuid": "uuid-b"}"#.to_string()))
            .await;
        _ = client_d
            .send(Message::Text(r#"{"Uuid": "uuid-d"}"#.to_string()))
            .await;
        _ = client_e
            .send(Message::Text(r#"{"Uuid": "uuid-e"}"#.to_string()))
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
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = client_d.next() => panic!("unexpected message"),
            _ = client_e.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
}
