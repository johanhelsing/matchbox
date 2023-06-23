use axum::{extract::ws::Message, Error};
use matchbox_protocol::PeerId;
use matchbox_signaling::{
    common_logic::{self},
    SignalingError, SignalingState,
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};

#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RoomId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RequestedRoom {
    pub id: RoomId,
    pub next: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Peer {
    pub uuid: PeerId,
    pub room: RequestedRoom,
    pub sender: tokio::sync::mpsc::UnboundedSender<std::result::Result<Message, Error>>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct ServerState {
    clients_waiting: HashMap<SocketAddr, RequestedRoom>,
    clients_in_queue: HashMap<PeerId, RequestedRoom>,
    clients: HashMap<PeerId, Peer>,
    rooms: HashMap<RequestedRoom, HashSet<PeerId>>,
}
impl SignalingState for ServerState {}

impl ServerState {
    /// Add a waiting client to matchmaking
    pub fn add_waiting_client(&mut self, origin: SocketAddr, room: RequestedRoom) {
        self.clients_waiting.insert(origin, room);
    }

    /// Assign a peer id to a waiting client
    pub fn assign_id_to_waiting_client(&mut self, origin: SocketAddr, peer_id: PeerId) {
        let room = self
            .clients_waiting
            .remove(&origin)
            .expect("waiting client");
        self.clients_in_queue.insert(peer_id, room);
    }

    /// Remove the waiting peer, returning the peer's requested room
    pub fn remove_waiting_peer(&mut self, peer_id: PeerId) -> RequestedRoom {
        self.clients_in_queue
            .remove(&peer_id)
            .expect("waiting peer")
    }

    /// Add a peer, returning the peers already in room
    pub fn add_peer(&mut self, peer: Peer) -> Vec<PeerId> {
        let peer_id = peer.uuid;
        let room = peer.room.clone();
        self.clients.insert(peer.uuid, peer);

        let peers = self.rooms.entry(room.clone()).or_default();
        let prev_peers = peers.iter().cloned().collect();

        match room.next {
            None => {
                peers.insert(peer_id);
            }
            Some(num_players) => {
                if peers.len() == num_players - 1 {
                    peers.clear(); // room is complete
                } else {
                    peers.insert(peer_id);
                }
            }
        };

        prev_peers
    }

    /// Get a peer
    pub fn get_peer(&self, peer_id: &PeerId) -> Option<Peer> {
        self.clients.get(peer_id).cloned()
    }

    /// Get the peers in a room currently
    pub fn get_room_peers(&self, room: &RequestedRoom) -> Vec<PeerId> {
        self.rooms
            .get(room)
            .map(|room_peers| room_peers.iter().copied().collect::<Vec<PeerId>>())
            .unwrap_or_default()
    }

    /// Remove a peer from the state if it existed, returning the peer removed.
    #[must_use]
    pub fn remove_peer(&mut self, peer_id: &PeerId) -> Option<Peer> {
        let peer = self.clients.remove(peer_id);
        if let Some(ref peer) = peer {
            // Best effort to remove peer from their room
            _ = self
                .rooms
                .get_mut(&peer.room)
                .map(|room| room.remove(peer_id));
        }
        peer
    }

    /// Send a message to a peer without blocking.
    pub fn try_send(&self, id: PeerId, message: Message) -> Result<(), SignalingError> {
        match self.clients.get(&id) {
            Some(peer) => Ok(common_logic::try_send(&peer.sender, message)?),
            None => Err(SignalingError::UnknownPeer),
        }
    }
}
