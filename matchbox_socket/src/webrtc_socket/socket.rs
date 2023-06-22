use super::error::GetChannelError;
use crate::{
    webrtc_socket::{
        message_loop, signaling_loop, MessageLoopFuture, Packet, PeerEvent, PeerRequest,
        UseMessenger, UseSignaller,
    },
    Error,
};
use futures::{future::Fuse, select, Future, FutureExt, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use log::{debug, error};
use matchbox_protocol::PeerId;
use std::{collections::HashMap, marker::PhantomData, pin::Pin, time::Duration};

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
/// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel>
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

/// Tags types which are used to indicate the number of [`WebRtcChannel`]s or
/// [`ChannelConfig`]s in a [`WebRtcSocket`] or [`WebRtcSocketBuilder`] respectively.
pub trait ChannelPlurality: Send + Sync {}

/// Tags types which are used to indicate a quantity of [`ChannelConfig`]s which can be
/// used to build a [`WebRtcSocket`].
pub trait BuildablePlurality: ChannelPlurality {}

/// Indicates that the type has no [`WebRtcChannel`]s or [`ChannelConfig`]s.
#[derive(Debug)]
pub struct NoChannels;
impl ChannelPlurality for NoChannels {}

/// Indicates that the type has exactly one [`WebRtcChannel`] or [`ChannelConfig`].
#[derive(Debug)]
pub struct SingleChannel;
impl ChannelPlurality for SingleChannel {}
impl BuildablePlurality for SingleChannel {}

/// Indicates that the type has more than one [`WebRtcChannel`]s or [`ChannelConfig`]s.
#[derive(Debug)]
pub struct MultipleChannels;
impl ChannelPlurality for MultipleChannels {}
impl BuildablePlurality for MultipleChannels {}

#[derive(Debug, Clone)]
pub(crate) struct SocketConfig {
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
    /// Interval at which to send empty requests to the signaling server
    pub(crate) keep_alive_interval: Option<Duration>,
}

/// Builder for [`WebRtcSocket`]s.
///
/// Begin with [`WebRtcSocketBuilder::new`] and add at least one channel with
/// [`WebRtcSocketBuilder::add_channel`] before calling
/// [`WebRtcSocketBuilder::build`] to produce the desired [`WebRtcSocket`].
#[derive(Debug, Clone)]
pub struct WebRtcSocketBuilder<C: ChannelPlurality = NoChannels> {
    pub(crate) config: SocketConfig,
    pub(crate) channel_plurality: PhantomData<C>,
}

impl WebRtcSocketBuilder {
    /// Creates a new builder for a connection to a given room with the default ICE
    /// server configuration, and three reconnection attempts.
    ///
    /// You must add at least one channel with [`WebRtcSocketBuilder::add_channel`]
    /// before you can build the [`WebRtcSocket`]
    pub fn new(room_url: impl Into<String>) -> Self {
        Self {
            config: SocketConfig {
                room_url: room_url.into(),
                ice_server: RtcIceServerConfig::default(),
                channels: Vec::default(),
                attempts: Some(3),
                keep_alive_interval: Some(Duration::from_secs(10)),
            },
            channel_plurality: PhantomData::default(),
        }
    }

    /// Sets the socket ICE server configuration.
    pub fn ice_server(mut self, ice_server: RtcIceServerConfig) -> Self {
        self.config.ice_server = ice_server;
        self
    }

    /// Sets the number of attempts to make at reconnecting to the signaling server,
    /// if `None` the socket will attempt to connect indefinitely.
    ///
    /// The default is 2 reconnection attempts.
    pub fn reconnect_attempts(mut self, attempts: Option<u16>) -> Self {
        self.config.attempts = attempts;
        self
    }

    /// Sets the interval at which to send empty requests to the signaling server.
    ///
    /// Some web services (like e.g. nginx as a reverse proxy) will close idle
    /// web sockets. Setting this interval will periodically send empty requests
    /// to let the signaling server know the client is still connected and
    /// prevent disconnections.
    ///
    /// The defaults is 10 seconds.
    pub fn signaling_keep_alive_interval(mut self, interval: Option<Duration>) -> Self {
        self.config.keep_alive_interval = interval;
        self
    }
}

impl WebRtcSocketBuilder<NoChannels> {
    /// Adds a new channel to the [`WebRtcSocket`] configuration according to a [`ChannelConfig`].
    pub fn add_channel(mut self, config: ChannelConfig) -> WebRtcSocketBuilder<SingleChannel> {
        self.config.channels.push(config);
        WebRtcSocketBuilder {
            config: self.config,
            channel_plurality: PhantomData::default(),
        }
    }

    /// Adds a new unreliable channel to the [`WebRtcSocket`] configuration.
    pub fn add_unreliable_channel(mut self) -> WebRtcSocketBuilder<SingleChannel> {
        self.config.channels.push(ChannelConfig::unreliable());
        WebRtcSocketBuilder {
            config: self.config,
            channel_plurality: PhantomData::default(),
        }
    }

    /// Adds a new reliable channel to the [`WebRtcSocket`] configuration.
    pub fn add_reliable_channel(mut self) -> WebRtcSocketBuilder<SingleChannel> {
        self.config.channels.push(ChannelConfig::reliable());
        WebRtcSocketBuilder {
            config: self.config,
            channel_plurality: PhantomData::default(),
        }
    }
}

impl WebRtcSocketBuilder<SingleChannel> {
    /// Adds a new channel to the [`WebRtcSocket`] configuration according to a [`ChannelConfig`].
    pub fn add_channel(mut self, config: ChannelConfig) -> WebRtcSocketBuilder<MultipleChannels> {
        self.config.channels.push(config);
        WebRtcSocketBuilder {
            config: self.config,
            channel_plurality: PhantomData::default(),
        }
    }

    /// Adds a new unreliable channel to the [`WebRtcSocket`] configuration.
    pub fn add_unreliable_channel(mut self) -> WebRtcSocketBuilder<MultipleChannels> {
        self.config.channels.push(ChannelConfig::unreliable());
        WebRtcSocketBuilder {
            config: self.config,
            channel_plurality: PhantomData::default(),
        }
    }

    /// Adds a new reliable channel to the [`WebRtcSocket`] configuration.
    pub fn add_reliable_channel(mut self) -> WebRtcSocketBuilder<MultipleChannels> {
        self.config.channels.push(ChannelConfig::reliable());
        WebRtcSocketBuilder {
            config: self.config,
            channel_plurality: PhantomData::default(),
        }
    }
}
impl WebRtcSocketBuilder<MultipleChannels> {
    /// Adds a new channel to the [`WebRtcSocket`] configuration according to a [`ChannelConfig`].
    pub fn add_channel(mut self, config: ChannelConfig) -> WebRtcSocketBuilder<MultipleChannels> {
        self.config.channels.push(config);
        self
    }

    /// Adds a new unreliable channel to the [`WebRtcSocket`] configuration.
    pub fn add_unreliable_channel(mut self) -> WebRtcSocketBuilder<MultipleChannels> {
        self.config.channels.push(ChannelConfig::unreliable());
        WebRtcSocketBuilder {
            config: self.config,
            channel_plurality: PhantomData::default(),
        }
    }

    /// Adds a new reliable channel to the [`WebRtcSocket`] configuration.
    pub fn add_reliable_channel(mut self) -> WebRtcSocketBuilder<MultipleChannels> {
        self.config.channels.push(ChannelConfig::reliable());
        WebRtcSocketBuilder {
            config: self.config,
            channel_plurality: PhantomData::default(),
        }
    }
}

impl<C: BuildablePlurality> WebRtcSocketBuilder<C> {
    /// Creates a [`WebRtcSocket`] and the corresponding [`MessageLoopFuture`] according to the
    /// configuration supplied.
    ///
    /// The returned [`MessageLoopFuture`] should be awaited in order for messages to be sent and
    /// received.
    pub fn build(self) -> (WebRtcSocket<C>, MessageLoopFuture) {
        if self.config.channels.is_empty() {
            unreachable!();
        }

        let (peer_state_tx, peer_state_rx) = futures_channel::mpsc::unbounded();

        let (messages_from_peers_tx, messages_from_peers_rx) =
            new_senders_and_receivers(&self.config.channels);
        let (peer_messages_out_tx, peer_messages_out_rx) =
            new_senders_and_receivers(&self.config.channels);
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
                channel_plurality: PhantomData::default(),
            },
            Box::pin(run_socket(
                id_tx,
                self.config,
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
    /// - The peer hasn't left the signaling server
    Connected,
    /// We no longer have a connection to this peer:
    ///
    /// This means either:
    ///
    /// - Some of the the data channels got disconnected/closed
    /// - The peer left the signaling server
    Disconnected,
}
/// Used to send and receive packets on a given WebRTC channel. Must be created as part of a
/// [`WebRtcSocket`].
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

/// Contains a set of [`WebRtcChannel`]s and connection metadata.
#[derive(Debug)]
pub struct WebRtcSocket<C: ChannelPlurality = SingleChannel> {
    id: once_cell::race::OnceBox<PeerId>,
    id_rx: crossbeam_channel::Receiver<PeerId>,
    peer_state_rx: futures_channel::mpsc::UnboundedReceiver<(PeerId, PeerState)>,
    peers: HashMap<PeerId, PeerState>,
    channels: Vec<Option<WebRtcChannel>>,
    channel_plurality: PhantomData<C>,
}

impl WebRtcSocket {
    /// Creates a new builder for a connection to a given room with a given number of
    /// re-connection attempts.
    ///
    /// You must add at least one channel with [`WebRtcSocketBuilder::add_channel`]
    /// before you can build the [`WebRtcSocket`]
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
    ) -> (WebRtcSocket<SingleChannel>, MessageLoopFuture) {
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
    ) -> (WebRtcSocket<SingleChannel>, MessageLoopFuture) {
        WebRtcSocketBuilder::new(room_url)
            .add_channel(ChannelConfig::reliable())
            .build()
    }
}

impl<C: ChannelPlurality> WebRtcSocket<C> {
    /// Handle peers connecting or disconnecting
    ///
    /// Constructed using [`WebRtcSocketBuilder`].
    ///
    /// Update the set of peers used by [`WebRtcSocket::connected_peers`] and
    /// [`WebRtcSocket::disconnected_peers`].
    ///
    /// Returns the peers that connected or disconnected since the last time
    /// this method was called.
    ///
    /// See also: [`PeerState`]
    ///
    /// # Panics
    ///
    /// Will panic if the socket has been closed or is broken.
    ///
    /// [`WebRtcSocket::try_update_peers`] is the equivalent method that will instead return a
    /// `Result`.
    pub fn update_peers(&mut self) -> Vec<(PeerId, PeerState)> {
        self.try_update_peers().unwrap()
    }

    /// Similar to [`WebRtcSocket::update_peers`]. Will instead return a Result::Err if the
    /// socket is closed or broken.
    pub fn try_update_peers(&mut self) -> Result<Vec<(PeerId, PeerState)>, &'static str> {
        let mut changes = Vec::new();
        while let Ok(res) = self.peer_state_rx.try_next() {
            match res {
                Some((id, state)) => {
                    let old = self.peers.insert(id, state);
                    if old != Some(state) {
                        changes.push((id, state));
                    }
                }
                None => return Err("Channel closed"),
            }
        }

        Ok(changes)
    }

    /// Returns an iterator of the ids of the connected peers.
    ///
    /// Note: You have to call [`WebRtcSocket::update_peers`] for this list to be accurate.
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
    /// Note: You have to call [`WebRtcSocket::update_peers`] for this list to be
    /// accurate.
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

    /// Gets a mutable reference to the [`WebRtcChannel`] of a given id.
    ///
    /// ```
    /// use matchbox_socket::*;
    ///
    /// let (mut socket, message_loop) = WebRtcSocketBuilder::new("wss://example.invalid/")
    ///     .add_channel(ChannelConfig::reliable())
    ///     .add_channel(ChannelConfig::unreliable())
    ///     .build();
    /// let reliable_channel_messages = socket.channel(0).receive();
    /// ```
    ///
    /// See also: [`WebRtcSocket::get_channel`], [`WebRtcSocket::take_channel`]
    ///
    /// # Panics
    ///
    /// will panic if the channel cannot be found.
    pub fn channel(&mut self, channel: usize) -> &mut WebRtcChannel {
        self.get_channel(channel).unwrap()
    }

    /// Gets a mutable reference to the [`WebRtcChannel`] of a given id.
    ///
    /// Returns an error if the channel was not found.
    ///
    /// ```
    /// use matchbox_socket::*;
    ///
    /// let (mut socket, message_loop) = WebRtcSocketBuilder::new("wss://example.invalid/")
    ///     .add_channel(ChannelConfig::reliable())
    ///     .add_channel(ChannelConfig::unreliable())
    ///     .build();
    /// let reliable_channel_messages = socket.get_channel(0).unwrap().receive();
    /// ```
    ///
    /// See also: [`WebRtcSocket::channel`], [`WebRtcSocket::take_channel`]
    pub fn get_channel(&mut self, channel: usize) -> Result<&mut WebRtcChannel, GetChannelError> {
        self.channels
            .get_mut(channel)
            .ok_or(GetChannelError::NotFound)?
            .as_mut()
            .ok_or(GetChannelError::Taken)
    }

    /// Takes the [`WebRtcChannel`] of a given id.
    ///
    /// ```
    /// use matchbox_socket::*;
    ///
    /// let (mut socket, message_loop) = WebRtcSocketBuilder::new("wss://example.invalid/")
    ///     .add_channel(ChannelConfig::reliable())
    ///     .add_channel(ChannelConfig::unreliable())
    ///     .build();
    /// let reliable_channel = socket.take_channel(0).unwrap();
    /// let unreliable_channel = socket.take_channel(1).unwrap();
    /// ```
    ///
    /// See also: [`WebRtcSocket::channel`]
    pub fn take_channel(&mut self, channel: usize) -> Result<WebRtcChannel, GetChannelError> {
        self.channels
            .get_mut(channel)
            .ok_or(GetChannelError::NotFound)?
            .take()
            .ok_or(GetChannelError::Taken)
    }
}

impl WebRtcSocket<SingleChannel> {
    /// Call this where you want to handle new received messages.
    ///
    /// Messages are removed from the socket when called.
    pub fn receive(&mut self) -> Vec<(PeerId, Packet)> {
        self.channels
            .get_mut(0)
            .unwrap()
            .as_mut()
            .unwrap()
            .receive()
    }

    /// Send a packet to the given peer.
    pub fn send(&mut self, packet: Packet, peer: PeerId) {
        self.channels
            .get_mut(0)
            .unwrap()
            .as_mut()
            .unwrap()
            .send(packet, peer)
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

async fn run_socket(
    id_tx: crossbeam_channel::Sender<PeerId>,
    config: SocketConfig,
    peer_messages_out_rx: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    peer_state_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
    messages_from_peers_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
) -> Result<(), Error> {
    debug!("Starting WebRtcSocket");

    let (requests_sender, requests_receiver) = futures_channel::mpsc::unbounded::<PeerRequest>();
    let (events_sender, events_receiver) = futures_channel::mpsc::unbounded::<PeerEvent>();

    let signaling_loop_fut = signaling_loop::<UseSignaller>(
        config.attempts,
        config.room_url,
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
    let message_loop_fut = message_loop::<UseMessenger>(
        id_tx,
        &config.ice_server,
        &config.channels,
        channels,
        config.keep_alive_interval,
    );

    let mut message_loop_done = Box::pin(message_loop_fut.fuse());
    let mut signaling_loop_done = Box::pin(signaling_loop_fut.fuse());
    loop {
        select! {
            _ = message_loop_done => {
                debug!("Message loop completed");
                break;
            }

            sigloop = signaling_loop_done => {
                match sigloop {
                    Ok(()) => debug!("Signaling loop completed"),
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
    use crate::{webrtc_socket::error::SignalingError, ChannelConfig, Error, WebRtcSocketBuilder};

    #[futures_test::test]
    async fn unreachable_server() {
        // .invalid is a reserved tld for testing and documentation
        let (_socket, fut) = WebRtcSocketBuilder::new("wss://example.invalid")
            .add_channel(ChannelConfig::unreliable())
            .build();

        let result = fut.await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Signaling(_)));
    }

    #[futures_test::test]
    async fn test_signaling_attempts() {
        let (_socket, loop_fut) = WebRtcSocketBuilder::new("wss://example.invalid/")
            .reconnect_attempts(Some(3))
            .add_channel(ChannelConfig::reliable())
            .build();

        let result = loop_fut.await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::Signaling(SignalingError::ConnectionFailed(_))
        ));
    }
}
