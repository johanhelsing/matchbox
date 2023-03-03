use crate::{
    webrtc_socket::{
        message_loop, messages::PeerId, signalling_loop, MessageLoopFuture, Packet, PeerEvent,
        PeerRequest, UseMessenger, UseSignaller,
    },
    Error,
};
use futures::{future::Fuse, select, Future, FutureExt, StreamExt};
use futures_channel::mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender};
use log::{debug, error};
use std::{collections::HashMap, pin::Pin};

/// Configuration options for an ICE server connection.
/// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCIceServer#example>
#[derive(Debug)]
pub struct RtcIceServerConfig {
    /// An ICE server instance can have several URLs
    pub urls: Vec<String>,
    /// A username for authentication with the ICE server
    ///
    /// See: <https://developer.mozilla.org/en-US/docs/Web/API/RTCIceServer/username>
    pub username: Option<String>,
    /// A password or token when authenticating with a turn server
    ///
    /// See: <https://developer.mozilla.org/en-US/docs/Web/API/RTCIceServer/credential>
    pub credential: Option<String>,
}

/// Configuration options for a data channel
/// See also: https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel
#[derive(Debug)]
pub struct ChannelConfig {
    /// Whether messages sent on the channel are guaranteed to arrive in order
    /// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/ordered>
    pub ordered: bool,
    /// Maximum number of retransmit attempts of a message before giving up
    /// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/maxRetransmits>
    pub max_retransmits: Option<u16>,
}

impl ChannelConfig {
    /// Messages sent via an unreliable channel may arrive in any order or not at all, but arrive as
    /// quickly as possible
    pub fn unreliable() -> Self {
        ChannelConfig {
            ordered: false,
            max_retransmits: Some(0),
        }
    }

    /// Messages sent via a reliable channel are guaranteed to arrive in order and will be resent
    /// until they arrive
    pub fn reliable() -> Self {
        ChannelConfig {
            ordered: true,
            max_retransmits: None,
        }
    }
}

impl Default for RtcIceServerConfig {
    fn default() -> Self {
        Self {
            urls: vec![
                "stun:stun.l.google.com:19302".to_string(),
                "stun:stun1.l.google.com:19302".to_string(),
                "stun:stun2.l.google.com:19302".to_string(),
                "stun:stun3.l.google.com:19302".to_string(),
                "stun:stun4.l.google.com:19302".to_string(),
            ],
            username: Default::default(),
            credential: Default::default(),
        }
    }
}

/// General configuration options for a WebRtc connection.
///
/// See [`WebRtcSocket::new_with_config`]
#[derive(Debug)]
pub struct WebRtcSocketConfig {
    /// The url for the room to connect to
    ///
    /// This is a websocket url, starting with `ws://` or `wss://` followed by
    /// the hostname and path to a matchbox server, followed by a room id and
    /// optional query parameters.
    ///
    /// e.g.: `wss://matchbox.example.com/your_game`
    ///
    /// or: `wss://matchbox.example.com/your_game?next=2`
    ///
    /// The last form will pair player in the order they connect.
    pub room_url: String,
    /// Configuration for the (single) ICE server
    pub ice_server: RtcIceServerConfig,
    /// Configuration for one or multiple reliable or unreliable data channels
    pub channels: Vec<ChannelConfig>,
    /// The amount of attempts to initiate connection
    pub attempts: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The state of a connection to a peer
pub enum PeerState {
    // todo: add connecting state?
    /// The peer is connected, and the requested data channels have been established
    Connected,
    // todo: add separate left/disconnected from signalling state?
    /// The peer is disconnected, or some of the its data channels are
    Disconnected,
}

/// Contains the interface end of a full-mesh web rtc connection
///
/// Used to send and receive messages from other peers
#[derive(Debug)]
pub struct WebRtcSocket {
    id: Option<PeerId>,
    id_rx: Receiver<PeerId>,
    messages_from_peers: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    peer_state_changes_rx: futures_channel::mpsc::UnboundedReceiver<(PeerId, PeerState)>,
    peer_messages_out: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
    peers: HashMap<PeerId, PeerState>,
}

impl WebRtcSocket {
    /// Create a new connection to the given room with a single unreliable data channel
    ///
    /// See [`WebRtcSocketConfig::room_url`] for details on the room url.
    ///
    /// The returned future should be awaited in order for messages to be sent and received.
    #[must_use]
    pub fn new_unreliable(room_url: impl Into<String>) -> (Self, MessageLoopFuture) {
        WebRtcSocket::new_with_config(WebRtcSocketConfig {
            room_url: room_url.into(),
            ice_server: RtcIceServerConfig::default(),
            channels: vec![ChannelConfig::unreliable()],
            attempts: Some(3),
        })
    }

