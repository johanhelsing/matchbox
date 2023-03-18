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
pub struct SignallingServer {
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,
    /// The router used by the signalling server
    pub(crate) router: Router,
}

/// Common methods
impl SignallingServer {
    pub fn builder<Topology>(
        socket_addr: impl Into<SocketAddr>,
    ) -> SignallingServerBuilder<Topology> {
        SignallingServerBuilder::new(socket_addr)
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
