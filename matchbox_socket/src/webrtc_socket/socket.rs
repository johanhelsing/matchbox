use super::{
    SignallerBuilder,
    error::{ChannelError, SignalingError},
};
use crate::{
    Error,
    webrtc_socket::{
        MessageLoopFuture, Packet, PeerEvent, PeerRequest, UseMessenger, UseSignallerBuilder,
        message_loop, signaling_loop,
    },
};
use bytes::Bytes;
use futures::{
    AsyncRead, AsyncWrite, Future, FutureExt, Sink, SinkExt, Stream, StreamExt, TryStreamExt,
    future::Fuse, select,
};
use futures_channel::mpsc::{SendError, TrySendError, UnboundedReceiver, UnboundedSender};
use log::{debug, error};
use matchbox_protocol::PeerId;
use std::{collections::HashMap, future::ready, pin::Pin, sync::Arc, task::Poll, time::Duration};
#[cfg(not(target_arch = "wasm32"))]
use tokio_util::{
    compat::TokioAsyncWriteCompatExt,
    io::{CopyToBytes, SinkWriter},
};

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
#[derive(Debug, Clone, Copy)]
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
    /// quickly as possible.
    pub fn unreliable() -> Self {
        ChannelConfig {
            ordered: false,
            max_retransmits: Some(0),
        }
    }

    /// Messages sent via a reliable channel are guaranteed to arrive (and guaranteed to arrive in
    /// order the order they were sent) and will be retransmitted until they arrive.
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
pub struct WebRtcSocketBuilder {
    pub(crate) config: SocketConfig,
    pub(crate) signaller_builder: Option<Arc<dyn SignallerBuilder>>,
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
            signaller_builder: None,
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
    /// The default is 3 reconnection attempts.
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

    /// Adds a new channel to the [`WebRtcSocket`] configuration according to a [`ChannelConfig`].
    pub fn add_channel(mut self, config: ChannelConfig) -> WebRtcSocketBuilder {
        self.config.channels.push(config);
        self
    }

    /// Adds a new unreliable channel to the [`WebRtcSocket`] configuration.
    pub fn add_unreliable_channel(mut self) -> WebRtcSocketBuilder {
        self.config.channels.push(ChannelConfig::unreliable());
        self
    }

    /// Adds a new reliable channel to the [`WebRtcSocket`] configuration.
    pub fn add_reliable_channel(mut self) -> WebRtcSocketBuilder {
        self.config.channels.push(ChannelConfig::reliable());
        self
    }

    /// Sets an alternative signalling implementation for this [`WebRtcSocket`].
    pub fn signaller_builder(mut self, signaller_builder: Arc<dyn SignallerBuilder>) -> Self {
        self.signaller_builder = Some(signaller_builder);
        self
    }

