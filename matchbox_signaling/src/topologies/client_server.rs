use super::{ClientServer, SignalingTopology};
use crate::signaling_server::{
    error::{ClientRequestError, SignalingError},
    handlers::WsStateMeta,
};
use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use futures::{stream::SplitSink, StreamExt};
use matchbox_protocol::{JsonPeerEvent, JsonPeerRequest, PeerEvent, PeerId, PeerRequest};
use std::{collections::HashMap, str::FromStr};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{error, info, warn};

#[async_trait]
impl SignalingTopology<ClientServerState> for ClientServer {
    async fn state_machine(upgrade: WsStateMeta<ClientServerState>) {
        let WsStateMeta {
            ws,
            upgrade_meta,
            mut state,
            callbacks,
        } = upgrade;

        let (ws_sender, mut ws_receiver) = ws.split();
        let sender = spawn_sender_task(ws_sender);

        // Generate a UUID for the user
        let peer_uuid = uuid::Uuid::new_v4().into();
        // Lifecycle event: On Connected
        callbacks.on_peer_connected.emit(peer_uuid);
        {
            state.add_client(peer_uuid, sender.clone());

            let event_text = JsonPeerEvent::IdAssigned(peer_uuid).to_string();
            let event = Message::Text(event_text.clone());

            if let Err(e) = state.try_send(peer_uuid, event) {
                error!("error sending to {peer_uuid:?}: {e:?}");
                return;
            } else {
                info!("{peer_uuid:?} -> {event_text}");
            };

            // TODO: Make some real way to validate hosts, authentication?
            // Currently, the first person to connect becomes host.
            if state.host.is_none() {
                // Set host
                state.host.replace((peer_uuid, sender.clone()));
                info!("SET HOST: {peer_uuid:?}");
            } else {
                // Set client
                let event_text = JsonPeerEvent::NewPeer(peer_uuid).to_string();
                let event = Message::Text(event_text.clone());

                // Tell host about this new client
                if let Err(e) = state.try_send_to_host(event) {
                    error!("error sending peer {peer_uuid:?} to host: {e:?}");
                    return;
                } else {
                    info!("{peer_uuid:?} -> {event_text}");
                }
            }
        }

        // The state machine for the data channel established for this websocket.
        while let Some(request) = ws_receiver.next().await {
            // Parse the message
            let request = match parse_request(request.map_err(ClientRequestError::from)) {
                Ok(request) => request,
                Err(ClientRequestError::Axum(e)) => {
                    // Most likely a ConnectionReset or similar.
                    error!("Axum error while receiving request: {:?}", e);
                    warn!("Severing connection with {peer_uuid:?}");
                    break; // give up on this peer.
                }
                Err(ClientRequestError::Close) => {
                    info!("Received websocket close from {peer_uuid:?}");
                    break;
                }
                Err(e) => {
                    error!("Error untangling request: {:?}", e);
                    continue;
                }
            };

            // Lifecycle event: On Signal
            callbacks.on_signal.emit(request.clone());
            match request {
                PeerRequest::Signal { receiver, data } => {
                    info!("<-- {peer_uuid:?}: {data:?}");
                    let event = Message::Text(
                        JsonPeerEvent::Signal {
                            sender: peer_uuid,
                            data,
                        }
                        .to_string(),
                    );
                    if let Some(peer) = state.clients.get(&receiver) {
                        if let Err(e) = peer.send(Ok(event)) {
                            error!("error sending: {e:?}");
                        }
                    } else {
                        warn!("peer not found ({receiver:?}), ignoring signal");
                    }
                }
                PeerRequest::KeepAlive => {
                    // Do nothing. KeepAlive packets are used to protect against
                    // users' browsers disconnecting idle websocket connections.
                }
            }
        }

        // Lifecycle event: On Connected
        callbacks.on_peer_disconnected.emit(peer_uuid);
        // Peer disconnected or otherwise ended communication.
        let is_host = state.host.is_some() && {
            let (id, _sender) = state.host.as_ref().unwrap();
            *id == peer_uuid
        };
        if is_host {
            let (host_id, _host_sender) = state.host.as_ref().unwrap();
            // Tell each connected peer about the disconnected host.
            let event = Message::Text(JsonPeerEvent::PeerLeft(*host_id).to_string());
            for peer_id in state.clients.keys().filter(|id| *id != host_id) {
                match state.try_send(*peer_id, event.clone()) {
                    Ok(()) => {
                        info!("Sent host peer remove to: {peer_id:?}")
                    }
                    Err(e) => {
                        error!("Failure sending host peer remove to {peer_id:?}: {e:?}")
                    }
                }
            }
            state.reset();
        } else if let Some((removed_peer_id, _removed_peer_sender)) =
            state.remove_client(&peer_uuid)
        {
            if state.host.is_some() {
                // Tell host about disconnected clent
                let event = Message::Text(JsonPeerEvent::PeerLeft(removed_peer_id).to_string());
                match state.try_send_to_host(event) {
                    Ok(()) => {
                        info!("Notified host of peer remove: {:?}", removed_peer_id)
                    }
                    Err(e) => {
                        error!("Failure sending peer remove to host: {e:?}")
                    }
                }
            }
        }
    }
}