    /// Create a new connection to the given room with a single reliable data channel
    ///
    /// See [`WebRtcSocketConfig::room_url`] for details on the room url.
    ///
    /// The returned future should be awaited in order for messages to be sent and received.
    #[must_use]
    pub fn new_reliable(room_url: impl Into<String>) -> (Self, MessageLoopFuture) {
        WebRtcSocket::new_with_config(WebRtcSocketConfig {
            room_url: room_url.into(),
            ice_server: RtcIceServerConfig::default(),
            channels: vec![ChannelConfig::reliable()],
            attempts: Some(3),
        })
    }

    /// Create a new connection with the given [`WebRtcSocketConfig`]
    ///
    /// The returned future should be awaited in order for messages to be sent and received.
    #[must_use]
    pub fn new_with_config(config: WebRtcSocketConfig) -> (Self, MessageLoopFuture) {
        if config.channels.is_empty() {
            panic!("You need to configure at least one channel in WebRtcSocketConfig");
        }

        let (messages_from_peers_tx, messages_from_peers) = new_senders_and_receivers(&config);
        let (peer_state_changes_tx, peer_state_changes_rx) = futures_channel::mpsc::unbounded();
        let (peer_messages_out_tx, peer_messages_out_rx) = new_senders_and_receivers(&config);

        let (id_tx, id_rx) = futures_channel::mpsc::channel(1);

        (
            Self {
                id: None,
                id_rx,
                messages_from_peers,
                peer_messages_out: peer_messages_out_tx,
                peer_state_changes_rx,
                peers: Default::default(),
            },
            Box::pin(run_socket(
                id_tx,
                config,
                peer_messages_out_rx,
                peer_state_changes_tx,
                messages_from_peers_tx,
            )),
        )
    }

    /// Check if new peers have connected and if so add them as peers
    // todo: think about name?
    pub fn handle_peer_changes(&mut self) -> Vec<(PeerId, PeerState)> {
        let mut changes = Vec::new();
        while let Ok(Some((id, state))) = self.peer_state_changes_rx.try_next() {
            let old = self.peers.insert(id.clone(), state);
            if old != Some(state) {
                changes.push((id, state));
            }
        }
        changes
    }

    /// Returns an iterator of the ids of the connected peers.
    ///
    /// Note: You have to call [`handle_peer_changes`] for this list to be accurate.
    ///
    /// See also: [`WebRtcSocket::disconnected_peers`]
    pub fn connected_peers(&'_ self) -> impl std::iter::Iterator<Item = &PeerId> {
        self.peers.iter().filter_map(|(id, state)| {
            if state == &PeerState::Connected {
                Some(id)
            } else {
                None
            }
        })
    }

    /// Returns an iterator of the ids of peers that are no longer connected.
    ///
    /// Note: You have to call [`handle_peer_changes`] for this list to be accurate.
    ///
    /// See also: [`WebRtcSocket::connected_peers`]
    pub fn disconnected_peers(&self) -> impl std::iter::Iterator<Item = &PeerId> {
        self.peers.iter().filter_map(|(id, state)| {
            if state == &PeerState::Disconnected {
                Some(id)
            } else {
                None
            }
        })
    }

    /// Call this where you want to handle new received messages from the default channel (with
    /// index 0) which will be the only channel if you didn't configure any explicitly
    ///
    /// messages are removed from the socket when called
    ///
    /// See also: [`WebRtcSocket::receive_on_channel`]
    pub fn receive(&mut self) -> Vec<(PeerId, Packet)> {
        self.receive_on_channel(0)
    }

    /// Call this where you want to handle new received messages from a specific channel as
    /// configured in [`WebRtcSocketConfig::channels`]. The index of a channel is its index in
    /// the vec [`WebRtcSocketConfig::channels`] as you configured it before (or 0 for the
    /// default channel if you use the default configuration).
    ///
    /// messages are removed from the socket when called
    pub fn receive_on_channel(&mut self, index: usize) -> Vec<(PeerId, Packet)> {
        std::iter::repeat_with(|| {
            self.messages_from_peers
                .get_mut(index)
                .unwrap_or_else(|| panic!("No data channel with index {index}"))
                .try_next()
        })
        // .map_while(|poll| match p { // map_while is nightly-only :(
        .take_while(|p| !p.is_err())
        .map(|p| match p.unwrap() {
            Some((peer_id, packet)) => (peer_id, packet),
            None => todo!("Handle connection closed??"),
        })
        .collect()
    }

    /// Send a packet to the given peer on the default channel (with index 0) which will be the only
    /// channel if you didn't configure any explicitly
    ///
    /// See also [`WebRtcSocket::send_on_channel`]
    pub fn send<T: Into<PeerId>>(&mut self, packet: Packet, id: T) {
        self.send_on_channel(packet, id, 0);
    }

    /// Send a packet to the given peer on a specific channel as configured in
    /// [`WebRtcSocketConfig::channels`].
    ///
    /// The index of a channel is its index in the vec [`WebRtcSocketConfig::channels`] as you
    /// configured it before (or 0 for the default channel if you use the default
    /// configuration).
    pub fn send_on_channel<T: Into<PeerId>>(&mut self, packet: Packet, id: T, index: usize) {
        self.peer_messages_out
            .get(index)
            .unwrap_or_else(|| panic!("No data channel with index {index}"))
            .unbounded_send((id.into(), packet))
            .expect("send_to failed");
    }

