use async_trait::async_trait;
use axum::extract::ws::Message;
use futures::StreamExt;
use matchbox_protocol::{JsonPeerEvent, PeerRequest};
use matchbox_signaling::{
    common_logic::parse_request, ClientRequestError, NoCallbacks, SignalingTopology, WsStateMeta,
};
use std::collections::{HashMap, HashSet};
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct Peer {
    pub uuid: Uuid,
    pub sender: tokio::sync::mpsc::Sender<Result<axum::extract::ws::Message, axum::Error>>,
    pub room: String,
}

#[derive(Default)]
pub struct ServerState {
    peers: HashMap<Uuid, Peer>,
    rooms: HashMap<String, HashSet<Uuid>>,
}

impl ServerState {
    pub fn assign_peer_to_room(&mut self, peer_id: Uuid, room_id: String) -> String {
        self.rooms.entry(room_id.clone()).or_default().insert(peer_id);
        room_id
    }

    pub fn get_peers_in_room(&self, room_id: &str) -> Vec<&Peer> {
        self.rooms
            .get(room_id)
            .map(|peers| peers.iter().filter_map(|id| self.peers.get(id)).collect())
            .unwrap_or_default()
    }

    pub fn remove_peer(&mut self, peer_id: &Uuid) {
        if let Some(peer) = self.peers.remove(peer_id) {
            if let Some(peers_in_room) = self.rooms.get_mut(&peer.room) {
                peers_in_room.remove(peer_id);
            }
        }
    }

    pub fn get_peer(&self, peer_id: &Uuid) -> Option<&Peer> {
        self.peers.get(peer_id)
    }

    pub fn add_peer(&mut self, peer: Peer) {
        self.peers.insert(peer.uuid, peer);
    }
}

#[derive(Debug, Default)]
pub struct HybridTopology;

#[async_trait]
impl SignalingTopology<NoCallbacks, ServerState> for HybridTopology {
    async fn state_machine(upgrade: WsStateMeta<NoCallbacks, ServerState>) {
        let WsStateMeta {
            peer_id,
            sender,
            mut receiver,
            mut state,
            ..
        } = upgrade;

        // Assign the peer to a room (e.g., based on query parameters or round-robin)
        let room_id = state.assign_peer_to_room(peer_id, "default".to_string());
        info!("Peer {} assigned to room {}", peer_id, room_id);

        // Add the peer to the server state
        let peer = Peer {
            uuid: peer_id,
            sender: sender.clone(),
            room: room_id.clone(),
        };
        state.add_peer(peer);

        // Notify other peers in the room about the new peer
        let peers_in_room = state.get_peers_in_room(&room_id);
        let event = Message::Text(JsonPeerEvent::NewPeer(peer_id).to_string());
        for peer in peers_in_room {
            if peer.uuid != peer_id {
                if let Err(e) = peer.sender.send(Ok(event.clone())) {
                    error!("Failed to notify peer {}: {:?}", peer.uuid, e);
                }
            }
        }

        // Handle incoming messages from the peer
        while let Some(request) = receiver.next().await {
            let request = match parse_request(request) {
                Ok(request) => request,
                Err(e) => {
                    match e {
                        ClientRequestError::Axum(_) => {
                            warn!("Connection error with {}: {:?}", peer_id, e);
                            break;
                        }
                        ClientRequestError::Close => {
                            info!("Peer {} disconnected", peer_id);
                            break;
                        }
                        _ => {
                            error!("Invalid request from {}: {:?}", peer_id, e);
                            continue;
                        }
                    }
                }
            };

            match request {
                PeerRequest::Signal { receiver, data } => {
                    // Relay the signal to the target peer
                    let event = Message::Text(
                        JsonPeerEvent::Signal {
                            sender: peer_id,
                            data,
                        }
                        .to_string(),
                    );
                    if let Some(peer) = state.get_peer(&receiver) {
                        if let Err(e) = peer.sender.send(Ok(event)) {
                            error!("Failed to relay signal to {}: {:?}", receiver, e);
                        }
                    } else {
                        warn!("Peer {} not found", receiver);
                    }
                }
                PeerRequest::KeepAlive => {
                    // Do nothing
                }
            }
        }

        // Peer disconnected
        info!("Peer {} left room {}", peer_id, room_id);
        state.remove_peer(&peer_id);

        // Notify remaining peers in the room
        let peers_in_room = state.get_peers_in_room(&room_id);
        let event = Message::Text(JsonPeerEvent::PeerLeft(peer_id).to_string());
        for peer in peers_in_room {
            if let Err(e) = peer.sender.send(Ok(event.clone())) {
                error!("Failed to notify peer {}: {:?}", peer.uuid, e);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // Initialize the server state
    let state = ServerState::default();

    // Start the signaling server
    let server = matchbox_signaling::SignalingServerBuilder::new(
        ([0, 0, 0, 0], 3536), // Listen on all interfaces, port 3536
        HybridTopology,
        state,
    )
    .build();

    println!("Super peer server running on ws://0.0.0.0:3536");
    server.serve().await.unwrap();
}