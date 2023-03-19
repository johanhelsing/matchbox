use crate::signalling_socket::{callbacks, state::Peer};

use super::{callbacks::Callbacks, error::ClientRequestError, state::SignalingState};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, Path, Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures::{lock::Mutex, stream::SplitSink, StreamExt};
use matchbox_protocol::{JsonPeerEvent, JsonPeerRequest, PeerRequest};
use std::{collections::HashMap, net::SocketAddr, str::FromStr, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{error, info, warn};

pub struct WsExtract {
    addr: SocketAddr,
    path: Option<String>,
    query_params: Option<HashMap<String, String>>,
}

/// The handler for the HTTP request to upgrade to WebSockets.
/// This is the last point where we can extract metadata such as IP address of the client.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    path: Option<Path<String>>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<Mutex<SignalingState>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    info!("`{addr}` connected.");

    let path = path.map(|path| path.0);
    let query_params = Some(params);
    let extract = WsExtract {
        addr,
        path,
        query_params,
    };

    // Finalize the upgrade process by returning upgrade callback to client
    ws.on_upgrade(move |websocket| full_mesh_ws(websocket, extract, state))
}

/// One of these handlers is spawned for every web socket.
pub(crate) async fn full_mesh_ws(
    websocket: WebSocket,
    extract: WsExtract,
    state: Arc<Mutex<SignalingState>>,
) {
    let (ws_sender, mut ws_receiver) = websocket.split();
    let sender = spawn_sender_task(ws_sender);

    let peer_uuid = uuid::Uuid::new_v4().into();

    {
        let mut add_peer_state = state.lock().await;
        let peers = add_peer_state.add_peer(Peer {
            uuid: peer_uuid,
            sender: sender.clone(),
        });

        let event_text = JsonPeerEvent::IdAssigned(peer_uuid).to_string();
        let event = Message::Text(event_text.clone());

        if let Err(e) = add_peer_state.try_send(peer_uuid, event) {
            error!("error sending to {peer_uuid:?}: {e:?}");
        } else {
            info!("{:?} -> {:?}", peer_uuid, event_text);
        };

        let event_text = JsonPeerEvent::NewPeer(peer_uuid).to_string();
        let event = Message::Text(event_text.clone());

        for (peer_id, _) in peers {
            // Tell everyone about this new peer
            if let Err(e) = add_peer_state.try_send(peer_id, event.clone()) {
                error!("error sending to {peer_id:?}: {e:?}");
            } else {
                info!("{:?} -> {:?}", peer_id, event_text);
            }
        }
    }

    // The state machine for the data channel established for this websocket.
    while let Some(request) = ws_receiver.next().await {
        let request = match parse_request(request.map_err(ClientRequestError::from)) {
            Ok(request) => request,
            Err(ClientRequestError::Axum(e)) => {
                // Most likely a ConnectionReset or similar.
                error!("Axum error while receiving request: {:?}", e);
                warn!("Severing connection with {peer_uuid:?}");
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
                    JsonPeerEvent::Signal {
                        sender: peer_uuid,
                        data,
                    }
                    .to_string(),
                );
                let state = state.lock().await;
                if let Some(peer) = state.peers.get(&receiver) {
                    if let Err(e) = peer.sender.send(Ok(event)) {
                        error!("error sending: {:?}", e);
                    }
                } else {
                    warn!("peer not found ({receiver:?}), ignoring signal");
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
        // Tell each connected peer about the disconnected peer.
        let event = Message::Text(JsonPeerEvent::PeerLeft(removed_peer.uuid).to_string());
        for (peer_id, _) in state.peers.iter() {
            match state.try_send(*peer_id, event.clone()) {
                Ok(()) => info!("Sent peer remove to: {:?}", peer_id),
                Err(e) => error!("Failure sending peer remove: {e:?}"),
            }
        }
    }
}

fn parse_request(
    request: Result<Message, ClientRequestError>,
) -> Result<JsonPeerRequest, ClientRequestError> {
    let request = request?;

    let request = match request {
        Message::Text(text) => JsonPeerRequest::from_str(&text)?,
        Message::Close(_) => return Err(ClientRequestError::Close),
        _ => return Err(ClientRequestError::UnsupportedType),
    };

    Ok(request)
}

fn spawn_sender_task(
    sender: SplitSink<WebSocket, Message>,
) -> mpsc::UnboundedSender<Result<Message, axum::Error>> {
    let (client_sender, receiver) = mpsc::unbounded_channel();
    tokio::task::spawn(UnboundedReceiverStream::new(receiver).forward(sender));
    client_sender
}
