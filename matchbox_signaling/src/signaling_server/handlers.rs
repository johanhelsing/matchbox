use crate::{
    signaling_server::{callbacks::SharedCallbacks, SignalingState},
    topologies::SignalingStateMachine,
    SignalingCallbacks,
};
use axum::{
    extract::{ws::WebSocket, ConnectInfo, Path, Query, State, WebSocketUpgrade},
    response::IntoResponse,
    Extension,
};
use std::{collections::HashMap, net::SocketAddr};
use tracing::info;

pub struct WsStateMeta<Cb, S> {
    pub ws: WebSocket,
    pub upgrade_meta: WsUpgradeMeta,
    pub shared_callbacks: SharedCallbacks,
    pub callbacks: Cb,
    pub state: S,
}

/// Metadata generated at the time of websocket upgrade
pub struct WsUpgradeMeta {
    pub origin: SocketAddr,
    pub path: Option<String>,
    pub query_params: Option<HashMap<String, String>>,
}

/// The handler for the HTTP request to upgrade to WebSockets.
/// This is the last point where we can extract metadata such as IP address of the client.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ws_handler<Cb, S>(
    ws: WebSocketUpgrade,
    path: Option<Path<String>>,
    Query(params): Query<HashMap<String, String>>,
    Extension(shared_callbacks): Extension<SharedCallbacks>,
    Extension(callbacks): Extension<Cb>,
    State(state): State<S>,
    Extension(state_machine): Extension<SignalingStateMachine<Cb, S>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse
where
    Cb: SignalingCallbacks,
    S: SignalingState,
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
            shared_callbacks,
            callbacks,
            state,
        };
        (*state_machine.0)(upgrade)
    })
}
