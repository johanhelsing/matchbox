use futures::{FutureExt, SinkExt, StreamExt};
use matchbox_socket::{Packet, PeerId, PeerState, WebRtcSocket};
use n0_future::task::{AbortOnDropHandle, spawn};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::{
    RwLock,
    mpsc::{Receiver, Sender, channel},
};
use tracing::{info, warn};

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

    let (tx0, rx0) = socket.take_channel(CHANNEL_ID).unwrap().split();

    #[allow(clippy::type_complexity)]
    let tasks: Arc<
        RwLock<
            BTreeMap<
                PeerId,
                (
                    (AbortOnDropHandle<()>, AbortOnDropHandle<()>),
                    Sender<Box<[u8]>>,
                ),
            >,
        >,
    > = Arc::new(RwLock::new(BTreeMap::new()));
    let tasks_ = tasks.clone();
    let _task_rx_route = spawn(async move {
        futures::pin_mut!(rx0);
        while let Some((peer, packet)) = rx0.next().await {
            let tx = {
                let r = tasks_.read().await;
                let Some((_, tx)) = r.get(&peer) else {
                    warn!("Received packet from unknown peer: {peer}");
                    continue;
                };
                tx.clone()
            };
            tx.send(packet).await.unwrap();
        }
    });

    let tasks_ = tasks.clone();
    let _dispatch_task = AbortOnDropHandle::new(spawn(async move {
        // Handle any new peers
        while let Some((peer, state)) = socket.next().await {
            let mut tx0 = tx0.clone();
            match state {
                PeerState::Connected => {
                    info!("Peer joined: {peer}");
                    let (rx_sender, rx) = channel(1);
                    let (tx, mut tx_recv) = channel(1);
                    let _task_tx_combine = AbortOnDropHandle::new(spawn(async move {
                        while let Some(packet) = tx_recv.recv().await {
                            tx0.send((peer, packet)).await.unwrap();
                        }
                    }));
                    let task = AbortOnDropHandle::new(spawn(socket_task(peer, tx, rx)));
                    {
                        tasks_
                            .write()
                            .await
                            .insert(peer, ((task, _task_tx_combine), rx_sender));
                    }
                }
                PeerState::Disconnected => {
                    info!("Peer left: {peer}");
                    {
                        tasks_.write().await.remove(&peer);
                    }
                }
            }
        }
    }));

    let _ = loop_fut.await;
    tasks.write().await.clear();
}

async fn socket_task(peer: PeerId, tx: Sender<Packet>, mut rx: Receiver<Packet>) {
    let writer = tx.clone();
    let ping_task = spawn(async move {
        for _i in 0..20 {
            n0_future::time::sleep(Duration::from_secs_f32(0.25)).await;
            if _i == 0 {
                writer.send(b"hello friend!".to_vec().into()).await.unwrap();
            }
            writer
                .send(
                    format!("ping {}", get_timestamp())
                        .as_bytes()
                        .to_vec()
                        .into(),
                )
                .await
                .unwrap();
        }
    });
    let writer = tx.clone();
    let pong_task = spawn(async move {
        while let Some(packet) = rx.recv().await {
            let message = String::from_utf8_lossy(&packet);
            if message.starts_with("ping") {
                let ts = message.split(" ").nth(1).unwrap().parse::<u128>().unwrap();
                let packet = format!("pong {ts}").as_bytes().to_vec();
                writer.send(packet.into()).await.unwrap();
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
    ping_task.await.unwrap();
    pong_task.await.unwrap();
}
