use super::{
    HandshakeResult, PacketSendError, PeerDataSender, SignallerBuilder,
    messages::{PeerEvent, PeerRequest},
};
use crate::{
    RtcIceServerConfig,
    webrtc_socket::{
        ChannelConfig, Messenger, Packet, Signaller, error::SignalingError, messages::PeerSignal,
        signal_peer::SignalPeer, socket::create_data_channels_ready_fut,
    },
};
use async_trait::async_trait;
use async_tungstenite::{
    WebSocketStream,
    smol::{ConnectStream, connect_async},
    tungstenite::Message,
};
use bytes::BytesMut;
use futures::{
    Future, FutureExt, StreamExt,
    future::{Fuse, FusedFuture, join_all},
    stream::FuturesUnordered,
};
use futures_channel::mpsc::{Receiver, Sender, TrySendError, UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::select;
use log::{debug, error, info, trace, warn};
use matchbox_protocol::PeerId;
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceCandidateInit, RTCIceServer, RTCPeerConnectionIceEvent, RTCPeerConnectionState,
        RTCSessionDescription,
    },
    runtime::{Runtime, SmolRuntime},
};

pub(crate) struct NativeSignaller {
    websocket_stream: WebSocketStream<ConnectStream>,
}

#[derive(Debug, Default)]
pub(crate) struct NativeSignallerBuilder;

#[async_trait]
impl SignallerBuilder for NativeSignallerBuilder {
    async fn new_signaller(
        &self,
        mut attempts: Option<u16>,
        room_url: String,
    ) -> Result<Box<dyn Signaller>, SignalingError> {
        let websocket_stream = 'signaling: loop {
            match connect_async(&room_url).await.map_err(SignalingError::from) {
                Ok((wss, _)) => break wss,
                Err(e) => {
                    if let Some(attempts) = attempts.as_mut() {
                        if *attempts <= 1 {
                            return Err(SignalingError::NegotiationFailed(Box::new(e)));
                        } else {
                            *attempts -= 1;
                            warn!(
                                "connection to signaling server failed, {attempts} attempt(s) remain"
                            );
                            warn!("waiting 3 seconds to re-try connection...");
                            Delay::new(Duration::from_secs(3)).await;
                            info!("retrying connection...");
                            continue 'signaling;
                        }
                    } else {
                        continue 'signaling;
                    }
                }
            };
        };
        Ok(Box::new(NativeSignaller { websocket_stream }))
    }
}

#[async_trait]
impl Signaller for NativeSignaller {
    async fn send(&mut self, request: PeerRequest) -> Result<(), SignalingError> {
        let request = serde_json::to_string(&request).expect("serializing request");
        self.websocket_stream
            .send(Message::Text(request.into()))
            .await
            .map_err(SignalingError::from)
    }

    async fn next_message(&mut self) -> Result<PeerEvent, SignalingError> {
        let message = match self.websocket_stream.next().await {
            Some(Ok(Message::Text(message))) => Ok(message),
            Some(Ok(_)) => Err(SignalingError::UnknownFormat),
            Some(Err(err)) => Err(SignalingError::from(err)),
            None => Err(SignalingError::StreamExhausted),
        }?;
        let message = serde_json::from_str(&message).map_err(|e| {
            error!("failed to deserialize message: {e:?}");
            SignalingError::UnknownFormat
        })?;
        Ok(message)
    }
}

pub(crate) struct NativeMessenger;

impl PeerDataSender for UnboundedSender<Packet> {
    fn send(&mut self, packet: Packet) -> Result<(), PacketSendError> {
        self.unbounded_send(packet)
            .map_err(|source| PacketSendError {
                source: TrySendError::into_send_error(source),
            })
    }
}

/// Forwards the events of a peer's data channels, completing once all of them have closed.
type EventForwarding = Pin<Box<dyn FusedFuture<Output = ()> + Send>>;

/// Adds the remote peer's ICE candidates to the connection for as long as signaling is up.
type CandidateListener =
    Pin<Box<dyn FusedFuture<Output = Result<(), webrtc::error::Error>> + Send>>;

