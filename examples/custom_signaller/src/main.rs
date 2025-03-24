use custom_signaller::IrohGossipSignallerBuilder;
use futures::{select, FutureExt};
use futures_timer::Delay;
use matchbox_socket::{PeerState, WebRtcSocket};
use std::{sync::Arc, time::Duration};
use tracing::info;

const CHANNEL_ID: usize = 0;
const LOG_FILTER: &str = "info,custom_signaller=info,iroh=error";

#[cfg(target_arch = "wasm32")]
fn main() {
    // see  https://github.com/DioxusLabs/dioxus/issues/3774#issuecomment-2733307383
    use tracing::{subscriber::set_global_default, Level};
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    let layer_config = tracing_wasm::WASMLayerConfigBuilder::new()
        .set_max_level(Level::INFO)
        .build();
    let layer = tracing_wasm::WASMLayer::new(layer_config);
    let filter: tracing_subscriber::EnvFilter = LOG_FILTER.parse().unwrap();

    let event_format = tracing_subscriber::fmt::format()
        .with_level(false) // don't include levels in formatted output
        .with_target(false) // don't include targets
        .with_thread_ids(false) // include the thread ID of the current thread
        .with_thread_names(false) // include the name of the current thread
        .with_line_number(false)
        .with_source_location(false)
        .with_file(false)
        .with_ansi(false)
        .compact()
        .without_time(); // use the `Compact` formatting style.

    let reg = Registry::default().with(layer).with(filter).with(
        tracing_subscriber::fmt::layer()
            .without_time()
            .event_format(event_format),
    );

    console_error_panic_hook::set_once();
    let _ = set_global_default(reg);

    // use tracing_log::LogTracer;
    // LogTracer::init().unwrap();

    use custom_signaller::get_browser_url::get_browser_url_hash;
    wasm_bindgen_futures::spawn_local(async_main(get_browser_url_hash()));
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    // Setup logging
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| LOG_FILTER.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    async_main(std::env::args().nth(1)).await
}

async fn async_main(node_id: Option<String>) {
    info!("Connecting to matchbox, room url: {:?}", node_id);
    let room_url = node_id.unwrap_or("".to_string());
    let signaller_builder = Arc::new(IrohGossipSignallerBuilder::new().await.unwrap());
    let (mut socket, loop_fut) = WebRtcSocket::builder(room_url)
        .signaller_builder(signaller_builder.clone())
        .add_reliable_channel()
        .build();

    let loop_fut = loop_fut.fuse();
    futures::pin_mut!(loop_fut);

    let timeout = Delay::new(Duration::from_millis(100));
    futures::pin_mut!(timeout);

    loop {
        // Handle any new peers
        for (peer, state) in socket.update_peers() {
            match state {
                PeerState::Connected => {
                    info!("Peer joined: {peer}");
                    let packet = "hello friend!".as_bytes().to_vec().into_boxed_slice();
                    socket.channel_mut(CHANNEL_ID).send(packet, peer);

                    for _i in 0..3 {
                        let packet = format!("ping {}", custom_signaller::get_timestamp())
                            .as_bytes()
                            .to_vec()
                            .into_boxed_slice();
                        socket.channel_mut(CHANNEL_ID).send(packet, peer);
                    }
                }
                PeerState::Disconnected => {
                    info!("Peer left: {peer}");
                }
            }
        }

        // Accept any messages incoming
        for (peer, packet) in socket.channel_mut(CHANNEL_ID).receive() {
            let message = String::from_utf8_lossy(&packet);
            if message.contains("ping") {
                let ts = message.split(" ").nth(1).unwrap().parse::<u128>().unwrap();
                let packet = format!("pong {}", ts)
                    .as_bytes()
                    .to_vec()
                    .into_boxed_slice();
                socket.channel_mut(CHANNEL_ID).send(packet, peer);
            }
            if message.contains("pong") {
                let ts = message.split(" ").nth(1).unwrap().parse::<u128>().unwrap();
                let now = custom_signaller::get_timestamp();
                let diff = now - ts;
                info!("\n\n\t peer ping: {}ms\n\n", diff / 1000);
            } else {
                info!("Message from {peer}: \n\n {message:?} \n");
            }
        }

        select! {
            // Restart this loop every 100ms
            _ = (&mut timeout).fuse() => {
                timeout.reset(Duration::from_millis(100));
            }

            // Or break if the message loop ends (disconnected, closed, etc.)
            _ = &mut loop_fut => {
                break;
            }
        }
    }
    signaller_builder.shutdown().await.unwrap();
}
