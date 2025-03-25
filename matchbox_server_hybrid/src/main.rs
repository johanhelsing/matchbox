mod args;
mod state;
mod topology;

use crate::{
    state::HybridState,
    topology::HybridTopology,
};
use args::Args;
use axum::{http::StatusCode, response::IntoResponse, routing::get};
use clap::Parser;
use matchbox_signaling::SignalingServerBuilder;
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

    let state = HybridState::default();
    let server = SignalingServerBuilder::new(args.host, HybridTopology, state.clone())
        .cors()
        .trace()
        .mutate_router(|router| router.route("/health", get(health_handler)))
        .build();
    server
        .serve()
        .await
        .expect("Unable to run signaling server, is it already running?")
}

pub async fn health_handler() -> impl IntoResponse {
    StatusCode::OK
}