pub(crate) struct NativeHandshakeMeta {
    to_peer_message_rx: Vec<UnboundedReceiver<Packet>>,
    data_channels: Vec<Arc<dyn DataChannel>>,
    event_forwarding: EventForwarding,
    trickle_fut: CandidateListener,
    peer_disconnected_rx: Receiver<()>,
    _connection: ConnectionGuard,
}

#[async_trait]
impl Messenger for NativeMessenger {
    type DataChannel = UnboundedSender<Packet>;
    type HandshakeMeta = NativeHandshakeMeta;

    async fn offer_handshake(
        signal_peer: SignalPeer,
        mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        ice_server_config: &RtcIceServerConfig,
        channel_configs: &[ChannelConfig],
    ) -> HandshakeResult<Self::DataChannel, Self::HandshakeMeta> {
        let (to_peer_message_tx, to_peer_message_rx) = new_senders_and_receivers(channel_configs);
        let (peer_disconnected_tx, peer_disconnected_rx) = futures_channel::mpsc::channel(1);

        debug!("making offer");
        let (connection, trickle) = create_rtc_peer_connection(
            signal_peer.clone(),
            ice_server_config,
            peer_disconnected_tx.clone(),
        )
        .await
        .unwrap();

        let (data_channel_ready_txs, data_channels_ready_fut) =
            create_data_channels_ready_fut(channel_configs);

        let (data_channels, mut event_forwarding) = create_data_channels(
            &**connection,
            data_channel_ready_txs,
            signal_peer.id,
            peer_disconnected_tx,
            messages_from_peers_tx,
            channel_configs,
        )
        .await;

        // TODO: maybe pass in options? ice restart etc.?
        let offer = connection.create_offer(None).await.unwrap();
        let sdp = offer.sdp.clone();
        connection.set_local_description(offer).await.unwrap();
        signal_peer.send(PeerSignal::Offer(sdp));

        let answer = loop {
            let signal = peer_signal_rx
                .next()
                .await
                .expect("Signal server connection lost in the middle of a handshake");

            match signal {
                PeerSignal::Answer(answer) => {
                    break answer;
                }
                PeerSignal::Offer(_) => {
                    warn!("Got an unexpected Offer, while waiting for Answer. Ignoring.")
                }
                PeerSignal::IceCandidate(_) => {
                    warn!("Got an unexpected IceCandidate, while waiting for Answer. Ignoring.")
                }
            };
        };

        let remote_description = RTCSessionDescription::answer(answer).unwrap();
        connection
            .set_remote_description(remote_description)
            .await
            .unwrap();

        let trickle_fut = complete_handshake(
            &trickle,
            &connection,
            peer_signal_rx,
            data_channels_ready_fut,
            &mut event_forwarding,
        )
        .await;

        HandshakeResult::<Self::DataChannel, Self::HandshakeMeta> {
            peer_id: signal_peer.id,
            data_channels: to_peer_message_tx,
            metadata: NativeHandshakeMeta {
                to_peer_message_rx,
                data_channels,
                event_forwarding,
                trickle_fut,
                peer_disconnected_rx,
                _connection: connection,
            },
        }
    }

