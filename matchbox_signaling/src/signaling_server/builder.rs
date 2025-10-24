use crate::{
    SignalingCallbacks, SignalingServer, SignalingState,
    signaling_server::{
        NoCallbacks, NoState,
        callbacks::{Callback, SharedCallbacks},
        handlers::{WsUpgradeMeta, ws_handler},
    },
    topologies::{SignalingStateMachine, SignalingTopology},
};
use axum::{Extension, Router, response::Response, routing::get};
use matchbox_protocol::PeerId;
use std::{convert::identity, net::SocketAddr};
use tower_http::{
    LatencyUnit,
    cors::{Any, CorsLayer},
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;

/// Builder for [`SignalingServer`]s.
///
/// Begin with [`SignalingServerBuilder::new`] and add parameters before calling
/// [`SignalingServerBuilder::build`] to produce the desired [`SignalingServer`].
pub struct SignalingServerBuilder<Topology, Cb = NoCallbacks, S = NoState>
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,

    /// The router used by the signaling server
    pub(crate) router: Router,

    /// Shared callouts used by all signaling servers
    pub(crate) shared_callbacks: SharedCallbacks,

    /// The callbacks used by the signaling server
    pub(crate) callbacks: Cb,

    /// The state machine that runs a websocket to completion, also where topology is implemented
    pub(crate) topology: Topology,

    /// Arbitrary state accompanying a server
    pub(crate) state: S,
}

impl<Topology, Cb, S> SignalingServerBuilder<Topology, Cb, S>
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    /// Creates a new builder for a [`SignalingServer`].
    pub fn new(socket_addr: impl Into<SocketAddr>, topology: Topology, state: S) -> Self {
        Self {
            socket_addr: socket_addr.into(),
            router: Router::new(),
            shared_callbacks: SharedCallbacks::default(),
            callbacks: Cb::default(),
            topology,
            state,
        }
    }

    /// Modify the router with a mutable closure. This is where one may apply middleware or other
    /// layers to the Router.
    pub fn mutate_router(mut self, alter: impl FnOnce(Router) -> Router) -> Self {
        self.router = alter(self.router);
        self
    }

    /// Set a callback triggered before websocket upgrade to determine if the connection is allowed.
    pub fn on_connection_request<F>(mut self, callback: F) -> Self
    where
        F: FnMut(WsUpgradeMeta) -> Result<bool, Response> + Send + Sync + 'static,
    {
        self.shared_callbacks.on_connection_request = Callback::from(callback);
        self
    }

    /// Set a callback triggered when a socket has been assigned an ID. This happens after a
    /// connection is allowed, right before finalizing the websocket upgrade.
    pub fn on_id_assignment<F>(mut self, callback: F) -> Self
    where
        F: FnMut((SocketAddr, PeerId)) + Send + Sync + 'static,
    {
        self.shared_callbacks.on_id_assignment = Callback::from(callback);
        self
    }

    /// Apply permissive CORS middleware for debug purposes.
    pub fn cors(mut self) -> Self {
        self.router = self.router.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
        self
    }

    /// Apply a default tracing middleware layer for debug purposes.
    pub fn trace(mut self) -> Self {
        self.router = self.router.layer(
            // Middleware for logging from tower-http
            TraceLayer::new_for_http().on_response(
                DefaultOnResponse::new()
                    .level(Level::INFO)
                    .latency_unit(LatencyUnit::Micros),
            ),
        );
        self
    }

    /// Create a [`SignalingServer`].
    pub fn build(self) -> SignalingServer {
        self.build_with(identity)
    }

    /// Create a [`SignalingServer`] with a closure that modifies the signaling router
    pub fn build_with(self, alter: impl FnOnce(Router) -> Router) -> SignalingServer {
        // Insert topology
        let state_machine: SignalingStateMachine<Cb, S> =
            SignalingStateMachine::from_topology(self.topology);
        let info = alter(
            self.router
                .route("/", get(ws_handler::<Cb, S>))
                .route("/{path}", get(ws_handler::<Cb, S>))
                .layer(Extension(state_machine))
                .layer(Extension(self.shared_callbacks))
                .layer(Extension(self.callbacks))
                .layer(Extension(self.state)),
        )
        .into_make_service_with_connect_info::<SocketAddr>();
        SignalingServer {
            requested_addr: self.socket_addr,
            info,
            listener: None,
        }
    }
}
