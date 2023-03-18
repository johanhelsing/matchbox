use super::builder::SignalingServerBuilder;
use axum::Router;
use std::net::SocketAddr;

/// Contains the interface end of a signaling server
#[derive(Debug)]
pub struct SignalingServer {
    /// The socket address to broadcast on
    pub(crate) socket_addr: SocketAddr,
    /// The router used by the signaling server
    pub(crate) router: Router,
}

/// Common methods
impl SignalingServer {
    /// Creates a new builder for a [`SignalingServer`].
    pub fn builder<Topology>(
        socket_addr: impl Into<SocketAddr>,
    ) -> SignalingServerBuilder<Topology> {
        SignalingServerBuilder::new(socket_addr)
    }

    /// Serve the signaling server
    pub async fn serve(self) -> Result<(), crate::Error> {
        // TODO: Shouldn't this return Result<!, crate::Error>?
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
