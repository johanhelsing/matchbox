use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Error;
use futures::{lock::Mutex, stream::SplitSink, StreamExt};
use log::{error, info, warn};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

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

/// The handler for the HTTP request to upgrade to WebSockets.
/// This is the last point where we can extract TCP/IP metadata such as IP address of the client.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    room_id: Option<Path<RoomId>>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<Mutex<ServerState>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    println!("`{addr}` connected.");

    let room_id = room_id.map(|path| path.0).unwrap_or_default();
    let next = params
        .get("next")
        .and_then(|next| next.parse::<usize>().ok());

    // Finalize the upgrade process by returning upgrade callback to client
    ws.on_upgrade(move |websocket| handle_ws(websocket, state, RequestedRoom { id: room_id, next }))
}

#[derive(Debug, thiserror::Error)]
enum RequestError {
    #[error("Axum error")]
    Axum(#[from] axum::Error),
    #[error("Not text error")]
    NotText,
    #[error("Message is close")]
    Close,
    #[error("Json error")]
    Json(#[from] serde_json::Error),
}

fn parse_request(request: Result<Message, Error>) -> Result<PeerRequest, RequestError> {
    let request = request?;

    let request: PeerRequest = match request {
        Message::Text(text) => serde_json::from_str(&text)?,
        Message::Binary(_) => return Err(RequestError::NotText),
        Message::Close(_) => return Err(RequestError::Close),
        _ => unimplemented!("unsupported message"),
    };

    Ok(request)
}

fn spawn_sender_task(
    sender: SplitSink<WebSocket, Message>,
) -> mpsc::UnboundedSender<std::result::Result<Message, Error>> {
    let (client_sender, receiver) = mpsc::unbounded_channel();
    tokio::task::spawn(UnboundedReceiverStream::new(receiver).forward(sender));
    client_sender
}

async fn handle_ws(
    websocket: WebSocket,
    state: Arc<Mutex<ServerState>>,
    requested_room: RequestedRoom,
) {
    let (ws_sender, mut ws_receiver) = websocket.split();
    let sender = spawn_sender_task(ws_sender);
    let mut peer_uuid = None;

    while let Some(request) = ws_receiver.next().await {
        let request = match parse_request(request) {
            Ok(request) => request,
            Err(RequestError::Axum(e)) => {
                error!("Axum error while receiving request: {:?}", e);
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

                let event_text = serde_json::to_string(&PeerEvent::NewPeer(id.clone()))
                    .expect("error serializing message");
                let event = Message::Text(event_text.clone());

                for peer_id in peers {
                    // Tell everyone about this new peer
                    info!("{:?} -> {:?}", peer_id, event_text);
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
        state.remove_peer(&uuid);
    }
}
