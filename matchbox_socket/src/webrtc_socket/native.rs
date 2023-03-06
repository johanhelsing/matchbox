use super::{error::MessagingError, HandshakeResult, PeerDataSender};
use crate::webrtc_socket::{
    error::SignallingError,
    messages::PeerSignal,
    signal_peer::SignalPeer,
    socket::{create_data_channels_ready_fut, new_senders_and_receivers, PeerState},
    ChannelConfig, Messenger, Packet, Signaller, WebRtcSocketConfig,
};
use async_compat::{Compat, CompatExt};
use async_trait::async_trait;
use async_tungstenite::{
    async_std::{connect_async, ConnectStream},
    tungstenite::Message,
    WebSocketStream,
};
use bytes::Bytes;
use futures::{
    future::{Fuse, FusedFuture},
    stream::FuturesUnordered,
    Future, FutureExt, SinkExt, StreamExt,
};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_util::{lock::Mutex, select};
use log::{debug, error, trace, warn};
use matchbox_protocol::PeerId;
use std::{error::Error, pin::Pin, sync::Arc};
use webrtc::{
    api::APIBuilder,
    data_channel::{data_channel_init::RTCDataChannelInit, RTCDataChannel},
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_server::RTCIceServer,
    },
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, RTCPeerConnection,
    },
};

pub(crate) struct NativeSignaller {
    websocket_stream: WebSocketStream<ConnectStream>,
}

#[async_trait]
impl Signaller for NativeSignaller {
    async fn new(mut attempts: Option<u16>, room_url: &str) -> Result<Self, SignallingError> {
        let websocket_stream = 'signalling: loop {
            match connect_async(room_url).await.map_err(SignallingError::from) {
                Ok((wss, _)) => break wss,
                Err(e) => {
                    if let Some(attempts) = attempts.as_mut() {
                        if *attempts <= 1 {
                            return Err(SignallingError::ConnectionFailed(Box::new(e)));
                        } else {
                            *attempts -= 1;
                            warn!("connection to signalling server failed, {attempts} attempt(s) remain");
                        }
                    } else {
                        continue 'signalling;
                    }
                }
            };
        };
        Ok(Self { websocket_stream })
    }

    async fn send(&mut self, request: String) -> Result<(), SignallingError> {
        self.websocket_stream
            .send(Message::Text(request))
            .await
            .map_err(SignallingError::from)
    }

    async fn next_message(&mut self) -> Result<String, SignallingError> {
        match self.websocket_stream.next().await {
            Some(Ok(Message::Text(message))) => Ok(message),
            Some(Ok(_)) => Err(SignallingError::UnknownFormat),
            Some(Err(err)) => Err(SignallingError::from(err)),
            None => Err(SignallingError::StreamExhausted),
        }
    }
}

pub(crate) struct NativeMessenger;

impl PeerDataSender for UnboundedSender<Packet> {
    fn send(&mut self, packet: Packet) -> Result<(), super::error::MessagingError> {
        self.unbounded_send(packet).map_err(MessagingError::from)
    }
}

#[async_trait]
impl Messenger for NativeMessenger {
    type DataChannel = UnboundedSender<Packet>;
    type HandshakeMeta = (
        Vec<UnboundedReceiver<Packet>>,
        (
            Vec<Arc<RTCDataChannel>>,
            Pin<Box<dyn FusedFuture<Output = Result<(), Box<dyn Error>>> + Send>>,
        ),
    );