fn parse_request(
    request: Result<Message, ClientRequestError>,
) -> Result<JsonPeerRequest, ClientRequestError> {
    let request = request?;

    let request = match request {
        Message::Text(text) => JsonPeerRequest::from_str(&text)?,
        Message::Close(_) => return Err(ClientRequestError::Close),
        _ => return Err(ClientRequestError::UnsupportedType),
    };

    Ok(request)
}

fn spawn_sender_task(
    sender: SplitSink<WebSocket, Message>,
) -> mpsc::UnboundedSender<Result<Message, axum::Error>> {
    let (client_sender, receiver) = mpsc::unbounded_channel();
    tokio::task::spawn(UnboundedReceiverStream::new(receiver).forward(sender));
    client_sender
}

/// Contains the signaling server state
#[derive(Default, Debug, Clone)]
pub struct ClientServerState {
    pub(crate) host: Option<(PeerId, UnboundedSender<Result<Message, axum::Error>>)>,
    pub(crate) clients: HashMap<PeerId, UnboundedSender<Result<Message, axum::Error>>>,
}

impl ClientServerState {
    /// Add a peer, returning peers that already existed
    pub fn add_client(
        &mut self,
        peer: PeerId,
        sender: UnboundedSender<Result<Message, axum::Error>>,
    ) -> HashMap<PeerId, UnboundedSender<Result<Message, axum::Error>>> {
        let prior_peers = self.clients.clone();
        self.clients.insert(peer, sender);
        prior_peers
    }

    /// Remove a peer from the state if it existed, returning the peer removed.
    #[must_use]
    pub fn remove_client(
        &mut self,
        peer_id: &PeerId,
    ) -> Option<(PeerId, UnboundedSender<Result<Message, axum::Error>>)> {
        self.clients
            .remove(peer_id)
            .map(|sender| (peer_id.to_owned(), sender))
    }

    /// Send a message to a peer without blocking.
    pub fn try_send(&self, id: PeerId, message: Message) -> Result<(), SignalingError> {
        let peer = self.clients.get(&id);
        let peer = match peer {
            Some(peer) => peer,
            None => {
                return Err(SignalingError::UnknownPeer);
            }
        };
        peer.send(Ok(message)).map_err(SignalingError::from)
    }

    /// Send a message to the host without blocking.
    pub fn try_send_to_host(&self, message: Message) -> Result<(), SignalingError> {
        let peer = &self.host;
        let (_id, sender) = match peer {
            Some(peer) => peer,
            None => {
                return Err(SignalingError::UnknownPeer);
            }
        };
        sender.send(Ok(message)).map_err(SignalingError::from)
    }

    pub fn reset(&mut self) {
        self.host = None;
        self.clients.clear();
    }
}
