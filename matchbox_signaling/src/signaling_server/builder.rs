use crate::{
    signaling_server::state::SignalingState,
    topologies::{ClientServer, FullMesh, SignalingStateMachine, SignalingTopology},
    SignalingServer,
};
use axum::{routing::get, Extension, Router};
use futures::{lock::Mutex, Future};
use std::{marker::PhantomData, net::SocketAddr, rc::Rc, sync::Arc};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::{DefaultOnResponse, TraceLayer},
    LatencyUnit,
};
use tracing::Level;

use super::{
    callbacks::{Callback, Callbacks},
    handlers::ws_handler,
};

/// Builder for [`SignalingServer`]s.
///
/// Begin with [`SignalingServerBuilder::new`] and add parameters before calling
/// [`SignalingServerBuilder::build`] to produce the desired [`SignalingServer`].
pub struct SignalingServerBuilder<Topology: SignalingTopology> {
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,

    /// The router used by the signaling server
    pub(crate) router: Router,

    /// The callbacks used by the signalling server
    pub(crate) callbacks: Callbacks,

    /// The state machine that runs a websocket to completion, also where topology is implemented
    pub(crate) topology: Topology,
}

impl<Topology: SignalingTopology> SignalingServerBuilder<Topology> {
    /// Creates a new builder for a [`SignalingServer`].
    pub fn new(socket_addr: impl Into<SocketAddr>, topology: Topology) -> Self {
        let state = Arc::new(Mutex::new(SignalingState::default()));
        let callbacks = Callbacks::default();
        Self {
            socket_addr: socket_addr.into(),
            router: Router::new()
                .route("/:path", get(ws_handler))
                .with_state(state),
            callbacks,
            topology,
        }
    }

    /// Modify the router.
    pub fn router(mut self, mut alter: impl FnMut(&mut Router)) -> Self {
        alter(&mut self.router);
        self
    }

    /// Change the topology.
    pub fn topology(mut self, topology: Topology) -> Self {
        self.topology = topology;
        self
    }

    // Set a callback triggered on new peer connections.
    pub fn on_peer_connected<F>(mut self, callback: F) -> Self
    where
        F: Fn(()) -> () + 'static,
    {
        self.callbacks.on_peer_connected = Callback::from(callback);
        self
    }

    // Set a callback triggered on peer disconnections.
    pub fn on_peer_disconnected<F>(mut self, callback: F) -> Self
    where
        F: Fn(()) -> () + 'static,
    {
        self.callbacks.on_peer_disconnected = Callback::from(callback);
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
        let state_machine = SignalingStateMachine::from_topology(self.topology);
        self.router = self
            .router
            .layer(Extension(state_machine))
            .layer(Extension(self.callbacks));
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

impl SignalingServerBuilder<ClientServer> {
    pub fn on_host_connected<F, Fut>(self, callback: F) -> Self
    where
        F: Fn() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static + Send,
    {
        todo!()
    }

    pub fn on_host_disconnected<F, Fut>(self, callback: F) -> Self
    where
        F: Fn() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static + Send,
    {
        todo!()
    }
}
