use axum::response::IntoResponse;
use axum::Router;
use axum::{http::StatusCode, routing::get};
use clap::Parser;
use log::info;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tower_http::LatencyUnit;
use tracing::Level;
use tracing_subscriber::prelude::*;

pub use args::Args;
pub use signaling::matchbox::PeerId;

use crate::signaling::{ws_handler, ServerState};

mod args;
mod signaling;

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
    info!("Matchbox Signaling Server: {}", args.host,);
    axum::Server::bind(&args.host)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .expect("Unable to run signalling server, is it already running?");
}

pub async fn health_handler() -> impl IntoResponse {
    StatusCode::OK
}
