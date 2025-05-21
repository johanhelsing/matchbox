use ggrs::{Message, PlayerType};
use matchbox_protocol::PeerId;

use crate::{Packet, WebRtcChannel, WebRtcSocket};

impl WebRtcSocket {
    /// Returns a Vec of connected peers as [`ggrs::PlayerType`]
    pub fn players(&mut self) -> Vec<PlayerType<PeerId>> {
        let Some(our_id) = self.id() else {
            // we're still waiting for the server to initialize our id
            // no peers should be added at this point anyway
            return vec![PlayerType::Local];
        };

        // player order needs to be consistent order across all peers
        let mut ids: Vec<_> = self
            .connected_peers()
            .chain(std::iter::once(our_id))
            .collect();
        ids.sort();

        ids.into_iter()
            .map(|id| {
                if id == our_id {
                    PlayerType::Local
                } else {
                    PlayerType::Remote(id)
                }
            })
            .collect()
    }
}

fn build_packet(msg: &Message) -> Packet {
    bincode::serde::encode_to_vec(msg, bincode::config::standard())
        .expect("failed to serialize ggrs packet")
        .into_boxed_slice()
}

fn deserialize_packet(message: (PeerId, Packet)) -> (PeerId, Message) {
    (
        message.0,
        bincode::serde::decode_from_slice(&message.1, bincode::config::standard())
            .expect("failed to deserialize ggrs packet")
            .0,
    )
}

impl ggrs::NonBlockingSocket<PeerId> for WebRtcChannel {
    fn send_to(&mut self, msg: &Message, addr: &PeerId) {
        if self.config().max_retransmits != Some(0) || self.config().ordered {
            // Using a reliable or ordered channel with ggrs is wasteful in that ggrs implements its
            // own reliability layer (including unconfirmed inputs in frames) and can
            // handle out of order messages just fine on its own.
            // It's likely that in poor network conditions this will cause GGRS to unnecessarily
            // delay confirming or rolling back simulation frames, which will impact performance
            // (or, worst case, cause GGRS to temporarily stop advancing frames).
            // So we better warn the user about this.
            log::warn!(
                "Sending GGRS traffic over reliable or ordered channel ({:?}), which may reduce performance.\
                You should use an unreliable and unordered channel instead.",
                self.config()
            );
        }
        self.send(build_packet(msg), *addr);
    }

    fn receive_all_messages(&mut self) -> Vec<(PeerId, Message)> {
        self.receive().into_iter().map(deserialize_packet).collect()
    }
}