    async fn offer_handshake(
        signal_peer: SignalPeer,
        mut from_peer_rx: UnboundedReceiver<PeerSignal>,
        peer_state_tx: UnboundedSender<(PeerId, PeerState)>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        config: &WebRtcSocketConfig,
    ) -> HandshakeResult<Self::DataChannel, Self::HandshakeMeta> {
        let (to_peer_message_tx, to_peer_message_rx) = new_senders_and_receivers(config);

        debug!("making offer");
        let (connection, trickle) =
            create_rtc_peer_connection(signal_peer.clone(), peer_state_tx.clone(), config)
                .compat()
                .await
                .unwrap();

        let (channel_ready_tx, wait_for_channels) = create_data_channels_ready_fut(config);
        let data_channels = create_data_channels(
            &connection,
            channel_ready_tx,
            signal_peer.id.clone(),
            peer_state_tx.clone(),
            messages_from_peers_tx,
            &config.channels,
        )
        .await;

        // TODO: maybe pass in options? ice restart etc.?
        let offer = connection.create_offer(None).compat().await.unwrap();
        let sdp = offer.sdp.clone();
        connection
            .set_local_description(offer)
            .compat()
            .await
            .unwrap();
        signal_peer.send(PeerSignal::Offer(sdp));

        let answer = loop {
            let signal = from_peer_rx
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

        let trickle_fut =
            complete_handshake(trickle, &connection, from_peer_rx, wait_for_channels).await;

        HandshakeResult {
            peer_id: signal_peer.id,
            data_channels: to_peer_message_tx,
            metadata: (to_peer_message_rx, (data_channels, trickle_fut)),
        }
    }

    async fn accept_handshake(
        signal_peer: SignalPeer,
        mut from_peer_rx: UnboundedReceiver<PeerSignal>,
        peer_state_tx: UnboundedSender<(PeerId, PeerState)>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        config: &WebRtcSocketConfig,
    ) -> HandshakeResult<Self::DataChannel, Self::HandshakeMeta> {
        let (to_peer_message_tx, to_peer_message_rx) = new_senders_and_receivers(config);

        debug!("handshake_accept");
        let (connection, trickle) =
            create_rtc_peer_connection(signal_peer.clone(), peer_state_tx.clone(), config)
                .compat()
                .await
                .unwrap();

        let (channel_ready_tx, wait_for_channels) = create_data_channels_ready_fut(config);
        let data_channels = create_data_channels(
            &connection,
            channel_ready_tx,
            signal_peer.id.clone(),
            peer_state_tx.clone(),
            messages_from_peers_tx,
            &config.channels,
        )
        .await;

        let offer = loop {
            match from_peer_rx.next().await.expect("error") {
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

        let answer = connection.create_answer(None).compat().await.unwrap();
        signal_peer.send(PeerSignal::Answer(answer.sdp.clone()));
        connection
            .set_local_description(answer)
            .compat()
            .await
            .unwrap();

        let trickle_fut =
            complete_handshake(trickle, &connection, from_peer_rx, wait_for_channels).await;

        HandshakeResult {
            peer_id: signal_peer.id,
            data_channels: to_peer_message_tx,
            metadata: (to_peer_message_rx, (data_channels, trickle_fut)),
        }
    }

    async fn peer_loop(peer_uuid: PeerId, handshake_meta: Self::HandshakeMeta) -> PeerId {
        let (mut to_peer_message_rx, (data_channels, mut trickle_fut)) = handshake_meta;

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
                    trace!("sending packet {:?}", message);
                    let message = message.clone();
                    let message = Bytes::from(message);
                    if let Err(e) = data_channel.send(&message).await {
                        error!("error sending to data channel: {e:?}")
                    }
                }
            })
            .collect();

        loop {
            select! {
                _ = message_loop_futs.next() => break,
                // TODO: this means that the signalling is down, should return an
                // error
                _ = trickle_fut => continue,
            }
        }

        peer_uuid
    }
}

async fn complete_handshake(
    trickle: Arc<CandidateTrickle>,
    connection: &Arc<RTCPeerConnection>,
    from_peer_rx: UnboundedReceiver<PeerSignal>,
    mut wait_for_channels: Pin<Box<Fuse<impl Future<Output = ()>>>>,
) -> Pin<Box<Fuse<Compat<impl Future<Output = Result<(), Box<dyn Error>>>>>>> {
    trickle.send_pending_candidates().await;
    let mut trickle_fut = Box::pin(
        CandidateTrickle::listen_for_remote_candidates(Arc::clone(connection), from_peer_rx)
            .compat()
            .fuse(),
    );

    loop {
        select! {
            _ = wait_for_channels => {
                break;
            },
            // TODO: this means that the signalling is down, should return an
            // error
            _ = trickle_fut => continue,
        };
    }

    trickle_fut
}

struct CandidateTrickle {
    signal_peer: SignalPeer,
    pending: Mutex<Vec<String>>,
}

impl CandidateTrickle {
    fn new(signal_peer: SignalPeer) -> Self {
        Self {
            signal_peer,
            pending: Default::default(),
        }
    }

    async fn on_local_candidate(
        &self,
        peer_connection: &RTCPeerConnection,
        candidate: RTCIceCandidate,
    ) {
        let candidate_init = match candidate.to_json() {
            Ok(candidate_init) => candidate_init,
            Err(err) => {
                error!("failed to convert ice candidate to candidate init, ignoring: {err}");
                return;
            }
        };

        let candidate_json =
            serde_json::to_string(&candidate_init).expect("failed to serialize candidate to json");

        // Local candidates can only be sent after the remote description
        if peer_connection.remote_description().await.is_some() {
            // Can send local candidate already
            debug!("sending IceCandidate signal {}", candidate);
            self.signal_peer
                .send(PeerSignal::IceCandidate(candidate_json));
        } else {
            // Can't send yet, store in pending
            debug!("storing pending IceCandidate signal {}", candidate_json);
            self.pending.lock().await.push(candidate_json);
        }
    }

    async fn send_pending_candidates(&self) {
        let mut pending = self.pending.lock().await;
        for candidate in std::mem::take(&mut *pending) {
            self.signal_peer.send(PeerSignal::IceCandidate(candidate));
        }
    }

