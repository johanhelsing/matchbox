use crate::{
    signaling_server::{
        callbacks::{Callback, SharedCallbacks},
        handlers::{ws_handler, WsUpgradeMeta},
        NoCallbacks, NoState,
    },
    topologies::{SignalingStateMachine, SignalingTopology},
    SignalingCallbacks, SignalingServer, SignalingState,
};
use axum::{response::Response, routing::get, Extension, Router};
use axum_server::{accept::DefaultAcceptor, tls_rustls::RustlsConfig, Handle};
use matchbox_protocol::PeerId;
use std::net::SocketAddr;
use std::path::Path;
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
pub struct SignalingServerBuilder<Topology, Cb = NoCallbacks, S = NoState, Tls = DefaultAcceptor>
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

    /// A server acceptor
    pub(crate) acceptor: Tls,
}

impl<Topology, Cb, S> SignalingServerBuilder<Topology, Cb, S>
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    /// Creates a new builder for a [`SignalingServer`].
    pub fn new(
        socket_addr: impl Into<SocketAddr>,
        topology: Topology,
        state: S,
    ) -> SignalingServerBuilder<Topology, Cb, S, DefaultAcceptor> {
        SignalingServerBuilder {
            socket_addr: socket_addr.into(),
            router: Router::new(),
            shared_callbacks: SharedCallbacks::default(),
            callbacks: Cb::default(),
            topology,
            state,
            acceptor: DefaultAcceptor,
        }
    }

    /// Modify the router with a mutable closure. This is where one may apply middleware or other
    /// layers to the Router.
    pub fn mutate_router(mut self, mut alter: impl FnMut(Router) -> Router) -> Self {
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

    /// Configure TLS with a certificate (.pem) and private key (.key) file
    pub async fn tls(
        self,
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> SignalingServerBuilder<Topology, Cb, S, RustlsConfig> {
        let config = RustlsConfig::from_pem_file(cert, key).await.unwrap();
        SignalingServerBuilder {
            socket_addr: self.socket_addr,
            router: self.router,
            shared_callbacks: self.shared_callbacks,
            callbacks: self.callbacks,
            topology: self.topology,
            state: self.state,
            acceptor: config,
        }
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
}

impl<Topology, Cb, S> SignalingServerBuilder<Topology, Cb, S, DefaultAcceptor>
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    /// Create a [`SignalingServer`].
    ///
    /// # Panics
    /// This method will panic if the socket address requested cannot be bound.
    pub fn build(self) -> SignalingServer {
        let state_machine: SignalingStateMachine<Cb, S> =
            SignalingStateMachine::from_topology(self.topology);
        let router = self
            .router
            .route("/", get(ws_handler::<Cb, S>))
            .route("/:path", get(ws_handler::<Cb, S>))
            .layer(Extension(state_machine))
            .layer(Extension(self.shared_callbacks))
            .layer(Extension(self.callbacks))
            .layer(Extension(self.state));
        let handle = Handle::new();
        SignalingServer {
            server: Box::pin(
                axum_server::bind(self.socket_addr)
                    .handle(handle.clone())
                    .serve(router.into_make_service_with_connect_info::<SocketAddr>()),
            ),
            handle,
        }
    }
}

impl<Topology, Cb, S> SignalingServerBuilder<Topology, Cb, S, RustlsConfig>
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    /// Create a [`SignalingServer`].
    ///
    /// # Panics
    /// This method will panic if the socket address requested cannot be bound.
    pub fn build(self) -> SignalingServer {
        let state_machine: SignalingStateMachine<Cb, S> =
            SignalingStateMachine::from_topology(self.topology);
        let router = self
            .router
            .route("/", get(ws_handler::<Cb, S>))
            .route("/:path", get(ws_handler::<Cb, S>))
            .layer(Extension(state_machine))
            .layer(Extension(self.shared_callbacks))
            .layer(Extension(self.callbacks))
            .layer(Extension(self.state));
        let handle = Handle::new();
        SignalingServer {
            server: Box::pin(
                axum_server::bind_rustls(self.socket_addr, self.acceptor)
                    .handle(handle.clone())
                    .serve(router.into_make_service_with_connect_info::<SocketAddr>()),
            ),
            handle,
        }
    }
}
