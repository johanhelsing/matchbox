use async_compat::CompatExt;
use bytes::Bytes;
use futures::{
    future::FusedFuture, pin_mut, stream::FuturesUnordered, Future, FutureExt, SinkExt, StreamExt,
};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::{lock::Mutex, select};
use log::{debug, warn};
use std::time::Duration;
use std::{collections::HashMap, pin::Pin, sync::Arc};
use webrtc::{
    api::APIBuilder,
    data_channel::{data_channel_init::RTCDataChannelInit, RTCDataChannel},
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_server::RTCIceServer,
    },
    peer_connection::{
        configuration::RTCConfiguration,
        sdp::{sdp_type::RTCSdpType, session_description::RTCSessionDescription},
        RTCPeerConnection,
    },
};

use crate::webrtc_socket::{
    messages::{PeerEvent, PeerId, PeerRequest, PeerSignal},
    signal_peer::SignalPeer,
    Packet, WebRtcSocketConfig, KEEP_ALIVE_INTERVAL,
};

pub async fn message_loop(
    id: PeerId,
    config: WebRtcSocketConfig,
    requests_sender: futures_channel::mpsc::UnboundedSender<PeerRequest>,
    events_receiver: futures_channel::mpsc::UnboundedReceiver<PeerEvent>,
    peer_messages_out_rx: futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
) {
    message_loop_impl(
        id,
        &config,
        requests_sender,
        events_receiver,
        peer_messages_out_rx,
        new_connected_peers_tx,
        messages_from_peers_tx,
    )
    // web-rtc is tokio-based so we use compat here to make it work with other async run-times
    .compat()
    .await
}

