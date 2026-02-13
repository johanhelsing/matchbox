//! The webrtc_socket module provides a unified API for dealing with WebRtc on both native and wasm
//! builds. This common API only implements behaviors needed by the rest of matchbox_socket: it is
//! not a general WebRtc abstraction.

pub mod error;
mod messages;
mod signal_peer;
mod socket;

use self::error::SignalingError;
use crate::{Error, webrtc_socket::signal_peer::SignalPeer};
use async_trait::async_trait;
use cfg_if::cfg_if;
use futures::{Future, FutureExt, StreamExt, future::Either, stream::FuturesUnordered};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::select;
use log::{debug, error, warn};
use matchbox_protocol::PeerId;
pub use messages::*;
pub(crate) use socket::MessageLoopChannels;
pub use socket::{
    ChannelConfig, PeerState, RtcIceServerConfig, WebRtcChannel, WebRtcSocket, WebRtcSocketBuilder,
};
use socket::{PeerMessage, SocketConfig, UnboundedDataChannelReceiver};
use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
};

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        mod wasm;
        type UseMessenger = wasm::WasmMessenger;
        type UseSignallerBuilder = wasm::WasmSignallerBuilder;
        /// A future which runs the message loop for the socket and completes
        /// when the socket closes or disconnects
        pub type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>>>>;
    } else {
        mod native;
        type UseMessenger = native::NativeMessenger;
        type UseSignallerBuilder = native::NativeSignallerBuilder;
        /// A future which runs the message loop for the socket and completes
        /// when the socket closes or disconnects
        pub type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;
    }
}

/// A builder that constructs a new [Signaller] from a room URL.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait SignallerBuilder: std::fmt::Debug + Sync + Send + 'static {
    /// Create a new [Signaller]. The Room URL is an implementation specific identifier for joining
    /// a room.
    async fn new_signaller(
        &self,
        attempts: Option<u16>,
        room_url: String,
    ) -> Result<Box<dyn Signaller>, SignalingError>;
}

/// A signalling implementation.
///
/// The Signaller is responsible for passing around
/// [WebRTC signals](https://developer.mozilla.org/en-US/docs/Web/API/WebRTC_API/Signaling_and_video_calling#the_signaling_server)
/// between room peers, encoded as [PeerEvent::Signal] which holds a [PeerSignal].
///
/// It is also responsible for notifying each peer of the following special events:
///
/// - [PeerEvent::IdAssigned] -- first event in stream, received once at connection time.
/// - [PeerEvent::NewPeer] -- received when a new peer has joined the room, AND when this node must
///   send an offer to them.
/// - [PeerEvent::PeerLeft] -- received when a peer leaves the room.
///
/// To achieve a full mesh configuration, the signaller must do the following for **each pair** of
/// peers in a room:
///
/// 1. It decides which peer is the "Offerer" and which is the "Answerer".
/// 2. It sends [PeerEvent::NewPeer] to the "Offerer" only.
/// 3. It passes [PeerEvent::Signal] events back and forth between them, until one of them
///    disconnects.
/// 4. It sends [PeerEvent::PeerLeft] with the ID of the disconnected peer to the remaining peer.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Signaller: Sync + Send + 'static {
    /// Request the signaller to pass a message to another peer.
    async fn send(&mut self, request: PeerRequest) -> Result<(), SignalingError>;

    /// Get the next event from the signaller.
    async fn next_message(&mut self) -> Result<PeerEvent, SignalingError>;
}

async fn signaling_loop(
    builder: Arc<dyn SignallerBuilder>,
    attempts: Option<u16>,
    room_url: String,
    mut requests_receiver: futures_channel::mpsc::UnboundedReceiver<PeerRequest>,
    events_sender: futures_channel::mpsc::UnboundedSender<PeerEvent>,
) -> Result<(), SignalingError> {
    let mut signaller = builder.new_signaller(attempts, room_url).await?;

    loop {
        select! {
            request = requests_receiver.next().fuse() => {
                debug!("-> {request:?}");
                let Some(request) = request else {break Err(SignalingError::StreamExhausted)};
                signaller.send(request).await?;
            }

            message = signaller.next_message().fuse() => {
                match message {
                    Ok(message) => {
                        debug!("Received {message:?}");
                        events_sender.unbounded_send(message).map_err(SignalingError::from)?;
                    }
                    Err(SignalingError::UnknownFormat) => {
                        warn!("ignoring unexpected non-text message from signaling server")
                    },
                    Err(err) => break Err(err)
                }

            }

            complete => break Ok(())
        }
    }

    // TODO: should this `events_sender.close_channel();` to communicate disconnects?
}

/// The raw format of data being sent and received.
pub type Packet = Box<[u8]>;

