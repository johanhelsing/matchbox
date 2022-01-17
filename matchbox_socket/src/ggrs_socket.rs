use futures::Future;
use ggrs::{PlayerType, UdpMessage};
use std::pin::Pin;

use crate::WebRtcSocket;

#[derive(Debug)]
pub struct WebRtcNonBlockingSocket {
    socket: WebRtcSocket,
}

impl WebRtcNonBlockingSocket {
    #[must_use]
    pub fn new<T: Into<String>>(room_url: T) -> (Self, Pin<Box<dyn Future<Output = ()>>>) {
        let (socket, message_loop) = WebRtcSocket::new(room_url);
        (Self { socket }, message_loop)
    }

    pub async fn wait_for_peers(&mut self, peers: usize) -> Vec<String> {
        self.socket.wait_for_peers(peers).await
    }

    pub fn accept_new_connections(&mut self) -> Vec<String> {
        self.socket.accept_new_connections()
    }

    #[must_use]
    pub fn connected_peers(&self) -> Vec<String> {
        self.socket.connected_peers()
    }

    #[must_use]
    pub fn players(&self) -> Vec<PlayerType<String>> {
        // needs to be consistent order across all peers
        let mut ids = self.socket.connected_peers();
        ids.push(self.socket.id().to_owned());
        ids.sort();
        ids.iter()
            .map(|id| {
                if id == self.socket.id() {
                    PlayerType::Local
                } else {
                    PlayerType::Remote(id.to_owned())
                }
            })
            .collect()
    }
}

impl ggrs::NonBlockingSocket<String> for WebRtcNonBlockingSocket {
    fn send_to(&mut self, msg: &UdpMessage, addr: &String) {
        let buf = bincode::serialize(&msg).unwrap();
        let packet = buf.into_boxed_slice();
        self.socket.send(packet, addr);
    }

    fn receive_all_messages(&mut self) -> Vec<(String, UdpMessage)> {
        let mut messages = vec![];
        for (id, packet) in self.socket.receive().into_iter() {
            let msg = bincode::deserialize(&packet).unwrap();
            messages.push((id, msg));
        }
        messages
    }
}
