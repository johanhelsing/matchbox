use crate::{
    signaling_server::{
        auth::{AuthKey, BasicAuthentication, NoAuthentication},
        callbacks::{Callback, SharedCallbacks},
        handlers::{ws_handler, WsUpgradeMeta},
        Authentication, NoOpCallouts, NoState,
    },
    topologies::{SignalingStateMachine, SignalingTopology},
    SignalingCallbacks, SignalingServer, SignalingState,
};
use axum::{response::Response, routing::get, Extension, Router};
use base64::{engine::general_purpose::STANDARD, Engine};
use matchbox_protocol::PeerId;
use std::{marker::PhantomData, net::SocketAddr};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::{DefaultOnResponse, TraceLayer},
    LatencyUnit,
};
use tracing::Level;

/// Builder for [`SignalingServer`]s.
///
/// Begin with [`SignalingServerBuilder::new`] and add parameters before calling
/// [`SignalingServerBuilder::build`] to produce the desired [`SignalingServer`].
pub struct SignalingServerBuilder<Topology, Cb = NoOpCallouts, S = NoState, A = NoAuthentication>
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
    A: Authentication,
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

    /// Authentication method used by the server
    pub(crate) auth: PhantomData<A>,

    /// The expected auth key used by the server
    pub(crate) auth_key: AuthKey,
}

impl<Topology, Cb, S, A> SignalingServerBuilder<Topology, Cb, S, A>
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
    A: Authentication,
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
            auth: PhantomData,
            auth_key: AuthKey::default(),
        }
    }

    /// Require all peers to present basic authentication matching a provided username and password
    /// to connect.
    pub fn basic_auth(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> SignalingServerBuilder<Topology, Cb, S, BasicAuthentication> {
        let (username, password) = (username.into(), password.into());
        self.auth_key = AuthKey(STANDARD.encode(format!("{username}:{password}")));
        SignalingServerBuilder {
            socket_addr: self.socket_addr,
            router: self.router,
            shared_callbacks: self.shared_callbacks,
            callbacks: self.callbacks,
            topology: self.topology,
            state: self.state,
            auth: PhantomData,
            auth_key: self.auth_key,
        }
    }

    /// Modify the router with a mutable closure. This is where one may apply middleware or other
    /// layers to the Router.
    pub fn mutate_router(mut self, mut alter: impl FnMut(&mut Router)) -> Self {
        alter(&mut self.router);
        self
    }

    /// Set a callback triggered before websocket upgrade to determine if the connection is allowed.
    pub fn on_connection_request<F>(mut self, callback: F) -> Self
    where
        F: Fn(WsUpgradeMeta) -> Result<bool, Response> + Send + Sync + 'static,
    {
        self.shared_callbacks.on_connection_request = Callback::from(callback);
        self
    }

    /// Set a callback triggered when a socket has been assigned an ID. This happens after a
    /// connection is allowed, right before finalizing the websocket upgrade.
    pub fn on_id_assignment<F>(mut self, callback: F) -> Self
    where
        F: Fn((SocketAddr, PeerId)) + Send + Sync + 'static,
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
    ///
    /// # Panics
    /// This method will panic if the socket address requested cannot be bound.
    pub fn build(mut self) -> SignalingServer {
        // Insert topology
        let state_machine: SignalingStateMachine<Cb, S> =
            SignalingStateMachine::from_topology(self.topology);
        self.router = self
            .router
            .route("/", get(ws_handler::<Cb, S, A>))
            .route("/:path", get(ws_handler::<Cb, S, A>))
            .layer(Extension(state_machine))
            .layer(Extension(self.shared_callbacks))
            .layer(Extension(self.callbacks))
            .layer(Extension(self.state))
            .layer(Extension(self.auth_key));
        let server = axum::Server::bind(&self.socket_addr).serve(
            self.router
                .into_make_service_with_connect_info::<SocketAddr>(),
        );
        let socket_addr = server.local_addr();
        SignalingServer {
            server,
            socket_addr,
        }
    }
}
