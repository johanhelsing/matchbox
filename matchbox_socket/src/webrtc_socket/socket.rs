use super::error::ChannelError;
use crate::{
    webrtc_socket::{
        message_loop, signalling_loop, MessageLoopFuture, Packet, PeerEvent, PeerRequest,
        UseMessenger, UseSignaller,
    },
    Error,
};
use futures::{future::Fuse, select, Future, FutureExt, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use log::{debug, error};
use matchbox_protocol::PeerId;
use std::{collections::HashMap, marker::PhantomData, pin::Pin};

/// Configuration options for an ICE server connection.
/// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCIceServer#example>
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
            ],
            username: Default::default(),
            credential: Default::default(),
        }
    }
}

/// Builder for [`WebRtcSocket`]s.
///
/// Begin with [`WebRtcSocketBuilder::new`] and add at least one channel with
/// [`WebRtcSocketBuilder::add_channel`],
/// [`WebRtcSocketBuilder::add_reliable_channel`], or
/// [`WebRtcSocketBuilder::add_unreliable_channel`] before calling
/// [`WebRtcSocketBuilder::build`] to produce the desired [`WebRtcSocket`].
#[derive(Debug, Clone)]
pub struct WebRtcSocketBuilder<C = ()> {
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
    pub(crate) room_url: String,
    /// Configuration for the (single) ICE server
    pub(crate) ice_server: RtcIceServerConfig,
    /// Configuration for one or multiple reliable or unreliable data channels
    pub(crate) channels: Vec<ChannelConfig>,
    /// The amount of attempts to initiate connection
    pub(crate) attempts: Option<u16>,
    channel_plurality: PhantomData<C>,
}

impl WebRtcSocketBuilder {
    /// Creates a new builder for a connection to a given room with the default ICE
    /// server configuration, and three reconnection attempts.
    ///
    /// You must add at least one channel with [`WebRtcSocketBuilder::add_channel`],
    /// [`WebRtcSocketBuilder::add_reliable_channel`], or
    /// [`WebRtcSocketBuilder::add_unreliable_channel`] before you can build the
    /// [`WebRtcSocket`]
    pub fn new(room_url: impl Into<String>) -> Self {
        Self {
            room_url: room_url.into(),
            ice_server: RtcIceServerConfig::default(),
            channels: Vec::default(),
            attempts: Some(3),
            channel_plurality: PhantomData::default(),
        }
    }

    /// Sets the socket ICE server configuration.
    pub fn ice_server(mut self, ice_server: RtcIceServerConfig) -> Self {
        self.ice_server = ice_server;
        self
    }

    /// Sets the number of attempts to make at reconnecting to the signalling server,
    /// if `None` the socket will attempt to connect indefinitely.
    pub fn reconnect_attempts(mut self, attempts: Option<u16>) -> Self {
        self.attempts = attempts;
        self
    }
}

impl WebRtcSocketBuilder<()> {
    /// Adds a new channel to the [`WebRtcSocket`] configuration according to a [`ChannelConfig`].
    pub fn add_channel(mut self, config: ChannelConfig) -> WebRtcSocketBuilder<WebRtcChannel> {
        self.channels.push(config);
        WebRtcSocketBuilder {
            room_url: self.room_url,
            ice_server: self.ice_server,
            channels: self.channels,
            attempts: self.attempts,
            channel_plurality: PhantomData::default(),
        }
    }
}

impl WebRtcSocketBuilder<WebRtcChannel> {
    /// Adds a new channel to the [`WebRtcSocket`] configuration according to a [`ChannelConfig`].
    pub fn add_channel(mut self, config: ChannelConfig) -> WebRtcSocketBuilder<WebRtcChannels> {
        self.channels.push(config);
        WebRtcSocketBuilder {
            room_url: self.room_url,
            ice_server: self.ice_server,
            channels: self.channels,
            attempts: self.attempts,
            channel_plurality: PhantomData::default(),
        }
    }
}
impl WebRtcSocketBuilder<WebRtcChannels> {
    /// Adds a new channel to the [`WebRtcSocket`] configuration according to a [`ChannelConfig`].
    pub fn add_channel(mut self, config: ChannelConfig) -> WebRtcSocketBuilder<WebRtcChannels> {
        self.channels.push(config);
        WebRtcSocketBuilder {
            room_url: self.room_url,
            ice_server: self.ice_server,
            channels: self.channels,
            attempts: self.attempts,
            channel_plurality: PhantomData::default(),
        }
    }
}

