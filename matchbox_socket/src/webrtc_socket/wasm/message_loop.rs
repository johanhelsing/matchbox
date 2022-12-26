use futures::FutureExt;
use futures::{stream::FuturesUnordered, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::select;
use js_sys::Reflect;
use log::{debug, error, warn};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use wasm_bindgen::{prelude::*, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit, RtcDataChannelType,
    RtcIceCandidateInit, RtcPeerConnection, RtcPeerConnectionIceEvent, RtcSdpType,
    RtcSessionDescriptionInit,
};

use crate::webrtc_socket::{
    messages::{PeerEvent, PeerId, PeerRequest, PeerSignal},
    signal_peer::SignalPeer,
    Packet, WebRtcSocketConfig, DATA_CHANNEL_ID, KEEP_ALIVE_INTERVAL,
};

pub async fn message_loop(
    id: PeerId,
    config: WebRtcSocketConfig,
    requests_sender: futures_channel::mpsc::UnboundedSender<PeerRequest>,
    mut events_receiver: futures_channel::mpsc::UnboundedReceiver<PeerEvent>,
    mut peer_messages_out_rx: futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
) {
    debug!("Entering WebRtcSocket message loop");

    requests_sender
        .unbounded_send(PeerRequest::Uuid(id))
        .expect("failed to send uuid");

    let mut offer_handshakes = FuturesUnordered::new();
    let mut accept_handshakes = FuturesUnordered::new();
    let mut handshake_signals = HashMap::new();
    let mut data_channels: HashMap<PeerId, RtcDataChannel> = HashMap::new();

    let mut timeout = Delay::new(Duration::from_millis(KEEP_ALIVE_INTERVAL)).fuse();

    loop {
        select! {
            _ = &mut timeout => {
                requests_sender.unbounded_send(PeerRequest::KeepAlive).expect("send failed");
                timeout = Delay::new(Duration::from_millis(KEEP_ALIVE_INTERVAL)).fuse();
            }

            res = offer_handshakes.select_next_some() => {
                check(&res);
                let peer = res.unwrap();
                data_channels.insert(peer.0.clone(), peer.1.clone());
                debug!("Notifying about new peer");
                new_connected_peers_tx.unbounded_send(peer.0).expect("send failed");
            },
            res = accept_handshakes.select_next_some() => {
                // TODO: this could be de-duplicated
                check(&res);
                let peer = res.unwrap();
                data_channels.insert(peer.0.clone(), peer.1.clone());
                debug!("Notifying about new peer");
                new_connected_peers_tx.unbounded_send(peer.0).expect("send failed");
            },

            message = events_receiver.next() => {
                if let Some(event) = message {
                    debug!("{:?}", event);

                    match event {
                        PeerEvent::NewPeer(peer_uuid) => {
                            let (signal_sender, signal_receiver) = futures_channel::mpsc::unbounded();
                            handshake_signals.insert(peer_uuid.clone(), signal_sender);
                            let signal_peer = SignalPeer::new(peer_uuid, requests_sender.clone());
                            offer_handshakes.push(handshake_offer(signal_peer, signal_receiver, messages_from_peers_tx.clone(), &config));
                        }
                        PeerEvent::Signal { sender, data } => {
                            let from_peer_sender = handshake_signals.entry(sender.clone()).or_insert_with(|| {
                                let (from_peer_sender, from_peer_receiver) = futures_channel::mpsc::unbounded();
                                let signal_peer = SignalPeer::new(sender.clone(), requests_sender.clone());
                                // We didn't start signalling with this peer, assume we're the accepting part
                                accept_handshakes.push(handshake_accept(signal_peer, from_peer_receiver, messages_from_peers_tx.clone(), &config));
                                from_peer_sender
                            });
                            if let Err(e) = from_peer_sender.unbounded_send(data) {
                                if e.is_disconnected() && data_channels.contains_key(&sender) {
                                    // when the handshake finishes, it currently drops the receiver.
                                    // ideally, we should keep this channel open and process additional ice candidates,
                                    // but currently we don't.

                                    // If this happens, however, it means that the handshake is already is done,
                                    // so it should probably be nothing to worry about
                                    warn!("ignoring signal from peer after handshake completed: {e:?}");
                                } else {
                                    error!("failed to forward signal to handshaker: {e:?}");
                                }
                            }
                        }
                    }
                } else {
                    error!("Disconnected from signalling server!");
                    break;
                }
            }

            message = peer_messages_out_rx.next() => {
                match message {
                    Some(message) => {
                        let data_channel = data_channels.get(&message.0).expect("couldn't find data channel for peer");
                        if let Err(err) = data_channel.send_with_u8_array(&message.1) {
                            // This likely means the other peer disconnected
                            // todo: we should probably remove the data channel object in this case
                            // and try reconnecting. For now we will just stop panicking.
                            error!("Failed to send: {err:?}");
                        }
                    },
                    None => {
                        // Receiver end of outgoing message channel closed,
                        // which most likely means the socket was dropped.
                        // There could probably be cleaner ways to handle this,
                        // but for now, just exit cleanly.
                        debug!("Outgoing message queue closed");
                        break;
                    }
                }
            }

            complete => break
        }
    }
    debug!("Message loop finished");
}

async fn handshake_offer(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    messages_from_peers_tx: UnboundedSender<(PeerId, Packet)>,
    config: &WebRtcSocketConfig,
) -> Result<(PeerId, RtcDataChannel), Box<dyn std::error::Error>> {
    debug!("making offer");

    let conn = create_rtc_peer_connection(config);
    let (channel_open_tx, mut channel_open_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(
        conn.clone(),
        messages_from_peers_tx,
        signal_peer.id.clone(),
        channel_open_tx,
    );

    // Create offer
    let offer = JsFuture::from(conn.create_offer()).await.efix()?;
    let offer_sdp = Reflect::get(&offer, &JsValue::from_str("sdp"))
        .efix()?
        .as_string()
        .ok_or("")?;
    let mut rtc_session_desc_init_dict = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
    let offer_description = rtc_session_desc_init_dict.sdp(&offer_sdp);
    JsFuture::from(conn.set_local_description(offer_description))
        .await
        .efix()?;
    debug!("created offer for new peer");
    signal_peer.send(PeerSignal::Offer(conn.local_description().unwrap().sdp()));

    let mut received_candidates = vec![];

    // Wait for answer
    let sdp = loop {
        let signal = signal_receiver
            .next()
            .await
            .ok_or("Signal server connection lost in the middle of a handshake")?;

        match signal {
            PeerSignal::Answer(answer) => break answer,
            PeerSignal::IceCandidate(candidate) => {
                debug!(
                    "offerer: received an ice candidate while waiting for answer: {candidate:?}"
                );
                received_candidates.push(candidate);
            }
            _ => {
                warn!("ignoring unexpected signal: {signal:?}");
            }
        };
    };

    // Set remote description
    let mut remote_description = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
    remote_description.sdp(&sdp);
    debug!("setting remote description");
    JsFuture::from(conn.set_remote_description(&remote_description))
        .await
        .efix()?;

    // send ICE candidates to remote peer
    let signal_peer_ice = signal_peer.clone();
    let onicecandidate: Box<dyn FnMut(RtcPeerConnectionIceEvent)> = Box::new(
        move |event: RtcPeerConnectionIceEvent| {
            let candidate_json = match event.candidate() {
                Some(candidate) => js_sys::JSON::stringify(&candidate.to_json())
                    .expect("failed to serialize candidate")
                    .as_string()
                    .unwrap(),
                None => {
                    debug!("Received RtcPeerConnectionIceEvent with no candidate. This means there are no further ice candidates for this session");
                    "null".to_string()
                }
            };

            debug!("sending IceCandidate signal: {candidate_json:?}");
            signal_peer_ice.send(PeerSignal::IceCandidate(candidate_json));
        },
    );
    let onicecandidate = Closure::wrap(onicecandidate);
    conn.set_onicecandidate(Some(onicecandidate.as_ref().unchecked_ref()));
    // note: we can let rust keep ownership of this closure, since we replace
    // the event handler later in this method when ice is finished

    // handle pending ICE candidates
    for candidate in received_candidates {
        debug!("offerer: adding ice candidate {candidate:?}");
        try_add_rtc_ice_candidate(&conn, &candidate).await;
    }

    // select for channel ready or ice candidates
    debug!("waiting for data channel to open");
    loop {
        select! {
            _ = channel_open_rx.next() => {
                debug!("channel ready");
                break;
            }
            msg = signal_receiver.next() => {
                if let Some(PeerSignal::IceCandidate(candidate)) = msg {
                    debug!("offerer: received ice candidate {candidate:?}");
                    try_add_rtc_ice_candidate(&conn, &candidate).await;
                }
            }
        };
    }

    // stop listening for ICE candidates
    // TODO: we should support getting new ICE candidates even after connecting,
    //       since it's possible to return to the ice gathering state
    // See: <https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection/iceGatheringState>
    let onicecandidate: Box<dyn FnMut(RtcPeerConnectionIceEvent)> =
        Box::new(move |_event: RtcPeerConnectionIceEvent| {
            warn!("received ice candidate event after handshake completed");
        });
    let onicecandidate = Closure::wrap(onicecandidate);
    conn.set_onicecandidate(Some(onicecandidate.as_ref().unchecked_ref()));
    onicecandidate.forget();

    debug!("Ice completed: {:?}", conn.ice_gathering_state());

    Ok((signal_peer.id, data_channel))
}

async fn try_add_rtc_ice_candidate(connection: &RtcPeerConnection, candidate_string: &str) {
    let parsed_candidate = match js_sys::JSON::parse(candidate_string) {
        Ok(c) => c,
        Err(err) => {
            error!("failed to parse candidate json: {err:?}");
            return;
        }
    };

    let candidate_init = if parsed_candidate.is_null() {
        debug!("Received null ice candidate, this means there are no further ice candidates");
        None
    } else {
        Some(RtcIceCandidateInit::from(parsed_candidate))
    };

    JsFuture::from(
        connection.add_ice_candidate_with_opt_rtc_ice_candidate_init(candidate_init.as_ref()),
    )
    .await
    .expect("failed to add ice candidate");
}

async fn handshake_accept(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    messages_from_peers_tx: UnboundedSender<(PeerId, Packet)>,
    config: &WebRtcSocketConfig,
) -> Result<(PeerId, RtcDataChannel), Box<dyn std::error::Error>> {
    debug!("handshake_accept");

    let conn = create_rtc_peer_connection(config);
    let (channel_ready_tx, mut channel_ready_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(
        conn.clone(),
        messages_from_peers_tx,
        signal_peer.id.clone(),
        channel_ready_tx,
    );

    let mut received_candidates = vec![];

    let offer = loop {
        let signal = signal_receiver
            .next()
            .await
            .ok_or("Signal server connection lost in the middle of a handshake")?;

        match signal {
            PeerSignal::Offer(o) => {
                break o;
            }
            PeerSignal::IceCandidate(candidate) => {
                debug!("got an IceCandidate signal! {}", candidate);
                received_candidates.push(candidate);
            }
            _ => {
                warn!("ignoring unexpected signal: {signal:?}");
            }
        }
    };
    debug!("received offer");

    // Set remote description
    {
        let mut remote_description = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        let sdp = offer;
        remote_description.sdp(&sdp);
        JsFuture::from(conn.set_remote_description(&remote_description))
            .await
            .expect("failed to set remote description");
        debug!("set remote_description from offer");
    }

    let answer = JsFuture::from(conn.create_answer())
        .await
        .expect("error creating answer");

    debug!("created answer");

    let mut session_desc_init = RtcSessionDescriptionInit::new(RtcSdpType::Answer);

    let answer_sdp = Reflect::get(&answer, &JsValue::from_str("sdp"))
        .efix()?
        .as_string()
        .ok_or("")?;

    let answer_description = session_desc_init.sdp(&answer_sdp);

    JsFuture::from(conn.set_local_description(answer_description))
        .await
        .efix()?;

    let answer = PeerSignal::Answer(conn.local_description().unwrap().sdp());
    signal_peer.send(answer);

    // send ICE candidates to remote peer
    let signal_peer_ice = signal_peer.clone();
    // todo: exactly the same as offer, dedup?
    let onicecandidate: Box<dyn FnMut(RtcPeerConnectionIceEvent)> = Box::new(
        move |event: RtcPeerConnectionIceEvent| {
            let candidate_json = match event.candidate() {
                Some(candidate) => js_sys::JSON::stringify(&candidate.to_json())
                    .expect("failed to serialize candidate")
                    .as_string()
                    .unwrap(),
                None => {
                    debug!("Received RtcPeerConnectionIceEvent with no candidate. This means there are no further ice candidates for this session");
                    "null".to_string()
                }
            };

            debug!("sending IceCandidate signal: {candidate_json:?}");
            signal_peer_ice.send(PeerSignal::IceCandidate(candidate_json));
        },
    );
    let onicecandidate = Closure::wrap(onicecandidate);
    conn.set_onicecandidate(Some(onicecandidate.as_ref().unchecked_ref()));
    // note: we can let rust keep ownership of this closure, since we replace
    // the event handler later in this method when ice is finished

    // handle pending ICE candidates
    for candidate in received_candidates {
        debug!("accepter: adding ice candidate {candidate:?}");
        try_add_rtc_ice_candidate(&conn, &candidate).await;
    }

    // select for channel ready or ice candidates
    debug!("waiting for data channel to open");
    loop {
        select! {
            _ = channel_ready_rx.next() => {
                debug!("channel ready");
                break;
            }
            msg = signal_receiver.next() => {
                if let Some(PeerSignal::IceCandidate(candidate)) = msg {
                    debug!("accepter: received ice candidate: {candidate:?}");
                    try_add_rtc_ice_candidate(&conn, &candidate).await;
                }
            }
        };
    }

    // stop listening for ICE candidates
    // TODO: we should support getting new ICE candidates even after connecting,
    //       since it's possible to return to the ice gathering state
    // See: <https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection/iceGatheringState>
    let onicecandidate: Box<dyn FnMut(RtcPeerConnectionIceEvent)> =
        Box::new(move |_event: RtcPeerConnectionIceEvent| {
            warn!("received ice candidate event after handshake completed");
        });
    let onicecandidate = Closure::wrap(onicecandidate);
    conn.set_onicecandidate(Some(onicecandidate.as_ref().unchecked_ref()));
    onicecandidate.forget();

    debug!("Ice completed: {:?}", conn.ice_gathering_state());

    Ok((signal_peer.id, data_channel))
}

fn create_rtc_peer_connection(config: &WebRtcSocketConfig) -> RtcPeerConnection {
    #[derive(Serialize)]
    struct IceServerConfig {
        urls: Vec<String>,
        username: String,
        credential: String,
    }

    let mut peer_config = RtcConfiguration::new();
    let ice_server = &config.ice_server;
    let ice_server_config = IceServerConfig {
        urls: ice_server.urls.clone(),
        username: ice_server.username.clone().unwrap_or_default(),
        credential: ice_server.credential.clone().unwrap_or_default(),
    };
    let ice_server_config_list = [ice_server_config];
    peer_config.ice_servers(&serde_wasm_bindgen::to_value(&ice_server_config_list).unwrap());
    let connection = RtcPeerConnection::new_with_configuration(&peer_config).unwrap();

    let connection_1 = connection.clone();
    let oniceconnectionstatechange: Box<dyn FnMut(_)> = Box::new(move |_event: JsValue| {
        debug!(
            "ice connection state changed: {:?}",
            connection_1.ice_connection_state()
        );
    });
    let oniceconnectionstatechange = Closure::wrap(oniceconnectionstatechange);
    connection
        .set_oniceconnectionstatechange(Some(oniceconnectionstatechange.as_ref().unchecked_ref()));
    oniceconnectionstatechange.forget();

    connection
}

fn create_data_channel(
    connection: RtcPeerConnection,
    incoming_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
    peer_id: PeerId,
    mut channel_open: futures_channel::mpsc::Sender<u8>,
) -> RtcDataChannel {
    let mut data_channel_config = RtcDataChannelInit::new();
    data_channel_config.ordered(false);
    data_channel_config.max_retransmits(0);
    data_channel_config.negotiated(true);
    data_channel_config.id(DATA_CHANNEL_ID);

    let channel =
        connection.create_data_channel_with_data_channel_dict("webudp", &data_channel_config);

    channel.set_binary_type(RtcDataChannelType::Arraybuffer);

    // onmsg callback
    let channel_onmsg_func: Box<dyn FnMut(MessageEvent)> = Box::new(move |event: MessageEvent| {
        debug!("incoming {:?}", event);
        if let Ok(arraybuf) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
            let uarray = js_sys::Uint8Array::new(&arraybuf);
            let body = uarray.to_vec();
            incoming_tx
                .unbounded_send((peer_id.clone(), body.into_boxed_slice()))
                .unwrap();
        }
    });
    let channel_onmsg_closure = Closure::wrap(channel_onmsg_func);
    channel.set_onmessage(Some(channel_onmsg_closure.as_ref().unchecked_ref()));
    channel_onmsg_closure.forget();

    // onopen callback
    let channel_onopen_func: Box<dyn FnMut(JsValue)> = Box::new(move |_| {
        debug!("Rtc data channel opened :D");
        channel_open
            .try_send(1)
            .expect("failed to notify about opened data channel");
    });
    let channel_onopen_closure = Closure::wrap(channel_onopen_func);
    channel.set_onopen(Some(channel_onopen_closure.as_ref().unchecked_ref()));
    channel_onopen_closure.forget();

    // onclose callback
    let on_close_func: Box<dyn FnMut(JsValue)> = Box::new(move |_| {
        error!("data channel closed, todo: try reconnect if appropriate/possible");
    });
    let on_close_closure = Closure::wrap(on_close_func);
    channel.set_onclose(Some(on_close_closure.as_ref().unchecked_ref()));
    on_close_closure.forget();

    channel
}

// Expect/unwrap is broken in select for some reason :/
fn check(res: &Result<(PeerId, RtcDataChannel), Box<dyn std::error::Error>>) {
    // but doing it inside a typed function works fine
    res.as_ref().expect("handshake failed");
}

// The bellow is just to wrap Result<JsValue, JsValue> into something sensible-ish

trait JsErrorExt<T> {
    fn efix(self) -> Result<T, Box<dyn std::error::Error>>;
}

impl<T> JsErrorExt<T> for Result<T, JsValue> {
    fn efix(self) -> Result<T, Box<dyn std::error::Error>> {
        self.map_err(|e| {
            let e: Box<dyn std::error::Error> = Box::new(JsError(e));
            e
        })
    }
}

#[derive(Debug)]
struct JsError(JsValue);

impl std::error::Error for JsError {}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