/// Errors that can happen when sending packets
#[derive(Debug, thiserror::Error)]
#[error("The socket was dropped and package could not be sent")]
pub struct PacketSendError {
    #[cfg(not(target_arch = "wasm32"))]
    source: futures_channel::mpsc::SendError,
    #[cfg(target_arch = "wasm32")]
    source: error::JsError,
}

pub(crate) trait PeerDataSender {
    fn send(&mut self, packet: Packet) -> Result<(), PacketSendError>;
}

struct HandshakeResult<M> {
    peer_id: PeerId,
    metadata: M,
}

/// A [RTCDataChannel](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel).
/// A connection to a single peer for a single channel.
///
/// Inbound events are bound via [DataChannelEventReceiver].
pub(crate) trait MatchboxDataChannel: PeerDataSender {
    // See [futures::Sink::poll_flush].
    fn poll_buffer_low(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), PacketSendError>>;

    /// Start the closing process, wake when done.
    /// See [RTCDataChannel.close](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/close)
    /// and [futures::Sink::poll_close].
    fn poll_close(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), PacketSendError>>;
}

/// A platform independent abstraction for the subset of Web Rtc APIs needed by matchbox_socket
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
trait Messenger<Tx: DataChannelEventReceiver> {
    type DataChannel: MatchboxDataChannel;
    type HandshakeMeta: Send;

    async fn offer_handshake(
        signal_peer: SignalPeer,
        mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
        builders: Vec<ChannelBuilder<Tx>>,
        ice_server_config: &RtcIceServerConfig,
        data_channels_ready_fut: Pin<Box<Fuse<impl Future<Output = Result<(), Canceled>>>>>,
    ) -> HandshakeResult<Self::HandshakeMeta>;

    async fn accept_handshake(
        signal_peer: SignalPeer,
        peer_signal_rx: UnboundedReceiver<PeerSignal>,
        builders: Vec<ChannelBuilder<Tx>>,
        ice_server_config: &RtcIceServerConfig,
        data_channels_ready_fut: Pin<Box<Fuse<impl Future<Output = Result<(), Canceled>>>>>,
    ) -> HandshakeResult<Self::HandshakeMeta>;

    async fn peer_loop(peer_uuid: PeerId, handshake_meta: Self::HandshakeMeta) -> PeerId;
}

struct ChannelBuilder<Tx: DataChannelEventReceiver> {
    /// Where to send all incoming events triggered by the channel.
    event_handlers: Tx,
    /// Configuration for the channel to build.
    channel_config: ChannelConfig,
}

/// Receiver for events from a [MatchboxDataChannel].
///
/// Receives events from a [RTCDataChannel](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel).
trait DataChannelEventReceiver: Send + Sync + 'static {
    fn on_open(&mut self);
    fn on_close(&mut self);
    fn on_error(&mut self, message: String);
    fn on_message(&mut self, packet: Packet);
    /// https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/bufferedamountlow_event
    fn on_buffered_amount_low(&mut self);
}

