use futures::{select, FutureExt};
use futures_timer::Delay;
use log::{info, warn};
use matchbox_socket::{Error as SocketError, PeerId, PeerState, WebRtcSocket};
use std::time::Duration;

const CHANNEL_ID: usize = 0;

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
                .unwrap_or_else(|_| "error_handling_example=info,matchbox_socket=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    async_main().await
}

async fn async_main() {
    info!("Connecting to matchbox");
    let (mut socket, loop_fut) =
        WebRtcSocket::new_reliable("ws://localhost:3536/error_handling_example");

    let loop_fut = async {
        match loop_fut.await {
            Ok(()) => info!("Exited cleanly :)"),
            Err(e) => match e {
                SocketError::ConnectionFailed(e) => {
                    warn!("couldn't connect to signaling server, please check your connection: {e}");
                    // todo: show prompt and reconnect?
                }
                SocketError::Disconnected(e)  => {
                    warn!("you were kicked, or your connection went down, or the signaling server stopped: {e}");
                }
            },
        }
    }
    .fuse();

    futures::pin_mut!(loop_fut);

    let timeout = Delay::new(Duration::from_millis(100));
    futures::pin_mut!(timeout);

    let fake_user_quit = Delay::new(Duration::from_millis(20050)); // longer than reconnection timeouts
    futures::pin_mut!(fake_user_quit);

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
                let peers: Vec<PeerId> = socket.connected_peers().collect();
                for peer in peers {
                    let packet = "ping!".as_bytes().to_vec().into_boxed_slice();
                    socket.channel_mut(CHANNEL_ID).send(packet, peer);
                }
                timeout.reset(Duration::from_millis(10));
            }

            _ = (&mut fake_user_quit).fuse() => {
                info!("timeout, stopping sending/receiving");
                break;
            }

            // Or break if the message loop ends (disconnected, closed, etc.)
            _ = &mut loop_fut => {
                break;
            }
        }
    }

    info!("dropping socket (intentionally disconnecting if connected)");
    drop(socket);

    // join!(Delay::new(Duration::from_millis(2000)), loop_fut);

    Delay::new(Duration::from_millis(2000)).await;
    loop_fut.await;

    info!("Finished");
}
