//! The peer states a native socket reports for scripted signaling events, no signaling server or
//! network involved.

#![cfg(not(target_arch = "wasm32"))]

use matchbox_socket::{
    ChannelConfig, PeerEvent, PeerId, PeerRequest, PeerSignal, PeerState, RtcIceServerConfig,
    SignalingError, Signaller, SignallerBuilder, WebRtcSocket, async_trait::async_trait,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::{sleep, timeout};

const TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(10);

/// Hands the socket the given events, then nothing. Drops whatever the socket sends.
#[derive(Debug)]
struct ScriptedSignallerBuilder(Mutex<Option<Vec<PeerEvent>>>);

#[async_trait]
impl SignallerBuilder for ScriptedSignallerBuilder {
    async fn new_signaller(
        &self,
        _attempts: Option<u16>,
        _room_url: String,
    ) -> Result<Box<dyn Signaller>, SignalingError> {
        let events = self.0.lock().unwrap().take().expect("one signaller only");
        Ok(Box::new(ScriptedSignaller(events.into_iter())))
    }
}

struct ScriptedSignaller(std::vec::IntoIter<PeerEvent>);

#[async_trait]
impl Signaller for ScriptedSignaller {
    async fn send(&mut self, _request: PeerRequest) -> Result<(), SignalingError> {
        Ok(())
    }

    async fn next_message(&mut self) -> Result<PeerEvent, SignalingError> {
        match self.0.next() {
            Some(event) => Ok(event),
            None => std::future::pending().await,
        }
    }
}

fn peer_id(n: u8) -> PeerId {
    serde_json::from_str(&format!("\"00000000-0000-0000-0000-0000000000{n:02x}\"")).unwrap()
}

fn scripted_socket(events: Vec<PeerEvent>) -> WebRtcSocket {
    let (socket, message_loop) = WebRtcSocket::builder("ws://scripted.invalid")
        .signaller_builder(Arc::new(ScriptedSignallerBuilder(Mutex::new(Some(events)))))
        .ice_server(RtcIceServerConfig {
            urls: vec![],
            ..Default::default()
        })
        .add_channel(ChannelConfig::reliable())
        .build();
    tokio::spawn(message_loop);
    socket
}

/// Every change until a peer is disconnected
async fn changes_until_disconnected(socket: &mut WebRtcSocket) -> Vec<(PeerId, PeerState)> {
    let mut changes = Vec::new();
    timeout(TIMEOUT, async {
        loop {
            changes.extend(socket.update_peers());
            if changes.iter().any(|(_, s)| *s == PeerState::Disconnected) {
                return;
            }
            sleep(POLL).await;
        }
    })
    .await
    .expect("no peer disconnected");
    changes
}

#[tokio::test]
async fn offered_peer_is_connecting_until_it_leaves() {
    let peer = peer_id(2);
    let mut socket = scripted_socket(vec![
        PeerEvent::IdAssigned(peer_id(1)),
        PeerEvent::NewPeer(peer),
        PeerEvent::PeerLeft(peer),
    ]);

    assert_eq!(
        changes_until_disconnected(&mut socket).await,
        vec![
            (peer, PeerState::Connecting),
            (peer, PeerState::Disconnected)
        ]
    );
    assert_eq!(socket.connected_peers().count(), 0);
}

#[tokio::test]
async fn answered_peer_is_connecting_once_until_it_leaves() {
    let peer = peer_id(2);
    let signal = || PeerEvent::Signal {
        sender: peer,
        data: PeerSignal::IceCandidate("null".to_string()),
    };
    let mut socket = scripted_socket(vec![
        PeerEvent::IdAssigned(peer_id(1)),
        // The first signal starts the handshake, the second goes to it
        signal(),
        signal(),
        PeerEvent::PeerLeft(peer),
    ]);

    assert_eq!(
        changes_until_disconnected(&mut socket).await,
        vec![
            (peer, PeerState::Connecting),
            (peer, PeerState::Disconnected)
        ]
    );
    assert_eq!(socket.connected_peers().count(), 0);
}
