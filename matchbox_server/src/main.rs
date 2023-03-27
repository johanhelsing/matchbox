mod args;
mod error;
mod signaling;

use crate::signaling::{ws_handler, ServerState};
use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
use clap::Parser;
use std::{net::SocketAddr, sync::Arc};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::{DefaultOnResponse, TraceLayer},
    LatencyUnit,
};
use tracing::{info, Level};
use tracing_subscriber::prelude::*;

pub use args::Args;
pub use matchbox_protocol::PeerId;

#[tokio::main]
async fn main() {
    // Initialize logger
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

    // Parse clap arguments
    let args = Args::parse();

    // Setup router
    let server_state = Arc::new(futures::lock::Mutex::new(ServerState::default()));
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/", get(ws_handler))
        .route("/:room_id", get(ws_handler))
        .layer(
            // Allow requests from anywhere - Not ideal for production!
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(
            // Middleware for logging from tower-http
            TraceLayer::new_for_http().on_response(
                DefaultOnResponse::new()
                    .level(Level::INFO)
                    .latency_unit(LatencyUnit::Micros),
            ),
        )
        .with_state(server_state);

    // Run server
    info!("Matchbox Signaling Server: {}", args.host);
    axum::Server::bind(&args.host)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .expect("Unable to run signaling server, is it already running?");
}

pub async fn health_handler() -> impl IntoResponse {
    StatusCode::OK
}
