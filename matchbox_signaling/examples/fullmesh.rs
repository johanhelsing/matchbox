use matchbox_signaling::SignalingServer;
use std::net::Ipv4Addr;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), matchbox_signaling::Error> {
    // Setup logging
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let server = SignalingServer::full_mesh_builder((Ipv4Addr::LOCALHOST, 3536))
        .on_peer_connected(|id| info!("Joined: {id:?}"))
        .on_peer_disconnected(|id| info!("Left: {id:?}"))
        .on_signal(|s| info!("signal: {s:?}"))
        .cors()
        .trace()
        .build();
    server.serve().await
}
