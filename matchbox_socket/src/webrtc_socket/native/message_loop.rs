use bytes::Bytes;
use futures::{pin_mut, stream::FuturesUnordered, Future, FutureExt, SinkExt, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_util::select;
use log::{debug, warn};
use std::{collections::HashMap, sync::Arc};
use webrtc::{
    api::APIBuilder,
    data_channel::{data_channel_init::RTCDataChannelInit, RTCDataChannel},
    ice_transport::{
        ice_gatherer_state::RTCIceGathererState, ice_gathering_state::RTCIceGatheringState,
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
    Packet,
};

pub async fn message_loop(
    id: PeerId,
    requests_sender: futures_channel::mpsc::UnboundedSender<PeerRequest>,
    events_receiver: futures_channel::mpsc::UnboundedReceiver<PeerEvent>,
    peer_messages_out_rx: futures_channel::mpsc::Receiver<(PeerId, Packet)>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
) {
    message_loop_impl(
        id,
        requests_sender,
        events_receiver,
        peer_messages_out_rx,
        new_connected_peers_tx,
        messages_from_peers_tx,
    )
    .await
}

async fn message_loop_impl(
    id: PeerId,
    requests_sender: futures_channel::mpsc::UnboundedSender<PeerRequest>,
    mut events_receiver: futures_channel::mpsc::UnboundedReceiver<PeerEvent>,
    mut peer_messages_out_rx: futures_channel::mpsc::Receiver<(PeerId, Packet)>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
) {
    debug!("Entering native WebRtcSocket message loop");

    requests_sender
        .unbounded_send(PeerRequest::Uuid(id))
        .expect("failed to send uuid");

    // let mut offer_handshakes = FuturesUnordered::new();
    // let mut accept_handshakes = FuturesUnordered::new();
    let mut peer_loops_a = FuturesUnordered::new();
    let mut peer_loops_b = FuturesUnordered::new();
    let mut handshake_signals = HashMap::new();
    // let mut data_channels: HashMap<PeerId, RTCDataChannel> = HashMap::new();
    let mut connected_peers = HashMap::new();

    loop {
        let next_signal_event = events_receiver.next().fuse();
        let next_peer_message_out = peer_messages_out_rx.next().fuse();

        pin_mut!(next_signal_event, next_peer_message_out);

        select! {
                _ = peer_loops_a.select_next_some() => {
                    debug!("peer finished")
                //     check(&res);
                //     let peer = res.unwrap();
                //     data_channels.insert(peer.0.clone(), peer.1);
                //     debug!("Notifying about new peer");
                //     new_connected_peers_tx.unbounded_send(peer.0).expect("send failed");
                //     todo!{}
                },
                _ = peer_loops_b.select_next_some() => {
                    debug!("peer finished")
                //     // TODO: this could be de-duplicated
                //     check(&res);
                //     let peer = res.unwrap();
                //     data_channels.insert(peer.0.clone(), peer.1);
                //     debug!("Notifying about new peer");
                //     new_connected_peers_tx.unbounded_send(peer.0).expect("send failed");
                //     todo!{};
                },

                message = next_signal_event => {
                    match message {
                        Some(event) => {
                            debug!("{:?}", event);
                            match event {
                                PeerEvent::NewPeer(peer_uuid) => {
                                    let (signal_sender, signal_receiver) = futures_channel::mpsc::unbounded();
                                    handshake_signals.insert(peer_uuid.clone(), signal_sender);
                                    let signal_peer = SignalPeer::new(peer_uuid.clone(), requests_sender.clone());
                                    let handshake_fut = handshake_offer(signal_peer, signal_receiver);
                                    let (to_peer_data_tx, to_peer_data_rx) = futures_channel::mpsc::unbounded();
                                    connected_peers.insert(peer_uuid, to_peer_data_tx);
                                    peer_loops_a.push(peer_loop(handshake_fut, new_connected_peers_tx.clone(), messages_from_peers_tx.clone(), to_peer_data_rx));
                                }
                                PeerEvent::Signal { sender, data } => {
                                    let from_peer_sender = handshake_signals.entry(sender.clone()).or_insert_with(|| {
                                        let (from_peer_sender, from_peer_receiver) = futures_channel::mpsc::unbounded();
                                        let signal_peer = SignalPeer::new(sender.clone(), requests_sender.clone());
                                        // We didn't start signalling with this peer, assume we're the accepting part
                                        let handshake_fut = handshake_accept(signal_peer, from_peer_receiver);
                                        let (to_peer_data_tx, to_peer_data_rx) = futures_channel::mpsc::unbounded();
                                        connected_peers.insert(sender, to_peer_data_tx);
                                        let peer_loop_fut = peer_loop(handshake_fut, new_connected_peers_tx.clone(), messages_from_peers_tx.clone(), to_peer_data_rx);
                                        peer_loops_b.push(peer_loop_fut);
                                        from_peer_sender
                                    });
                                    from_peer_sender.unbounded_send(data)
                                        .expect("failed to forward signal to handshaker");
                                }
                            }
                        },
                        None => {} // Disconnected from signalling server
                    };
                }

                // TODO: maybe use some forward trait instead?
                message = next_peer_message_out => {
                    let message = message.unwrap();
                    let sender = &connected_peers.get(&message.0).unwrap();
                    sender.unbounded_send(message.1).unwrap();
                }

                complete => break
        }
    }
}

// Expect/unwrap is broken in select for some reason :/
// fn check(res: &Result<(PeerId, RTCDataChannel), Box<dyn std::error::Error>>) {
//     // but doing it inside a typed function works fine
//     res.as_ref().expect("handshake failed");
// }

async fn handshake_offer(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
) -> Result<(PeerId, Arc<RTCDataChannel>), Box<dyn std::error::Error>> {
    debug!("making offer");
    let connection = create_rtc_peer_connection().await?;

    let (channel_ready_tx, mut channel_ready_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(&connection, channel_ready_tx).await;

    // TODO: maybe pass in options? ice restart etc.?
    let offer = connection.create_offer(None).await?;

    connection.set_local_description(offer).await?;

    wait_for_ice_complete(&connection).await;

    let local_description = connection.local_description().await.unwrap();
    signal_peer.send(PeerSignal::Offer(local_description.sdp));

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
                warn!(
                    "Got an ice candidate message, but ice trickle is not yet supported. Ignoring."
                )
            }
        };
    }

    let mut remote_description = RTCSessionDescription::default();
    remote_description.sdp = sdp;
    remote_description.sdp_type = RTCSdpType::Answer; // TODO: Or leave unspecified?
    connection
        .set_remote_description(remote_description)
        .await?;

    channel_ready_rx.next().await;

    Ok((signal_peer.id, data_channel))
}