    async fn accept_handshake(
        signal_peer: SignalPeer,
        mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        ice_server_config: &RtcIceServerConfig,
        channel_configs: &[ChannelConfig],
    ) -> HandshakeResult<Self::DataChannel, Self::HandshakeMeta> {
        let (to_peer_message_tx, to_peer_message_rx) = new_senders_and_receivers(channel_configs);
        let (peer_disconnected_tx, peer_disconnected_rx) = futures_channel::mpsc::channel(1);

        debug!("handshake_accept");
        let (connection, trickle) = create_rtc_peer_connection(
            signal_peer.clone(),
            ice_server_config,
            peer_disconnected_tx.clone(),
        )
        .await
        .unwrap();

        let (data_channel_ready_txs, data_channels_ready_fut) =
            create_data_channels_ready_fut(channel_configs);

        let (data_channels, mut event_forwarding) = create_data_channels(
            &**connection,
            data_channel_ready_txs,
            signal_peer.id,
            peer_disconnected_tx,
            messages_from_peers_tx,
            channel_configs,
        )
        .await;

        let offer = loop {
            match peer_signal_rx.next().await.expect("error") {
                PeerSignal::Offer(offer) => {
                    break offer;
                }
                _ => {
                    warn!("ignoring other signal!!!");
                }
            }
        };
        debug!("received offer");
        let remote_description = RTCSessionDescription::offer(offer).unwrap();
        connection
            .set_remote_description(remote_description)
            .await
            .unwrap();

        let answer = connection.create_answer(None).await.unwrap();
        signal_peer.send(PeerSignal::Answer(answer.sdp.clone()));
        connection.set_local_description(answer).await.unwrap();

        let trickle_fut = complete_handshake(
            &trickle,
            &connection,
            peer_signal_rx,
            data_channels_ready_fut,
            &mut event_forwarding,
        )
        .await;

        HandshakeResult::<Self::DataChannel, Self::HandshakeMeta> {
            peer_id: signal_peer.id,
            data_channels: to_peer_message_tx,
            metadata: NativeHandshakeMeta {
                to_peer_message_rx,
                data_channels,
                event_forwarding,
                trickle_fut,
                peer_disconnected_rx,
                _connection: connection,
            },
        }
    }

    async fn peer_loop(peer_uuid: PeerId, handshake_meta: Self::HandshakeMeta) -> PeerId {
        let NativeHandshakeMeta {
            mut to_peer_message_rx,
            data_channels,
            mut event_forwarding,
            mut trickle_fut,
            mut peer_disconnected_rx,
            _connection,
        } = handshake_meta;

        assert_eq!(
            data_channels.len(),
            to_peer_message_rx.len(),
            "amount of data channels and receivers differ"
        );

        let mut message_loop_futs: FuturesUnordered<_> = data_channels
            .iter()
            .zip(to_peer_message_rx.iter_mut())
            .map(|(data_channel, rx)| async move {
                while let Some(message) = rx.next().await {
                    trace!("sending packet {message:?}");
                    if let Err(e) = data_channel.send(BytesMut::from(&message[..])).await {
                        error!("error sending to data channel: {e:?}")
                    }
                }
            })
            .collect();

        loop {
            select! {
                _ = peer_disconnected_rx.next() => break,

                _ = message_loop_futs.next() => break,
                // Every data channel closed, which is reported on `peer_disconnected_rx`.
                _ = event_forwarding => continue,
                // TODO: this means that the signaling is down, should return an
                // error
                _ = trickle_fut => continue,
            }
        }

        peer_uuid
    }
}

/// The runtime driving the peer connections.
///
/// smol's executor runs on threads of its own, so the connections don't depend on the executor
/// the socket is polled by, and no tokio context is needed.
fn runtime() -> Arc<dyn Runtime> {
    Arc::new(SmolRuntime)
}

/// Closes the peer connection when dropped.
///
/// A webrtc-rs peer connection that is dropped without being closed keeps its driver task, and
/// with it its sockets, running. The remote peer would then see it as connected until ICE times
/// out, even though no data channel is served anymore.
struct ConnectionGuard(Arc<dyn PeerConnection>);

impl std::ops::Deref for ConnectionGuard {
    type Target = Arc<dyn PeerConnection>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let connection = Arc::clone(&self.0);
        // Dropping the returned handle detaches the task.
        runtime().spawn(Box::pin(async move {
            if let Err(e) = connection.close().await {
                debug!("failed to close peer connection: {e:?}");
            }
        }));
    }
}

/// Handles the events of a peer connection.
struct ConnectionEventHandler {
    trickle: Arc<CandidateTrickle>,
    peer_disconnected_tx: Sender<()>,
}

#[async_trait]
impl PeerConnectionEventHandler for ConnectionEventHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        self.trickle.on_local_candidate(event);
    }

    /// Treats the peer connection failing like a data channel closing.
    ///
    /// A peer that vanishes without closing its data channels (a crash, lost network, a socket
    /// dropped without closing the connection) never closes them on our side, so it would stay
    /// connected forever whenever the signaling server cannot report it gone (e.g. after the
    /// signaling connection is lost). ICE declares the connection failed about 30 seconds after
    /// the peer stops answering.
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if matches!(
            state,
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
        ) {
            debug!("peer connection {state}");
            // Full only when a disconnect is already pending, which is just as good.
            let _ = self.peer_disconnected_tx.clone().try_send(());
        }
    }
}