impl From<WebRtcSocket<WebRtcChannels>> for WebRtcSocket<WebRtcChannel> {
    fn from(mut socket: WebRtcSocket<WebRtcChannels>) -> Self {
        let channel = socket.take_channel(0).unwrap();
        Self {
            id: socket.id,
            id_rx: socket.id_rx,
            peers: socket.peers,
            peer_state_rx: socket.peer_state_rx,
            channels: channel,
        }
    }
}

impl WebRtcSocketBuilder<WebRtcChannel> {
    /// Creates a [`WebRtcSocket`] and the corresponding [`MessageLoopFuture`] according to the configuration supplied.
    ///
    /// The returned [`MessageLoopFuture`] should be awaited in order for messages to be sent and received.
    pub fn build(self) -> (WebRtcSocket<WebRtcChannel>, MessageLoopFuture) {
        let (socket, message_loop) =
            WebRtcSocketBuilder::<WebRtcChannels>::build(WebRtcSocketBuilder {
                room_url: self.room_url,
                ice_server: self.ice_server,
                channels: self.channels,
                attempts: self.attempts,
                channel_plurality: PhantomData::default(),
            });
        (socket.into(), message_loop)
    }
}

impl WebRtcSocketBuilder<WebRtcChannels> {
    /// Creates a [`WebRtcSocket`] and the corresponding [`MessageLoopFuture`] according to the configuration supplied.
    ///
    /// The returned [`MessageLoopFuture`] should be awaited in order for messages to be sent and received.
    pub fn build(self) -> (WebRtcSocket<WebRtcChannels>, MessageLoopFuture) {
        if self.channels.is_empty() {
            panic!("You need to configure at least one channel in WebRtcSocketBuilder");
        }

        let (peer_state_tx, peer_state_rx) = futures_channel::mpsc::unbounded();

        let (messages_from_peers_tx, messages_from_peers_rx) =
            new_senders_and_receivers(&self.channels);
        let (peer_messages_out_tx, peer_messages_out_rx) =
            new_senders_and_receivers(&self.channels);
        let channels = messages_from_peers_rx
            .into_iter()
            .zip(peer_messages_out_tx.into_iter())
            .map(|(rx, tx)| Some(WebRtcChannel { rx, tx }))
            .collect();

        let (id_tx, id_rx) = crossbeam_channel::bounded(1);

        (
            WebRtcSocket {
                id: Default::default(),
                id_rx,
                peer_state_rx,
                peers: Default::default(),
                channels,
            },
            Box::pin(run_socket(
                id_tx,
                self,
                peer_messages_out_rx,
                peer_state_tx,
                messages_from_peers_tx,
            )),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The state of a connection to a peer
pub enum PeerState {
    /// The peer is connected
    ///
    /// This means all of the following should be true:
    ///
    /// - The requested data channels have been established and are healthy
    /// - The peer hasn't left the signalling server
    Connected,
    /// We no longer have a connection to this peer:
    ///
    /// This means either:
    ///
    /// - Some of the the data channels got disconnected/closed
    /// - The peer left the signalling server
    Disconnected,
}
/// Used to send and recieve packets on a given web rtc channel
#[derive(Debug)]
pub struct WebRtcChannel {
    tx: UnboundedSender<(PeerId, Packet)>,
    rx: UnboundedReceiver<(PeerId, Packet)>,
}

impl WebRtcChannel {
    /// Call this where you want to handle new received messages.
    ///
    /// Messages are removed from the socket when called.
    pub fn receive(&mut self) -> Vec<(PeerId, Packet)> {
        std::iter::repeat_with(|| self.rx.try_next())
            .map_while(Result::ok)
            .flatten()
            .collect()
    }

    /// Send a packet to the given peer.
    pub fn send(&mut self, packet: Packet, peer: PeerId) {
        self.tx.unbounded_send((peer, packet)).expect("Send failed");
    }
}

type WebRtcChannels = Vec<Option<WebRtcChannel>>;

/// Contains a set of web rtc channels and some connection metadata.
#[derive(Debug)]
pub struct WebRtcSocket<C = WebRtcChannel> {
    id: once_cell::race::OnceBox<PeerId>,
    id_rx: crossbeam_channel::Receiver<PeerId>,
    peer_state_rx: futures_channel::mpsc::UnboundedReceiver<(PeerId, PeerState)>,
    peers: HashMap<PeerId, PeerState>,
    channels: C,
}

impl WebRtcSocket {
    /// Creates a new builder for a connection to a given room with a given number of
    /// re-connection attempts.
    ///
    /// You must add at least one channel with [`WebRtcSocketBuilder::add_channel`],
    /// [`WebRtcSocketBuilder::add_reliable_channel`], or
    /// [`WebRtcSocketBuilder::add_unreliable_channel`] before you can build the
    /// [`WebRtcSocket`]
    pub fn builder(room_url: impl Into<String>) -> WebRtcSocketBuilder {
        WebRtcSocketBuilder::new(room_url)
    }

    /// Creates a [`WebRtcSocket`] and the corresponding [`MessageLoopFuture`] for a
    /// socket with a single unreliable channel.
    ///
    /// The returned [`MessageLoopFuture`] should be awaited in order for messages to
    /// be sent and received.
    ///
    /// Please use the [`WebRtcSocketBuilder`] to create non-trivial sockets.
    pub fn new_unreliable(
        room_url: impl Into<String>,
    ) -> (WebRtcSocket<WebRtcChannel>, MessageLoopFuture) {
        WebRtcSocketBuilder::new(room_url)
            .add_channel(ChannelConfig::unreliable())
            .build()
    }

    /// Creates a [`WebRtcSocket`] and the corresponding [`MessageLoopFuture`] for a
    /// socket with a single reliable channel.
    ///
    /// The returned [`MessageLoopFuture`] should be awaited in order for messages to
    /// be sent and received.
    ///
    /// Please use the [`WebRtcSocketBuilder`] to create non-trivial sockets.
    pub fn new_reliable(
        room_url: impl Into<String>,
    ) -> (WebRtcSocket<WebRtcChannel>, MessageLoopFuture) {
        WebRtcSocketBuilder::new(room_url)
            .add_channel(ChannelConfig::reliable())
            .build()
    }

    /// Handle peers connecting or disconnecting
    ///
    /// Constructed using [`WebRtcSocketBuilder`].
    ///
    /// Update the set of peers used by [`connected_peers`],
    /// [`disconnected_peers`], and [`broadcast_on_channel`].
    ///
    /// Returns the peers that connected or disconnected since the last time
    /// this method was called.
    ///
    /// See also: [`PeerSate`]
    pub fn update_peers(&mut self) -> Vec<(PeerId, PeerState)> {
        let mut changes = Vec::new();
        while let Ok(Some((id, state))) = self.peer_state_rx.try_next() {
            let old = self.peers.insert(id, state);
            if old != Some(state) {
                changes.push((id, state));
            }
        }
        changes
    }

    /// Returns an iterator of the ids of the connected peers.
    ///
    /// Note: You have to call [`update_peers`] for this list to be accurate.
    ///
    /// See also: [`WebRtcSocket::disconnected_peers`]
    pub fn connected_peers(&'_ self) -> impl std::iter::Iterator<Item = PeerId> + '_ {
        self.peers.iter().filter_map(|(id, state)| {
            if state == &PeerState::Connected {
                Some(*id)
            } else {
                None
            }
        })
    }

    /// Returns an iterator of the ids of peers that are no longer connected.
    ///
    /// Note: You have to call [`update_peers`] for this list to be accurate.
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

    /// Returns the id of this peer, this may be `None` if an id has not yet
    /// been assigned by the server.
    pub fn id(&self) -> Option<PeerId> {
        if let Some(id) = self.id.get() {
            Some(*id)
        } else if let Ok(id) = self.id_rx.try_recv() {
            let id = self.id.get_or_init(|| id.into());
            Some(*id)
        } else {
            None
        }
    }
}

impl WebRtcSocket<WebRtcChannel> {
    /// Call this where you want to handle new received messages.
    ///
    /// Messages are removed from the socket when called.
    pub fn receive(&mut self) -> Vec<(PeerId, Packet)> {
        self.channels.receive()
    }

    /// Send a packet to the given peer.
    pub fn send(&mut self, packet: Packet, peer: PeerId) {
        self.channels.send(packet, peer)
    }
}

impl WebRtcSocket<WebRtcChannels> {
    /// Gets a reference to the [`WebRtcChannel`] of a given id.
    ///
    /// See also: [`WebRtcSocket::take_channel`]
    pub fn channel(&mut self, channel: usize) -> Result<&mut WebRtcChannel, ChannelError> {
        self.channels
            .get_mut(channel)
            .ok_or(ChannelError::ChannelNotFound)?
            .as_mut()
            .ok_or(ChannelError::ChannelTaken)
    }

    /// Takes the [`WebRtcChannel`] of a given id.
    ///
    /// See also: [`WebRtcSocket::channel`]
    pub fn take_channel(&mut self, channel: usize) -> Result<WebRtcChannel, ChannelError> {
        self.channels
            .get_mut(channel)
            .ok_or(ChannelError::ChannelNotFound)?
            .take()
            .ok_or(ChannelError::ChannelTaken)
    }
}

pub(crate) fn new_senders_and_receivers<T>(
    channel_configs: &[ChannelConfig],
) -> (Vec<UnboundedSender<T>>, Vec<UnboundedReceiver<T>>) {
    (0..channel_configs.len())
        .map(|_| futures_channel::mpsc::unbounded())
        .unzip()
}

pub(crate) fn create_data_channels_ready_fut(
    channel_configs: &[ChannelConfig],
) -> (
    Vec<futures_channel::mpsc::Sender<()>>,
    Pin<Box<Fuse<impl Future<Output = ()>>>>,
) {
    let (senders, receivers) = (0..channel_configs.len())
        .map(|_| futures_channel::mpsc::channel(1))
        .unzip();

    (senders, Box::pin(wait_for_ready(receivers).fuse()))
}

async fn wait_for_ready(channel_ready_rx: Vec<futures_channel::mpsc::Receiver<()>>) {
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
    pub peer_state_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
    pub messages_from_peers_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
}

async fn run_socket<C>(
    id_tx: crossbeam_channel::Sender<PeerId>,
    config: WebRtcSocketBuilder<C>,
    peer_messages_out_rx: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    peer_state_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
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
        peer_state_tx,
        messages_from_peers_tx,
    };
    let message_loop_fut = message_loop::<UseMessenger, C>(id_tx, config, channels);

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
    use crate::{webrtc_socket::error::SignallingError, ChannelConfig, Error, WebRtcSocketBuilder};

    #[futures_test::test]
    async fn unreachable_server() {
        // .invalid is a reserved tld for testing and documentation
        let (_socket, fut) = WebRtcSocketBuilder::new("wss://example.invalid")
            .add_channel(ChannelConfig::unreliable())
            .build();

        let result = fut.await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Signalling(_)));
    }

    #[futures_test::test]
    async fn test_signalling_attempts() {
        let (_socket, loop_fut) = WebRtcSocketBuilder::new("wss://example.invalid/")
            .reconnect_attempts(Some(3))
            .add_channel(ChannelConfig::reliable())
            .build();

        let result = loop_fut.await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::Signalling(SignallingError::ConnectionFailed(_))
        ));
    }
}
