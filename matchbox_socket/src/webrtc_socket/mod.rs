pub(crate) mod error;
mod messages;
mod signal_peer;
mod socket;

use self::error::SignalingError;
use crate::{webrtc_socket::signal_peer::SignalPeer, Error};
use async_trait::async_trait;
use cfg_if::cfg_if;
use futures::{future::Either, stream::FuturesUnordered, Future, FutureExt, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::select;
use log::{debug, error, warn};
use matchbox_protocol::PeerId;
use messages::*;
pub(crate) use socket::MessageLoopChannels;
pub use socket::{
    BuildablePlurality, ChannelConfig, ChannelPlurality, MultipleChannels, NoChannels, PeerState,
    RtcIceServerConfig, SingleChannel, WebRtcChannel, WebRtcSocket, WebRtcSocketBuilder,
};
use std::{collections::HashMap, pin::Pin, time::Duration};

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        mod wasm;
        type UseMessenger = wasm::WasmMessenger;
        type UseSignaller = wasm::WasmSignaller;
        /// A future which runs the message loop for the socket and completes
        /// when the socket closes or disconnects
        pub type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>>>>;
    } else {
        mod native;
        type UseMessenger = native::NativeMessenger;
        type UseSignaller = native::NativeSignaller;
        /// A future which runs the message loop for the socket and completes
        /// when the socket closes or disconnects
        pub type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
trait Signaller: Sized {
    async fn new(mut attempts: Option<u16>, room_url: &str) -> Result<Self, SignalingError>;

    async fn send(&mut self, request: String) -> Result<(), SignalingError>;

    async fn next_message(&mut self) -> Result<String, SignalingError>;
}

async fn signaling_loop<S: Signaller>(
    attempts: Option<u16>,
    room_url: String,
    mut requests_receiver: futures_channel::mpsc::UnboundedReceiver<PeerRequest>,
    events_sender: futures_channel::mpsc::UnboundedSender<PeerEvent>,
) -> Result<(), SignalingError> {
    let mut signaller = S::new(attempts, &room_url).await?;

    loop {
        select! {
            request = requests_receiver.next().fuse() => {
                let request = serde_json::to_string(&request).expect("serializing request");
                debug!("-> {request}");
                signaller.send(request).await.map_err(SignalingError::from)?;
            }

            message = signaller.next_message().fuse() => {
                match message {
                    Ok(message) => {
                        debug!("Received {message}");
                        let event: PeerEvent = serde_json::from_str(&message)
                            .unwrap_or_else(|err| panic!("couldn't parse peer event: {err}.\nEvent: {message}"));
                        events_sender.unbounded_send(event).map_err(SignalingError::from)?;
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
}

/// The raw format of data being sent and received.
pub type Packet = Box<[u8]>;

/// Errors that can happen when sending packets
#[derive(Debug, thiserror::Error)]
#[error("The socket was dropped and package could not be sent")]
struct PacketSendError {
    #[cfg(not(target_arch = "wasm32"))]
    source: futures_channel::mpsc::SendError,
    #[cfg(target_arch = "wasm32")]
    source: error::JsError,
}

trait PeerDataSender {
    fn send(&mut self, packet: Packet) -> Result<(), PacketSendError>;
}

struct HandshakeResult<D: PeerDataSender, M> {
    peer_id: PeerId,
    data_channels: Vec<D>,
    metadata: M,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
trait Messenger {
    type DataChannel: PeerDataSender;
    type HandshakeMeta: Send;

    async fn offer_handshake(
        signal_peer: SignalPeer,
        mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        ice_server_config: &RtcIceServerConfig,
        channel_configs: &[ChannelConfig],
    ) -> HandshakeResult<Self::DataChannel, Self::HandshakeMeta>;

    async fn accept_handshake(
        signal_peer: SignalPeer,
        peer_signal_rx: UnboundedReceiver<PeerSignal>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        ice_server_config: &RtcIceServerConfig,
        channel_configs: &[ChannelConfig],
    ) -> HandshakeResult<Self::DataChannel, Self::HandshakeMeta>;

    async fn peer_loop(peer_uuid: PeerId, handshake_meta: Self::HandshakeMeta) -> PeerId;
}

async fn message_loop<M: Messenger>(
    id_tx: futures_channel::oneshot::Sender<PeerId>,
    ice_server_config: &RtcIceServerConfig,
    channel_configs: &[ChannelConfig],
    channels: MessageLoopChannels,
    keep_alive_interval: Option<Duration>,
) -> Result<(), SignalingError> {
    let MessageLoopChannels {
        requests_sender,
        mut events_receiver,
        mut peer_messages_out_rx,
        messages_from_peers_tx,
        peer_state_tx,
    } = channels;

    let mut handshakes = FuturesUnordered::new();
    let mut peer_loops = FuturesUnordered::new();
    let mut handshake_signals = HashMap::new();
    let mut data_channels = HashMap::new();
    let mut id_tx = Option::Some(id_tx);

    let mut timeout = if let Some(interval) = keep_alive_interval {
        Either::Left(Delay::new(interval))
    } else {
        Either::Right(std::future::pending())
    }
    .fuse();

    loop {
        let mut next_peer_messages_out = peer_messages_out_rx
            .iter_mut()
            .enumerate()
            .map(|(channel, rx)| async move { (channel, rx.next().await) })
            .collect::<FuturesUnordered<_>>();

        let mut next_peer_message_out = next_peer_messages_out.next().fuse();

        select! {
            _  = &mut timeout => {
                if requests_sender.unbounded_send(PeerRequest::KeepAlive).is_err() {
                    // socket dropped
                    break Ok(());
                }
                if let Some(interval) = keep_alive_interval {
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
                        PeerEvent::NewPeer(peer_uuid) => {
                            let (signal_tx, signal_rx) = futures_channel::mpsc::unbounded();
                            handshake_signals.insert(peer_uuid, signal_tx);
                            let signal_peer = SignalPeer::new(peer_uuid, requests_sender.clone());
                            handshakes.push(M::offer_handshake(signal_peer, signal_rx, messages_from_peers_tx.clone(), ice_server_config, channel_configs))
                        },
                        PeerEvent::PeerLeft(peer_uuid) => {
                            if peer_state_tx.unbounded_send((peer_uuid, PeerState::Disconnected)).is_err() {
                                // socket dropped, exit cleanly
                                break Ok(());
                            }
                        },
                        PeerEvent::Signal { sender, data } => {
                            let signal_tx = handshake_signals.entry(sender).or_insert_with(|| {
                                let (from_peer_tx, peer_signal_rx) = futures_channel::mpsc::unbounded();
                                let signal_peer = SignalPeer::new(sender, requests_sender.clone());
                                handshakes.push(M::accept_handshake(signal_peer, peer_signal_rx, messages_from_peers_tx.clone(), ice_server_config, channel_configs));
                                from_peer_tx
                            });

                            if signal_tx.unbounded_send(data).is_err() {
                                warn!("ignoring signal from peer {sender} because the handshake has already finished");
                            }
                        },
                    }
                }
            }

            handshake_result = handshakes.select_next_some() => {
                data_channels.insert(handshake_result.peer_id, handshake_result.data_channels);
                if peer_state_tx.unbounded_send((handshake_result.peer_id, PeerState::Connected)).is_err() {
                    // sending can only fail on socket drop, in which case connected_peers is unavailable, ignore
                    break Ok(());
                }
                peer_loops.push(M::peer_loop(handshake_result.peer_id, handshake_result.metadata));
            }

            peer_uuid = peer_loops.select_next_some() => {
                debug!("peer {peer_uuid} finished");
                if peer_state_tx.unbounded_send((peer_uuid, PeerState::Disconnected)).is_err() {
                    // sending can only fail on socket drop, in which case connected_peers is unavailable, ignore
                    break Ok(());
                }
            }

            message = next_peer_message_out => {
                match message {
                    Some((channel_index, Some((peer, packet)))) => {
                        let data_channel = data_channels
                            .get_mut(&peer)
                            .expect("couldn't find data channel for peer")
                            .get_mut(channel_index).unwrap_or_else(|| panic!("couldn't find data channel with index {channel_index}"));
                        if let Err(e) = data_channel.send(packet) {
                            // Peer we're sending to closed their end of the connection.
                            // We anticipate the PeerLeft event soon, but we sent a message before it came.
                            // Do nothing. Only log it.
                            warn!("failed to send to peer {peer} (socket closed): {e:?}");
                        };
                    }
                    Some((_, None)) | None => {
                        // Receiver end of outgoing message channel closed,
                        // which most likely means the socket was dropped.
                        // There could probably be cleaner ways to handle this,
                        // but for now, just exit cleanly.
                        warn!("Outgoing message queue closed, message not sent");
                        break Ok(());
                    }
                }
            }

            complete => break Ok(())
        }
    }
}
