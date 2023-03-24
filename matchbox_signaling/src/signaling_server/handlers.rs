use crate::{
    signaling_server::callbacks::Callbacks,
    topologies::{SignalingStateMachine, SignalingTopology},
};
use axum::{
    extract::{ws::WebSocket, ConnectInfo, Path, Query, State, WebSocketUpgrade},
    response::IntoResponse,
    Extension,
};
use futures::{future::BoxFuture, lock::Mutex, Future};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tracing::info;

pub struct WsStateMeta<State> {
    pub ws: WebSocket,
    pub upgrade_meta: WsUpgradeMeta,
    pub state: Arc<Mutex<State>>,
    pub callbacks: Callbacks,
}

/// Metadata generated at the time of websocket upgrade
pub struct WsUpgradeMeta {
    pub origin: SocketAddr,
    pub path: Option<String>,
    pub query_params: Option<HashMap<String, String>>,
}

/// The handler for the HTTP request to upgrade to WebSockets.
/// This is the last point where we can extract metadata such as IP address of the client.
pub(crate) async fn ws_handler<S>(
    ws: WebSocketUpgrade,
    path: Option<Path<String>>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<Mutex<S>>>,
    Extension(callbacks): Extension<Callbacks>,
    Extension(state_machine): Extension<SignalingStateMachine<S>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    info!("`{addr}` connected.");

    let path = path.map(|path| path.0);
    let query_params = Some(params);
    let extract = WsUpgradeMeta {
        origin: addr,
        path,
        query_params,
    };

    // Finalize the upgrade process by returning upgrade callback to client
    ws.on_upgrade(move |ws| {
        let upgrade = WsStateMeta {
            ws,
            upgrade_meta: extract,
            state,
            callbacks,
        };
        (*state_machine.0)(upgrade)
    })
}
