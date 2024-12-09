use ggrs::{Message, PlayerType};
use matchbox_protocol::PeerId;

use crate::{
    ChannelConfig, MessageLoopFuture, Packet, WebRtcChannel, WebRtcSocket, WebRtcSocketBuilder,
};

pub const GGRS_CHANNEL_ID: usize = 0;

impl ChannelConfig {
    /// Creates a [`ChannelConfig`] suitable for use with GGRS.
    pub fn ggrs() -> Self {
        Self::unreliable()
    }
}

impl WebRtcSocketBuilder {
    /// Adds a new channel suitable for use with GGRS to the [`WebRtcSocket`] configuration.
    ///
    /// This must be called as the first channel.
    pub fn add_ggrs_channel(mut self) -> WebRtcSocketBuilder {
        assert_eq!(
            self.config.channels.len(),
            GGRS_CHANNEL_ID,
            "ggrs channel is expected to be the first channel added"
        );
        self.config.channels.push(ChannelConfig::ggrs());
        self
    }
}

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
            .add_channel(ChannelConfig::ggrs())
            .build()
    }
}

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
    bincode::serialize(&msg).unwrap().into_boxed_slice()
}

fn deserialize_packet(message: (PeerId, Packet)) -> (PeerId, Message) {
    (message.0, bincode::deserialize(&message.1).unwrap())
}

impl ggrs::NonBlockingSocket<PeerId> for WebRtcSocket {
    fn send_to(&mut self, msg: &Message, addr: &PeerId) {
        self.channel_mut(GGRS_CHANNEL_ID)
            .send(build_packet(msg), *addr);
    }
    fn receive_all_messages(&mut self) -> Vec<(PeerId, Message)> {
        self.channel_mut(GGRS_CHANNEL_ID)
            .receive()
            .into_iter()
            .map(deserialize_packet)
            .collect()
    }
}

impl ggrs::NonBlockingSocket<PeerId> for WebRtcChannel {
    fn send_to(&mut self, msg: &Message, addr: &PeerId) {
        self.send(build_packet(msg), *addr);
    }

    fn receive_all_messages(&mut self) -> Vec<(PeerId, Message)> {
        self.receive().into_iter().map(deserialize_packet).collect()
    }
}
