use clap::Parser;
use log::info;
use std::env;
use warp::{http::StatusCode, hyper::Method, Filter, Rejection, Reply};

pub use args::Args;
pub use signaling::matchbox::PeerId;

mod args;
mod signaling;

#[tokio::main]
async fn main() {
    if env::var_os("RUST_LOG").is_none() {
        env::set_var("RUST_LOG", "matchbox_server=info");
    }
    pretty_env_logger::init();
    let args = Args::parse();

    let health_route = warp::path("health").and_then(health_handler);

    let log = warp::log("made_in_heaven");

    // let cors = warp::cors()
    //     .allow_methods(vec!["GET", "POST"])
    //     .allow_header("content-type")
    //     .allow_header("authorization")
    //     .allow_any_origin()
    //     .build();

    let cors = warp::cors()
        .allow_any_origin()
        .allow_headers(vec![
            "Access-Control-Allow-Headers",
            "Access-Control-Request-Method",
            "Access-Control-Request-Headers",
            "Origin",
            "Accept",
            "X-Requested-With",
            "Content-Type",
        ])
        .allow_methods(&[
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
            Method::HEAD,
        ]);

    // let cors = warp::cors()
    //     .allow_any_origin()
    //     .allow_methods(&[Method::GET]);

    let routes = health_route
        .or(signaling::ws_filter(Default::default()))
        .with(cors)
        .with(log);

    info!(
        "Starting matchbox signaling server at port {}",
        args.host.port()
    );
    warp::serve(routes).run(args.host).await;
}

pub async fn health_handler() -> std::result::Result<impl Reply, Rejection> {
    Ok(StatusCode::OK)
}
