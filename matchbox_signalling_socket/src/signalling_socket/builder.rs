use super::callbacks::Callbacks;
use crate::{
    signalling_socket::{signaling::ws_handler, state::SignalingState, topologies::ClientServer},
    SignalingServer,
};
use axum::{routing::get, Extension, Router};
use futures::{lock::Mutex, Future};
use std::{cell::RefCell, marker::PhantomData, net::SocketAddr, pin::Pin, rc::Rc, sync::Arc};
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
pub struct SignalingServerBuilder<Topology> {
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,

    /// The router used by the signaling server
    pub(crate) router: Router,

    /// The callbacks used by the signalling server
    pub(crate) callbacks: Arc<Mutex<Callbacks>>,

    _pd: PhantomData<Topology>,
}

impl<Topology> SignalingServerBuilder<Topology> {
    /// Creates a new builder for a [`SignalingServer`].
    pub fn new(socket_addr: impl Into<SocketAddr>) -> Self {
        let state = Arc::new(Mutex::new(SignalingState::default()));
        let callbacks = Arc::new(Mutex::new(Callbacks::default()));
        Self {
            socket_addr: socket_addr.into(),
            router: Router::new()
                .route("/:path", get(ws_handler))
                .with_state(state)
                .layer(Extension(Arc::clone(&callbacks))),
            callbacks,
            _pd: PhantomData,
        }
    }

    /// Modify the router.
    pub fn router(mut self, mut alter: impl FnMut(&mut Router)) -> Self {
        alter(&mut self.router);
        self
    }

    pub fn on_peer_connected<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn() -> Fut + 'static,
        Fut: Future<Output = ()> + 'static + Send,
    {
        self.callbacks.try_lock().unwrap().on_peer_connected = Box::pin(callback());
        self
    }

    pub fn on_peer_disconnected<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn() -> Fut + 'static + Send + Sync,
        Fut: Future<Output = ()> + 'static + Send,
    {
        self.callbacks.try_lock().unwrap().on_peer_disconnected = Box::pin(callback());
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
    pub fn on_host_connected(mut self) -> Self {
        todo!()
    }

    pub fn on_host_disconnected(mut self) -> Self {
        todo!()
    }
}
