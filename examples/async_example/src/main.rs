use futures::{FutureExt, SinkExt, StreamExt, channel::mpsc::UnboundedSender, select};
use futures_timer::Delay;
use matchbox_socket::{Packet, PeerId, PeerState, WebRtcSocket};
use n0_future::task::{AbortOnDropHandle, spawn};
use std::{collections::BTreeMap, time::Duration};
use tracing::info;

const CHANNEL_ID: usize = 0;

fn get_timestamp() -> u128 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap()
        .as_micros()
}

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
                .unwrap_or_else(|_| "async_example=info,matchbox_socket=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    async_main().await
}

async fn async_main() {
    let (mut socket, loop_fut) = WebRtcSocket::new_reliable("ws://localhost:3536/");

    let loop_fut = loop_fut.fuse();
    futures::pin_mut!(loop_fut);

    let timeout = Delay::new(Duration::from_millis(100));
    futures::pin_mut!(timeout);

    let (tx, rx) = socket.take_channel(CHANNEL_ID).unwrap().split();
    let (mut broadcast_tx, mut broadcast_rx) = async_broadcast::broadcast::<(PeerId, Packet)>(1024);
    broadcast_tx.set_overflow(true);
    broadcast_rx.set_overflow(true);
    let broadcast_rx = broadcast_rx.deactivate();
    let _broadcast_task = spawn(async move {
        futures::pin_mut!(rx);
        while let Some((peer, packet)) = rx.next().await {
            broadcast_tx.broadcast((peer, packet)).await.unwrap();
        }
    });

    let mut tasks = BTreeMap::new();

    loop {
        // Handle any new peers
        for (peer, state) in socket.update_peers() {
            match state {
                PeerState::Connected => {
                    info!("Peer joined: {peer}");
                    let tx = tx.clone();
                    let rx = broadcast_rx.activate_cloned();
                    tasks.insert(
                        peer,
                        AbortOnDropHandle::new(n0_future::task::spawn(socket_task(peer, tx, rx))),
                    );
                }
                PeerState::Disconnected => {
                    info!("Peer left: {peer}");
                    tasks.remove(&peer);
                }
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
    tasks.clear();
}

async fn socket_task(
    peer: PeerId,
    tx: UnboundedSender<(PeerId, Packet)>,
    mut rx: async_broadcast::Receiver<(PeerId, Packet)>,
) {
    let mut writer = tx.clone();
    let _ping_task = spawn(async move {
        for _i in 0..10 {
            n0_future::time::sleep(Duration::from_secs_f32(1.5)).await;
            if _i == 0 {
                writer
                    .send((peer, b"hello friend!".to_vec().into()))
                    .await
                    .unwrap();
            }
            writer
                .send((
                    peer,
                    format!("ping {}", get_timestamp())
                        .as_bytes()
                        .to_vec()
                        .into(),
                ))
                .await
                .unwrap();
        }
    });
    let mut writer = tx.clone();
    let _pong_task = spawn(async move {
        while let Ok((packet_peer, packet)) = rx.recv().await {
            if packet_peer != peer {
                continue;
            }
            let message = String::from_utf8_lossy(&packet);
            if message.starts_with("ping") {
                let ts = message.split(" ").nth(1).unwrap().parse::<u128>().unwrap();
                let packet = format!("pong {}", ts).as_bytes().to_vec();
                writer.send((peer, packet.into())).await.unwrap();
            } else if message.starts_with("pong") {
                let ts = message.split(" ").nth(1).unwrap().parse::<u128>().unwrap();
                let now = get_timestamp();
                let diff = now - ts;
                info!("Ping from {peer} took {}ms", diff as f32 / 1000.0);
            } else {
                info!("Message from {peer}: \n\n {message:?} \n");
            }
        }
    });
    _ping_task.await.unwrap();
    _pong_task.await.unwrap();
}
