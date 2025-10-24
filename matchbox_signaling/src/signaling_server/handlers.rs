use crate::{
    SignalingCallbacks,
    signaling_server::{SignalingState, callbacks::SharedCallbacks},
    topologies::{
        SignalingStateMachine,
        common_logic::{SignalingChannel, spawn_sender_task, try_send},
    },
};
use axum::{
    Extension,
    extract::{
        ConnectInfo, Path, Query, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::{StreamExt, stream::SplitStream};
use hyper::{HeaderMap, StatusCode};
use matchbox_protocol::{JsonPeerEvent, PeerId};
use std::{collections::HashMap, net::SocketAddr};
use tracing::{error, info};

/// Metastate used during by a signaling server's runtime
pub struct WsStateMeta<Cb, S> {
    /// The peer connecting, by their ID
    pub peer_id: PeerId,
    /// The channel to signal this peer through
    pub sender: SignalingChannel,
    /// The receiver to receive from this peer through
    pub receiver: SplitStream<WebSocket>,
    /// Callbacks associated with the topology
    pub callbacks: Cb,
    /// State associated with the topology
    pub state: S,
}

/// Metadata captured at the time of websocket upgrade
#[derive(Debug, Clone)]
pub struct WsUpgradeMeta {
    pub origin: SocketAddr,
    pub path: Option<String>,
    pub query_params: HashMap<String, String>,
    pub headers: HeaderMap,
}

/// The handler for the HTTP request to upgrade to WebSockets.
/// This is the last point where we can extract metadata such as IP address of the client.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ws_handler<Cb, S>(
    ws: WebSocketUpgrade,
    path: Option<Path<String>>,
    headers: HeaderMap,
    Query(query_params): Query<HashMap<String, String>>,
    Extension(shared_callbacks): Extension<SharedCallbacks>,
    Extension(callbacks): Extension<Cb>,
    Extension(state): Extension<S>,
    Extension(state_machine): Extension<SignalingStateMachine<Cb, S>>,
    ConnectInfo(origin): ConnectInfo<SocketAddr>,
) -> impl IntoResponse
where
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    info!("`{origin}` connected.");

    let path = path.map(|path| path.0);
    let meta = WsUpgradeMeta {
        origin,
        path,
        query_params,
        headers,
    };

    // Lifecycle event: On Connection Request
    match shared_callbacks.on_connection_request.emit(meta) {
        Ok(true) => {}
        Ok(false) => return (StatusCode::UNAUTHORIZED).into_response(),
        Err(e) => return e,
    };

    // Finalize the upgrade process by returning upgrade callback to client
    // Generate an ID for the peer
    let peer_id = uuid::Uuid::new_v4().into();

    // Lifecycle event: On ID Assignment
    shared_callbacks.on_id_assignment.emit((origin, peer_id));

    ws.on_upgrade(move |ws| {
        let (ws_sink, receiver) = ws.split();
        let sender = spawn_sender_task(ws_sink);

        // Send ID to peer
        let event_text = JsonPeerEvent::IdAssigned(peer_id).to_string();
        let event = Message::Text((&event_text).into());
        if let Err(e) = try_send(&sender, event) {
            error!("error sending to {peer_id}: {e:?}");
        } else {
            info!("{peer_id} -> {event_text}");
        };

        let meta = WsStateMeta {
            peer_id,
            sender,
            receiver,
            callbacks,
            state,
        };
        (*state_machine.0)(meta)
    })
}
