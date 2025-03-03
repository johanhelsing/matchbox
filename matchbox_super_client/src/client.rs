use log::info;
use matchbox_socket::{PeerId, WebRtcSocket};
use serde::{Deserialize, Serialize};
use serde_json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::rc::Rc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;

/// Mock WebRtcSocket that captures sent messages instead of sending them.
struct MockWebRtcSocket {
    sent_messages: Rc<RefCell<VecDeque<(PeerId, Vec<u8>)>>>,
}

impl MockWebRtcSocket {
    fn new() -> Self {
        Self {
            sent_messages: Rc::new(RefCell::new(VecDeque::new())),
        }
    }

    /// Mocks the behavior of sending a message to a peer.
    fn send(&self, peer: PeerId, data: Vec<u8>) {
        self.sent_messages.borrow_mut().push_back((peer, data));
    }

    /// Retrieves the last sent message (for testing).
    fn get_last_message(&self) -> Option<(PeerId, Vec<u8>)> {
        self.sent_messages.borrow_mut().pop_back()
    }

    /// Retrieves all sent messages.
    fn get_all_messages(&self) -> Vec<(PeerId, Vec<u8>)> {
        self.sent_messages.borrow().clone().into()
    }
}

pub trait SocketSender {
    fn send(&self, peer: PeerId, data: Vec<u8>);
}

impl SocketSender for MockWebRtcSocket {
    fn send(&self, peer: PeerId, data: Vec<u8>) {
        self.send(peer, data);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PeerPosition {
    peer_id: String,
    x: u32,
    y: u32,
}

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    PositionUpdate(PeerPosition),
    Forward(PeerPosition),
    PeerList(Vec<String>),
}

#[wasm_bindgen]
pub struct Client {
    peer_id: Option<PeerId>,
    is_super: bool,
    super_peer: Option<PeerId>, // Each client has a designated super-peer
    connected_peers: Rc<RefCell<Vec<PeerId>>>,
    peer_positions: Rc<RefCell<HashMap<PeerId, (u32, u32)>>>,
    known_super_peers: Rc<RefCell<Vec<PeerId>>>, // Super-peers track other super-peers
}

#[wasm_bindgen]
impl Client {
    #[wasm_bindgen(constructor)]
    pub fn new(is_super: bool) -> Client {
        Client {
            peer_id: None,
            is_super,
            super_peer: None,
            peer_positions: Rc::new(RefCell::new(HashMap::new())),
            connected_peers: Rc::new(RefCell::new(Vec::new())),
            known_super_peers: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Super-peer sends the peer list to other super-peers
    // fn send_peer_list(&self, socket: &mut WebRtcSocket) {
    //     if self.is_super {
    //         let peer_list: Vec<String> = self
    //             .known_super_peers
    //             .borrow()
    //             .iter()
    //             .map(|peer| peer.to_string())
    //             .collect();
    //         let msg = Message::PeerList(peer_list);
    //         let packet = serde_json::to_vec(&msg).unwrap();

    //         // Will need to fix this code to handle sending
    //         for peer in self.connected_peers.borrow().iter() {
    //             socket.channel_mut(0).send(packet.clone().into_boxed_slice(), *peer);
    //         }
    //         info!("Super-peer sent peer list.");
    //     }
    // }

    fn send_peer_list<T: SocketSender>(&self, socket: &T) {
        if self.is_super {
            let peer_list: Vec<String> = self
                .known_super_peers
                .borrow()
                .iter()
                .map(|peer| peer.0.to_string()) // Extract Uuid from PeerId
                .collect();
            let msg = Message::PeerList(peer_list);
            let packet = serde_json::to_vec(&msg).unwrap();

            for peer in self.connected_peers.borrow().iter() {
                socket.send(*peer, packet.clone());
            }
            info!("Super-peer sent peer list.");
        }
    }

    /// Super-peer forwards position updates to other super-peers
    fn forward_to_super_peers(&self, socket: &mut WebRtcSocket, position: &PeerPosition) {
        let msg = Message::Forward(position.clone());
        let packet = serde_json::to_vec(&msg).unwrap();

        // Need to fix this relating to the channels
        for peer in self.known_super_peers.borrow().iter() {
            socket
                .channel_mut(0)
                .send(packet.clone().into_boxed_slice(), *peer);
        }
        info!("Forwarded position update to super-peers.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_peer_list() {
        let mut client = Client::new(true); // Super-peer
        let mock_socket = MockWebRtcSocket::new();

        // Add known super-peers and connected peers
        let peer1 = PeerId(Uuid::new_v4());
        let peer2 = PeerId(Uuid::new_v4());
        let connected_peer = PeerId(Uuid::new_v4());

        client.known_super_peers.borrow_mut().push(peer1);
        client.known_super_peers.borrow_mut().push(peer2);
        client.connected_peers.borrow_mut().push(connected_peer);

        println!("Client is super-peer: {}", client.is_super);
        println!("Known super peers: {:?}", client.known_super_peers.borrow());
        println!("Connected peers: {:?}", client.connected_peers.borrow());

        // Call send_peer_list()
        client.send_peer_list(&mock_socket);

        // Retrieve last sent message
        let (peer, data) = mock_socket.get_last_message().expect("No message was sent");

        // Print last message (for debugging purposes)
        println!("Sent raw data to {}: {:?}", peer, data);

        // Deserialize the message
        let received_msg: Message = serde_json::from_slice(&data).expect("Failed to deserialize");

        // Print deserialized message content
        println!("Received message: {:?}", received_msg);

        // Validate that the message is a PeerList
        match received_msg {
            Message::PeerList(peers) => {
                assert_eq!(peers.len(), 2);
                assert!(peers.contains(&peer1.0.to_string()));
                assert!(peers.contains(&peer2.0.to_string()));
            }
            _ => panic!("Expected PeerList message"),
        }

        // Ensure the message was sent to the correct peer
        assert_eq!(peer, connected_peer);
    }
}
