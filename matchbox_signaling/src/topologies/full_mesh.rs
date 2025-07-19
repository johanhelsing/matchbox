use crate::{
    Callback, SignalingCallbacks, SignalingServerBuilder,
    signaling_server::{
        SignalingState,
        error::{ClientRequestError, SignalingError},
        handlers::WsStateMeta,
    },
    topologies::{
        SignalingTopology,
        common_logic::{SignalingChannel, StateObj, parse_request, try_send},
    },
};
use async_trait::async_trait;
use axum::extract::ws::Message;
use futures::StreamExt;
use matchbox_protocol::{JsonPeerEvent, PeerId, PeerRequest};
use std::collections::HashMap;
use tracing::{error, info, warn};

/// A full mesh network topolgoy
#[derive(Debug, Default)]
pub struct FullMesh;

impl SignalingServerBuilder<FullMesh, FullMeshCallbacks, FullMeshState> {
    /// Set a callback triggered on all websocket connections.
    pub fn on_peer_connected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + Send + Sync + 'static,
    {
        self.callbacks.on_peer_connected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on all websocket disconnections.
    pub fn on_peer_disconnected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + Send + Sync + 'static,
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
            room,
            peer_id,
            sender,
            mut receiver,
            mut state,
            callbacks,
        } = upgrade;
        // Add peer to state
        state.add_peer(peer_id, sender.clone(), room.clone());
        // Lifecycle event: On Connected
        callbacks.on_peer_connected.emit(peer_id);

        // The state machine for the data channel established for this websocket.
        while let Some(request) = receiver.next().await {
            let request = match parse_request(request) {
                Ok(request) => request,
                Err(e) => {
                    match e {
                        ClientRequestError::Axum(_) => {
                            // Most likely a ConnectionReset or similar.
                            warn!("Unrecoverable error with {peer_id}: {e:?}");
                        }
                        ClientRequestError::Close => {
                            info!("Connection closed by {peer_id}");
                        }
                        ClientRequestError::Json(_) | ClientRequestError::UnsupportedType(_) => {
                            error!("Error with request: {e:?}");
                            continue; // Recoverable error
                        }
                    };
                    state.remove_peer(&peer_id);
                    // Lifecycle event: On Disonnected
                    callbacks.on_peer_disconnected.emit(peer_id);
                    return;
                }
            };

            match request {
                PeerRequest::Signal { receiver, data } => {
                    let event = Message::Text(
                        JsonPeerEvent::Signal {
                            sender: peer_id,
                            data,
                        }
                        .to_string()
                        .into(),
                    );
                    if let Err(e) = state.try_send_to_peer(receiver, event, room.clone()) {
                        error!("error sending: {e:?}");
                    }
                }
                PeerRequest::KeepAlive => {
                    // Do nothing. KeepAlive packets are used to protect against idle websocket
                    // connections getting automatically disconnected, common for reverse proxies.
                }
            }
        }

        // Peer disconnected or otherwise ended communication.
        state.remove_peer(&peer_id);
        // Lifecycle event: On Disconnected
        callbacks.on_peer_disconnected.emit(peer_id);
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

/// Signaling server isolated registry of peers, rooms and channels
#[derive(Default, Debug, Clone)]
pub struct IsolatedMeshState {
    rooms : HashMap<String, HashMap<PeerId, SignalingChannel>>,
    peers: HashMap<PeerId, String>,
}

/// Signaling server state for full mesh topologies
#[derive(Default, Debug, Clone)]
pub struct FullMeshState {
    pub(crate) state: StateObj<IsolatedMeshState>
}
impl SignalingState for FullMeshState {}

impl FullMeshState {
    /// Add a peer, returning peers that already existed
    pub fn add_peer(&mut self, peer: PeerId, sender: SignalingChannel, room: String) {
        // Alert all peers of new user
        let event = Message::Text(JsonPeerEvent::NewPeer(peer).to_string().into());

        // Create the room if needed
        if !self.state.lock().as_ref().unwrap().rooms.contains_key(&room) {
            self.state.lock().as_mut().unwrap().rooms.insert(room.clone(), HashMap::new());
        }

        // Safety: Lock must be scoped/dropped to ensure no deadlock with loop
        // let rooms = { self.peers.lock().unwrap().clone() };
        let state = { self.state.lock().unwrap().clone() };
        // let rooms = self.peers.lock();
        state.rooms.iter().for_each(|(name,peers)| {
            if *name == room {
                peers.keys().for_each(|peer_id| {
                    if let Err(e) = self.try_send_to_peer(*peer_id, event.clone(), room.clone()) {
                        error!("error sending to {peer_id}: {e:?}");
                    }
                });
            }
        });

        // Safety: All prior locks in this method must be freed prior to this call
        self.state.lock().as_mut().unwrap().rooms.get_mut(&room).unwrap().insert(peer, sender);
        self.state.lock().as_mut().unwrap().peers.insert(peer,room.clone());
    }

    /// Remove a peer from the state if it existed, returning the peer removed.
    pub fn remove_peer(&mut self, peer_id: &PeerId) {
        let peer_room = self.state.lock().as_ref().unwrap().peers.get(peer_id).unwrap().clone();
        self.state.lock().as_mut().unwrap().peers.remove(peer_id);
        let removed_peer = self
            .state
            .lock()
            .as_mut()
            .unwrap()
            .rooms
            .get_mut(&peer_room).unwrap().remove(peer_id).map(|sender| (*peer_id, sender));
        if let Some((peer_id, _sender)) = removed_peer {
            // Tell each connected peer about the disconnected peer.
            let event = Message::Text(JsonPeerEvent::PeerLeft(peer_id).to_string().into());
            // Safety: Lock must be scoped/dropped to ensure no deadlock with loop
            let state = { self.state.lock().unwrap().clone() };
            state.rooms.get(&peer_room).unwrap().iter().for_each(|(peer_id,_)| {
                match self.try_send_to_peer(*peer_id, event.clone(), peer_room.to_string()) {
                    Ok(()) => info!("Sent peer remove to: {peer_id}"),
                    Err(e) => error!("Failure sending peer remove: {e:?}"),
                }
            });
        }
    }

    /// Send a message to a peer without blocking.
    pub fn try_send_to_peer(&self, id: PeerId, message: Message, room:String) -> Result<(), SignalingError> {
        self.state
            .lock()
            .unwrap()
            .rooms
            .get(&room)
            .unwrap()
            .get(&id)
            .ok_or_else(|| SignalingError::UnknownPeer)
            .and_then(|sender| try_send(sender, message))
    }
}
