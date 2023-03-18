use ggrs::{Message, PlayerType};
use matchbox_protocol::PeerId;

use crate::{ChannelConfig, MessageLoopFuture, WebRtcSocket, WebRtcSocketBuilder};

#[derive(Debug, thiserror::Error)]
#[error("The client has not yet been given a Peer Id")]
pub struct UnknownPeerId;

impl WebRtcSocket {
    /// Creates a [`WebRtcSocket`] and the corresponding [`MessageLoopFuture`] for a
    /// socket with a single channel configured correctly for usage with GGRS.
    ///
    /// The returned [`MessageLoopFuture`] should be awaited in order for messages to
    /// be sent and received.
    ///
    /// Please use the [`WebRtcSocketBuilder`] to create non-trivial sockets.
    pub fn new_ggrs(room_url: impl Into<String>) -> (WebRtcSocket, MessageLoopFuture) {
        WebRtcSocketBuilder::new(room_url)
            .add_ggrs_channel()
            .build()
    }

    /// Returns a Vec of connected peers as [`ggrs::PlayerType`]
    pub fn players(&self) -> Result<Vec<PlayerType<PeerId>>, UnknownPeerId> {
        let our_id = self.id().ok_or(UnknownPeerId)?;

        // player order needs to be consistent order across all peers
        let mut ids: Vec<_> = self
            .connected_peers()
            .chain(std::iter::once(our_id))
            .collect();
        ids.sort();

        let players = ids
            .into_iter()
            .map(|id| {
                if id == our_id {
                    PlayerType::Local
                } else {
                    PlayerType::Remote(id)
                }
            })
            .collect();
        Ok(players)
    }
}

impl ggrs::NonBlockingSocket<PeerId> for WebRtcSocket {
    fn send_to(&mut self, msg: &Message, addr: &PeerId) {
        let buf = bincode::serialize(&msg).unwrap();
        let packet = buf.into_boxed_slice();
        self.send(packet, *addr);
    }

    fn receive_all_messages(&mut self) -> Vec<(PeerId, Message)> {
        let mut messages = vec![];
        for (id, packet) in self.receive().into_iter() {
            let msg = bincode::deserialize(&packet).unwrap();
            messages.push((id, msg));
        }
        messages
    }
}

impl WebRtcSocketBuilder {
    /// Adds a new channel configured correctly for usage with GGRS to the [`WebRtcSocket`].
    pub fn add_ggrs_channel(mut self) -> Self {
        self.channels.push(ChannelConfig::unreliable());
        self
    }
}
