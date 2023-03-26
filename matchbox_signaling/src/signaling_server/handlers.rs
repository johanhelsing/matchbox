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
use hyper::StatusCode;
use std::{collections::HashMap, net::SocketAddr};
use tracing::info;

/// Metastate used during by a signaling server's runtime
pub struct WsStateMeta<Cb, S> {
    pub ws: WebSocket,
    pub callbacks: Cb,
    pub state: S,
}

/// Metadata captured at the time of websocket upgrade
#[derive(Debug)]
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

    // Lifecycle event: On Upgrade
    let allow_cxn = shared_callbacks.on_upgrade.emit(extract);

    // Finalize the upgrade process by returning upgrade callback to client
    if allow_cxn {
        ws.on_upgrade(move |ws| {
            let meta = WsStateMeta {
                ws,
                callbacks,
                state,
            };
            (*state_machine.0)(meta)
        })
    } else {
        (StatusCode::UNAUTHORIZED).into_response()
    }
}
