use crate::{
    signaling_server::{
        error::{ClientRequestError, SignalingError},
        handlers::WsStateMeta,
        SignalingState,
    },
    topologies::{
        common_logic::{parse_request, spawn_sender_task},
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

use super::common_logic::{try_send, SignalingChannel, StateObj};

#[derive(Debug, Default)]
pub struct ClientServer;

impl SignalingServerBuilder<ClientServer, ClientServerCallbacks, ClientServerState> {
    /// Set a callback triggered on all client websocket connections.
    pub fn on_client_connected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_client_connected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on all client websocket disconnections.
    pub fn on_client_disconnected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_client_disconnected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on host websocket connection.
    pub fn on_host_connected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_host_connected = Callback::from(callback);
        self
    }

    /// Set a callback triggered on host websocket disconnection.
    pub fn on_host_disconnected<F>(mut self, callback: F) -> Self
    where
        F: Fn(PeerId) + 'static,
    {
        self.callbacks.on_host_disconnected = Callback::from(callback);
        self
    }
}

#[async_trait]
impl SignalingTopology<ClientServerCallbacks, ClientServerState> for ClientServer {
    async fn state_machine(upgrade: WsStateMeta<ClientServerCallbacks, ClientServerState>) {
        let WsStateMeta {
            ws,
            mut state,
            callbacks,
        } = upgrade;

        let (ws_sender, mut ws_receiver) = ws.split();
        let sender = spawn_sender_task(ws_sender);

        // Generate a UUID for the user
        let peer_uuid = uuid::Uuid::new_v4().into();

        // Send ID to peer
        let event_text = JsonPeerEvent::IdAssigned(peer_uuid).to_string();
        let event = Message::Text(event_text.clone());
        if let Err(e) = try_send(&sender, event) {
            error!("error sending to {peer_uuid:?}: {e:?}");
            return;
        } else {
            info!("{peer_uuid:?} -> {event_text}");
        };

        // TODO: Make some real way to validate hosts, authentication?
        // Currently, the first person to connect becomes host.
        if state.get_host().is_none() {
            // Set host
            state.set_host(peer_uuid, sender.clone());
            // Lifecycle event: On Host Connected
            callbacks.on_host_connected.emit(peer_uuid);
        } else {
            // Alert server of new user
            let event = Message::Text(JsonPeerEvent::NewPeer(peer_uuid).to_string());
            // Tell host about this new client
            match state.try_send_to_host(event) {
                Ok(_) => {
                    // Add peer to state
                    state.add_client(peer_uuid, sender.clone());
                    // Lifecycle event: On Client Connected
                    callbacks.on_client_connected.emit(peer_uuid);
                }
                Err(e) => {
                    error!("error sending peer {peer_uuid:?} to host: {e:?}");
                    return;
                }
            }
        }

        // Check whether this connection is host
        let is_host = {
            let host = state.get_host();
            host.is_some() && host.unwrap() == peer_uuid
        };

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
                            error!("Error with request: {:?}", e);
                            continue; // Recoverable error
                        }
                    };
                    if is_host {
                        state.reset();
                        // Lifecycle event: On Host Disonnected
                        callbacks.on_host_disconnected.emit(peer_uuid);
                    } else {
                        state.remove_client(&peer_uuid);
                        // Lifecycle event: On Client Disonnected
                        callbacks.on_client_disconnected.emit(peer_uuid);
                    }
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
                    if let Err(e) = {
                        if is_host {
                            state.try_send_to_client(receiver, event)
                        } else {
                            state.try_send_to_host(event)
                        }
                    } {
                        error!("error sending: {:?}", e);
                    }
                }
                PeerRequest::KeepAlive => {
                    // Do nothing. KeepAlive packets are used to protect against users' browsers
                    // disconnecting idle websocket connections.
                }
            }
        }

        if is_host {
            state.reset();
            // Lifecycle event: On Host Disonnected
            callbacks.on_host_disconnected.emit(peer_uuid);
        } else {
            state.remove_client(&peer_uuid);
            // Lifecycle event: On Client Disonnected
            callbacks.on_client_disconnected.emit(peer_uuid);
        }
    }
}

