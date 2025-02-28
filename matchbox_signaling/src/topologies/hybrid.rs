use crate::{
    signaling_server::{
        error::{ClientRequestError, SignalingError},
        handlers::WsStateMeta,
        SignalingState,
    },
    topologies::{
        common_logic::{parse_request, try_send, SignalingChannel, StateObj},
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
    
/// A client server network topology
#[derive(Debug, Default)]
pub struct Hybrid;

impl SignalingServerBuilder<Hybrid, HybridCallbacks, HybridState> {
/// Set a callback triggered on all super peer websocket connections.
pub fn on_super_peer_connected<F>(mut self, callback: F) -> Self
where
    F: Fn(PeerId) + Send + Sync + 'static,
{
    self.callbacks.on_super_peer_connected = Callback::from(callback);
    self
}

/// Set a callback triggered on all super peer websocket disconnections.
pub fn on_super_peer_disconnected<F>(mut self, callback: F) -> Self
where
    F: Fn(PeerId) + Send + Sync + 'static,
{
    self.callbacks.on_super_peer_disconnected = Callback::from(callback);
    self
}

/// Set a callback triggered on peer websocket connection.
pub fn on_peer_connected<F>(mut self, callback: F) -> Self
where
    F: Fn(PeerId) + Send + Sync + 'static,
{
    self.callbacks.on_peer_connected = Callback::from(callback);
    self
}

/// Set a callback triggered on peer websocket disconnection.
pub fn on_peer_disconnected<F>(mut self, callback: F) -> Self
where
    F: Fn(PeerId) + Send + Sync + 'static,
{
    self.callbacks.on_peer_disconnected = Callback::from(callback);
    self
}
}

#[async_trait]
impl SignalingTopology<HybridCallbacks, HybridState> for Hybrid {
    async fn state_machine(upgrade: WsStateMeta<HybridCallbacks, HybridState>) {
        let WsStateMeta {
            peer_id,
            sender,
            mut receiver,
            mut state,
            callbacks,
        } = upgrade;

        info!("Peer {:?} connected", peer_id);

        // Add peer and determine if they are a super peer
        let is_super_peer = state.add_peer(peer_id.clone(), sender.clone());

        // Trigger appropriate callback
        if is_super_peer {
            info!("Peer {:?} promoted to Super Peer", peer_id);
            callbacks.on_super_peer_connected.emit(peer_id);
        } else {
            callbacks.on_peer_connected.emit(peer_id);
        }

        // Listen for messages from this peer
        while let Some(Ok(message)) = receiver.next().await {
            match message {
                Message::Text(text) => {
                    match serde_json::from_str::<JsonPeerEvent>(&text) {
                        Ok(event) => {
                            // Should be able to find in full_mesh and client_server variants
                            if is_super_peer {
                                info!("Super peer {:?} handling event: {:?}", peer_id, event);
                                // TODO Add Super Peer Messaging
                            } else {
                                info!("Regular peer {:?} handling event: {:?}", peer_id, event);
                                // TODO Add Peer Messaging
                            }
                        }
                        Err(err) => {
                            warn!("Invalid message from {:?}: {:?}", peer_id, err);
                        }
                    }
                }
                Message::Close(_) => break, // Handle disconnection below
                _ => {}
            }
        }

        // Handle disconnection
        let was_super_peer = state.remove_peer(&peer_id);
        if was_super_peer {
            info!("Super peer {:?} disconnected", peer_id);
            callbacks.on_super_peer_disconnected.emit(peer_id);
        } else {
            callbacks.on_peer_disconnected.emit(peer_id);
        }
    }
}


/// Signaling callbacks for hybrid topologies
#[derive(Default, Debug, Clone)]
pub struct HybridCallbacks {
    /// Triggered on a new super peer connection to the signaling server
    pub(crate) on_super_peer_connected: Callback<PeerId>,
    /// Triggered on a super peer disconnection to the signaling server
    pub(crate) on_super_peer_disconnected: Callback<PeerId>,
    /// Triggered on peer connection to the signaling server
    pub(crate) on_peer_connected: Callback<PeerId>,
    /// Triggered on peer disconnection to the signaling server
    pub(crate) on_peer_disconnected: Callback<PeerId>,
}
impl SignalingCallbacks for HybridCallbacks {}

/// Signaling server state for hybrid topologies
#[derive(Default, Debug, Clone)]
pub struct HybridState {
    pub(crate) super_peers: StateObj<HashMap<PeerId, SignalingChannel>>,
    pub(crate) clients: StateObj<HashMap<PeerId, SignalingChannel>>,
    pub(crate) peer_count: StateObj<usize>,
}
impl SignalingState for HybridState {}

impl HybridState {
    // implement functions to be called in state machine logic

    // Adds a new peer and decide if it should be a super peer
    pub fn add_peer(&mut self, peer: PeerId, sender: SignalingChannel) -> bool {
        let mut count = self.peer_count.lock().unwrap();
        let sender_for_super_peer = sender.clone();
        let event = Message::Text(JsonPeerEvent::NewPeer(peer).to_string());
        let clients = { self.clients.lock().unwrap().clone() };
        clients.keys().for_each(|peer_id| {
            if let Err(e) = self.try_send_to_peer(*peer_id, event.clone()) {
                error!("error sending to {peer_id}: {e:?}");
            }
        });

        self.clients.lock().as_mut().unwrap().insert(peer, sender);
        
        // Need to add logic relating to inserting for super peer
        if *count % 5 == 0 {
            self.super_peers.lock().as_mut().unwrap().insert(peer, sender_for_super_peer);
            *count += 1;
            return true; 
        }
        *count += 1;
        return false;
    }
    
    // TODO: Get a super peer for a given peer.
    pub fn get_super_peer(&self) -> Option<(PeerId, SignalingChannel)> {
        let super_peers = self.super_peers.lock().unwrap();
        super_peers
            .iter()
            .next()
            .map(|(id, ch)| (id.clone(), ch.clone()))
    }

    pub fn remove_peer(&self, peer_id: &PeerId) -> bool {
        let mut clients = self.clients.lock();
        let mut super_peers = self.super_peers.lock();
        let mut count = self.peer_count.lock().unwrap();

        // Need to determine if we removed a super peer from the network
        let was_super_peer = super_peers.as_mut().unwrap().remove(peer_id).is_some();
        clients.as_mut().unwrap().remove(peer_id);
        *count -= 1;

        return was_super_peer;
    }

    /// Send a message to a peer without blocking.
    pub fn try_send_to_peer(&self, id: PeerId, message: Message) -> Result<(), SignalingError> {
        self.clients
            .lock()
            .unwrap()
            .get(&id)
            .ok_or_else(|| SignalingError::UnknownPeer)
            .and_then(|sender| try_send(sender, message))
    }

    // TODO: Will need an additional function to handle promotion of a super peer. 


}

