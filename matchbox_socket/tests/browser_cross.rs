//! One side of a cross-browser pair: tests/browser.rs runs this in two browsers at once.

#![cfg(target_arch = "wasm32")]

use futures_timer::Delay;
use matchbox_socket::{ChannelConfig, PeerState, RtcIceServerConfig, WebRtcSocket};
use std::time::Duration;
use wasm_bindgen_futures::spawn_local;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

/// Served by tests/browser.rs
const ROOM_URL: &str = "ws://127.0.0.1:3536/cross";
const POLL: Duration = Duration::from_millis(10);
/// The other browser may take a while to start
const MAX_POLLS: usize = 6000;

#[wasm_bindgen_test]
async fn exchanges_packets_with_the_other_browser() {
    let (mut socket, message_loop) = WebRtcSocket::builder(ROOM_URL)
        .ice_server(RtcIceServerConfig {
            urls: vec![],
            ..Default::default()
        })
        .add_channel(ChannelConfig::reliable())
        .build();
    spawn_local(async move {
        let _ = message_loop.await;
    });

    let mut peer = None;
    for _ in 0..MAX_POLLS {
        peer = socket
            .update_peers()
            .into_iter()
            .find(|(_, state)| *state == PeerState::Connected)
            .map(|(id, _)| id);
        if peer.is_some() {
            break;
        }
        Delay::new(POLL).await;
    }
    let peer = peer.expect("no peer connected");

    socket.channel_mut(0).send(b"hello".to_vec().into(), peer);
    for _ in 0..MAX_POLLS {
        if let Some((from, packet)) = socket.channel_mut(0).receive().pop() {
            assert_eq!(from, peer);
            assert_eq!(&packet[..], b"hello");
            return;
        }
        Delay::new(POLL).await;
    }
    panic!("no packet received");
}
