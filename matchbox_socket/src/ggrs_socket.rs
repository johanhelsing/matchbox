use ggrs::{Message, PlayerType};

use crate::WebRtcSocket;

impl WebRtcSocket {
    #[must_use]
    pub fn players(&self) -> Vec<PlayerType<String>> {
        // needs to be consistent order across all peers
        let mut ids = self.connected_peers();
        ids.push(self.id().to_owned());
        ids.sort();
        ids.iter()
            .map(|id| {
                if id == self.id() {
                    PlayerType::Local
                } else {
                    PlayerType::Remote(id.to_owned())
                }
            })
            .collect()
    }
}

impl ggrs::NonBlockingSocket<String> for WebRtcSocket {
    fn send_to(&mut self, msg: &Message, addr: &String) {
        let buf = bincode::serialize(&msg).unwrap();
        let packet = buf.into_boxed_slice();
        self.send(packet, addr);
    }

    fn receive_all_messages(&mut self) -> Vec<(String, Message)> {
        // let fake_socket_addrs = self.fake_socket_addrs.clone();
        let mut messages = vec![];
        for (id, packet) in self.receive().into_iter() {
            let msg = bincode::deserialize(&packet).unwrap();
            messages.push((id, msg));
        }
        messages
    }
}
