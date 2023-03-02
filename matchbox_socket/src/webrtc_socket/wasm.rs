use crate::webrtc_socket::{
    error::SignallingError,
    messages::{PeerEvent, PeerId, PeerRequest, PeerSignal},
    signal_peer::SignalPeer,
    socket::create_data_channels_ready_fut,
    ChannelConfig, MessageLoopChannels, Messenger, Packet, PeerState, Signaller,
    WebRtcSocketConfig, KEEP_ALIVE_INTERVAL,
};
use async_trait::async_trait;
use futures::{
    stream::{Fuse, FuturesUnordered},
    FutureExt, SinkExt, StreamExt,
};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::select;
use js_sys::{Function, Reflect};
use log::{debug, error, warn};
use serde::Serialize;
use std::{collections::HashMap, time::Duration};
use wasm_bindgen::{convert::FromWasmAbi, prelude::*, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Event, MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit, RtcDataChannelType,
    RtcIceCandidateInit, RtcIceGatheringState, RtcPeerConnection, RtcPeerConnectionIceEvent,
    RtcSdpType, RtcSessionDescriptionInit,
};
use ws_stream_wasm::{WsMessage, WsMeta, WsStream};

pub(crate) struct WasmSignaller {
    websocket_stream: Fuse<WsStream>,
}

