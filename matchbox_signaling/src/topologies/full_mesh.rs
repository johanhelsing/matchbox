use crate::{
    signaling_server::{
        error::{ClientRequestError, SignalingError},
        handlers::WsStateMeta,
        SignalingState,
    },
    topologies::{
        common_logic::{parse_request, spawn_sender_task, StateObj},
        SignalingTopology,
    },
    Callback, SignalingCallbacks, SignalingServerBuilder,
};
use async_trait::async_trait;
use axum::extract::ws::Message;
use futures::StreamExt;
use matchbox_protocol::{JsonPeerEvent, PeerId, PeerRequest};
use std::collections::HashMap;
use tracing::{error, info, warn};

use super::common_logic::{try_send, SignalingChannel};

#[derive(Debug, Default)]
pub struct FullMesh;

impl SignalingServerBuilder<FullMesh, FullMeshCallbacks, FullMeshState> {
    /// Set a callback triggered on all websocket connections.
    pub fn on_peer_connected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_peer_connected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on all websocket disconnections.
    pub fn on_peer_disconnected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_peer_disconnected = Callback::from(callback);
        self
    }
}

#[async_trait]
impl SignalingTopology<FullMeshCallbacks, FullMeshState> for FullMesh {
    /// One of these handlers is spawned for every web socket.
    async fn state_machine(upgrade: WsStateMeta<FullMeshCallbacks, FullMeshState>) {
        let WsStateMeta {
            ws,
            mut state,
            callbacks,
        } = upgrade;

        let (ws_sender, mut ws_receiver) = ws.split();
        let sender = spawn_sender_task(ws_sender);

        let peer_uuid = uuid::Uuid::new_v4().into();

        // Send ID to peer
        let event_text = JsonPeerEvent::IdAssigned(peer_uuid).to_string();
        let event = Message::Text(event_text.clone());
        if let Err(e) = try_send(&sender, event) {
            error!("error sending to {peer_uuid:?}: {e:?}");
        } else {
            info!("{peer_uuid:?} -> {event_text:?}");
        };

        // Add peer to state
        state.add_peer(peer_uuid, sender.clone());
        // Lifecycle event: On Connected
        callbacks.on_peer_connected.emit(peer_uuid);

        // The state machine for the data channel established for this websocket.
        while let Some(request) = ws_receiver.next().await {
            let request = match parse_request(request) {
                Ok(request) => request,
                Err(e) => {
                    match e {
                        ClientRequestError::Axum(_) => {
                            // Most likely a ConnectionReset or similar.
                            warn!("Unrecoverable error with {peer_uuid:?}: {e:?}");
                        }
                        ClientRequestError::Close => {
                            info!("Connection closed by {peer_uuid:?}");
                        }
                        ClientRequestError::Json(_) | ClientRequestError::UnsupportedType(_) => {
                            error!("Error with request: {e:?}");
                            continue; // Recoverable error
                        }
                    };
                    state.remove_peer(&peer_uuid);
                    // Lifecycle event: On Disonnected
                    callbacks.on_peer_disconnected.emit(peer_uuid);
                    return;
                }
            };

            match request {
                PeerRequest::Signal { receiver, data } => {
                    let event = Message::Text(
                        JsonPeerEvent::Signal {
                            sender: peer_uuid,
                            data,
                        }
                        .to_string(),
                    );
                    if let Err(e) = state.try_send_to_peer(receiver, event) {
                        error!("error sending: {:?}", e);
                    }
                }
                PeerRequest::KeepAlive => {
                    // Do nothing. KeepAlive packets are used to protect against users' browsers
                    // disconnecting idle websocket connections.
                }
            }
        }

        // Peer disconnected or otherwise ended communication.
        state.remove_peer(&peer_uuid);
        // Lifecycle event: On Disconnected
        callbacks.on_peer_disconnected.emit(peer_uuid);
    }
}

/// Signaling callbacks for full mesh topologies
#[derive(Default, Debug, Clone)]
pub struct FullMeshCallbacks {
    /// Triggered on a new connection to the signaling server
    pub(crate) on_peer_connected: Callback<PeerId>,
    /// Triggered on a disconnection to the signaling server
    pub(crate) on_peer_disconnected: Callback<PeerId>,
}
impl SignalingCallbacks for FullMeshCallbacks {}
#[allow(unsafe_code)]
unsafe impl Send for FullMeshCallbacks {}
#[allow(unsafe_code)]
unsafe impl Sync for FullMeshCallbacks {}

/// Signaling server state for full mesh topologies
#[derive(Default, Debug, Clone)]
pub struct FullMeshState {
    pub peers: StateObj<HashMap<PeerId, SignalingChannel>>,
}
impl SignalingState for FullMeshState {}

impl FullMeshState {
    /// Add a peer, returning peers that already existed
    pub fn add_peer(&mut self, peer: PeerId, sender: SignalingChannel) {
        // Alert all peers of new user
        let event = Message::Text(JsonPeerEvent::NewPeer(peer).to_string());
        let peers = { self.peers.try_lock().unwrap().clone() };
        peers.keys().for_each(|peer_id| {
            if let Err(e) = self.try_send_to_peer(*peer_id, event.clone()) {
                error!("error sending to {peer_id:?}: {e:?}");
            }
        });
        self.peers.try_lock().as_mut().unwrap().insert(peer, sender);
    }

    /// Remove a peer from the state if it existed, returning the peer removed.
    pub fn remove_peer(&mut self, peer_id: &PeerId) {
        let removed_peer = self
            .peers
            .try_lock()
            .as_mut()
            .unwrap()
            .remove(peer_id)
            .map(|sender| (*peer_id, sender));
        if let Some((peer_id, _sender)) = removed_peer {
            // Tell each connected peer about the disconnected peer.
            let event = Message::Text(JsonPeerEvent::PeerLeft(peer_id).to_string());
            let peers = { self.peers.try_lock().unwrap().clone() };
            peers.keys().for_each(
                |peer_id| match self.try_send_to_peer(*peer_id, event.clone()) {
                    Ok(()) => info!("Sent peer remove to: {peer_id:?}"),
                    Err(e) => error!("Failure sending peer remove: {e:?}"),
                },
            );
        }
    }

    /// Send a message to a peer without blocking.
    pub fn try_send_to_peer(&self, id: PeerId, message: Message) -> Result<(), SignalingError> {
        self.peers
            .try_lock()
            .unwrap()
            .get(&id)
            .ok_or_else(|| SignalingError::UnknownPeer)
            .and_then(|sender| try_send(sender, message))
    }
}