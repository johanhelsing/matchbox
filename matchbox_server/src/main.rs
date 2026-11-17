mod args;
mod state;
mod topology;

use crate::{
    state::{RequestedRoom, RoomId, ServerState},
    topology::MatchmakingDemoTopology,
};
use args::Args;
use axum::{http::StatusCode, response::IntoResponse, routing::get};
use clap::Parser;
use matchbox_signaling::SignalingServerBuilder;
use tokio::sync::oneshot;
use tracing::info;
use tracing_subscriber::prelude::*;

fn setup_logging() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "matchbox_server=info,tower_http=debug".into()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .with_file(false)
                .with_target(false),
        )
        .init();
}

#[tokio::main]
async fn main() {
    setup_logging();
    let args = Args::parse();

    // Setup router
    info!("Matchbox Signaling Server: {}", args.host);

    let mut state = ServerState::default();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Shutting down");
        let _ = shutdown_tx.send(());
    });

    let server = SignalingServerBuilder::new(args.host, MatchmakingDemoTopology, state.clone())
        .on_connection_request({
            let mut state = state.clone();
            move |connection| {
                let room_id = RoomId(connection.path.clone().unwrap_or_default());
                let next = connection
                    .query_params
                    .get("next")
                    .and_then(|next| next.parse::<usize>().ok());
                let room = RequestedRoom { id: room_id, next };
                state.add_waiting_client(connection.origin, room);
                Ok(true) // allow all clients
            }
        })
        .on_id_assignment({
            move |(origin, peer_id)| {
                info!("Client connected {origin:?}: {peer_id:?}");
                state.assign_id_to_waiting_client(origin, peer_id);
            }
        })
        .cors()
        .trace()
        .mutate_router(|router| router.route("/health", get(health_handler)))
        .with_shutdown(shutdown_rx)
        .build();
    server
        .serve()
        .await
        .expect("Unable to run signaling server, is it already running?")
}

/// Resolves on Ctrl+C, or on SIGTERM where there is one (`docker stop`, Kubernetes)
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl+C");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

pub async fn health_handler() -> impl IntoResponse {
    StatusCode::OK
}
