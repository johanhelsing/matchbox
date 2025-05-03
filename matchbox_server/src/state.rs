use axum::{Error, extract::ws::Message};
use matchbox_protocol::PeerId;
use matchbox_signaling::{
    SignalingError, SignalingState,
    common_logic::{self, StateObj},
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};
use tokio::sync::mpsc::UnboundedSender;

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
    pub sender: UnboundedSender<Result<Message, Error>>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct ServerState {
    clients_waiting: StateObj<HashMap<SocketAddr, RequestedRoom>>,
    clients_in_queue: StateObj<HashMap<PeerId, RequestedRoom>>,
    clients: StateObj<HashMap<PeerId, Peer>>,
    rooms: StateObj<HashMap<RequestedRoom, HashSet<PeerId>>>,
    matched_by_next: StateObj<HashSet<Vec<PeerId>>>,
}
impl SignalingState for ServerState {}

impl ServerState {
    /// Add a waiting client to matchmaking
    pub fn add_waiting_client(&mut self, origin: SocketAddr, room: RequestedRoom) {
        self.clients_waiting.lock().unwrap().insert(origin, room);
    }

    /// Assign a peer id to a waiting client
    pub fn assign_id_to_waiting_client(&mut self, origin: SocketAddr, peer_id: PeerId) {
        let room = {
            let mut lock = self.clients_waiting.lock().unwrap();
            lock.remove(&origin).expect("waiting client")
        };
        {
            let mut lock = self.clients_in_queue.lock().unwrap();
            lock.insert(peer_id, room);
        }
    }

    /// Remove the waiting peer, returning the peer's requested room
    pub fn remove_waiting_peer(&mut self, peer_id: PeerId) -> RequestedRoom {
        let room = {
            let mut lock = self.clients_in_queue.lock().unwrap();
            lock.remove(&peer_id).expect("waiting peer")
        };
        room
    }

    /// Add a peer, returning the peers already in room
    pub fn add_peer(&mut self, peer: Peer) -> Vec<PeerId> {
        let peer_id = peer.uuid;
        let room = peer.room.clone();
        {
            let mut clients = self.clients.lock().unwrap();
            clients.insert(peer.uuid, peer);
        };
        let mut rooms = self.rooms.lock().unwrap();
        let peers = rooms.entry(room.clone()).or_default();
        let prev_peers = peers.iter().cloned().collect();

        match room.next {
            None => {
                peers.insert(peer_id);
            }
            Some(num_players) => {
                if peers.len() == num_players - 1 {
                    let mut matched_by_next = self.matched_by_next.lock().unwrap();
                    let mut updated_peers = peers.clone();
                    updated_peers.insert(peer_id);
                    matched_by_next.insert(updated_peers.into_iter().collect());

                    peers.clear(); // room is complete
                } else {
                    peers.insert(peer_id);
                }
            }
        };

        prev_peers
    }

    pub fn remove_matched_peer(&mut self, peer: PeerId) -> Vec<PeerId> {
        let mut matched_by_next = self.matched_by_next.lock().unwrap();
        let mut peers = vec![];
        matched_by_next.retain(|group| {
            if group.contains(&peer) {
                peers = group.clone();
                return false;
            }

            true
        });

        peers.retain(|p| p != &peer);

        if !peers.is_empty() {
            matched_by_next.insert(peers.clone());
        }

        peers
    }

    /// Get a peer
    pub fn get_peer(&self, peer_id: &PeerId) -> Option<Peer> {
        let clients = self.clients.lock().unwrap();
        clients.get(peer_id).cloned()
    }

    /// Get the peers in a room currently
    pub fn get_room_peers(&self, room: &RequestedRoom) -> Vec<PeerId> {
        self.rooms
            .lock()
            .unwrap()
            .get(room)
            .map(|room_peers| room_peers.iter().copied().collect::<Vec<PeerId>>())
            .unwrap_or_default()
    }

    /// Remove a peer from the state if it existed, returning the peer removed.
    #[must_use]
    pub fn remove_peer(&mut self, peer_id: &PeerId) -> Option<Peer> {
        let peer = { self.clients.lock().unwrap().remove(peer_id) };

        if let Some(ref peer) = peer {
            // Best effort to remove peer from their room
            _ = self
                .rooms
                .lock()
                .unwrap()
                .get_mut(&peer.room)
                .map(|room| room.remove(peer_id));
        }
        peer
    }

    /// Send a message to a peer without blocking.
    pub fn try_send(&self, id: PeerId, message: Message) -> Result<(), SignalingError> {
        let clients = self.clients.lock().unwrap();
        match clients.get(&id) {
            Some(peer) => Ok(common_logic::try_send(&peer.sender, message)?),
            None => Err(SignalingError::UnknownPeer),
        }
    }
}
