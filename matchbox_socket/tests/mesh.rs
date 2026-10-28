//! Two native sockets through a signaling server on localhost.

#![cfg(not(target_arch = "wasm32"))]

use matchbox_signaling::SignalingServer;
use matchbox_socket::{
    ChannelConfig, Error, Packet, PeerId, PeerState, RtcIceServerConfig, WebRtcSocket,
};
use std::{net::Ipv4Addr, time::Duration};
use tokio::{
    runtime::Runtime,
    task::JoinHandle,
    time::{sleep, timeout},
};

const TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(10);

/// A signaling server on its own runtime, so that killing it also drops its connections.
struct Server {
    runtime: Option<Runtime>,
    url: String,
}

impl Server {
    fn start() -> Self {
        let runtime = Runtime::new().unwrap();
        let _guard = runtime.enter();
        let mut server = SignalingServer::full_mesh_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        runtime.spawn(server.serve());
        let url = format!("ws://{addr}/room");
        Self {
            runtime: Some(runtime),
            url,
        }
    }

    fn kill(&mut self) {
        self.runtime.take().unwrap().shutdown_background();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

fn connect(server: &Server) -> (WebRtcSocket, JoinHandle<Result<(), Error>>) {
    connect_to(&server.url)
}

fn connect_to(url: &str) -> (WebRtcSocket, JoinHandle<Result<(), Error>>) {
    let (socket, message_loop) = WebRtcSocket::builder(url)
        // Localhost only, no STUN
        .ice_server(RtcIceServerConfig {
            urls: vec![],
            ..Default::default()
        })
        .add_channel(ChannelConfig::reliable())
        .build();
    (socket, tokio::spawn(message_loop))
}

async fn wait_for_peer(socket: &mut WebRtcSocket, state: PeerState) -> PeerId {
    wait_for_peer_within(socket, state, TIMEOUT).await
}

async fn wait_for_peer_within(
    socket: &mut WebRtcSocket,
    state: PeerState,
    duration: Duration,
) -> PeerId {
    timeout(duration, async {
        loop {
            if let Some((id, _)) = socket.update_peers().into_iter().find(|(_, s)| *s == state) {
                return id;
            }
            sleep(POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no peer became {state:?}"))
}

/// Everything received the first time something arrives, so that nothing is dropped
async fn wait_for_packets(socket: &mut WebRtcSocket) -> Vec<(PeerId, Packet)> {
    timeout(TIMEOUT, async {
        loop {
            let packets = socket.channel_mut(0).receive();
            if !packets.is_empty() {
                return packets;
            }
            sleep(POLL).await;
        }
    })
    .await
    .expect("no packet received")
}

async fn connect_pair(server: &Server) -> (WebRtcSocket, WebRtcSocket, PeerId, PeerId) {
    let (mut a, _a_loop) = connect(server);
    let (mut b, _b_loop) = connect(server);
    // The loops are detached, the sockets end them when dropped
    let b_id = wait_for_peer(&mut a, PeerState::Connected).await;
    let a_id = wait_for_peer(&mut b, PeerState::Connected).await;
    assert_eq!(a.id(), Some(a_id));
    assert_eq!(b.id(), Some(b_id));
    (a, b, a_id, b_id)
}

#[tokio::test]
async fn two_peers_connect_and_exchange_packets() {
    let server = Server::start();
    let (mut a, mut b, a_id, b_id) = connect_pair(&server).await;

    a.channel_mut(0).send(b"hello".to_vec().into(), b_id);
    assert_eq!(
        wait_for_packets(&mut b).await,
        vec![(a_id, b"hello".to_vec().into())]
    );

    b.channel_mut(0).send(b"hi".to_vec().into(), a_id);
    assert_eq!(
        wait_for_packets(&mut a).await,
        vec![(b_id, b"hi".to_vec().into())]
    );
}

#[tokio::test]
async fn dropping_a_socket_disconnects_its_peer() {
    let server = Server::start();
    let (mut a, b, _a_id, b_id) = connect_pair(&server).await;

    drop(b);
    assert_eq!(wait_for_peer(&mut a, PeerState::Disconnected).await, b_id);
}

#[tokio::test]
async fn signaling_loss_keeps_connected_peers() {
    let mut server = Server::start();
    let (mut a, mut b, a_id, b_id) = connect_pair(&server).await;

    server.kill();
    sleep(Duration::from_millis(500)).await;
    assert_eq!(a.update_peers(), vec![]);
    assert_eq!(b.update_peers(), vec![]);

    a.channel_mut(0).send(b"still here".to_vec().into(), b_id);
    assert_eq!(
        wait_for_packets(&mut b).await,
        vec![(a_id, b"still here".to_vec().into())]
    );
}

#[tokio::test]
async fn signaling_loss_without_peers_ends_the_socket() {
    let mut server = Server::start();
    let (mut socket, message_loop) = connect(&server);
    timeout(TIMEOUT, async {
        while socket.id().is_none() {
            sleep(POLL).await;
        }
    })
    .await
    .expect("no id assigned");

    server.kill();
    let result = timeout(TIMEOUT, message_loop)
        .await
        .expect("socket kept running");
    assert!(
        matches!(result, Ok(Err(Error::Disconnected(_)))),
        "{result:?}"
    );
    assert!(socket.try_update_peers().is_err());
}

const CHILD_ROOM_ENV: &str = "MATCHBOX_TEST_ROOM";

/// Runs in a child process of `vanished_peer_is_disconnected_without_signaling`, which kills it
/// once connected. No-op when run directly.
#[tokio::test]
#[ignore]
async fn child_peer() {
    let Ok(url) = std::env::var(CHILD_ROOM_ENV) else {
        return;
    };
    let (mut socket, _message_loop) = connect_to(&url);
    wait_for_peer(&mut socket, PeerState::Connected).await;
    std::future::pending::<()>().await;
}

/// A peer killed without closing its connection, while signaling is down, is reported
/// disconnected once ICE gives up on it. Slow: ICE takes about 30 seconds to do so.
#[tokio::test]
#[ignore]
async fn vanished_peer_is_disconnected_without_signaling() {
    let mut server = Server::start();
    let (mut a, _a_loop) = connect(&server);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["child_peer", "--ignored", "--exact"])
        .env(CHILD_ROOM_ENV, &server.url)
        .spawn()
        .unwrap();
    let child_id = wait_for_peer(&mut a, PeerState::Connected).await;

    server.kill();
    child.kill().unwrap();
    child.wait().unwrap();
    let disconnected =
        wait_for_peer_within(&mut a, PeerState::Disconnected, Duration::from_secs(60)).await;
    assert_eq!(disconnected, child_id);
}