async fn message_loop_impl(
    id: PeerId,
    config: &WebRtcSocketConfig,
    requests_sender: futures_channel::mpsc::UnboundedSender<PeerRequest>,
    mut events_receiver: futures_channel::mpsc::UnboundedReceiver<PeerEvent>,
    mut peer_messages_out_rx: futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
) {
    debug!("Entering native WebRtcSocket message loop");

    debug!("I am {:?}", id);

    requests_sender
        .unbounded_send(PeerRequest::Uuid(id))
        .expect("failed to send uuid");

    let mut peer_loops_a = FuturesUnordered::new();
    let mut peer_loops_b = FuturesUnordered::new();
    let mut handshake_signals = HashMap::new();
    let mut connected_peers = HashMap::new();

    let timeout = Delay::new(Duration::from_millis(KEEP_ALIVE_INTERVAL));
    futures::pin_mut!(timeout);

    loop {
        let next_signal_event = events_receiver.next().fuse();
        let next_peer_message_out = peer_messages_out_rx.next().fuse();

        pin_mut!(next_signal_event, next_peer_message_out);

        select! {
            _ = (&mut timeout).fuse() => {
                requests_sender.unbounded_send(PeerRequest::KeepAlive).expect("send failed");
                timeout.reset(Duration::from_millis(KEEP_ALIVE_INTERVAL));
            }

            _ = peer_loops_a.select_next_some() => {
                debug!("peer finished");
            },
            _ = peer_loops_b.select_next_some() => {
                debug!("peer finished");
            },

            message = next_signal_event => {
                if let Some(event) = message {
                    debug!("{:?}", event);
                    match event {
                        PeerEvent::NewPeer(peer_uuid) => {
                            let (signal_sender, signal_receiver) = futures_channel::mpsc::unbounded();
                            handshake_signals.insert(peer_uuid.clone(), signal_sender);
                            let signal_peer = SignalPeer::new(peer_uuid.clone(), requests_sender.clone());
                            let handshake_fut = handshake_offer(signal_peer, signal_receiver, new_connected_peers_tx.clone(), messages_from_peers_tx.clone(), config);
                            let (to_peer_data_tx, to_peer_data_rx) = futures_channel::mpsc::unbounded();
                            connected_peers.insert(peer_uuid, to_peer_data_tx);
                            peer_loops_a.push(peer_loop(handshake_fut, to_peer_data_rx));
                        }
                        PeerEvent::Signal { sender, data } => {
                            let from_peer_sender = handshake_signals.entry(sender.clone()).or_insert_with(|| {
                                let (from_peer_sender, from_peer_receiver) = futures_channel::mpsc::unbounded();
                                let signal_peer = SignalPeer::new(sender.clone(), requests_sender.clone());
                                let (to_peer_data_tx, to_peer_data_rx) = futures_channel::mpsc::unbounded();
                                // We didn't start signalling with this peer, assume we're the accepting part
                                let handshake_fut = handshake_accept(signal_peer, from_peer_receiver, new_connected_peers_tx.clone(), messages_from_peers_tx.clone(), config);
                                connected_peers.insert(sender, to_peer_data_tx);
                                let peer_loop_fut = peer_loop(handshake_fut, to_peer_data_rx);
                                peer_loops_b.push(peer_loop_fut);
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
            message = next_peer_message_out => {
                match message {
                    Some(message) => {
                        let peer = &message.0;
                        let sender = &connected_peers.get(peer).unwrap_or_else(|| panic!("couldn't find data channel for peer {}", peer));
                        sender.unbounded_send(message.1).unwrap();
                    },
                    None => {
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
        let candidate = candidate.to_json().await.unwrap().candidate;

        // Local candidates can only be sent after the remote description
        if peer_connection.remote_description().await.is_some() {
            // Can send local candidate already
            debug!("sending IceCandidate signal {}", candidate);
            self.signal_peer.send(PeerSignal::IceCandidate(candidate));
        } else {
            // Can't send yet, store in pending
            debug!("storing pending IceCandidate signal {}", candidate);
            self.pending.lock().await.push(candidate);
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
                PeerSignal::IceCandidate(candidate) => {
                    debug!("got an IceCandidate signal! {}", candidate);
                    peer_connection
                        .add_ice_candidate(RTCIceCandidateInit {
                            candidate,
                            ..Default::default()
                        })
                        .await?;
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
    new_peer_tx: UnboundedSender<PeerId>,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
    config: &WebRtcSocketConfig,
) -> Result<
    (
        PeerId,
        Arc<RTCDataChannel>,
        Pin<Box<dyn FusedFuture<Output = Result<(), Box<dyn std::error::Error>>> + Send>>,
    ),
    Box<dyn std::error::Error>,
> {
    debug!("making offer");
    let (connection, trickle) = create_rtc_peer_connection(signal_peer.clone(), config).await?;

    let (channel_ready_tx, mut channel_ready_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(
        &connection,
        channel_ready_tx,
        signal_peer.id.clone(),
        new_peer_tx,
        from_peer_message_tx,
    )
    .await;

    // TODO: maybe pass in options? ice restart etc.?
    let offer = connection.create_offer(None).await?;
    let sdp = offer.sdp.clone();
    connection.set_local_description(offer).await?;
    signal_peer.send(PeerSignal::Offer(sdp));

    let sdp: String;

    loop {
        let signal = signal_receiver
            .next()
            .await
            .ok_or("Signal server connection lost in the middle of a handshake")?;

        match signal {
            PeerSignal::Answer(answer) => {
                sdp = answer;
                break;
            }
            PeerSignal::Offer(_) => {
                warn!("Got an unexpected Offer, while waiting for Answer. Ignoring.")
            }
            PeerSignal::IceCandidate(_) => {
                warn!("Got an unexpected IceCandidate, while waiting for Answer. Ignoring.")
            }
        };
    }

    let mut remote_description = RTCSessionDescription::default();
    remote_description.sdp = sdp;
    remote_description.sdp_type = RTCSdpType::Answer; // TODO: Or leave unspecified?
    connection
        .set_remote_description(remote_description)
        .await?;

    trickle.send_pending_candidates().await;
    let mut trickle_fut = Box::pin(
        CandidateTrickle::listen_for_remote_candidates(connection, signal_receiver).fuse(),
    );

    let mut channel_ready_fut = channel_ready_rx.next();
    loop {
        select! {
            _ = channel_ready_fut => break,
            // TODO: this means that the signalling is down, should return an
            // error
            _ = trickle_fut => continue,
        };
    }

    Ok((signal_peer.id, data_channel, trickle_fut))
}

async fn handshake_accept(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    new_peer_tx: UnboundedSender<PeerId>,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
    config: &WebRtcSocketConfig,
) -> Result<
    (
        PeerId,
        Arc<RTCDataChannel>,
        Pin<Box<dyn FusedFuture<Output = Result<(), Box<dyn std::error::Error>>> + Send>>,
    ),
    Box<dyn std::error::Error>,
> {
    debug!("handshake_accept");
    let (connection, trickle) = create_rtc_peer_connection(signal_peer.clone(), config).await?;

    let offer;
    loop {
        match signal_receiver.next().await.ok_or("error")? {
            PeerSignal::Offer(o) => {
                offer = o;
                break;
            }
            _ => {
                warn!("ignoring other signal!!!");
            }
        }
    }
    debug!("received offer");
    let mut remote_description = RTCSessionDescription::default();
    remote_description.sdp = offer;
    remote_description.sdp_type = RTCSdpType::Offer; // TODO: Or leave unspecified?
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

    let data_channel_fut = wait_for_data_channel(
        &connection,
        signal_peer.id.clone(),
        new_peer_tx,
        from_peer_message_tx,
    )
    .fuse();
    pin_mut!(data_channel_fut);

    let data_channel = loop {
        select! {
            data_channel = data_channel_fut => break data_channel,
            // TODO: this means that the signalling is down, should return an
            // error
            _ = trickle_fut => continue,
        };
    };

    Ok((signal_peer.id, data_channel, trickle_fut))
}

async fn create_rtc_peer_connection(
    signal_peer: SignalPeer,
    config: &WebRtcSocketConfig,
) -> Result<(Arc<RTCPeerConnection>, Arc<CandidateTrickle>), Box<dyn std::error::Error>> {
    let api = APIBuilder::new().build();

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: config.ice_server.urls.clone(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let connection = api.new_peer_connection(config).await?;
    let connection = Arc::new(connection);

    let trickle = Arc::new(CandidateTrickle::new(signal_peer));

    let connection2 = Arc::downgrade(&connection);
    let trickle2 = trickle.clone();
    connection
        .on_ice_candidate(Box::new(move |c| {
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
        }))
        .await;

    connection
        .on_peer_connection_state_change(Box::new(move |s| {
            debug!("Peer Connection State has changed: {}", s);
            Box::pin(async {})
        }))
        .await;

    Ok((connection, trickle))
}

async fn create_data_channel(
    connection: &RTCPeerConnection,
    mut channel_ready: futures_channel::mpsc::Sender<u8>,
    peer_id: PeerId,
    mut new_peer_tx: UnboundedSender<PeerId>,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
) -> Arc<RTCDataChannel> {
    let config = RTCDataChannelInit {
        ordered: Some(false),
        max_retransmits: Some(0),
        id: Some(0),
        ..Default::default()
    };

    let channel = connection
        .create_data_channel("webudp", Some(config))
        .await
        .unwrap();

    let peer_id2 = peer_id.clone();
    channel
        .on_open(Box::new(move || {
            debug!("Data channel ready");
            Box::pin(async move {
                new_peer_tx.send(peer_id2.clone()).await.unwrap();
                channel_ready.try_send(1).unwrap();
            })
        }))
        .await;

    setup_data_channel(&channel, peer_id, from_peer_message_tx).await;

    channel
}

async fn wait_for_data_channel(
    connection: &RTCPeerConnection,
    peer_id: PeerId,
    new_peer_tx: UnboundedSender<PeerId>,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
) -> Arc<RTCDataChannel> {
    let (channel_tx, mut channel_rx) = futures_channel::mpsc::channel(1);

    connection
        .on_data_channel(Box::new(move |channel| {
            debug!("new data channel");
            let peer_id = peer_id.clone();
            let mut new_peer_tx = new_peer_tx.clone();
            let from_peer_message_tx = from_peer_message_tx.clone();
            let mut channel_tx = channel_tx.clone();
            Box::pin(async move {
                let peer_id2 = peer_id.clone();
                let channel2 = Arc::clone(&channel);

                // TODO: register close & error callbacks
                channel
                    .on_open(Box::new(move || {
                        debug!("Data channel ready");
                        Box::pin(async move {
                            new_peer_tx.send(peer_id2).await.unwrap();
                            channel_tx.try_send(channel2).unwrap();
                        })
                    }))
                    .await;

                setup_data_channel(&channel, peer_id, from_peer_message_tx).await;
            })
        }))
        .await;

    channel_rx.next().await.unwrap()
}

async fn setup_data_channel(
    data_channel: &RTCDataChannel,
    peer_id: PeerId,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
) {
    data_channel
        .on_close(Box::new(move || {
            // TODO: handle this somehow
            debug!("Data channel closed");
            Box::pin(async move {})
        }))
        .await;

    data_channel
        .on_error(Box::new(move |e| {
            // TODO: handle this somehow
            warn!("Data channel error {:?}", e);
            Box::pin(async move {})
        }))
        .await;

    data_channel
        .on_message(Box::new(move |message| {
            let packet = (*message.data).into();
            debug!("rx {:?}", packet);
            from_peer_message_tx
                .unbounded_send((peer_id.clone(), packet))
                .unwrap();
            Box::pin(async move {})
        }))
        .await;
}

async fn peer_loop(
    handshake_fut: impl Future<
        Output = Result<
            (
                PeerId,
                Arc<RTCDataChannel>,
                Pin<Box<dyn FusedFuture<Output = Result<(), Box<dyn std::error::Error>>> + Send>>,
            ),
            Box<dyn std::error::Error>,
        >,
    >,
    mut to_peer_message_rx: UnboundedReceiver<Packet>,
) {
    let (_peer_id, data_channel, mut trickle_fut) = handshake_fut.await.unwrap();

    let message_loop_fut = async move {
        while let Some(message) = to_peer_message_rx.next().await {
            debug!("tx {:?}", message);
            let message = message.clone();
            let message = Bytes::from(message);
            data_channel.send(&message).await.unwrap();
        }
    };
    let message_loop_fut = message_loop_fut.fuse();
    pin_mut!(message_loop_fut);

    loop {
        select! {
            _ = message_loop_fut => break,
            // TODO: this means that the signalling is down, should return an
            // error
            _ = trickle_fut => continue,
        }
    }

    // TODO: clear on_message?
}
