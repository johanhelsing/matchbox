use axum::{
    extract::{ws::WebSocket, ConnectInfo, Path, Query, State, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::lock::Mutex;
use matchbox_protocol::PeerId;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use tracing::info;

use super::state::SignalingState;

pub struct WsExtract {
    addr: SocketAddr,
    path: Option<String>,
    query_params: Option<HashMap<String, String>>,
    state: Arc<Mutex<SignalingState>>,
}

#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RoomId(String);

/// The handler for the HTTP request to upgrade to WebSockets.
/// This is the last point where we can extract TCP/IP metadata such as IP address of the client.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    path: Option<Path<RoomId>>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<Mutex<SignalingState>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    info!("`{addr}` connected.");

    let path = path.map(|path| path.0 .0);
    let query_params = Some(params);
    let extract = WsExtract {
        addr,
        path,
        query_params,
        state,
    };

    // Finalize the upgrade process by returning upgrade callback to client
    ws.on_upgrade(move |websocket| handle_ws(websocket, extract))
}

/// One of these handlers is spawned for every web socket.
pub(crate) async fn handle_ws(websocket: WebSocket, extract: WsExtract) {
    // TODO
}