    /// Send a packet to all connected peers on a specific channel as configured in
    /// [`WebRtcSocketConfig::channels`].
    ///
    /// The index of a channel is its index in the vec [`WebRtcSocketConfig::channels`] on socket
    /// creation.
    pub fn broadcast_on_channel(&mut self, packet: Packet, index: usize) {
        let sender = self
            .peer_messages_out
            .get(index)
            .unwrap_or_else(|| panic!("No data channel with index {index}"));

        for peer_id in self.connected_peers() {
            sender
                .unbounded_send((peer_id.clone(), packet.clone()))
                .expect("send_to failed");
        }
    }

    /// Returns the id of this peer, this may be None if a value has not yet been recieved from the server.
    pub fn id(&mut self) -> Option<PeerId> {
        if let Some(id) = self.id.to_owned() {
            Some(id)
        } else if let Ok(Some(id)) = self.id_rx.try_next() {
            self.id = Some(id.to_owned());
            Some(id)
        } else {
            None
        }
    }
}

pub(crate) fn new_senders_and_receivers<T>(
    config: &WebRtcSocketConfig,
) -> (Vec<UnboundedSender<T>>, Vec<UnboundedReceiver<T>>) {
    (0..config.channels.len())
        .map(|_| futures_channel::mpsc::unbounded())
        .unzip()
}

pub(crate) fn create_data_channels_ready_fut(
    config: &WebRtcSocketConfig,
) -> (
    Vec<futures_channel::mpsc::Sender<u8>>,
    Pin<Box<Fuse<impl Future<Output = ()>>>>,
) {
    let (senders, receivers) = (0..config.channels.len())
        .map(|_| futures_channel::mpsc::channel(1))
        .unzip();

    (senders, Box::pin(wait_for_ready(receivers).fuse()))
}

async fn wait_for_ready(channel_ready_rx: Vec<futures_channel::mpsc::Receiver<u8>>) {
    for mut receiver in channel_ready_rx {
        if receiver.next().await.is_none() {
            panic!("Sender closed before channel was ready");
        }
    }
}

/// All the channels needed for the messaging loop.
pub struct MessageLoopChannels {
    pub requests_sender: futures_channel::mpsc::UnboundedSender<PeerRequest>,
    pub events_receiver: futures_channel::mpsc::UnboundedReceiver<PeerEvent>,
    pub peer_messages_out_rx: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    pub peer_state_change_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
    pub messages_from_peers_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
}

async fn run_socket(
    id_tx: Sender<PeerId>,
    config: WebRtcSocketConfig,
    peer_messages_out_rx: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    peer_state_change_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
    messages_from_peers_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
) -> Result<(), Error> {
    debug!("Starting WebRtcSocket");

    let (requests_sender, requests_receiver) = futures_channel::mpsc::unbounded::<PeerRequest>();
    let (events_sender, events_receiver) = futures_channel::mpsc::unbounded::<PeerEvent>();

    let signalling_loop_fut = signalling_loop::<UseSignaller>(
        config.attempts,
        config.room_url.clone(),
        requests_receiver,
        events_sender,
    );

    let channels = MessageLoopChannels {
        requests_sender,
        events_receiver,
        peer_messages_out_rx,
        peer_state_change_tx,
        messages_from_peers_tx,
    };
    let message_loop_fut = message_loop::<UseMessenger>(id_tx, config, channels);

    let mut message_loop_done = Box::pin(message_loop_fut.fuse());
    let mut signalling_loop_done = Box::pin(signalling_loop_fut.fuse());
    loop {
        select! {
            _ = message_loop_done => {
                debug!("Message loop completed");
                break;
            }

            sigloop = signalling_loop_done => {
                match sigloop {
                    Ok(()) => debug!("Signalling loop completed"),
                    Err(e) => {
                        // TODO: Reconnect X attempts if configured to reconnect.
                        error!("{e:?}");
                        return Err(Error::from(e));
                    },
                }
            }

            complete => break
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use crate::{
        webrtc_socket::error::SignallingError, ChannelConfig, Error, RtcIceServerConfig,
        WebRtcSocket, WebRtcSocketConfig,
    };

    #[futures_test::test]
    async fn unreachable_server() {
        // .invalid is a reserved tld for testing and documentation
        let (_socket, fut) = WebRtcSocket::new_reliable("wss://example.invalid");

        let result = fut.await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Signalling(_)));
    }

    #[futures_test::test]
    async fn test_signalling_attempts() {
        let (_socket, loop_fut) = WebRtcSocket::new_with_config(WebRtcSocketConfig {
            room_url: "wss://example.invalid/".to_string(),
            attempts: Some(3),
            ice_server: RtcIceServerConfig::default(),
            channels: vec![ChannelConfig::unreliable()],
        });

        let result = loop_fut.await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::Signalling(SignallingError::ConnectionFailed(_))
        ));
    }
}