    /// Creates a [`WebRtcSocket`] and the corresponding [`MessageLoopFuture`] according to the
    /// configuration supplied.
    ///
    /// The returned [`MessageLoopFuture`] should be awaited in order for messages to be sent and
    /// received.
    pub fn build(self) -> (WebRtcSocket, MessageLoopFuture) {
        assert!(
            !self.config.channels.is_empty(),
            "Must have added at least one channel"
        );

        let (peer_state_tx, peer_state_rx) = futures_channel::mpsc::unbounded();

        let mut peer_messages_out_rx = Vec::with_capacity(self.config.channels.len());
        let mut messages_from_peers_tx = Vec::with_capacity(self.config.channels.len());
        let mut channels = Vec::with_capacity(self.config.channels.len());
        for channel_config in self.config.channels.iter() {
            let (messages_from_peers_tx_curr, messages_from_peers_rx_curr) =
                futures_channel::mpsc::unbounded();
            let (peer_messages_out_tx_curr, peer_messages_out_rx_curr) =
                futures_channel::mpsc::unbounded();
            peer_messages_out_rx.push(peer_messages_out_rx_curr);
            messages_from_peers_tx.push(messages_from_peers_tx_curr);
            channels.push(Some(WebRtcChannel {
                config: *channel_config,
                rx: messages_from_peers_rx_curr,
                tx: peer_messages_out_tx_curr,
            }));
        }

        let (id_tx, id_rx) = futures_channel::oneshot::channel();
        let signaller_builder = self
            .signaller_builder
            .unwrap_or_else(|| Arc::new(UseSignallerBuilder::default()));

        let socket_fut = run_socket(
            signaller_builder,
            id_tx,
            self.config,
            peer_messages_out_rx,
            peer_state_tx,
            messages_from_peers_tx,
        )
        // Transform the source into a user-error.
        .map(|f| {
            f.map_err(|e| match e {
                SignalingError::UndeliverableSignal(e) => Error::Disconnected(e.into()),
                SignalingError::NegotiationFailed(e) => Error::ConnectionFailed(*e),
                SignalingError::WebSocket(e) => Error::Disconnected(e.into()),
                SignalingError::UserImplementationError(_) => Error::ConnectionFailed(e),
                SignalingError::UnknownFormat | SignalingError::StreamExhausted => {
                    unimplemented!("these errors should never be propagated here")
                }
            })
        });

        (
            WebRtcSocket {
                id: Default::default(),
                id_rx,
                peer_state_rx,
                peers: Default::default(),
                channels,
            },
            Box::pin(socket_fut),
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
    config: ChannelConfig,
    tx: UnboundedSender<(PeerId, Packet)>,
    rx: UnboundedReceiver<(PeerId, Packet)>,
}

impl WebRtcChannel {
    /// Split the channel into a reader and writer.
    /// Useful for concurrently sending and receiving messages using async code.
    #[allow(clippy::type_complexity)]
    pub fn split(
        self,
    ) -> (
        UnboundedSender<(PeerId, Packet)>,
        UnboundedReceiver<(PeerId, Packet)>,
    ) {
        (self.tx, self.rx)
    }

    /// Clone a sender for this channel.
    /// Useful for sending messages to the same channel from multiple threads/async tasks.
    pub fn sender_clone(&self) -> UnboundedSender<(PeerId, Packet)> {
        self.tx.clone()
    }

    /// Returns the [`ChannelConfig`] used to create this channel.
    pub fn config(&self) -> &ChannelConfig {
        &self.config
    }

    /// Returns whether it's still possible to send messages.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// Close this channel.
    ///
    /// This prevents sending and receiving any messages in the future, but does not drain messages
    /// that are buffered.
    pub fn close(&mut self) {
        self.tx.close_channel();
        self.rx.close();
    }

    /// Call this where you want to handle new received messages. Returns immediately.
    ///
    /// Messages are removed from the socket when called.
    pub fn receive(&mut self) -> Vec<(PeerId, Packet)> {
        let mut messages = vec![];
        while let Ok(Some(x)) = self.rx.try_next() {
            messages.push(x);
        }
        messages
    }

    /// Try to send a packet to the given peer. An error is propagated if the socket future
    /// is dropped. `Ok` is not a guarantee of delivery.
    pub fn try_send(&mut self, packet: Packet, peer: PeerId) -> Result<(), SendError> {
        self.tx
            .unbounded_send((peer, packet))
            .map_err(TrySendError::into_send_error)
    }

    /// Send a packet to the given peer. There is no guarantee of delivery.
    ///
    /// # Panics
    /// Panics if the socket future is dropped.
    pub fn send(&mut self, packet: Packet, peer: PeerId) {
        self.try_send(packet, peer).expect("Send failed");
    }
}

impl Stream for WebRtcChannel {
    type Item = (PeerId, Packet);

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut rx = Pin::new(&mut self.get_mut().rx);
        rx.as_mut().poll_next(cx)
    }
}

impl Sink<(PeerId, Packet)> for WebRtcChannel {
    type Error = SendError;

