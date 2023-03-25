use crate::{signaling_server::callbacks::Callbacks, topologies::SignalingStateMachine};
use axum::{
    extract::{ws::WebSocket, ConnectInfo, Path, Query, WebSocketUpgrade},
    response::IntoResponse,
    Extension,
};
use std::{collections::HashMap, net::SocketAddr};
use tracing::info;

use super::server::SignalingState;

pub struct WsStateMeta<State> {
    pub ws: WebSocket,
    pub upgrade_meta: WsUpgradeMeta,
    pub state: State,
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
    Extension(state): Extension<S>,
    Extension(callbacks): Extension<Callbacks>,
    Extension(state_machine): Extension<SignalingStateMachine<S>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse
where
    S: SignalingState + 'static,
{
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
