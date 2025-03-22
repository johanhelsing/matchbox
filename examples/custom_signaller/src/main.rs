
use futures::{select, FutureExt};
use futures_timer::Delay;
use custom_signaller::IrohGossipSignallerBuilder;
use log::info;
use matchbox_socket::{PeerState, WebRtcSocket};
use std::{sync::Arc, time::Duration};

const CHANNEL_ID: usize = 0;

#[cfg(target_arch = "wasm32")]
fn main() {
    // Setup logging
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Info).unwrap();

    wasm_bindgen_futures::spawn_local(async_main(None));
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    // Setup logging
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "error,custom_signaller=info,matchbox_socket=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    async_main(std::env::args().nth(1)).await
}

async fn async_main(node_id: Option<String>) {
    info!("Connecting to matchbox");
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
                }
                PeerState::Disconnected => {
                    info!("Peer left: {peer}");
                }
            }
        }

        // Accept any messages incoming
        for (peer, packet) in socket.channel_mut(CHANNEL_ID).receive() {
            let message = String::from_utf8_lossy(&packet);
            info!("Message from {peer}: {message:?}");
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