    fn poll_ready(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let mut tx = Pin::new(&mut self.get_mut().tx);
        tx.as_mut().poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: (PeerId, Packet)) -> Result<(), Self::Error> {
        let mut tx = Pin::new(&mut self.get_mut().tx);
        tx.as_mut().start_send(item)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let mut tx = Pin::new(&mut self.get_mut().tx);
        tx.as_mut().poll_flush(cx)
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let mut tx = Pin::new(&mut self.get_mut().tx);
        tx.as_mut().poll_close(cx)
    }
}

/// A channel which supports reading and writing raw bytes.
pub struct RawPeerChannel<R, W> {
    id: Option<PeerId>,
    remote: PeerId,
    reader: R,
    writer: W,
}

impl<R, W> RawPeerChannel<R, W> {
    /// Returns the id of this peer.
    ///
    /// Also see [`WebRtcSocket::id`].
    pub fn id(&self) -> Option<PeerId> {
        self.id
    }

    /// Returns the id of the remote peer to which this channel is connected.
    pub fn remote(&self) -> PeerId {
        self.remote
    }
}

impl<R, W> AsyncRead for RawPeerChannel<R, W>
where
    Self: Unpin,
    R: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut reader = Pin::new(&mut self.get_mut().reader);
        reader.as_mut().poll_read(cx, buf)
    }
}

impl<R, W> AsyncWrite for RawPeerChannel<R, W>
where
    Self: Unpin,
    W: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut writer = Pin::new(&mut self.get_mut().writer);
        writer.as_mut().poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut writer = Pin::new(&mut self.get_mut().writer);
        writer.as_mut().poll_flush(cx)
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut writer = Pin::new(&mut self.get_mut().writer);
        writer.as_mut().poll_close(cx)
    }
}

/// Contains a set of [`WebRtcChannel`]s and connection metadata.
#[derive(Debug)]
pub struct WebRtcSocket {
    id: once_cell::race::OnceBox<PeerId>,
    id_rx: futures_channel::oneshot::Receiver<PeerId>,
    peer_state_rx: futures_channel::mpsc::UnboundedReceiver<(PeerId, PeerState)>,
    peers: HashMap<PeerId, PeerState>,
    channels: Vec<Option<WebRtcChannel>>,
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
    pub fn new_unreliable(room_url: impl Into<String>) -> (WebRtcSocket, MessageLoopFuture) {
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
    pub fn new_reliable(room_url: impl Into<String>) -> (WebRtcSocket, MessageLoopFuture) {
        WebRtcSocketBuilder::new(room_url)
            .add_channel(ChannelConfig::reliable())
            .build()
    }
}

impl Stream for WebRtcSocket {
    type Item = (PeerId, PeerState);

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut peer_state_rx = Pin::new(&mut self.get_mut().peer_state_rx);
        peer_state_rx.as_mut().poll_next(cx)
    }
}

impl WebRtcSocket {
    // Todo: Disconnect from the peer, severing all communication channels.
    // pub fn disconnect(&mut self, peer: PeerId) {}

    /// Close this socket, disconnecting all channels.
    pub fn close(&mut self) {
        self.channels
            .iter_mut()
            .filter_map(Option::as_mut)
            .for_each(|c| c.close());
    }

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
    /// Will panic if the socket future has been dropped.
    ///
    /// [`WebRtcSocket::try_update_peers`] is the equivalent method that will instead return a
    /// `Result`.
    pub fn update_peers(&mut self) -> Vec<(PeerId, PeerState)> {
        self.try_update_peers().unwrap()
    }