#[async_trait(?Send)]
impl Signaller for WasmSignaller {
    async fn new(mut attempts: Option<u16>, room_url: &str) -> Result<Self, SignallingError> {
        let websocket_stream = 'signalling: loop {
            match WsMeta::connect(room_url, None)
                .await
                .map_err(SignallingError::from)
            {
                Ok((_, wss)) => break wss.fuse(),
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
            .send(WsMessage::Text(request))
            .await
            .map_err(SignallingError::from)
    }

    async fn next_message(&mut self) -> Result<String, SignallingError> {
        match self.websocket_stream.next().await {
            Some(WsMessage::Text(message)) => Ok(message),
            Some(_) => Err(SignallingError::UnknownFormat),
            None => Err(SignallingError::StreamExhausted),
        }
    }
}

pub(crate) struct WasmMessenger;

#[async_trait(?Send)]
impl Messenger for WasmMessenger {
    async fn message_loop(id: PeerId, config: WebRtcSocketConfig, channels: MessageLoopChannels) {
        let MessageLoopChannels {
            requests_sender,
            mut events_receiver,
            mut peer_messages_out_rx,
            peer_state_change_tx,
            messages_from_peers_tx,
        } = channels;
        debug!("Entering WebRtcSocket message loop");

        requests_sender
            .unbounded_send(PeerRequest::Uuid(id))
            .expect("failed to send uuid");

        let mut handshakes = FuturesUnordered::new();
        let mut handshake_signals = HashMap::new();
        let mut data_channels: HashMap<PeerId, Vec<RtcDataChannel>> = HashMap::new();

        let mut timeout = Delay::new(Duration::from_millis(KEEP_ALIVE_INTERVAL)).fuse();

        loop {
            let mut next_peer_messages_out: FuturesUnordered<_> = peer_messages_out_rx
                .iter_mut()
                .enumerate()
                .map(|(index, r)| async move { (index, r.next().await) })
                .collect();

            let mut next_peer_message_out = next_peer_messages_out.next().fuse();

            select! {
                _ = &mut timeout => {
                    requests_sender.unbounded_send(PeerRequest::KeepAlive).expect("send failed");
                    timeout = Delay::new(Duration::from_millis(KEEP_ALIVE_INTERVAL)).fuse();
                }

                res = handshakes.select_next_some() => {
                    check(&res);
                    let (peer, channels) = res.unwrap();
                    data_channels.insert(peer.clone(), channels);
                    debug!("Notifying about new peer");
                    peer_state_change_tx.unbounded_send((peer, PeerState::Connected)).expect("send failed");
                },

                message = events_receiver.next() => {
                    if let Some(event) = message {
                        debug!("{:?}", event);

                        match event {
                            PeerEvent::NewPeer(peer_uuid) => {
                                let (signal_sender, signal_receiver) = futures_channel::mpsc::unbounded();
                                handshake_signals.insert(peer_uuid.clone(), signal_sender);
                                let signal_peer = SignalPeer::new(peer_uuid, requests_sender.clone());
                                handshakes.push(handshake_offer(signal_peer, signal_receiver, peer_state_change_tx.clone(), messages_from_peers_tx.clone(), &config).boxed_local());
                            }
                            PeerEvent::PeerLeft(peer_uuid) => {
                                peer_state_change_tx.unbounded_send((peer_uuid, PeerState::Disconnected)).expect("fail to send disconnected peer");
                            }
                            PeerEvent::Signal { sender, data } => {
                                let from_peer_sender = handshake_signals.entry(sender.clone()).or_insert_with(|| {
                                    let (from_peer_sender, from_peer_receiver) = futures_channel::mpsc::unbounded();
                                    let signal_peer = SignalPeer::new(sender.clone(), requests_sender.clone());
                                    // We didn't start signalling with this peer, assume we're the accepting part
                                    handshakes.push(handshake_accept(signal_peer, from_peer_receiver, peer_state_change_tx.clone(), messages_from_peers_tx.clone(), &config).boxed_local());
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

                message = next_peer_message_out => {
                    match message {
                        Some((channel_index, Some((peer, packet)))) => {
                            let data_channel = data_channels.get(&peer)
                                .expect("couldn't find data channel for peer")
                                .get(channel_index)
                                .unwrap_or_else(|| panic!("couldn't find data channel with index {channel_index}"));

                            if let Err(err) = data_channel.send_with_u8_array(&packet) {
                                // This likely means the other peer disconnected
                                // todo: we should probably remove the data channel object in this case
                                // and try reconnecting. For now we will just stop panicking.
                                error!("Failed to send: {err:?}");
                            }
                        },
                        Some((_, None)) | None => {
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
}

async fn handshake_offer(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    peer_state_tx: UnboundedSender<(PeerId, PeerState)>,
    messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
    config: &WebRtcSocketConfig,
) -> Result<(PeerId, Vec<RtcDataChannel>), Box<dyn std::error::Error>> {
    debug!("making offer");

    let conn = create_rtc_peer_connection(config);
    let (channel_ready_tx, mut wait_for_channels) = create_data_channels_ready_fut(config);

    let data_channels = create_data_channels(
        conn.clone(),
        messages_from_peers_tx,
        signal_peer.id.clone(),
        peer_state_tx,
        channel_ready_tx,
        &config.channels,
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

    // todo: the point of implementing ice trickle is to avoid this wait...
    // however, for some reason removing this wait causes problems with NAT
    // punching in practice.
    // We should figure out why this is happening.
    wait_for_ice_gathering_complete(conn.clone()).await;

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
    debug!("waiting for data channels to open");
    loop {
        select! {
            _ = wait_for_channels => {
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

    debug!(
        "handshake_offer completed, ice gathering state: {:?}",
        conn.ice_gathering_state()
    );

    Ok((signal_peer.id, data_channels))
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
    peer_state_tx: UnboundedSender<(PeerId, PeerState)>,
    messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
    config: &WebRtcSocketConfig,
) -> Result<(PeerId, Vec<RtcDataChannel>), Box<dyn std::error::Error>> {
    debug!("handshake_accept");

    let conn = create_rtc_peer_connection(config);
    let (channel_ready_tx, mut wait_for_channels) = create_data_channels_ready_fut(config);
    let data_channels = create_data_channels(
        conn.clone(),
        messages_from_peers_tx,
        signal_peer.id.clone(),
        peer_state_tx,
        channel_ready_tx,
        &config.channels,
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

    // todo: the point of implementing ice trickle is to avoid this wait...
    // however, for some reason removing this wait causes problems with NAT
    // punching in practice.
    // We should figure out why this is happening.
    wait_for_ice_gathering_complete(conn.clone()).await;

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
            _ = wait_for_channels => {
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

    debug!(
        "handshake_accept completed, ice gathering state: {:?}",
        conn.ice_gathering_state()
    );

    Ok((signal_peer.id, data_channels))
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
    // NOTE: Not attaching a handler on this event causes FF to disconnect after a couple of seconds
    // see: https://github.com/johanhelsing/matchbox/issues/36
    connection
        .set_oniceconnectionstatechange(Some(oniceconnectionstatechange.as_ref().unchecked_ref()));
    oniceconnectionstatechange.forget();

    connection
}

async fn wait_for_ice_gathering_complete(conn: RtcPeerConnection) {
    if conn.ice_gathering_state() == RtcIceGatheringState::Complete {
        debug!("Ice gathering already completed");
        return;
    }

    let (mut tx, mut rx) = futures_channel::mpsc::channel(1);

    let conn_clone = conn.clone();
    let onstatechange: Box<dyn FnMut(JsValue)> = Box::new(move |_| {
        if conn_clone.ice_gathering_state() == RtcIceGatheringState::Complete {
            tx.try_send(()).unwrap();
        }
    });

    let onstatechange = Closure::wrap(onstatechange);

    conn.set_onicegatheringstatechange(Some(onstatechange.as_ref().unchecked_ref()));

    rx.next().await;

    conn.set_onicegatheringstatechange(None);
    debug!("Ice gathering completed");
}

fn create_data_channels(
    connection: RtcPeerConnection,
    mut incoming_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
    peer_id: PeerId,
    peer_state_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
    mut channel_ready: Vec<futures_channel::mpsc::Sender<u8>>,
    channel_config: &[ChannelConfig],
) -> Vec<RtcDataChannel> {
    channel_config
        .iter()
        .enumerate()
        .map(|(i, channel)| {
            create_data_channel(
                connection.clone(),
                incoming_tx.get_mut(i).unwrap().clone(),
                peer_id.clone(),
                peer_state_tx.clone(),
                channel_ready.pop().unwrap(),
                channel,
                i,
            )
        })
        .collect()
}

fn create_data_channel(
    connection: RtcPeerConnection,
    incoming_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
    peer_id: PeerId,
    peer_state_tx: futures_channel::mpsc::UnboundedSender<(PeerId, PeerState)>,
    mut channel_open: futures_channel::mpsc::Sender<u8>,
    channel_config: &ChannelConfig,
    channel_id: usize,
) -> RtcDataChannel {
    let mut data_channel_config = data_channel_config(channel_config);
    data_channel_config.id(channel_id as u16);

    let channel = connection.create_data_channel_with_data_channel_dict(
        &format!("matchbox_socket_{channel_id}"),
        &data_channel_config,
    );

    channel.set_binary_type(RtcDataChannelType::Arraybuffer);

    leaking_channel_event_handler(
        |f| channel.set_onopen(f),
        move |_: JsValue| {
            debug!("Rtc data channel opened :D :D");
            channel_open
                .try_send(1)
                .expect("failed to notify about open connection");
        },
    );

    let peer_id_1 = peer_id.clone();
    leaking_channel_event_handler(
        |f| channel.set_onmessage(f),
        move |event: MessageEvent| {
            debug!("incoming {:?}", event);
            if let Ok(arraybuf) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                let uarray = js_sys::Uint8Array::new(&arraybuf);
                let body = uarray.to_vec();

                incoming_tx
                    .unbounded_send((peer_id_1.clone(), body.into_boxed_slice()))
                    .unwrap();
            }
        },
    );

    leaking_channel_event_handler(
        |f| channel.set_onerror(f),
        move |event: Event| {
            error!("Error in data channel: {:?}", event);
        },
    );

    leaking_channel_event_handler(
        |f| channel.set_onclose(f),
        move |event: Event| {
            warn!("Channel closed: {:?}", event);
            if let Err(err) =
                peer_state_tx.unbounded_send((peer_id.clone(), PeerState::Disconnected))
            {
                warn!("failed to notify about channel disconnecting: {err:?}");
            }
        },
    );

    channel
}

/// Note that this fuction leaks some memory because the rust closure is dropped but still needs to
/// be accessed by javascript of the browser
///
/// See also: https://rustwasm.github.io/wasm-bindgen/api/wasm_bindgen/closure/struct.Closure.html#method.into_js_value
fn leaking_channel_event_handler<T: FromWasmAbi + 'static>(
    mut setter: impl FnMut(Option<&Function>),
    handler: impl FnMut(T) + 'static,
) {
    let closure: Closure<dyn FnMut(T)> = Closure::wrap(Box::new(handler));

    setter(Some(closure.as_ref().unchecked_ref()));

    closure.forget();
}

fn data_channel_config(channel_config: &ChannelConfig) -> RtcDataChannelInit {
    let mut data_channel_config = RtcDataChannelInit::new();

    data_channel_config.ordered(channel_config.ordered);
    data_channel_config.negotiated(true);

    if let Some(n) = channel_config.max_retransmits {
        data_channel_config.max_retransmits(n);
    }

    data_channel_config
}

// Expect/unwrap is broken in select for some reason :/
fn check(res: &Result<(PeerId, Vec<RtcDataChannel>), Box<dyn std::error::Error>>) {
    // but doing it inside a typed function works fine
    res.as_ref().expect("handshake failed");
}

// The below is just to wrap Result<JsValue, JsValue> into something sensible-ish

trait JsErrorExt<T> {
    fn efix(self) -> Result<T, JsError>;
}

impl<T> JsErrorExt<T> for Result<T, JsValue> {
    fn efix(self) -> Result<T, JsError> {
        self.map_err(JsError)
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
