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
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
    
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
impl SignalingTopology<HybridCallbacks, HybridState> for Hybrid{
    async fn state_machine(upgrade: WsStateMeta<HybridCallbacks, HybridState>) {
        let WsStateMeta {
            peer_id,
            sender,
            mut receiver,
            mut state,
            callbacks,
        } = upgrade;

        // Implement state machine logic for hybrid architechture
        if state.get_num_super_peers() < 2 {
            state.add_super_peer(peer_id, sender.clone());
            callbacks.on_super_peer_connected.emit(peer_id);
        } else {
            state.add_child_peer(peer_id, sender.clone());
            if let Some(parent) = state.find_super_peer() {
                let event = Message::Text(JsonPeerEvent::NewPeer(peer_id).to_string());
                match state.try_send_to_super_peer(parent, event) {
                    Ok(_) => {
                        state.connect_child(peer_id, parent);
                        callbacks.on_peer_connected.emit(peer_id);
                    }
                    Err(e) => {
                        error!("error sending peer {peer_id} to super: {e:?}");
                        return;
                    }
                }
            }
            else {
                error!("error finding super peer");
            }
        }

        let super_peer = state.is_super_peer(&peer_id);

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
                    if super_peer {
                        state.remove_super_peer(&peer_id);
                        callbacks.on_super_peer_disconnected.emit(peer_id);
                        // more call backs should happen here
                    } else {
                        state.remove_child_peer(&peer_id);
                        callbacks.on_peer_disconnected.emit(peer_id);
                    }
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
                        .to_string(),
                    );
                    if let Err(e) = {
                        if super_peer {
                            state.try_send_to_super_peer(receiver, event)
                        } else {
                            state.try_send_to_child_peer(receiver, event)
                        }
                    } {
                        error!("error sending signal event: {e:?}");
                    }
                }
                PeerRequest::KeepAlive => {
                    // Do nothing. KeepAlive packets are used to protect against idle websocket
                    // connections getting automatically disconnected, common for reverse proxies.
                }
            }
        }

        if super_peer {
            state.remove_super_peer(&peer_id);
            callbacks.on_super_peer_disconnected.emit(peer_id);
        } else {
            state.remove_child_peer(&peer_id);
            // Lifecycle event: On ChildPeer Disonnected
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

/// Normal peer
#[derive(Debug, Clone)]
pub struct ChildPeer {
    /// Peer id
    pub id: PeerId,
    /// id of parent super peer
    pub parent_id: Option<PeerId>,
    /// Channel to communicate with signaling server
    pub signaling_channel: SignalingChannel,
}

impl ChildPeer {
    /// Creates a new Super Peer instance
    pub fn new(id: PeerId, signaling_channel: SignalingChannel) -> Self {
        ChildPeer {
            id,
            parent_id : None,
            signaling_channel,
        }   
    }
}

/// Super peer
#[derive(Debug, Clone)]
pub struct SuperPeer {
        /// Peer id
        pub id: PeerId,
        /// Set of children peers
        pub children: StateObj<HashSet<PeerId>>,                          
        /// Channel to communicate with signaling server
        pub signaling_channel: SignalingChannel,
}

impl SuperPeer {
    /// Creates a new Super Peer instance
    pub fn new(id: PeerId, signaling_channel: SignalingChannel) -> Self {
        SuperPeer {
            id,
            children: Arc::new(Mutex::new(HashSet::new())),
            signaling_channel,
        }  
    }
}

/// Signaling server state for hybrid topologies
#[derive(Default, Debug, Clone)]
pub struct HybridState {
    pub(crate) super_peers: StateObj<HashMap<PeerId, SuperPeer>>,
    pub(crate) child_peers: StateObj<HashMap<PeerId, ChildPeer>>,
}
impl SignalingState for HybridState {}  

impl HybridState {
    // TODO: implement functions to be called in state machine logic

    /// Get number of super peers connected
    pub fn get_num_super_peers(&self) -> usize {
        self.super_peers.lock().unwrap().iter().len()
    }

    /// Check if peer_id is associated with a super peer
    pub fn is_super_peer(&self, peer: &PeerId) -> bool {
        self.super_peers.lock()
                        .unwrap()
                        .contains_key(peer)
    }

    /// Find optimal super peer to give child
    pub fn find_super_peer(&mut self) -> Option<PeerId> {
        self.super_peers.lock()
                        .unwrap()
                        .iter()
                        .min_by_key(|(&id, sp)| (sp.children.lock().unwrap().len(), id))
                        .map(|(&id, _)| id)
    }

    /// Add a new super peer
    pub fn add_super_peer(&mut self, peer: PeerId, sender: SignalingChannel) {
        // Alert all super peers of new super peer
        let event = Message::Text(JsonPeerEvent::NewPeer(peer).to_string());
        // Safety: Lock must be scoped/dropped to ensure no deadlock with loop
        let super_peers = { self.super_peers.lock().unwrap().clone() };
        super_peers.keys().for_each(|peer_id| {
            if let Err(e) = self.try_send_to_super_peer(*peer_id, event.clone()) {
                error!("error sending to {peer_id}: {e:?}");
            }
        });
        // Safety: All prior locks in this method must be freed prior to this call
        self.super_peers.lock().as_mut().unwrap().insert(peer, SuperPeer::new(peer, sender));
    }

    /// Add child peer
    pub fn add_child_peer(&mut self, peer: PeerId, sender: SignalingChannel) {
        self.child_peers.lock()
                        .as_mut()
                        .unwrap()
                        .insert(peer, ChildPeer::new(peer, sender));
    }

    /// Connect child to super peer
    pub fn connect_child(&mut self, child_peer: PeerId, super_peer: PeerId) {
        if let Some(sp) = self.super_peers.lock().unwrap().get(&super_peer) {
            
            sp.children.lock()
                        .as_mut()
                        .unwrap()
                        .insert(child_peer);   
        }

        if let Some(cp) = self.child_peers.lock().unwrap().get_mut(&child_peer) {
            cp.parent_id = Some(super_peer);
        }            
    }   

    /// Remove child peer
    pub fn remove_child_peer(&mut self, peer: &PeerId) {
        if let Some(cp) = self.child_peers.lock().as_mut().unwrap().remove(peer) {
            if let Some(parent_id) = cp.parent_id {
                let event = Message::Text(JsonPeerEvent::PeerLeft(*peer).to_string());
                match self.try_send_to_super_peer(parent_id, event) {
                    Ok(()) => {
                        info!("Notified parent of child peer remove: {peer}")
                    }
                    Err(e) => {
                        error!("Failure sending peer remove to parent: {e:?}")
                    }
                }
                if let Some(sp) = self.super_peers.lock().unwrap().get(&parent_id) {
                    sp.children.lock().as_mut().unwrap().remove(peer);
                }
            }
        }
    }

    /// Remove super peer
    pub fn remove_super_peer(&mut self, peer: &PeerId) {
        let super_peer = { self.super_peers.lock().as_mut().unwrap().remove(peer) };

        if let Some(super_peer) = super_peer {
            let event = Message::Text(JsonPeerEvent::PeerLeft(super_peer.id).to_string());
            // Safety: Lock must be scoped/dropped to ensure no deadlock with loop
            let super_peers = { self.super_peers.lock().unwrap().clone() };
            super_peers.keys().for_each(
            |peer_id| match self.try_send_to_super_peer(*peer_id, event.clone()) {
                Ok(()) => info!("Sent peer remove to: {peer_id}"),                                      
                    Err(e) => error!("Failure sending peer remove: {e:?}"),
                },
            );

            let children = super_peer.children.lock().unwrap();

            children.iter().for_each(
                |peer_id| match self.try_send_to_child_peer(*peer_id, event.clone()) {
                    Ok(()) => info!("Sent peer remove to: {peer_id}"),                                      
                    Err(e) => error!("Failure sending peer remove: {e:?}"),
                },
            );

            if let Some(recruit_id) = children.iter().next(){ 

                let recruit = { self.child_peers.lock().as_mut().unwrap().remove(recruit_id) };

                if let Some(recruit) = recruit {
                    self.add_super_peer(*recruit_id, recruit.signaling_channel);
                    children.iter().for_each(
                        |child_id| if child_id != recruit_id { 
                            self.connect_child(*child_id, *recruit_id); 
                        }
                    )
                }
            }
        }
        
    }

    /// Send signaling message to super peer
    pub fn try_send_to_super_peer(&self, peer: PeerId, message: Message) -> Result<(), SignalingError> {
        self.super_peers
            .lock()
            .as_mut()
            .unwrap()
            .get(&peer)
            .ok_or_else(|| SignalingError::UnknownPeer)
            .and_then(|sender| try_send(&sender.signaling_channel, message))
        
    }

    /// Send signaling message to child peer
    pub fn try_send_to_child_peer(&self, peer: PeerId, message: Message) -> Result<(), SignalingError> {
        self.child_peers
            .lock()
            .as_mut()
            .unwrap()
            .get(&peer)
            .ok_or_else(|| SignalingError::UnknownPeer)
            .and_then(|sender| try_send(&sender.signaling_channel, message))      
    }
}