    /// Similar to [`WebRtcSocket::update_peers`]. Will instead return a Result::Err if the
    /// socket is closed.
    pub fn try_update_peers(&mut self) -> Result<Vec<(PeerId, PeerState)>, ChannelError> {
        let mut changes = Vec::new();
        while let Ok(res) = self.peer_state_rx.try_next() {
            match res {
                Some((id, state)) => {
                    let old = self.peers.insert(id, state);
                    if old != Some(state) {
                        changes.push((id, state));
                    }
                }
                None => return Err(ChannelError::Closed),
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
    pub fn id(&mut self) -> Option<PeerId> {
        if let Some(id) = self.id.get() {
            Some(*id)
        } else if let Ok(Some(id)) = self.id_rx.try_recv() {
            let id = self.id.get_or_init(|| id.into());
            Some(*id)
        } else {
            None
        }
    }

    /// Gets an immutable reference to the [`WebRtcChannel`] of a given id.
    ///
    /// ```
    /// use matchbox_socket::*;
    ///
    /// let (mut socket, message_loop) = WebRtcSocketBuilder::new("wss://example.invalid/")
    ///     .add_channel(ChannelConfig::reliable())
    ///     .add_channel(ChannelConfig::unreliable())
    ///     .build();
    /// let is_closed = socket.channel(0).is_closed();
    /// ```
    ///
    /// See also: [`WebRtcSocket::channel_mut`], [`WebRtcSocket::get_channel`],
    /// [`WebRtcSocket::take_channel`]
    ///
    /// # Panics
    ///
    /// will panic if the channel cannot be found.
    pub fn channel(&self, channel: usize) -> &WebRtcChannel {
        self.get_channel(channel).unwrap()
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
    /// let reliable_channel_messages = socket.channel_mut(0).receive();
    /// ```
    ///
    /// See also: [`WebRtcSocket::channel`], [`WebRtcSocket::get_channel_mut`],
    /// [`WebRtcSocket::take_channel`]
    ///
    /// # Panics
    ///
    /// will panic if the channel cannot be found.
    pub fn channel_mut(&mut self, channel: usize) -> &mut WebRtcChannel {
        self.get_channel_mut(channel).unwrap()
    }

    /// Gets an immutable reference to the [`WebRtcChannel`] of a given id.
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
    /// let is_closed = socket.get_channel(0).unwrap().is_closed();
    /// ```
    ///
    /// See also: [`WebRtcSocket::get_channel_mut`], [`WebRtcSocket::take_channel`]
    pub fn get_channel(&self, channel: usize) -> Result<&WebRtcChannel, ChannelError> {
        self.channels
            .get(channel)
            .ok_or(ChannelError::NotFound)?
            .as_ref()
            .ok_or(ChannelError::Taken)
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
    /// let reliable_channel_messages = socket.get_channel_mut(0).unwrap().receive();
    /// ```
    ///
    /// See also: [`WebRtcSocket::channel`], [`WebRtcSocket::take_channel`]
    pub fn get_channel_mut(&mut self, channel: usize) -> Result<&mut WebRtcChannel, ChannelError> {
        self.channels
            .get_mut(channel)
            .ok_or(ChannelError::NotFound)?
            .as_mut()
            .ok_or(ChannelError::Taken)
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
    pub fn take_channel(&mut self, channel: usize) -> Result<WebRtcChannel, ChannelError> {
        self.channels
            .get_mut(channel)
            .ok_or(ChannelError::NotFound)?
            .take()
            .ok_or(ChannelError::Taken)
    }

    /// Takes the [`WebRtcChannel`] of a given [`PeerId`].
    pub fn take_channel_by_id(&mut self, id: PeerId) -> Result<WebRtcChannel, ChannelError> {
        let pos = self
            .connected_peers()
            .position(|peer_id| peer_id == id)
            .ok_or(ChannelError::NotFound)?;

        self.take_channel(pos)
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Converts the [`WebRtcChannel`] of a given [`PeerId`] into a [`RawPeerChannel`].
    pub fn take_raw_by_id(
        &mut self,
        remote: PeerId,
    ) -> Result<RawPeerChannel<impl AsyncRead + use<>, impl AsyncWrite + use<>>, ChannelError> {
        let channel = self.take_channel_by_id(remote)?;
        let id = self.id();

        let (reader, writer) = compat_read_write(remote, channel.rx, channel.tx);

        let peer_channel = RawPeerChannel {
            id,
            remote,
            reader,
            writer,
        };

        Ok(peer_channel)
    }

    /// Returns whether any socket channel is closed
    pub fn any_channel_closed(&self) -> bool {
        self.channels
            .iter()
            .filter_map(Option::as_ref)
            .any(|c| c.is_closed())
    }

    /// Returns whether all socket channels are closed
    pub fn all_channels_closed(&self) -> bool {
        self.channels
            .iter()
            .filter_map(Option::as_ref)
            .all(|c| c.is_closed())
    }
}

pub(crate) fn create_data_channels_ready_fut(
    channel_configs: &[ChannelConfig],
) -> (
    Vec<futures_channel::mpsc::Sender<()>>,
    Pin<Box<Fuse<impl Future<Output = ()> + use<>>>>,
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
    builder: Arc<dyn SignallerBuilder>,
    id_tx: futures_channel::oneshot::Sender<PeerId>,
    config: SocketConfig,
    peer_messages_out_rx: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    peer_state_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
    messages_from_peers_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
) -> Result<(), SignalingError> {
    debug!("Starting WebRtcSocket");

    let (requests_sender, requests_receiver) = futures_channel::mpsc::unbounded::<PeerRequest>();
    let (events_sender, events_receiver) = futures_channel::mpsc::unbounded::<PeerEvent>();

    let signaling_loop_fut = signaling_loop(
        builder,
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
            msgloop = message_loop_done => {
                match msgloop {
                    Ok(()) | Err(SignalingError::StreamExhausted) => {
                        debug!("Message loop completed");
                        break Ok(())
                    },
                    Err(e) => {
                        // TODO: Reconnect X attempts if configured to reconnect.
                        error!("The message loop finished with an error: {e:?}");
                        break Err(e);
                    },
                }
            }

            sigloop = signaling_loop_done => {
                match sigloop {
                    Ok(()) => debug!("Signaling loop completed"),
                    Err(SignalingError::StreamExhausted) => {
                        debug!("Signaling loop completed");
                        break Ok(());
                    },
                    Err(e) => {
                        // TODO: Reconnect X attempts if configured to reconnect.
                        error!("The signaling loop finished with an error: {e:?}");
                        break Err(e);
                    },
                }
            }

            complete => break Ok(())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn compat_read_write(
    remote: PeerId,
    stream: UnboundedReceiver<(PeerId, Packet)>,
    sink: UnboundedSender<(PeerId, Packet)>,
) -> (impl AsyncRead, impl AsyncWrite) {
    let reader = stream
        .then(|(_, packet)| ready(Ok::<_, std::io::Error>(packet)))
        .into_async_read();

    let writer = sink
        .with(move |packet: Bytes| ready(Ok::<_, SendError>((remote, Box::from(packet.as_ref())))));

    let writer = writer.sink_map_err(std::io::Error::other);
    let writer = CopyToBytes::new(writer);
    let writer = SinkWriter::new(writer);
    let writer = TokioAsyncWriteCompatExt::compat_write(writer);

    (reader, writer)
}

#[cfg(test)]
mod test {
    use crate::{ChannelConfig, Error, WebRtcSocketBuilder};

    #[futures_test::test]
    async fn unreachable_server() {
        // .invalid is a reserved tld for testing and documentation
        let (_socket, fut) = WebRtcSocketBuilder::new("wss://example.invalid")
            .add_channel(ChannelConfig::unreliable())
            .build();

        let result = fut.await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::ConnectionFailed { .. }
        ));
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
            Error::ConnectionFailed { .. },
        ));
    }
}