async fn message_loop<M: Messenger<AsyncDataChannelReceiver>>(
    id_tx: futures_channel::oneshot::Sender<PeerId>,
    config: SocketConfig,
    channels: MessageLoopChannels,
) -> Result<(), SignalingError> {
    let MessageLoopChannels {
        requests_sender,
        mut events_receiver,
        mut message_sender,
    } = channels;

    let (messages_from_peers_tx, mut messages_from_peers) =
        futures_channel::mpsc::unbounded::<(PeerId, PeerMessage)>();

    let handshakes = FuturesUnordered::new();
    let mut handshake_signals: HashMap<PeerId, futures_channel::mpsc::UnboundedSender<PeerSignal>> =
        HashMap::new();

    // TODO: might need to send errors when join fails.
    let mut ready_signals: HashMap<PeerId, oneshot::Sender<()>> = HashMap::new();

    let mut id_tx = Option::Some(id_tx);

    let mut timeout = if let Some(interval) = config.keep_alive_interval {
        Either::Left(Delay::new(interval))
    } else {
        Either::Right(std::future::pending())
    }
    .fuse();

    let setup_peer =
        |sender: PeerId,
         handshake_signals: &mut HashMap<PeerId, _>,
         ready_signals: &mut HashMap<PeerId, oneshot::Sender<()>>| {
            let (signal_tx, signal_rx) = futures_channel::mpsc::unbounded();
            let (ready_sender, ready_receiver) = oneshot::channel();
            handshake_signals.insert(sender, signal_tx.clone());
            ready_signals.insert(sender, ready_sender);
            let signal_peer = SignalPeer::new(sender, requests_sender.clone());
            let builders = AsyncDataChannelReceiver::channels_for_peer(
                sender,
                &config.channels,
                messages_from_peers_tx.clnext_messageone(),
            );
            (
                signal_peer,
                signal_rx,
                builders,
                Box::pin(ready_receiver.fuse()),
                signal_tx,
            )
        };

    loop {
        select! {
            _  = &mut timeout => {
                if requests_sender.unbounded_send(PeerRequest::KeepAlive).is_err() {
                    // socket dropped
                    break Ok(());
                }
                if let Some(interval) = config.keep_alive_interval {
                    timeout = Either::Left(Delay::new(interval)).fuse();
                } else {
                    error!("no keep alive timeout, please file a bug");
                }
            }

            message = events_receiver.next().fuse() => {
                if let Some(event) = message {
                    debug!("{event:?}");
                    match event {
                        PeerEvent::IdAssigned(peer_uuid) => {
                            if id_tx.take().expect("already sent peer id").send(peer_uuid.to_owned()).is_err() {
                                // Socket receiver was dropped, exit cleanly.
                                break Ok(());
                            };
                        },
                        PeerEvent::NewPeer(sender) => {
                            let (signal_peer, signal_rx, builders, ready_receiver, _from_peer_tx) = setup_peer(sender, &mut handshake_signals, &mut ready_signals);
                            handshakes.push(M::offer_handshake(signal_peer, signal_rx, builders, &config.ice_server, ready_receiver))
                        },
                        PeerEvent::PeerLeft(peer_uuid) => {
                            // TODO: Better cleanup
                            handshake_signals.remove(&peer_uuid);
                        },
                        PeerEvent::Signal { sender, data } => {
                            let signal_tx = handshake_signals.entry(sender).or_insert_with(|| {
                                let (signal_peer, signal_rx, builders, ready_receiver, from_peer_tx) = setup_peer(sender, &mut handshake_signals, &mut ready_signals);
                                handshakes.push(M::accept_handshake(signal_peer, signal_rx, builders, &config.ice_server, ready_receiver), );
                                from_peer_tx
                            });

                            if signal_tx.unbounded_send(data).is_err() {
                                warn!("ignoring signal from peer {sender} because the handshake has already finished");
                            }
                        },
                    }
                }
            }

            message = messages_from_peers.next().fuse() => {
                match message {
                    Some(m)=>{
                        if let (peer,PeerMessage::Join) = m {
                            ready_signals.remove(&peer).unwrap().send(());
                        }
                        message_sender.send(m);
                    }
                    None => {
                        message_sender.close_channel();
                    }
                }
            }

            complete => break Ok(())
        }
    }
}

pub struct AsyncDataChannelReceiver {
    peer: Arc<UnboundedPeerReceiver>,
}

/// Forwards messages from all channels for a given peer

pub struct UnboundedPeerReceiver {
    remote_id: PeerId,

    connecting_count: AtomicUsize,

    closed: AtomicBool,
}

impl AsyncDataChannelReceiver {
    pub fn channels_for_peer(
        remote_id: PeerId,
        config: &[ChannelConfig],
        messages_from_peers_tx: UnboundedSender<(PeerId, PeerMessage)>,
    ) -> Vec<ChannelBuilder<AsyncDataChannelReceiver>> {
        assert!(config.len() > 0);

        let peer = UnboundedPeerReceiver {
            remote_id,

            connecting_count: config.len().into(),

            closed: false.into(),
        };

        let peer = Arc::new(peer);

        config
            .iter()
            .enumerate()
            .map(|(channel, channel_config)| ChannelBuilder {
                channel_config: channel_config.clone(),

                event_handlers: AsyncDataChannelReceiver {
                    channel: ChannelIndex(channel),

                    peer: peer.clone(),

                    messages_from_peers_tx: messages_from_peers_tx.clone(),
                },
            })
            .collect()
    }

    fn send(&mut self, msg: PeerMessage) {
        self.messages_from_peers_tx
            .unbounded_send((self.peer.remote_id, msg))
            .unwrap();
    }
}

impl DataChannelEventReceiver for AsyncDataChannelReceiver {
    fn on_open(&mut self) {
        // Decrement number of connecting_count
        let old = self
            .peer
            .connecting_count
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);

        if old == 1 {
            // If this is the last channel to connect based on connecting_count, send Join message.
            self.send(PeerMessage::Join);
        }
    }

    fn on_close(&mut self) {
        // Set closed to true

        let old = self
            .peer
            .closed
            .swap(true, std::sync::atomic::Ordering::AcqRel);

        if !old {
            // If closed was false, this is first channel closing for this peer, so send event

            self.send(PeerMessage::Close);
        }
    }

    fn on_error(&mut self, message: String) {
        warn!("data channel error {message}");
        self.send(PeerMessage::Error(self.channel, message))
    }

    fn on_message(&mut self, packet: Packet) {
        self.send(PeerMessage::Packet(self.channel, packet))
    }

    fn on_buffered_amount_low(&mut self) {
        todo!()
    }
}
