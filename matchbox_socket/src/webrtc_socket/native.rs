use crate::webrtc_socket::{
    error::SignallingError,
    messages::{PeerEvent, PeerId, PeerRequest, PeerSignal},
    signal_peer::SignalPeer,
    socket::{create_data_channels_ready_fut, new_senders_and_receivers},
    ChannelConfig, MessageLoopChannels, Messenger, Packet, Signaller, WebRtcSocketConfig,
    KEEP_ALIVE_INTERVAL,
};
use async_compat::CompatExt;
use async_trait::async_trait;
use async_tungstenite::{
    async_std::{connect_async, ConnectStream},
    tungstenite::Message,
    WebSocketStream,
};
use bytes::Bytes;
use futures::{
    future::FusedFuture, stream::FuturesUnordered, Future, FutureExt, SinkExt, StreamExt,
};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::{lock::Mutex, select};
use log::{debug, error, info, trace, warn};
use std::{collections::HashMap, pin::Pin, sync::Arc, time::Duration};
use webrtc::{
    api::APIBuilder,
    data_channel::{data_channel_init::RTCDataChannelInit, RTCDataChannel},
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_server::RTCIceServer,
    },
    peer_connection::{
        configuration::RTCConfiguration, sdp::session_description::RTCSessionDescription,
        RTCPeerConnection,
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

#[async_trait]
impl Messenger for NativeMessenger {
    async fn message_loop(config: WebRtcSocketConfig, channels: MessageLoopChannels) {
        message_loop_impl(&config, channels)
            // web-rtc is tokio-based so we use compat here to make it work with other async
            // run-times
            .compat()
            .await
    }
}

async fn message_loop_impl(config: &WebRtcSocketConfig, channels: MessageLoopChannels) {
    let MessageLoopChannels {
        requests_sender,
        mut events_receiver,
        mut peer_messages_out_rx,
        new_connected_peers_tx,
        disconnected_peers_tx,
        messages_from_peers_tx,
    } = channels;
    debug!("Entering native WebRtcSocket message loop");

    let mut peer_loops = FuturesUnordered::new();
    let mut handshake_signals = HashMap::new();
    let mut connected_peers = HashMap::new();

    let timeout = Delay::new(Duration::from_millis(KEEP_ALIVE_INTERVAL));
    futures::pin_mut!(timeout);

    loop {
        let mut next_peer_messages_out: FuturesUnordered<_> = peer_messages_out_rx
            .iter_mut()
            .enumerate()
            .map(|(index, rx)| async move { (index, rx.next().await) })
            .collect();

        select! {
            _ = (&mut timeout).fuse() => {
                requests_sender.unbounded_send(PeerRequest::KeepAlive).expect("send failed");
                timeout.reset(Duration::from_millis(KEEP_ALIVE_INTERVAL));
            }

            _ = peer_loops.select_next_some() => {
                debug!("peer finished");
            },

            message = events_receiver.next().fuse() => {
                if let Some(event) = message {
                    debug!("{:?}", event);
                    match event {
                        PeerEvent::IdAssigned(peer_uuid) => {
                            info!("Assigned UUID: {peer_uuid}");
                        }
                        PeerEvent::NewPeer(peer_uuid) => {
                            let (signal_sender, signal_receiver) = futures_channel::mpsc::unbounded();
                            handshake_signals.insert(peer_uuid.clone(), signal_sender);
                            let signal_peer = SignalPeer::new(peer_uuid.clone(), requests_sender.clone());
                            let handshake_fut = handshake_offer(signal_peer, signal_receiver, new_connected_peers_tx.clone(), messages_from_peers_tx.clone(), config);
                            let (to_peer_data_tx, to_peer_data_rx) = new_senders_and_receivers(config);

                            connected_peers.insert(peer_uuid, to_peer_data_tx);
                            peer_loops.push(peer_loop(handshake_fut, to_peer_data_rx).boxed());
                        }
                        PeerEvent::PeerLeft(peer_uuid) => {
                            disconnected_peers_tx.unbounded_send(peer_uuid).expect("fail to send disconnected peer");
                        }
                        PeerEvent::Signal { sender, data } => {
                            let from_peer_sender = handshake_signals.entry(sender.clone()).or_insert_with(|| {
                                let (from_peer_sender, from_peer_receiver) = futures_channel::mpsc::unbounded();
                                let signal_peer = SignalPeer::new(sender.clone(), requests_sender.clone());
                                let (to_peer_data_tx, to_peer_data_rx) = new_senders_and_receivers(config);
                                // We didn't start signalling with this peer, assume we're the accepting part
                                let handshake_fut = handshake_accept(signal_peer, from_peer_receiver, new_connected_peers_tx.clone(), messages_from_peers_tx.clone(), config);
                                connected_peers.insert(sender, to_peer_data_tx);
                                let peer_loop_fut = peer_loop(handshake_fut, to_peer_data_rx);
                                peer_loops.push(peer_loop_fut.boxed());
                                from_peer_sender
                            });
                            from_peer_sender.unbounded_send(data)
                                .expect("failed to forward signal to handshaker");
                        }
                    }
                } else {
                    // Disconnected from signalling server
                };
            }

            // TODO: maybe use some forward trait instead?
            message = next_peer_messages_out.next().fuse() => {
                match message {
                    Some((channel_index, Some((peer, packet)))) => {
                        let senders = connected_peers.get_mut(&peer)
                            .unwrap_or_else(|| panic!("couldn't find data channel for peer {peer}"));
                        let sender = senders.get_mut(channel_index)
                            .unwrap_or_else(|| panic!("Unexpected data channel index during send: {channel_index}"));
                        sender.unbounded_send(packet).unwrap();
                    },
                    Some((_, None)) | None => {
                        // Receiver end of outgoing message channel closed,
                        // which most likely means the socket was dropped.
                        // There could probably be cleaner ways to handle this,
                        // but for now, just exit.
                        debug!("Outgoing message queue closed");
                        break;
                    }
                }
            }

            complete => break
        }
    }
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

async fn handshake_offer(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    mut new_peer_tx: UnboundedSender<PeerId>,
    from_peer_message_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
    config: &WebRtcSocketConfig,
) -> Result<
    (
        PeerId,
        Vec<Arc<RTCDataChannel>>,
        Pin<Box<dyn FusedFuture<Output = Result<(), Box<dyn std::error::Error>>> + Send>>,
    ),
    Box<dyn std::error::Error>,
> {
    debug!("making offer");
    let (connection, trickle) = create_rtc_peer_connection(signal_peer.clone(), config).await?;

    let (channel_ready_tx, mut wait_for_channels) = create_data_channels_ready_fut(config);
    let data_channels = create_data_channels(
        &connection,
        channel_ready_tx,
        signal_peer.id.clone(),
        from_peer_message_tx,
        &config.channels,
    )
    .await;

    // TODO: maybe pass in options? ice restart etc.?
    let offer = connection.create_offer(None).await?;
    let sdp = offer.sdp.clone();
    connection.set_local_description(offer).await?;
    signal_peer.send(PeerSignal::Offer(sdp));

    let answer = loop {
        let signal = signal_receiver
            .next()
            .await
            .ok_or("Signal server connection lost in the middle of a handshake")?;

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

    let remote_description = RTCSessionDescription::answer(answer)?;
    connection
        .set_remote_description(remote_description)
        .await?;

    trickle.send_pending_candidates().await;
    let mut trickle_fut = Box::pin(
        CandidateTrickle::listen_for_remote_candidates(connection, signal_receiver).fuse(),
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

    new_peer_tx.send(signal_peer.id.clone()).await.unwrap();

    Ok((signal_peer.id, data_channels, trickle_fut))
}

async fn handshake_accept(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    mut new_peer_tx: UnboundedSender<PeerId>,
    from_peer_message_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
    config: &WebRtcSocketConfig,
) -> Result<
    (
        PeerId,
        Vec<Arc<RTCDataChannel>>,
        Pin<Box<dyn FusedFuture<Output = Result<(), Box<dyn std::error::Error>>> + Send>>,
    ),
    Box<dyn std::error::Error>,
> {
    debug!("handshake_accept");
    let (connection, trickle) = create_rtc_peer_connection(signal_peer.clone(), config).await?;

    let (channel_ready_tx, mut wait_for_channels) = create_data_channels_ready_fut(config);
    let data_channels = create_data_channels(
        &connection,
        channel_ready_tx,
        signal_peer.id.clone(),
        from_peer_message_tx,
        &config.channels,
    )
    .await;

    let offer = loop {
        match signal_receiver.next().await.ok_or("error")? {
            PeerSignal::Offer(offer) => {
                break offer;
            }
            _ => {
                warn!("ignoring other signal!!!");
            }
        }
    };
    debug!("received offer");
    let remote_description = RTCSessionDescription::offer(offer)?;
    connection
        .set_remote_description(remote_description)
        .await?;

    let answer = connection.create_answer(None).await?;
    signal_peer.send(PeerSignal::Answer(answer.sdp.clone()));
    connection.set_local_description(answer).await?;
    // Can only send candidates after sending the local description.
    trickle.send_pending_candidates().await;
    let mut trickle_fut = Box::pin(
        CandidateTrickle::listen_for_remote_candidates(Arc::clone(&connection), signal_receiver)
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

    new_peer_tx.send(signal_peer.id.clone()).await.unwrap();

    Ok((signal_peer.id, data_channels, trickle_fut))
}

async fn create_rtc_peer_connection(
    signal_peer: SignalPeer,
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

    connection.on_peer_connection_state_change(Box::new(move |s| {
        debug!("Peer Connection State has changed: {}", s);
        Box::pin(async {})
    }));

    Ok((connection, trickle))
}

async fn create_data_channels(
    connection: &RTCPeerConnection,
    mut channel_ready: Vec<futures_channel::mpsc::Sender<u8>>,
    peer_id: PeerId,
    from_peer_message_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
    channel_configs: &[ChannelConfig],
) -> Vec<Arc<RTCDataChannel>> {
    let mut channels = vec![];
    for (i, channel_config) in channel_configs.iter().enumerate() {
        let channel = create_data_channel(
            connection,
            channel_ready.pop().unwrap(),
            peer_id.clone(),
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

    setup_data_channel(&channel, peer_id, from_peer_message_tx).await;

    channel
}

async fn setup_data_channel(
    data_channel: &RTCDataChannel,
    peer_id: PeerId,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
) {
    data_channel.on_close(Box::new(move || {
        // TODO: handle this somehow
        debug!("Data channel closed");
        Box::pin(async move {})
    }));

    data_channel.on_error(Box::new(move |e| {
        // TODO: handle this somehow
        warn!("Data channel error {:?}", e);
        Box::pin(async move {})
    }));

    data_channel.on_message(Box::new(move |message| {
        let packet = (*message.data).into();
        debug!("rx {:?}", packet);
        from_peer_message_tx
            .unbounded_send((peer_id.clone(), packet))
            .unwrap();
        Box::pin(async move {})
    }));
}

async fn peer_loop(
    handshake_fut: impl Future<
        Output = Result<
            (
                PeerId,
                Vec<Arc<RTCDataChannel>>,
                Pin<Box<dyn FusedFuture<Output = Result<(), Box<dyn std::error::Error>>> + Send>>,
            ),
            Box<dyn std::error::Error>,
        >,
    >,
    mut to_peer_message_rx: Vec<UnboundedReceiver<Packet>>,
) {
    let (_peer_id, data_channels, mut trickle_fut) = handshake_fut.await.unwrap();

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

    // TODO: clear on_message?
}