    async fn listen_for_remote_candidates(
        peer_connection: Arc<RTCPeerConnection>,
        mut signal_receiver: UnboundedReceiver<PeerSignal>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        while let Some(signal) = signal_receiver.next().await {
            match signal {
                PeerSignal::IceCandidate(candidate_json) => {
                    debug!("received ice candidate: {candidate_json:?}");
                    match serde_json::from_str::<RTCIceCandidateInit>(&candidate_json) {
                        Ok(candidate_init) => {
                            peer_connection.add_ice_candidate(candidate_init).await?;
                        }
                        Err(err) => {
                            error!("failed to parse ice candidate json, ignoring: {err:?}");
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
    peer_state_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
    config: &WebRtcSocketConfig,
) -> Result<(Arc<RTCPeerConnection>, Arc<CandidateTrickle>), Box<dyn std::error::Error>> {
    let api = APIBuilder::new().build();

    let ice_server = &config.ice_server;
    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: ice_server.urls.clone(),
            username: ice_server.username.clone().unwrap_or_default(),
            credential: ice_server.credential.clone().unwrap_or_default(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let connection = api.new_peer_connection(config).await?;
    let connection = Arc::new(connection);

    let peer_id = signal_peer.id.clone();
    let trickle = Arc::new(CandidateTrickle::new(signal_peer));

    let connection2 = Arc::downgrade(&connection);
    let trickle2 = trickle.clone();
    connection.on_ice_candidate(Box::new(move |c| {
        let connection2 = connection2.clone();
        let trickle2 = trickle2.clone();
        Box::pin(async move {
            if let Some(c) = c {
                if let Some(connection2) = connection2.upgrade() {
                    trickle2.on_local_candidate(&connection2, c).await;
                } else {
                    warn!("missing peer_connection?");
                }
            }
        })
    }));

    connection.on_peer_connection_state_change(Box::new(move |state| {
        debug!("Peer Connection State has changed: {state}");
        match state {
            // These events are not currently available in web-sys (wasm)
            // so instead, we implement our own criteria for when a peer is
            // considered to be "Connected". See PeerState::Connected.
            RTCPeerConnectionState::Unspecified
            | RTCPeerConnectionState::New
            | RTCPeerConnectionState::Connecting
            | RTCPeerConnectionState::Connected => {}
            // ...in other words, we currently only care about when *something*
            // has definitely failed:
            RTCPeerConnectionState::Disconnected
            | RTCPeerConnectionState::Failed
            | RTCPeerConnectionState::Closed => {
                if let Err(err) =
                    peer_state_tx.unbounded_send((peer_id.clone(), PeerState::Disconnected))
                {
                    // should only happen if the socket is dropped, or we are out of memory
                    warn!("failed to report peer state change: {err:?}");
                }
            }
        }
        Box::pin(async {})
    }));

    Ok((connection, trickle))
}

async fn create_data_channels(
    connection: &RTCPeerConnection,
    mut channel_ready: Vec<futures_channel::mpsc::Sender<u8>>,
    peer_id: PeerId,
    peer_state_tx: UnboundedSender<(PeerId, PeerState)>,
    from_peer_message_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
    channel_configs: &[ChannelConfig],
) -> Vec<Arc<RTCDataChannel>> {
    let mut channels = vec![];
    for (i, channel_config) in channel_configs.iter().enumerate() {
        let channel = create_data_channel(
            connection,
            channel_ready.pop().unwrap(),
            peer_id.clone(),
            peer_state_tx.clone(),
            from_peer_message_tx.get(i).unwrap().clone(),
            channel_config,
            i,
        )
        .await;

        channels.push(channel);
    }

    channels
}

async fn create_data_channel(
    connection: &RTCPeerConnection,
    mut channel_ready: futures_channel::mpsc::Sender<u8>,
    peer_id: PeerId,
    peer_state_tx: UnboundedSender<(PeerId, PeerState)>,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
    channel_config: &ChannelConfig,
    channel_index: usize,
) -> Arc<RTCDataChannel> {
    let config = RTCDataChannelInit {
        ordered: Some(channel_config.ordered),
        negotiated: Some(channel_index as u16),
        max_retransmits: channel_config.max_retransmits,
        ..Default::default()
    };

    let channel = connection
        .create_data_channel(&format!("matchbox_socket_{channel_index}"), Some(config))
        .await
        .unwrap();

    channel.on_open(Box::new(move || {
        debug!("Data channel ready");
        Box::pin(async move {
            channel_ready.try_send(1).unwrap();
        })
    }));

    {
        let peer_id = peer_id.clone();
        let peer_state_tx = peer_state_tx.clone();
        channel.on_close(Box::new(move || {
            debug!("Data channel closed");
            if let Err(err) =
                peer_state_tx.unbounded_send((peer_id.clone(), PeerState::Disconnected))
            {
                // should only happen if the socket is dropped, or we are out of memory
                warn!("failed to notify about data channel closing: {err:?}");
            }
            Box::pin(async move {})
        }));
    }

    channel.on_close(Box::new(move || {
        // TODO: handle this somehow
        debug!("Data channel closed");

        Box::pin(async move {})
    }));

    channel.on_error(Box::new(move |e| {
        // TODO: handle this somehow
        warn!("Data channel error {:?}", e);
        Box::pin(async move {})
    }));

    channel.on_message(Box::new(move |message| {
        let packet = (*message.data).into();
        debug!("rx {:?}", packet);
        from_peer_message_tx
            .unbounded_send((peer_id.clone(), packet))
            .unwrap();
        Box::pin(async move {})
    }));

    channel
}
