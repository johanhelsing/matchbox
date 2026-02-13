//! A simple async API for communicating with match_box peers.
//!
//! This API is designed to be as simple as possible while exposing nearly all the possibly functionality.
//! Once thing it chooses to not expose is access to a Peer before its channel connections are established.
//!
//! More opinionated APIs could be built on this which to provide a more restricted but simpler user experience.
//!
//! For example, an API wanting to simplify connection state could choose to close all channels if disconnected from the signaling server,
//! and/or consider a peer disconnected if any of its channels are closed.
//!
//! Another simplified API could expose all incoming data over a single channel of `(PeerId, Connected | Disconnected | (ChannelIndex, Packet | BufferEmpty)))`
//! And send via something like `send(PeerId, ChannelIndex, Packet) -> Result`
//!
//! TODO: provide an implementation of such an API on-top of this one.
//! TODO: reimplement existing match_box socket synchronous API on-top of this, but with options to "take"/expose these underlying objects.

use crate::webrtc_socket::messages::PeerEvent;
use crate::webrtc_socket::{
    DataChannelEventReceiver, MatchboxDataChannel, Messenger, PacketSendError, Signaller,
    UseSignaller,
};
use crate::Packet;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, Sink, Stream, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use log::{debug, warn};
use matchbox_protocol::PeerId;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::{pin::Pin, task::Poll};

use super::SocketConfig;

/// WebRTC connection
///
/// `drop` to stop accepting new peers (and thus disconnect from the Signaller).
pub struct Connection {
    id: PeerId,

    pending_peers: FuturesUnordered<BoxFuture<'static, Result<Peer, PendingPeerError>>>,

    config: SocketConfig,
    signaller: UseSignaller, // TODO; Box<dyn Signaller>
}

enum PendingPeerError {
    FailedPeerConnection(PeerId, String),
    /// Means no more peers after any existing pending ones are finished.
    DisconnectedFromSignaler(String),
    // /// DisconnectedFromSignaler, and non pending.
    // NoMore,
}

/// Info the [Connection] holds onto about a peer.
struct PeerInfo {
    left_signaling_server: futures_channel::oneshot::Sender<()>,
}

impl Connection {
    /// The unique id for this (local) Peer, as assigned by the Signaller.
    /// Remote Peers receive a [Peer] whose [Peer::id] matches this to communicate with this one.
    pub fn id(&self) -> PeerId {
        self.id
    }

    pub(crate) fn new(config: SocketConfig) -> impl Future<Output = Result<Self, String>> {
        async move {
            let mut signaller = UseSignaller::new(config.attempts, &config.room_url)
                .await
                .map_err(|e| "signal error")?;

            // Grab id from first message from Signaller
            let id = {
                let message = signaller.next_message().await.map_err(|e| "signal error")?;
                debug!("Received {message}");
                let first_event: PeerEvent = serde_json::from_str(&message).map_err(|err| {
                    format!("couldn't parse peer event: {err}.\nEvent: {message}")
                })?;

                match first_event {
                    PeerEvent::IdAssigned(id) => id,
                    _ => return Result::Err("Signaller sent event before Id".to_owned()),
                }
            };

            let pending_peers = FuturesUnordered::new();
            pending_peers.push(future);

            Result::Ok(Self {
                id,
                signaller,
                config,
                peer_info: HashMap::default(),
                pending_peers: FuturesUnordered::new(),
            })
        }
    }

