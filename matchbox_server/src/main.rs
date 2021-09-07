use std::{collections::HashMap, env, sync::Arc};

use clap::Clap;
use futures::lock::Mutex;
use log::info;
use warp::{http::StatusCode, hyper::Method, ws::Message, Filter, Rejection, Reply};

pub use args::Args;

mod args;
mod signaling;

pub struct Peer {
    pub uuid: String,
    pub sender:
        Option<tokio::sync::mpsc::UnboundedSender<std::result::Result<Message, warp::Error>>>,
}

type Clients = Arc<Mutex<HashMap<String, Peer>>>;

fn new_clients() -> Clients {
    Arc::new(Mutex::new(HashMap::new()))
}

#[tokio::main]
async fn main() {
    if env::var_os("RUST_LOG").is_none() {
        env::set_var("RUST_LOG", "matchbox_server=info");
    }
    pretty_env_logger::init();
    let args = Args::parse();

    let clients = new_clients();

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
        .or(signaling::ws_filter(clients))
        .with(cors)
        .with(log);

    info!("Starting matchbox signaling server");
    warp::serve(routes).run(args.host).await;
}

pub async fn health_handler() -> std::result::Result<impl Reply, Rejection> {
    Ok(StatusCode::OK)
}
