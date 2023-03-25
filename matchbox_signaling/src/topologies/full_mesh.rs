use crate::{
    signaling_server::{
        error::{ClientRequestError, SignalingError},
        handlers::WsStateMeta,
    },
    topologies::{FullMesh, SignalingTopology},
};
use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use futures::{stream::SplitSink, StreamExt};
use matchbox_protocol::{JsonPeerEvent, JsonPeerRequest, PeerId, PeerRequest};
use std::{collections::HashMap, str::FromStr};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{error, info, warn};

#[async_trait]
impl SignalingTopology<FullMeshState> for FullMesh {
    /// One of these handlers is spawned for every web socket.
    async fn state_machine(upgrade: WsStateMeta<FullMeshState>) {
        let WsStateMeta {
            ws,
            upgrade_meta,
            mut state,
            callbacks,
        } = upgrade;

        let (ws_sender, mut ws_receiver) = ws.split();
        let sender = spawn_sender_task(ws_sender);

        let peer_uuid = uuid::Uuid::new_v4().into();
        // Lifecycle event: On Connected
        callbacks.on_peer_connected.emit(peer_uuid);
        {
            let peers = state.add_peer(peer_uuid, sender.clone());

            let event_text = JsonPeerEvent::IdAssigned(peer_uuid).to_string();
            let event = Message::Text(event_text.clone());

            if let Err(e) = state.try_send(peer_uuid, event) {
                error!("error sending to {peer_uuid:?}: {e:?}");
            } else {
                info!("{:?} -> {:?}", peer_uuid, event_text);
            };

            let event_text = JsonPeerEvent::NewPeer(peer_uuid).to_string();
            let event = Message::Text(event_text.clone());

            for (peer_id, _) in peers {
                // Tell everyone about this new peer
                if let Err(e) = state.try_send(peer_id, event.clone()) {
                    error!("error sending to {peer_id:?}: {e:?}");
                } else {
                    info!("{:?} -> {:?}", peer_id, event_text);
                }
            }
        }

        // The state machine for the data channel established for this websocket.
        while let Some(request) = ws_receiver.next().await {
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
                    let event = Message::Text(
                        JsonPeerEvent::Signal {
                            sender: peer_uuid,
                            data,
                        }
                        .to_string(),
                    );
                    if let Some(peer) = state.peers.get(&receiver) {
                        if let Err(e) = peer.send(Ok(event)) {
                            error!("error sending: {:?}", e);
                        }
                    } else {
                        warn!("peer not found ({receiver:?}), ignoring signal");
                    }
                }
                PeerRequest::KeepAlive => {
                    // Do nothing. KeepAlive packets are used to protect against users' browsers
                    // disconnecting idle websocket connections.
                }
            }
        }

        // Lifecycle event: On Connected
        callbacks.on_peer_disconnected.emit(peer_uuid);
        // Peer disconnected or otherwise ended communication.
        if let Some((removed_peer, _sender)) = state.remove_peer(&peer_uuid) {
            // Tell each connected peer about the disconnected peer.
            let event = Message::Text(JsonPeerEvent::PeerLeft(removed_peer).to_string());
            for (peer_id, _) in state.peers.iter() {
                match state.try_send(*peer_id, event.clone()) {
                    Ok(()) => info!("Sent peer remove to: {:?}", peer_id),
                    Err(e) => error!("Failure sending peer remove: {e:?}"),
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
pub struct FullMeshState {
    pub peers: HashMap<PeerId, UnboundedSender<Result<Message, axum::Error>>>,
}

impl FullMeshState {
    /// Add a peer, returning peers that already existed
    pub fn add_peer(
        &mut self,
        peer: PeerId,
        sender: UnboundedSender<Result<Message, axum::Error>>,
    ) -> HashMap<PeerId, UnboundedSender<Result<Message, axum::Error>>> {
        let prior_peers = self.peers.clone();
        self.peers.insert(peer, sender);
        prior_peers
    }

    /// Remove a peer from the state if it existed, returning the peer removed.
    #[must_use]
    pub fn remove_peer(
        &mut self,
        peer_id: &PeerId,
    ) -> Option<(PeerId, UnboundedSender<Result<Message, axum::Error>>)> {
        self.peers
            .remove(peer_id)
            .map(|sender| (peer_id.to_owned(), sender))
    }

    /// Send a message to a peer without blocking.
    pub fn try_send(&self, id: PeerId, message: Message) -> Result<(), SignalingError> {
        let peer = self.peers.get(&id);
        let peer = match peer {
            Some(peer) => peer,
            None => {
                return Err(SignalingError::UnknownPeer);
            }
        };
        peer.send(Ok(message)).map_err(SignalingError::from)
    }
}