async fn handshake_accept(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
) -> Result<(PeerId, Arc<RTCDataChannel>), Box<dyn std::error::Error>> {
    debug!("handshake_accept");
    let connection = create_rtc_peer_connection().await?;
    let (channel_ready_tx, mut channel_ready_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(&connection, channel_ready_tx).await;
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

    connection.set_local_description(answer).await?;

    wait_for_ice_complete(&connection).await;

    let local_description = connection.local_description().await.unwrap();
    signal_peer.send(PeerSignal::Answer(local_description.sdp));

    channel_ready_rx.next().await;

    Ok((signal_peer.id, data_channel))
}

async fn create_rtc_peer_connection() -> Result<RTCPeerConnection, Box<dyn std::error::Error>> {
    let api = APIBuilder::new().build();

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec![
                // "stun:stun.l.google.com:19302".to_owned()
                "stun:stun.johanhelsing.studio:3478".to_string(),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };

    let connection = api.new_peer_connection(config).await?;
    Ok(connection)
}

async fn create_data_channel(
    connection: &RTCPeerConnection,
    mut channel_ready: futures_channel::mpsc::Sender<u8>,
) -> Arc<RTCDataChannel> {
    let mut config: RTCDataChannelInit = RTCDataChannelInit::default();
    config.ordered = Some(false);
    config.max_retransmits = Some(0);
    config.negotiated = Some(true);
    config.id = Some(0);

    let channel = connection
        .create_data_channel("webudp", Some(config))
        .await
        .unwrap();

    channel
        .on_open(Box::new(move || {
            debug!("Data channel ready");
            channel_ready.try_send(1).unwrap();
            Box::pin(async move {})
        }))
        .await;

    channel
}

// TODO: return result
async fn wait_for_ice_complete(connection: &RTCPeerConnection) {
    if connection.ice_gathering_state() == RTCIceGatheringState::Complete {
        return;
    }

    let (mut tx, mut rx) = futures_channel::mpsc::channel(1);

    connection
        .on_ice_gathering_state_change(Box::new(move |state| {
            if state == RTCIceGathererState::Complete {
                tx.try_send(()).unwrap();
            }
            // TODO: handle failure
            // let packet = (*message.data).into();
            // from_peer_message_tx
            //     .unbounded_send((peer_id.clone(), packet))
            //     .unwrap();
            Box::pin(async move {})
        }))
        .await;

    rx.next().await;

    // TODO: state change handler?
    debug!("Ice gathering completed")
}

async fn peer_loop(
    handshake_fut: impl Future<
        Output = Result<(PeerId, Arc<RTCDataChannel>), Box<dyn std::error::Error>>,
    >,
    mut new_peer_tx: UnboundedSender<PeerId>,
    from_peer_message_tx: UnboundedSender<(PeerId, Packet)>,
    mut to_peer_message_rx: UnboundedReceiver<Packet>,
) {
    let (peer_id, data_channel) = handshake_fut.await.unwrap();
    debug!(
        "peer_loop: sending new_peer, data channel state: {:?}",
        data_channel.ready_state()
    );
    new_peer_tx.send(peer_id.clone()).await.unwrap();
    data_channel
        .on_message(Box::new(move |message| {
            let packet = (*message.data).into();
            debug!("received message");
            from_peer_message_tx
                .unbounded_send((peer_id.clone(), packet))
                .unwrap();
            Box::pin(async move {})
        }))
        .await;

    while let Some(message) = to_peer_message_rx.next().await {
        let message = Bytes::from(message);
        debug!("sending message");
        data_channel.send(&message).await.unwrap();
    }

    // TODO: clear on_message?
}
