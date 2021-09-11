use futures::Future;
use ggrs::{PlayerType, UdpMessage};
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    net::{Ipv6Addr, SocketAddr},
    pin::Pin,
};

use crate::WebRtcSocket;

#[derive(Debug)]
pub struct WebRtcNonBlockingSocket {
    socket: WebRtcSocket,
    fake_socket_addrs: HashMap<String, SocketAddr>,
    fake_socket_addrs_reverse: HashMap<SocketAddr, String>,
}

impl WebRtcNonBlockingSocket {
    pub async fn new(room_url: &str) -> (Self, Pin<Box<dyn Future<Output = ()>>>) {
        let (socket, message_loop) = WebRtcSocket::new(room_url).await;
        (
            Self {
                socket,
                fake_socket_addrs: Default::default(),
                fake_socket_addrs_reverse: Default::default(),
            },
            message_loop,
        )
    }

    pub async fn wait_for_peers(&mut self, peers: usize) {
        let new_peers = self.socket.wait_for_peers(peers).await;

        for id in new_peers {
            let fake_addr = make_fake_socket_addr(&id);
            self.fake_socket_addrs.insert(id.clone(), fake_addr.clone());
            self.fake_socket_addrs_reverse.insert(fake_addr, id);
        }
    }

    pub fn accept_new_connections(&mut self) {
        let new_peers = self.socket.process_new_connections();
        for peer in new_peers {
            self.handle_new_peer_id(peer);
        }
    }

    pub fn connected_peers(&self) -> Vec<SocketAddr> {
        self.socket
            .connected_peers()
            .iter()
            .map(|id| self.fake_socket_addrs.get(id).unwrap().clone())
            .collect()
    }

    pub fn players(&self) -> Vec<PlayerType> {
        // needs to be consistent order across all peers
        let mut ids = self.socket.connected_peers();
        ids.push(self.socket.id().to_owned());
        ids.sort();
        ids.iter()
            .map(|id| {
                if id == self.socket.id() {
                    PlayerType::Local
                } else {
                    let addr = self.fake_socket_addrs.get(id).unwrap().clone();
                    PlayerType::Remote(addr)
                }
            })
            .collect()
    }

    fn get_or_create_fake_addr(&mut self, id: &str) -> SocketAddr {
        match self.fake_socket_addrs.get(id) {
            Some(fake_addr) => fake_addr.clone(),
            None => self.handle_new_peer_id(id.to_string()),
        }
    }

    fn handle_new_peer_id(&mut self, id: String) -> SocketAddr {
        let fake_addr = make_fake_socket_addr(&id);
        self.fake_socket_addrs.insert(id.clone(), fake_addr.clone());
        self.fake_socket_addrs_reverse.insert(fake_addr.clone(), id);
        fake_addr
    }
}

impl ggrs::NonBlockingSocket for WebRtcNonBlockingSocket {
    fn send_to(&mut self, msg: &UdpMessage, addr: SocketAddr) {
        let id = self.fake_socket_addrs_reverse[&addr].clone();
        let buf = bincode::serialize(&msg).unwrap();
        let packet = buf.into_boxed_slice();
        self.socket.send(packet, id);
    }

    fn receive_all_messages(&mut self) -> Vec<(SocketAddr, UdpMessage)> {
        // let fake_socket_addrs = self.fake_socket_addrs.clone();
        let mut messages = vec![];
        for (id, packet) in self.socket.receive_messages().into_iter() {
            let msg = bincode::deserialize(&packet).unwrap();
            let addr = self.get_or_create_fake_addr(&id);
            messages.push((addr, msg));
        }
        messages
    }
}

fn make_fake_socket_addr(id: &str) -> SocketAddr {
    // TODO: This is a horrible lazy hack, but let's just try to get it working
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    let hash = hasher.finish();
    let a: u16 = hash as u16;
    SocketAddr::new(Ipv6Addr::new(a, a, a, a, a, a, a, a).into(), 1111)
}
