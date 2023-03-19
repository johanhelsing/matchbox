use crate::{
    signalling_socket::{
        signaling::ws_handler,
        state::SignalingState,
        topologies::{ClientServer, FullMesh},
    },
    SignalingServer,
};
use axum::{routing::get, Router};
use std::{marker::PhantomData, net::SocketAddr, sync::Arc};
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
#[derive(Debug, Clone)]
pub struct SignalingServerBuilder<Topology> {
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,

    /// The router used by the signaling server
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

impl<Topology> SignalingServerBuilder<Topology> {
    /// Creates a new builder for a [`SignalingServer`].
    pub fn new(socket_addr: impl Into<SocketAddr>) -> Self {
        Self {
            socket_addr: socket_addr.into(),
            router: default_router(),
            _pd: PhantomData,
        }
    }

    /// Changes the topology of the [`SignalingServer`] to full-mesh.
    pub fn full_mesh_topology(self) -> SignalingServerBuilder<FullMesh> {
        // TODO: When #![feature(type_changing_struct_update)] is stable, just do
        // TODO: - SignallingServerBuilder { ..self }
        SignalingServerBuilder::<FullMesh> {
            socket_addr: self.socket_addr,
            router: default_router(),
            _pd: PhantomData,
        }
    }

    /// Changes the topology of the [`SignalingServer`] to client-server.
    pub fn client_server_topology(self) -> SignalingServerBuilder<ClientServer> {
        // TODO: When #![feature(type_changing_struct_update)] is stable, just do
        // TODO: - SignallingServerBuilder { ..self }
        SignalingServerBuilder::<ClientServer> {
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

    /// Create a [`SignalingServer`].
    /// 
    /// Panics
    /// 
    pub fn build(self) -> SignalingServer {
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
