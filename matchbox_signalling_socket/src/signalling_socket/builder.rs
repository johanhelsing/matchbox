use crate::{
    signalling_socket::{
        signaling::ws_handler,
        state::SignalingState,
        topologies::{ClientServer, FullMesh},
    },
    SignallingServer,
};
use axum::{routing::get, Router};
use std::{marker::PhantomData, net::SocketAddr, sync::Arc};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::{DefaultOnResponse, TraceLayer},
    LatencyUnit,
};
use tracing::Level;

/// Builder for [`SignallingServer`]s.
///
/// Begin with [`SignallingServerBuilder::new`] and add parameters before calling
/// [`SignallingServerBuilder::build`] to produce the desired [`SignallingServer`].
#[derive(Debug, Clone)]
pub struct SignallingServerBuilder<Topology> {
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,

    /// The router used by the signalling server
    pub(crate) router: Router,

    _pd: PhantomData<Topology>,
}

fn default_router() -> Router {
    Router::new()
        .route("/:path", get(ws_handler))
        .with_state(Arc::new(futures::lock::Mutex::new(
            SignalingState::default(),
        )))
}

impl<Topology> SignallingServerBuilder<Topology> {
    /// Creates a new builder for a [`SignallingServer`].
    pub fn new(socket_addr: impl Into<SocketAddr>) -> Self {
        Self {
            socket_addr: socket_addr.into(),
            router: default_router(),
            _pd: PhantomData,
        }
    }

    /// Changes the topology of the [`SignallingServer`] to full-mesh.
    pub fn full_mesh_topology(self) -> SignallingServerBuilder<FullMesh> {
        // TODO: When #![feature(type_changing_struct_update)] is stable, just do
        // TODO: - SignallingServerBuilder { ..self }
        SignallingServerBuilder::<FullMesh> {
            socket_addr: self.socket_addr,
            router: default_router(),
            _pd: PhantomData,
        }
    }

    /// Changes the topology of the [`SignallingServer`] to client-server.
    pub fn client_server_topology(self) -> SignallingServerBuilder<ClientServer> {
        // TODO: When #![feature(type_changing_struct_update)] is stable, just do
        // TODO: - SignallingServerBuilder { ..self }
        SignallingServerBuilder::<ClientServer> {
            socket_addr: self.socket_addr,
            router: self.router,
            _pd: PhantomData,
        }
    }

    /// Modify the router.
    pub fn router(mut self, mut map: impl FnMut(&mut Router)) -> Self {
        map(&mut self.router);
        self
    }

    fn on_peer_connected(mut self) -> Self {
        todo!()
    }

    fn on_peer_disconnected(mut self) -> Self {
        todo!()
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

    /// Create a [`SignallingServer`].
    pub fn build(self) -> SignallingServer {
        SignallingServer {
            socket_addr: self.socket_addr,
            router: self.router,
        }
    }
}

impl SignallingServerBuilder<ClientServer> {
    pub fn on_host_connected(mut self) -> Self {
        todo!()
    }

    pub fn on_host_disconnected(mut self) -> Self {
        todo!()
    }
}