fn new_senders_and_receivers<T>(
    channel_configs: &[ChannelConfig],
) -> (Vec<UnboundedSender<T>>, Vec<UnboundedReceiver<T>>) {
    (0..channel_configs.len())
        .map(|_| futures_channel::mpsc::unbounded())
        .unzip()
}

async fn complete_handshake<T: Future<Output = ()>>(
    trickle: &CandidateTrickle,
    connection: &Arc<dyn PeerConnection>,
    peer_signal_rx: UnboundedReceiver<PeerSignal>,
    mut wait_for_channels: Pin<Box<Fuse<T>>>,
    event_forwarding: &mut EventForwarding,
) -> CandidateListener {
    trickle.send_pending_candidates();
    let mut trickle_fut = Box::pin(
        CandidateTrickle::listen_for_remote_candidates(Arc::clone(connection), peer_signal_rx)
            .fuse(),
    );

    loop {
        select! {
            _ = wait_for_channels => {
                break;
            },
            // The channels open through their events.
            _ = event_forwarding.as_mut() => continue,
            // TODO: this means that the signaling is down, should return an
            // error
            _ = trickle_fut => continue,
        };
    }

    trickle_fut
}

struct CandidateTrickle {
    signal_peer: SignalPeer,
    /// Local candidates gathered before the remote description was set, `None` once they have
    /// been sent.
    pending: Mutex<Option<Vec<String>>>,
}

impl CandidateTrickle {
    fn new(signal_peer: SignalPeer) -> Self {
        Self {
            signal_peer,
            pending: Mutex::new(Some(Vec::new())),
        }
    }

    fn on_local_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let candidate_init = match event.candidate.to_json() {
            Ok(candidate_init) => candidate_init,
            Err(err) => {
                error!("failed to convert ice candidate to candidate init, ignoring: {err}");
                return;
            }
        };

        let candidate_json =
            serde_json::to_string(&candidate_init).expect("failed to serialize candidate to json");

        // Local candidates can only be sent after the remote description
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        match pending.as_mut() {
            Some(pending) => {
                debug!("storing pending IceCandidate signal: {candidate_json:?}");
                pending.push(candidate_json);
            }
            None => {
                debug!("sending IceCandidate signal: {candidate_json:?}");
                self.signal_peer
                    .send(PeerSignal::IceCandidate(candidate_json));
            }
        }
    }

    fn send_pending_candidates(&self) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for candidate in pending.take().into_iter().flatten() {
            self.signal_peer.send(PeerSignal::IceCandidate(candidate));
        }
    }

    async fn listen_for_remote_candidates(
        peer_connection: Arc<dyn PeerConnection>,
        mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
    ) -> Result<(), webrtc::error::Error> {
        while let Some(signal) = peer_signal_rx.next().await {
            match signal {
                PeerSignal::IceCandidate(candidate_json) => {
                    debug!("received ice candidate: {candidate_json:?}");
                    match serde_json::from_str::<RTCIceCandidateInit>(&candidate_json) {
                        Ok(candidate_init) => {
                            debug!("ice candidate received: {}", candidate_init.candidate);
                            peer_connection.add_ice_candidate(candidate_init).await?;
                        }
                        Err(err) => {
                            if *candidate_json == *"null" {
                                debug!(
                                    "Received null ice candidate, this means there are no further ice candidates"
                                );
                            } else {
                                warn!("failed to parse ice candidate json, ignoring: {err:?}");
                            }
                        }
                    }
                }
                PeerSignal::Offer(_) => {
                    warn!("Got an unexpected Offer, while waiting for IceCandidate. Ignoring.")
                }
                PeerSignal::Answer(_) => {
                    warn!("Got an unexpected Answer, while waiting for IceCandidate. Ignoring.")
                }
            }
        }

        Ok(())
    }
}

