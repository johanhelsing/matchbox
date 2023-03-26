use crate::{
    signaling_server::{
        callbacks::{Callback, SharedCallbacks},
        handlers::{ws_handler, WsUpgradeMeta},
        NoOpCallouts, NoState,
    },
    topologies::{SignalingStateMachine, SignalingTopology},
    SignalingCallbacks, SignalingServer, SignalingState,
};
use axum::{
    response::{IntoResponse, Response},
    routing::get,
    Extension, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hyper::StatusCode;
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
pub struct SignalingServerBuilder<Topology, Cb = NoOpCallouts, S = NoState>
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
    pub(crate) state: PhantomData<S>,
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
            router: Router::new()
                .route("/", get(ws_handler::<Cb, S>))
                .route("/:path", get(ws_handler::<Cb, S>))
                .with_state(state),
            shared_callbacks: SharedCallbacks::default(),
            callbacks: Cb::default(),
            topology,
            state: PhantomData,
        }
    }

    /// Require peers to provide a `token` query parameter matching the given password in base64 to
    /// connect. For example, using `.token_auth("123")` would require users to connect to the
    /// websocket url: `ws://...?token=MTIz`. This is unfortunately a hack, since Tungstenite and
    /// other websocket client APIs do not allow header-based authentication for websockets yet.
    pub fn token_auth(
        mut self,
        key: Option<impl Into<String>>,
        password: impl Into<String>,
    ) -> Self {
        let prev_callback = self.shared_callbacks.on_upgrade.cb.clone();
        let (key, password) = (
            key.map(|p| p.into()).unwrap_or("token".to_string()),
            password.into(),
        );
        self.shared_callbacks.on_upgrade = Callback::from(move |extract: WsUpgradeMeta| {
            let raw_token = extract
                .query_params
                .get(&key)
                .ok_or(
                    (
                        StatusCode::BAD_REQUEST,
                        format!("`{key}` query arg is missing"),
                    )
                        .into_response(),
                )?
                .as_str();

            let decoded = STANDARD.decode(raw_token).map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("`{key}` query value is not valid base64"),
                )
                    .into_response()
            })?;
            let decoded = String::from_utf8(decoded).map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("`{key}` query value contains invalid characters"),
                )
                    .into_response()
            })?;

            let authenticated = password == decoded;
            Ok(authenticated && prev_callback(extract.clone())?)
        });
        self
    }

    /// Modify the router with a mutable closure. This is where one may apply middleware or other
    /// layers to the Router.
    pub fn mutate_router(mut self, mut alter: impl FnMut(&mut Router)) -> Self {
        alter(&mut self.router);
        self
    }

    /// Set a callback triggered before websocket upgrade to determine if the connection is allowed.
    pub fn on_upgrade<F>(mut self, callback: F) -> Self
    where
        F: Fn(WsUpgradeMeta) -> Result<bool, Response> + 'static,
    {
        self.shared_callbacks.on_upgrade = Callback::from(callback);
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
            .layer(Extension(self.shared_callbacks))
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
