use crate::{
    signaling_server::{callbacks::Callbacks, state::SignalingState},
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

pub struct WsUpgrade {
    pub ws: WebSocket,
    pub extract: WsExtract,
    pub state: Arc<Mutex<SignalingState>>,
    pub callbacks: Callbacks,
}

pub struct WsExtract {
    pub addr: SocketAddr,
    pub path: Option<String>,
    pub query_params: Option<HashMap<String, String>>,
}

/// The handler for the HTTP request to upgrade to WebSockets.
/// This is the last point where we can extract metadata such as IP address of the client.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    path: Option<Path<String>>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<Mutex<SignalingState>>>,
    Extension(callbacks): Extension<Callbacks>,
    Extension(state_machine): Extension<SignalingStateMachine>,
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
    ws.on_upgrade(move |ws| {
        let upgrade = WsUpgrade {
            ws,
            extract,
            state,
            callbacks,
        };
        (*state_machine.0)(upgrade)
    })
}