async fn create_rtc_peer_connection(
    signal_peer: SignalPeer,
    ice_server_config: &RtcIceServerConfig,
    peer_disconnected_tx: Sender<()>,
) -> Result<(ConnectionGuard, Arc<CandidateTrickle>), webrtc::error::Error> {
    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer {
            urls: ice_server_config.urls.clone(),
            username: ice_server_config.username.clone().unwrap_or_default(),
            credential: ice_server_config.credential.clone().unwrap_or_default(),
        }])
        .build();

    let trickle = Arc::new(CandidateTrickle::new(signal_peer));
    let handler = ConnectionEventHandler {
        trickle: Arc::clone(&trickle),
        peer_disconnected_tx,
    };

    let connection = PeerConnectionBuilder::new()
        .with_configuration(config)
        .with_runtime(runtime())
        .with_handler(Arc::new(handler))
        // One socket per local interface. A family the host doesn't have is skipped.
        .with_udp_addrs(vec!["0.0.0.0:0", "[::]:0"])
        .build()
        .await?;

    Ok((ConnectionGuard(Arc::new(connection)), trickle))
}

async fn create_data_channels(
    connection: &dyn PeerConnection,
    mut data_channel_ready_txs: Vec<futures_channel::mpsc::Sender<()>>,
    peer_id: PeerId,
    peer_disconnected_tx: Sender<()>,
    from_peer_message_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
    channel_configs: &[ChannelConfig],
) -> (Vec<Arc<dyn DataChannel>>, EventForwarding) {
    let mut channels = vec![];
    let mut event_forwarding = vec![];
    for (i, channel_config) in channel_configs.iter().enumerate() {
        let channel = create_data_channel(connection, channel_config, i).await;

        event_forwarding.push(forward_data_channel_events(
            Arc::clone(&channel),
            data_channel_ready_txs.pop().unwrap(),
            peer_id,
            peer_disconnected_tx.clone(),
            from_peer_message_tx.get(i).unwrap().clone(),
        ));
        channels.push(channel);
    }

    (
        channels,
        Box::pin(join_all(event_forwarding).map(|_| ()).fuse()),
    )
}

async fn create_data_channel(
    connection: &dyn PeerConnection,
    channel_config: &ChannelConfig,
    channel_index: usize,
) -> Arc<dyn DataChannel> {
    let config = RTCDataChannelInit {
        ordered: channel_config.ordered,
        negotiated: Some(channel_index as u16),
        max_retransmits: channel_config.max_retransmits,
        ..Default::default()
    };

    connection
        .create_data_channel(&format!("matchbox_socket_{channel_index}"), Some(config))
        .await
        .unwrap()
}

/// Forwards a data channel's events until it closes, then reports the peer disconnected.
async fn forward_data_channel_events(
    channel: Arc<dyn DataChannel>,
    mut channel_ready: futures_channel::mpsc::Sender<()>,
    peer_id: PeerId,
    mut peer_disconnected_tx: Sender<()>,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
) {
    while let Some(event) = channel.poll().await {
        match event {
            DataChannelEvent::OnOpen => {
                debug!("Data channel ready");
                // Fails if the socket was dropped mid-handshake
                if let Err(e) = channel_ready.try_send(()) {
                    debug!("data channel opened after handshake teardown: {e:?}");
                }
            }
            DataChannelEvent::OnMessage(message) => {
                let packet = message.data.freeze();
                trace!("data channel message received: {packet:?}");
                if let Err(e) = from_peer_message_tx.unbounded_send((peer_id, packet)) {
                    // should only happen if the socket is dropped, or we are out of memory
                    warn!("failed to notify about data channel message: {e:?}");
                }
            }
            DataChannelEvent::OnError => {
                // TODO: handle this somehow
                warn!("data channel error");
            }
            DataChannelEvent::OnClose => break,
            _ => {}
        }
    }

    debug!("Data channel closed");
    if let Err(err) = peer_disconnected_tx.try_send(()) {
        // should only happen if the socket is dropped, or we are out of memory
        warn!("failed to notify about data channel closing: {err:?}");
    }
}
