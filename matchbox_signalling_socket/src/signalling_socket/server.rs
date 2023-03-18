use super::{
    builder::SignallingServerBuilder,
    topologies::{ClientServer, FullMesh},
};
use axum::Router;
use matchbox_protocol::PeerId;
use std::{collections::HashSet, marker::PhantomData, net::SocketAddr};
use tracing_subscriber::fmt::format::Full;

/// Contains the interface end of a signalling server
#[derive(Debug)]
pub struct SignallingServer<Topology = FullMesh> {
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,
    /// The router used by the signalling server
    pub(crate) router: Router,
    _pd: PhantomData<Topology>,
}

/// Common methods
impl<Topology> SignallingServer<Topology> {
    pub fn builder(socket_addr: impl Into<SocketAddr>) -> SignallingServerBuilder<Topology> {
        SignallingServerBuilder::new(socket_addr)
    }

    fn on_peer_connected(mut self) -> Self {
        todo!()
    }

    fn on_peer_disconnected(mut self) -> Self {
        todo!()
    }

    pub async fn serve(self) -> Result<(), crate::Error> {
        let x = axum::Server::bind(&self.socket_addr)
            .serve(
                self.router
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        match x {
            Ok(()) => Ok(()),
            Err(e) => Err(crate::Error::from(e)),
        }
    }
}

/// Client-Server only methods
impl SignallingServer<ClientServer> {
    pub fn on_host_connected(mut self) -> Self {
        todo!()
    }

    pub fn on_host_disconnected(mut self) -> Self {
        todo!()
    }
}