/// Signaling callbacks for client/server topologies
#[derive(Default, Debug, Clone)]
pub struct ClientServerCallbacks {
    /// Triggered on a new client connection to the signaling server
    pub(crate) on_client_connected: Callback<PeerId>,
    /// Triggered on a client disconnection to the signaling server
    pub(crate) on_client_disconnected: Callback<PeerId>,
    /// Triggered on host connection to the signaling server
    pub(crate) on_host_connected: Callback<PeerId>,
    /// Triggered on host disconnection to the signaling server
    pub(crate) on_host_disconnected: Callback<PeerId>,
}
impl SignalingCallbacks for ClientServerCallbacks {}
#[allow(unsafe_code)]
unsafe impl Send for ClientServerCallbacks {}
#[allow(unsafe_code)]
unsafe impl Sync for ClientServerCallbacks {}

/// Signaling server state for client/server topologies
#[derive(Default, Debug, Clone)]
pub struct ClientServerState {
    pub(crate) host: StateObj<Option<(PeerId, SignalingChannel)>>,
    pub(crate) clients: StateObj<HashMap<PeerId, SignalingChannel>>,
}
impl SignalingState for ClientServerState {}

impl ClientServerState {
    /// Get the host
    pub fn get_host(&mut self) -> Option<PeerId> {
        self.host
            .try_lock()
            .unwrap()
            .as_ref()
            .map(|(peer, _)| *peer)
    }

    /// Set host
    pub fn set_host(&mut self, peer: PeerId, sender: SignalingChannel) {
        self.host
            .try_lock()
            .as_mut()
            .unwrap()
            .replace((peer, sender));
    }

    /// Add a client
    pub fn add_client(&mut self, peer: PeerId, sender: SignalingChannel) {
        self.clients
            .try_lock()
            .as_mut()
            .unwrap()
            .insert(peer, sender);
    }

    /// Remove a client from the state if it existed.
    pub fn remove_client(&mut self, peer_id: &PeerId) {
        // Tell host about disconnected clent
        let event = Message::Text(JsonPeerEvent::PeerLeft(*peer_id).to_string());
        match self.try_send_to_host(event) {
            Ok(()) => {
                info!("Notified host of peer remove: {:?}", peer_id)
            }
            Err(e) => {
                error!("Failure sending peer remove to host: {e:?}")
            }
        }
        self.clients.try_lock().as_mut().unwrap().remove(peer_id);
    }

    /// Send a message to a peer without blocking.
    pub fn try_send_to_client(&self, id: PeerId, message: Message) -> Result<(), SignalingError> {
        self.clients
            .try_lock()
            .as_mut()
            .unwrap()
            .get(&id)
            .ok_or_else(|| SignalingError::UnknownPeer)
            .and_then(|sender| try_send(sender, message))
    }

    /// Send a message to the host without blocking.
    pub fn try_send_to_host(&self, message: Message) -> Result<(), SignalingError> {
        self.host
            .try_lock()
            .as_mut()
            .unwrap()
            .as_ref()
            .ok_or_else(|| SignalingError::UnknownPeer)
            .and_then(|(_id, sender)| try_send(sender, message))
    }

    pub fn reset(&mut self) {
        if let Some((host_id, _)) = self.host.try_lock().as_mut().unwrap().take() {
            // Tell each connected peer about the disconnected host.
            let event = Message::Text(JsonPeerEvent::PeerLeft(host_id).to_string());
            let clients = { self.clients.try_lock().unwrap().clone() };
            clients.keys().for_each(|peer_id| {
                match self.try_send_to_client(*peer_id, event.clone()) {
                    Ok(()) => {
                        info!("Sent host peer remove to: {peer_id:?}")
                    }
                    Err(e) => {
                        error!("Failure sending host peer remove to {peer_id:?}: {e:?}")
                    }
                }
            });
        }
        self.clients.try_lock().as_mut().unwrap().clear();
    }
}