    async fn next<M, Tx: DataChannelEventReceiver>(mut self: Pin<&mut Self>) -> Result<Peer, String>
    where
        M: Messenger<Tx>,
    {
        // TODO: loop over select between pending_peers.
        loop {
            match self.signaller.next_message().await {
                Ok(message) => {
                    debug!("Received {message}");
                    let event: PeerEvent = serde_json::from_str(&message).map_err(|err| {
                        format!("couldn't parse peer event: {err}.\nEvent: {message}")
                    })?;

                    match event {
                        PeerEvent::IdAssigned(_) => Result::Err("IdAssigned again".to_owned()),
                        PeerEvent::NewPeer(sender) => {
                            let (leave_tx, leave_rx) = futures_channel::oneshot::channel();
                            let replaced = self.peer_info.insert(
                                sender,
                                PeerInfo {
                                    left_signaling_server: leave_tx,
                                },
                            );
                            if matches!(replaced, Some(_)) {
                                return Result::Err("Added peer that already exists".to_owned());
                            }
                            let peer = M::offer_handshake(
                                signal_peer,
                                signal_rx,
                                builders,
                                &self.config.ice_server,
                                ready_receiver,
                            );

                            self.pending_peers.push(future);

                            return Ok(Peer {
                                id: sender,
                                channels,
                                left_signaling_server: leave_rx,
                            });
                        }
                        PeerEvent::PeerLeft(peer_uuid) => {
                            // TODO: Better cleanup
                            handshake_signals.remove(&peer_uuid);
                            todo!()
                        }
                        PeerEvent::Signal { sender, data } => {
                            let signal_tx = handshake_signals.entry(sender).or_insert_with(|| {
                                let (
                                    signal_peer,
                                    signal_rx,
                                    builders,
                                    ready_receiver,
                                    from_peer_tx,
                                ) = setup_peer(sender, &mut handshake_signals, &mut ready_signals);
                                handshakes.push(M::accept_handshake(
                                    signal_peer,
                                    signal_rx,
                                    builders,
                                    &config.ice_server,
                                    ready_receiver,
                                ));
                                from_peer_tx
                            });

                            if signal_tx.unbounded_send(data).is_err() {
                                warn!("ignoring signal from peer {sender} because the handshake has already finished");
                            }
                            todo!()
                        }
                    }

                    todo!()
                }
                Err(error) => return Result::Err("signaling error".to_owned()),
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // This explicit drop implementation is likely unnecessary, but it makes the intended semantics more clear.
        // When the Signaller sees `events` has been closed (which drop would do by default anyway), it can shutdown.
        // self.events.close();
        todo!()
    }
}

impl Stream for Connection {
    type Item = Peer;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match self.next().poll_unpin(cx) {
            Poll::Ready(Result::Ok(peer)) => Poll::Ready(Some(peer)),
            Poll::Ready(Result::Err(error)) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct Peer {
    pub id: PeerId,
    pub channels: Box<[PeerDataChannel]>,

    /// This (remote) peer has left the signaling server.
    pub left_signaling_server: futures_channel::oneshot::Receiver<()>,
}

/// A [RTCDataChannel](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel).
pub struct PeerDataChannel {
    pub sink: DataChannelSink,
    pub stream: DataChannelStream,
}

/// [Sink] which sends packets to a specific [Peer] over a specific channel.
///
/// Sink is always ready for more data (buffer is unbounded).
///
/// Flush waits for the buffer to reach the [bufferedAmountLowThreshold](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/bufferedAmountLowThreshold).
///
/// The sink is closed when the [Peer] disconnects.
///
/// TODO: consider implementing ready threshold.
pub struct DataChannelSink {
    channel: Box<dyn MatchboxDataChannel>,
}

impl Sink<Packet> for DataChannelSink {
    // TODO: consider translating this error type to something platform independent.
    type Error = PacketSendError;

    fn poll_ready(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(mut self: Pin<&mut Self>, item: Packet) -> Result<(), Self::Error> {
        self.channel.send(item)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.channel.poll_buffer_low(cx)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.channel.poll_close(cx)
    }
}

/// A [Stream] of [Packet]s from a [PeerDataChannel].
///
/// Internally contains an unbounded buffer with explicit support for back-pressure over the network.
///
/// The stream is closed when the [Peer] disconnects.
///
/// TODO: The browser APIs for WebRTC seem to provide no access to flow control from the receive side, but the underlying [SCTP](https://en.wikipedia.org/wiki/Stream_Control_Transmission_Protocol) has some.
/// It is possible the native implementation could provide some explicit flow control,
/// and is also possible that that blocking the thread during the [message event](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/message_event) provides some back pressure.
/// Further experimentation and/or research is needed in this area.
pub struct DataChannelStream {
    rx: UnboundedReceiver<Packet>,
}

impl Stream for DataChannelStream {
    type Item = Packet;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.rx.poll_next_unpin(cx)
    }
}

fn signaller_loop(
    config: SocketConfig,
    signaller: UseSignaller,
    pending_peers: UnboundedSender<BoxFuture<'static, Result<Peer, PendingPeerError>>>,
) -> impl Future<Output = String> {
    async {
        let mut peer_info: HashMap<PeerId, PeerInfo> = HashMap::default();

        loop {
            match signaller.next_message().await {
                Ok(message) => {
                    debug!("Received {message}");
                    let event: PeerEvent = serde_json::from_str(&message).map_err(|err| {
                        format!("couldn't parse peer event: {err}.\nEvent: {message}")
                    })?;

                    match event {
                        PeerEvent::IdAssigned(_) => Result::Err("IdAssigned again".to_owned()),
                        PeerEvent::NewPeer(sender) => {
                            let (leave_tx, leave_rx) = futures_channel::oneshot::channel();
                            let replaced = peer_info.insert(
                                sender,
                                PeerInfo {
                                    left_signaling_server: leave_tx,
                                },
                            );
                            if matches!(replaced, Some(_)) {
                                return "Added peer that already exists".to_owned();
                            }

                            // TODO: make EventCollectors for each channel.
                            // await all futures for them to open -> put this future into pending_peers

                            let peer = M::offer_handshake(
                                signal_peer,
                                signal_rx,
                                builders,
                                &self.config.ice_server,
                                ready_receiver,
                            );

                            pending_peers.push(future);
                        }
                        PeerEvent::PeerLeft(peer_uuid) => match peer_info.get(&peer_uuid) {
                            Some(info) => {
                                info.left_signaling_server.send(());
                            }
                            None => return "Invalid peer left".to_owned(),
                        },
                        PeerEvent::Signal { sender, data } => {
                            let signal_tx = handshake_signals.entry(sender).or_insert_with(|| {
                                let (
                                    signal_peer,
                                    signal_rx,
                                    builders,
                                    ready_receiver,
                                    from_peer_tx,
                                ) = setup_peer(sender, &mut handshake_signals, &mut ready_signals);
                                handshakes.push(M::accept_handshake(
                                    signal_peer,
                                    signal_rx,
                                    builders,
                                    &config.ice_server,
                                    ready_receiver,
                                ));
                                from_peer_tx
                            });

                            if signal_tx.unbounded_send(data).is_err() {
                                warn!("ignoring signal from peer {sender} because the handshake has already finished");
                            }
                            todo!()
                        }
                    }

                    todo!()
                }
                Err(error) => return Result::Err("signaling error".to_owned()),
            }
        }
    }
}

struct EventCollector {
    peer: Arc<PeerInfo>,

    opened: Option<futures_channel::oneshot::Sender<Result<(), String>>>,

    packets: futures_channel::mpsc::UnboundedSender<Result<Packet, String>>,
}

impl DataChannelEventReceiver for EventCollector {
    fn on_open(&mut self) {
        self.opened.take().unwrap().send(Result::Ok(()));
    }

    fn on_close(&mut self) {
        self.fail("closed".to_string());
    }

    fn on_error(&mut self, message: String) {
        warn!("data channel error {message}");
        self.fail(message);
    }

    fn on_message(&mut self, packet: Packet) {
        self.packets.unbounded_send(Ok(packet));
    }

    fn on_buffered_amount_low(&mut self) {
        todo!()
    }
}

impl EventCollector {
    /// Send error to opened channel if not open yet.
    /// Prevents hang waiting for open after error.
    /// Closes packets channel.
    fn fail(&mut self, message: String) {
        self.opened.take().map(|x| x.send(Result::Err(message)));
        self.packets.close_channel();
    }
}
