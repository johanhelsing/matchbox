use ggrs::{Message, PlayerType};

use crate::WebRtcSocket;

#[derive(Debug, thiserror::Error)]
#[error("The client has not yet been given a Peer Id")]
pub struct UnknownPeerId;

impl WebRtcSocket {
    /// Returns a Vec of connected peers as [`ggrs::PlayerType`]
    pub fn players(&mut self) -> Result<Vec<PlayerType<String>>, UnknownPeerId> {
        let our_id = self.id().ok_or(UnknownPeerId)?;
        // needs to be consistent order across all peers
        let mut ids: Vec<_> = self.connected_peers().cloned().collect();
        ids.push(our_id.clone());
        ids.sort();
        let players = ids
            .iter()
            .map(|id| {
                if *id == our_id {
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
