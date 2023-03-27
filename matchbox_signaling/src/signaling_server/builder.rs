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
use hyper::{header::AUTHORIZATION, StatusCode};
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

    /// Require all peers to provide basic authentication that matches a given username and
    /// password. This must come AFTER any calls to
    /// [`SignalingServerBuilder::on_connection_request`] for the time being!
    pub fn basic_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        // TODO: Make authentication a generic on the builder, or make the callback separated from
        // on_connection_request, since this has the potential for ordering problems
        let prev_callback = self.shared_callbacks.on_connection_request.cb.clone();
        let (username, password) = (username.into(), password.into());
        self.shared_callbacks.on_connection_request =
            Callback::from(move |extract: WsUpgradeMeta| {
                let authorization = extract
                    .headers
                    .get(AUTHORIZATION)
                    .ok_or(
                        (StatusCode::BAD_REQUEST, "`Authorization` header is missing")
                            .into_response(),
                    )?
                    .to_str()
                    .map_err(|_| {
                        (
                            StatusCode::BAD_REQUEST,
                            "`Authorization` header contains invalid characters",
                        )
                            .into_response()
                    })?;

                // Check that its well-formed basic auth then decode
                let split = authorization.split_once(' ');
                let (rx_username, rx_password) = match split {
                    Some((name, contents)) if name == "Basic" => {
                        let decoded = STANDARD.decode(contents).map_err(|_| {
                            (
                                StatusCode::BAD_REQUEST,
                                "`Authorization` header could not be decoded",
                            )
                                .into_response()
                        })?;
                        let decoded = String::from_utf8(decoded).map_err(|_| {
                            (
                                StatusCode::BAD_REQUEST,
                                "`Authentication` contains invalid characters",
                            )
                                .into_response()
                        })?;

                        // Return depending on if password is present
                        if let Some((username, password)) = decoded.split_once(':') {
                            Ok((username.to_string(), password.to_string()))
                        } else {
                            Err((
                                StatusCode::BAD_REQUEST,
                                "`Authentication` header must be username:password",
                            )
                                .into_response())
                        }
                    }
                    _ => Err((
                        StatusCode::BAD_REQUEST,
                        "`Authorization` header must be for basic authentication",
                    )
                        .into_response())?,
                }?;

                let authenticated = (&username, &password) == (&rx_username, &rx_password);
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
    pub fn on_connection_request<F>(mut self, callback: F) -> Self
    where
        F: Fn(WsUpgradeMeta) -> Result<bool, Response> + 'static,
    {
        self.shared_callbacks.on_connection_request = Callback::from(callback);
        self
    }

    /// Set a callback triggered when a socket has been assigned an ID. This happens after a
    /// connection is allowed, right before finalizing the websocket upgrade.
    pub fn on_id_assignment<F>(mut self, callback: F) -> Self
    where
        F: Fn((SocketAddr, PeerId)) + 'static,
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
