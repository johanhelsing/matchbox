use crate::Error;
use axum::{
    extract::connect_info::IntoMakeServiceWithConnectInfo, response::IntoResponse, routing::get,
    Router, Server,
};
use futures::Future;
use hyper::{server::conn::AddrIncoming, StatusCode};
use matchbox_protocol::PeerId;
use std::{collections::HashSet, marker::PhantomData, net::SocketAddr, pin::Pin};

/// General configuration options for a WebRtc signalling server.
///
/// See [`WebRtcSocket::new_with_config`]
#[derive(Debug, Clone)]
pub struct SignallingConfig {
    address: SocketAddr,
    peers: HashSet<PeerId>,
}

#[derive(Debug, Default)]
pub struct FullMesh;
#[derive(Debug, Default)]
pub struct ClientServer;

/// A future which runs the message loop for the socket and completes when the socket closes or
/// disconnects
pub type SignallingLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;

/// Helper function to translate a hyper server into a [`SignallingLoopFuture`].
async fn run_server(
    s: Server<AddrIncoming, IntoMakeServiceWithConnectInfo<Router, SocketAddr>>,
) -> Result<(), Error> {
    // TODO: For some reason s.await.map_err(Error::Hyper) doesn't work? This could be shorter.
    let result = s.await;
    match result {
        Ok(()) => Ok(()),
        Err(e) => Err(Error::Hyper(e)),
    }
}

impl SignallingConfig {
    /// Create a new signalling server tailored for full-mesh connections
    ///
    /// The returned future should be awaited in order for messages to be sent and received.
    #[must_use]
    pub fn new_full_mesh(&self) -> (SignallingServer<FullMesh>, SignallingLoopFuture) {
        todo!()
    }

    /// Create a new signalling server tailored for full-mesh connections
    ///
    /// The returned future should be awaited in order for messages to be sent and received.
    #[must_use]
    pub fn new_client_server(&self) -> (SignallingServer<ClientServer>, SignallingLoopFuture) {
        let server = SignallingServer::<ClientServer>::default();
        async fn health_handler() -> impl IntoResponse {
            StatusCode::OK
        }
        let app = Router::new().route("/health", get(health_handler));
        let axum_server = axum::Server::bind(&self.address)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>());
        let server_fut = Box::pin(run_server(axum_server));

        (server, server_fut)
    }
}

/// Contains the interface end of a signalling server
#[derive(Debug, Default)]
pub struct SignallingServer<Topology = FullMesh> {
    peers: HashSet<PeerId>,
    host: Option<PeerId>,
    _pd: PhantomData<Topology>,
}

/// Common methods
impl<Topology> SignallingServer<Topology> {
    pub fn peers(&self) -> &HashSet<PeerId> {
        &self.peers
    }
}

/// Client-Server methods
impl SignallingServer<ClientServer> {
    pub fn is_host_connected(&self) -> bool {
        self.host.is_some()
    }

    pub fn host(&self) -> Option<&PeerId> {
        self.host.as_ref()
    }
}
