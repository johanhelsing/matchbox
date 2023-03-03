use futures::{select, FutureExt};
use futures_timer::Delay;
use log::info;
use matchbox_socket::{PeerState, WebRtcSocket};
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
fn main() {
    // Setup logging
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).unwrap();

    wasm_bindgen_futures::spawn_local(async_main());
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    // Setup logging
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "matchbox_simple_demo=info,matchbox_socket=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    async_main().await
}

async fn async_main() {
    info!("Connecting to matchbox");
    let (mut socket, loop_fut) = WebRtcSocket::new_unreliable("ws://localhost:3536/example_room");

    let loop_fut = loop_fut.fuse();
    futures::pin_mut!(loop_fut);

    let timeout = Delay::new(Duration::from_millis(100));
    futures::pin_mut!(timeout);

    loop {
        // Handle any new peers
        for (peer, state) in socket.handle_peer_changes() {
            match state {
                PeerState::Connected => {
                    info!("Peer joined: {:?}", peer);
                    let packet = "hello friend!".as_bytes().to_vec().into_boxed_slice();
                    socket.send(packet, peer);
                }
                PeerState::Disconnected => {
                    info!("Peer left: {peer:?}");
                }
            }
        }

        // Accept any messages incoming
        for (peer, packet) in socket.receive() {
            let message = String::from_utf8_lossy(&packet);
            info!("Message from {peer:?}: {message:?}");
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
}
