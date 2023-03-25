use crate::{
    signaling_server::{callbacks::Callback, handlers::ws_handler},
    topologies::{
        client_server::{ClientServer, ClientServerCallbacks, ClientServerState},
        full_mesh::{FullMesh, FullMeshCallbacks, FullMeshState},
        SignalingStateMachine, SignalingTopology,
    },
    SignalingCallbacks, SignalingServer, SignalingState,
};
use axum::{routing::get, Extension, Router};
use matchbox_protocol::{JsonPeerRequest, PeerId};
use std::net::SocketAddr;
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
pub struct SignalingServerBuilder<Topology, Cb, S>
where
    Topology: SignalingTopology<Cb, S>,
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,

    /// The router used by the signaling server
    pub(crate) router: Router,

    /// The callbacks used by the signalling server
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
            router: Router::new().route("/:path", get(ws_handler::<Cb, S>)),
            callbacks: Cb::default(),
            topology,
            state,
        }
    }

    /// Modify the router with a mutable closure. This is where one may apply middleware or other
    /// layers to the Router.
    pub fn middleware(mut self, mut alter: impl FnMut(&mut Router)) -> Self {
        alter(&mut self.router);
        self
    }

    /// Change the topology.
    pub fn topology(mut self, topology: Topology) -> Self {
        self.topology = topology;
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
            .layer(Extension(state_machine))
            .layer(Extension(self.callbacks))
            .layer(Extension(self.state));
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

impl SignalingServerBuilder<FullMesh, FullMeshCallbacks, FullMeshState> {
    /// Set a callback triggered on signals.
    pub fn on_signal<F>(mut self, callback: F) -> Self
    where
        F: Fn(JsonPeerRequest) + 'static,
    {
        self.callbacks.on_signal = Callback::from(callback);
        self
    }

    /// Set a callback triggered on all websocket connections.
    pub fn on_peer_connected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_peer_connected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on all websocket disconnections.
    pub fn on_peer_disconnected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_peer_disconnected = Callback::from(callback);
        self
    }
}

impl SignalingServerBuilder<ClientServer, ClientServerCallbacks, ClientServerState> {
    /// Set a callback triggered on signals.
    pub fn on_signal<F>(mut self, callback: F) -> Self
    where
        F: Fn(JsonPeerRequest) + 'static,
    {
        self.callbacks.on_signal = Callback::from(callback);
        self
    }

    /// Set a callback triggered on all client websocket connections.
    pub fn on_client_connected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_client_connected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on all client websocket disconnections.
    pub fn on_client_disconnected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_client_disconnected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on host websocket connection.
    pub fn on_host_connected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_host_connected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on host websocket disconnection.
    pub fn on_host_disconnected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_host_disconnected = Callback::from(callback);
        self
    }
}
