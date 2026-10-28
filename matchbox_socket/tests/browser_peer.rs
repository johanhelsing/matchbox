//! The browser side of tests/browser.rs: a peer against the native echo peer, and two peers
//! against each other.

#![cfg(target_arch = "wasm32")]

use futures_timer::Delay;
use matchbox_socket::{ChannelConfig, Packet, PeerId, PeerState, RtcIceServerConfig, WebRtcSocket};
use std::time::Duration;
use wasm_bindgen_futures::spawn_local;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

/// Served by tests/browser.rs, which has a native echo peer in the `browser` room
const SIGNALING_URL: &str = "ws://127.0.0.1:3536";
const POLL: Duration = Duration::from_millis(10);
const MAX_POLLS: usize = 1000;

fn connect(room: &str) -> WebRtcSocket {
    let (socket, message_loop) = WebRtcSocket::builder(format!("{SIGNALING_URL}/{room}"))
        .ice_server(RtcIceServerConfig {
            urls: vec![],
            ..Default::default()
        })
        .add_channel(ChannelConfig::reliable())
        .build();
    spawn_local(async move {
        let _ = message_loop.await;
    });
    socket
}

async fn wait_for_connected_peer(socket: &mut WebRtcSocket) -> PeerId {
    for _ in 0..MAX_POLLS {
        if let Some((id, _)) = socket
            .update_peers()
            .into_iter()
            .find(|(_, state)| *state == PeerState::Connected)
        {
            return id;
        }
        Delay::new(POLL).await;
    }
    panic!("no peer connected");
}

/// Everything received the first time something arrives, so that nothing is dropped
async fn wait_for_packets(socket: &mut WebRtcSocket) -> Vec<(PeerId, Packet)> {
    for _ in 0..MAX_POLLS {
        let packets = socket.channel_mut(0).receive();
        if !packets.is_empty() {
            return packets;
        }
        Delay::new(POLL).await;
    }
    panic!("no packet received");
}

#[wasm_bindgen_test]
async fn exchanges_packets_with_a_native_peer() {
    let mut socket = connect("browser");
    let peer = wait_for_connected_peer(&mut socket).await;

    socket.channel_mut(0).send(b"ping".to_vec().into(), peer);
    assert_eq!(
        wait_for_packets(&mut socket).await,
        vec![(peer, b"pong".to_vec().into())]
    );
}

#[wasm_bindgen_test]
async fn two_browser_peers_exchange_packets() {
    let mut a = connect("browsers");
    let mut b = connect("browsers");
    let b_id = wait_for_connected_peer(&mut a).await;
    let a_id = wait_for_connected_peer(&mut b).await;

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
