use crate::signalling_socket::handlers::handle_ws;
use axum::{extract::ws::WebSocket, routing::get, Router};
use futures::lock::Mutex;
use matchbox_protocol::PeerId;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::{DefaultOnResponse, MakeSpan, OnBodyChunk, OnEos, OnFailure, TraceLayer},
    LatencyUnit,
};
use tracing::Level;

use super::handlers::{ws_handler, SignallingState};
use crate::{
    signalling_socket::topologies::{ClientServer, FullMesh},
    SignallingServer,
};
use std::{marker::PhantomData, net::SocketAddr, sync::Arc};

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
            SignallingState::default(),
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
        SignallingServerBuilder {
            socket_addr: self.socket_addr,
            router: default_router(),
            _pd: PhantomData,
        }
    }

    /// Changes the topology of the [`SignallingServer`] to client-server.
    pub fn client_server_topology(self) -> SignallingServerBuilder<ClientServer> {
        // TODO: When #![feature(type_changing_struct_update)] is stable, just do
        // TODO: - SignallingServerBuilder { ..self }
        SignallingServerBuilder {
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

impl SignallingServerBuilder<FullMesh> {
    /// Create a [`SignallingServer`] with full-mesh topology.
    pub fn build_full_mesh(&self) -> SignallingServer<FullMesh> {
        todo!()
    }
}

impl SignallingServerBuilder<ClientServer> {
    /// Create a [`SignallingServer`] with client-server topology.
    pub fn build_client_server(&self) -> SignallingServer<FullMesh> {
        todo!()
    }
}
