//! tests/nat_browser.rs with a TURN server configured too, for the symmetric NAT tests.

#![cfg(target_arch = "wasm32")]

use futures_timer::Delay;
use matchbox_socket::{
    CandidatePair, ChannelConfig, Packet, PeerId, PeerState, RtcIceServerConfig, WebRtcSocket,
};
use std::time::Duration;
use wasm_bindgen_futures::spawn_local;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

/// The servers of tests/nat.rs
const SIGNALING_URL: &str = "ws://10.0.0.1:3536/nat";
const STUN_URL: &str = "stun:10.0.0.1:3478";
const TURN_URL: &str = "turn:10.0.0.1:3479";
const POLL: Duration = Duration::from_millis(10);
/// 20 seconds
const MAX_POLLS: usize = 2000;

#[wasm_bindgen_test]
async fn nat_peer() {
    let (mut socket, message_loop) = WebRtcSocket::builder(SIGNALING_URL)
        .ice_server(RtcIceServerConfig {
            urls: vec![STUN_URL.to_string(), TURN_URL.to_string()],
            username: Some("matchbox".to_string()),
            credential: Some("hunter2".to_string()),
        })
        .add_channel(ChannelConfig::reliable())
        .build();
    spawn_local(async move {
        let _ = message_loop.await;
    });

    let peer = wait_for_peer(&mut socket, PeerState::Connected).await;
    // Only used behind symmetric NATs, where the relay is the only way through
    let pair = wait_for_candidate_pair(&mut socket, peer).await;
    assert!(pair.is_relayed(), "{pair:?}");

    if socket.id().unwrap() < peer {
        socket.channel_mut(0).send(b"ping".to_vec().into(), peer);
        assert_eq!(
            wait_for_packets(&mut socket).await,
            vec![(peer, b"pong".to_vec().into())]
        );
    } else {
        assert_eq!(
            wait_for_packets(&mut socket).await,
            vec![(peer, b"ping".to_vec().into())]
        );
        socket.channel_mut(0).send(b"pong".to_vec().into(), peer);
        wait_for_peer(&mut socket, PeerState::Disconnected).await;
    }
}

async fn wait_for_peer(socket: &mut WebRtcSocket, state: PeerState) -> PeerId {
    for _ in 0..MAX_POLLS {
        if let Some((id, _)) = socket.update_peers().into_iter().find(|(_, s)| *s == state) {
            return id;
        }
        Delay::new(POLL).await;
    }
    panic!("no peer became {state:?}");
}

async fn wait_for_candidate_pair(socket: &mut WebRtcSocket, peer: PeerId) -> CandidatePair {
    for _ in 0..MAX_POLLS {
        socket.update_peers();
        if let Some(pair) = socket.candidate_pair(peer) {
            return pair;
        }
        Delay::new(POLL).await;
    }
    panic!("candidate pair never reported");
}

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
