use ggrs::{Message, PlayerType};

use crate::{webrtc_socket::error::UnknownPeerId, WebRtcSocket};

impl WebRtcSocket {
    /// Returns a Vec of connected peers as [`ggrs::PlayerType`]
    #[must_use]
    pub fn players(&mut self) -> Result<Vec<PlayerType<String>>, UnknownPeerId> {
        let client_id = self.id().ok_or(UnknownPeerId)?;
        // needs to be consistent order across all peers
        let mut ids = self.connected_peers();
        ids.push(client_id.to_owned());
        ids.sort();
        let players = ids
            .iter()
            .map(|id| {
                if *id == client_id {
                    PlayerType::Local
                } else {
                    PlayerType::Remote(id.to_owned())
                }
            })
            .collect();
        Ok(players)
    }
}

impl ggrs::NonBlockingSocket<String> for WebRtcSocket {
    fn send_to(&mut self, msg: &Message, addr: &String) {
        let buf = bincode::serialize(&msg).unwrap();
        let packet = buf.into_boxed_slice();
        self.send(packet, addr);
    }

    fn receive_all_messages(&mut self) -> Vec<(String, Message)> {
        let mut messages = vec![];
        for (id, packet) in self.receive().into_iter() {
            let msg = bincode::deserialize(&packet).unwrap();
            messages.push((id, msg));
        }
        messages
    }
}
