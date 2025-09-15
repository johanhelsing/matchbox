use super::{HandshakeResult, PacketSendError, PeerDataSender, SignallerBuilder, error::JsErrorExt, messages::{PeerEvent, PeerRequest}, BufferedChannel};
use crate::webrtc_socket::{
    ChannelConfig, Messenger, Packet, RtcIceServerConfig, Signaller, error::SignalingError,
    messages::PeerSignal, signal_peer::SignalPeer, socket::create_data_channels_ready_fut,
    PeerBuffered,
};
use async_trait::async_trait;
use futures::{Future, SinkExt, StreamExt};
use futures_channel::mpsc::{Receiver, UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::select;
use js_sys::{Function, Reflect};
use log::{debug, error, info, trace, warn};
use matchbox_protocol::PeerId;
use serde::Serialize;
use std::{pin::Pin, time::Duration};
use std::ops::{Deref, DerefMut};
use wasm_bindgen::{JsCast, JsValue, convert::FromWasmAbi, prelude::*};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Event, MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit, RtcDataChannelType,
    RtcIceCandidateInit, RtcIceGatheringState, RtcPeerConnection, RtcPeerConnectionIceEvent,
    RtcSdpType, RtcSessionDescriptionInit
};
use ws_stream_wasm::{WsMessage, WsMeta, WsStream};
use crate::webrtc_socket::error::PeerError;

pub(crate) struct WasmSignaller {
    websocket_stream: futures::stream::Fuse<WsStream>,
}

#[derive(Debug, Clone)]
pub struct RtcDataChannelWrapper(pub(crate) RtcDataChannel);

unsafe impl Send for RtcDataChannelWrapper {}

unsafe impl Sync for RtcDataChannelWrapper {}

impl Deref for RtcDataChannelWrapper {
    type Target = RtcDataChannel;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RtcDataChannelWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Default)]
pub(crate) struct WasmSignallerBuilder;

#[async_trait(?Send)]
impl SignallerBuilder for WasmSignallerBuilder {
    async fn new_signaller(
        &self,
        mut attempts: Option<u16>,
        room_url: String,
    ) -> Result<Box<dyn Signaller>, SignalingError> {
        let websocket_stream = 'signaling: loop {
            match WsMeta::connect(&room_url, None)
                .await
                .map_err(SignalingError::from)
            {
                Ok((_, wss)) => break wss.fuse(),
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
        Ok(Box::new(WasmSignaller { websocket_stream }))
    }
}

#[async_trait(?Send)]
impl Signaller for WasmSignaller {
    async fn send(&mut self, request: PeerRequest) -> Result<(), SignalingError> {
        let request = serde_json::to_string(&request).expect("serializing request");
        self.websocket_stream
            .send(WsMessage::Text(request))
            .await
            .map_err(SignalingError::from)
    }

    async fn next_message(&mut self) -> Result<PeerEvent, SignalingError> {
        let message = match self.websocket_stream.next().await {
            Some(WsMessage::Text(message)) => Ok(message),
            Some(_) => Err(SignalingError::UnknownFormat),
            None => Err(SignalingError::StreamExhausted),
        }?;
        let message = serde_json::from_str(&message).map_err(|e| {
            error!("failed to deserialize message: {e:?}");
            SignalingError::UnknownFormat
        })?;
        Ok(message)
    }
}

impl PeerDataSender for RtcDataChannelWrapper {
    fn send(&mut self, packet: Packet) -> Result<(), PacketSendError> {
        self.send_with_u8_array(&packet)
            .efix()
            .map_err(|source| PacketSendError { source })
    }
}

#[async_trait(?Send)]
impl BufferedChannel for RtcDataChannelWrapper {
    async fn buffered_amount(&self) -> usize {
        self.0.buffered_amount() as usize
    }
}

pub(crate) struct WasmMessenger;

#[async_trait(?Send)]
impl Messenger for WasmMessenger {
    type DataChannel = RtcDataChannelWrapper;
    type HandshakeMeta = Receiver<()>;

    async fn offer_handshake(
        signal_peer: SignalPeer,
        mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        ice_server_config: &RtcIceServerConfig,
        channel_configs: &[ChannelConfig],
    ) -> Result<HandshakeResult<Self::DataChannel, Self::HandshakeMeta>, PeerError> {
        debug!("making offer");

        let conn = create_rtc_peer_connection(ice_server_config);

        let (data_channel_ready_txs, data_channels_ready_fut) =
            create_data_channels_ready_fut(channel_configs);

        let (peer_disconnected_tx, peer_disconnected_rx) = futures_channel::mpsc::channel(1);

        let data_channels = create_data_channels(
            conn.clone(),
            messages_from_peers_tx,
            signal_peer.id,
            peer_disconnected_tx,
            data_channel_ready_txs,
            channel_configs,
        );

        let peer_buffered = PeerBuffered::new(data_channels.clone().iter().map(|it| {
            let channel: Box<dyn BufferedChannel> = Box::new(it.clone());
            return channel
        }).collect::<Vec<_>>());

        // Create offer
        let offer = JsFuture::from(conn.create_offer()).await.efix().unwrap();
        let offer_sdp = Reflect::get(&offer, &JsValue::from_str("sdp"))
            .efix()
            .unwrap()
            .as_string()
            .expect("");
        let rtc_session_desc_init_dict = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        rtc_session_desc_init_dict.set_sdp(&offer_sdp);
        JsFuture::from(conn.set_local_description(&rtc_session_desc_init_dict))
            .await
            .efix()
            .unwrap();
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
            let signal = match peer_signal_rx.next().await {
                Some(signal) => signal,
                None => {
                    warn!("Signal server connection lost in the middle of a handshake");
                    return Err(PeerError(signal_peer.id, SignalingError::HandshakeFailed));
                }
            };

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
        let remote_description = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        remote_description.set_sdp(&sdp);
        debug!("setting remote description");
        JsFuture::from(conn.set_remote_description(&remote_description))
            .await
            .efix()
            .unwrap();

        complete_handshake(
            signal_peer.clone(),
            conn,
            received_candidates,
            data_channels_ready_fut,
            peer_signal_rx,
        )
        .await;

        Ok(HandshakeResult {
            peer_id: signal_peer.id,
            data_channels,
            metadata: peer_disconnected_rx,
            peer_buffered,
        })
    }

    async fn accept_handshake(
        signal_peer: SignalPeer,
        mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        ice_server_config: &RtcIceServerConfig,
        channel_configs: &[ChannelConfig],
    ) -> Result<HandshakeResult<Self::DataChannel, Self::HandshakeMeta>, PeerError> {
        debug!("handshake_accept");

        let conn = create_rtc_peer_connection(ice_server_config);

        let (data_channel_ready_txs, data_channels_ready_fut) =
            create_data_channels_ready_fut(channel_configs);

        let (peer_disconnected_tx, peer_disconnected_rx) = futures_channel::mpsc::channel(1);

        let data_channels = create_data_channels(
            conn.clone(),
            messages_from_peers_tx,
            signal_peer.id,
            peer_disconnected_tx,
            data_channel_ready_txs,
            channel_configs,
        );

        let peer_buffered = PeerBuffered::new(data_channels.iter().map(|it| {
            let channel: Box<dyn BufferedChannel> = Box::new(it.clone());
            return channel
        }).collect::<Vec<_>>());

        let mut received_candidates = vec![];

        let offer = loop {
            let signal = match peer_signal_rx.next().await {
                Some(signal) => signal,
                None => {
                    warn!("Signal server connection lost in the middle of a handshake");
                    return Err(PeerError(signal_peer.id, SignalingError::HandshakeFailed));
                }
            };

            match signal {
                PeerSignal::Offer(o) => {
                    break o;
                }
                PeerSignal::IceCandidate(candidate) => {
                    debug!("received IceCandidate signal: {candidate:?}");
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
            let remote_description = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
            let sdp = offer;
            remote_description.set_sdp(&sdp);
            JsFuture::from(conn.set_remote_description(&remote_description))
                .await
                .expect("failed to set remote description");
            debug!("set remote_description from offer");
        }

        let answer = JsFuture::from(conn.create_answer())
            .await
            .expect("error creating answer");

        debug!("created answer");

        let session_desc_init = RtcSessionDescriptionInit::new(RtcSdpType::Answer);

        let answer_sdp = Reflect::get(&answer, &JsValue::from_str("sdp"))
            .efix()
            .unwrap()
            .as_string()
            .expect("");

        session_desc_init.set_sdp(&answer_sdp);

        JsFuture::from(conn.set_local_description(&session_desc_init))
            .await
            .efix()
            .unwrap();

        // todo: the point of implementing ice trickle is to avoid this wait...
        // however, for some reason removing this wait causes problems with NAT
        // punching in practice.
        // We should figure out why this is happening.
        wait_for_ice_gathering_complete(conn.clone()).await;

        let answer = PeerSignal::Answer(conn.local_description().unwrap().sdp());
        signal_peer.send(answer);

        complete_handshake(
            signal_peer.clone(),
            conn,
            received_candidates,
            data_channels_ready_fut,
            peer_signal_rx,
        )
        .await;

        Ok(HandshakeResult {
            peer_id: signal_peer.id,
            data_channels,
            metadata: peer_disconnected_rx,
            peer_buffered,
        })
    }

    async fn peer_loop(peer_uuid: PeerId, handshake_meta: Self::HandshakeMeta) -> PeerId {
        let mut peer_loop_finished_rx = handshake_meta;
        peer_loop_finished_rx.next().await;
        peer_uuid
    }
}

async fn complete_handshake(
    signal_peer: SignalPeer,
    conn: RtcPeerConnection,
    received_candidates: Vec<String>,
    mut data_channels_ready_fut: Pin<Box<futures::future::Fuse<impl Future<Output = ()>>>>,
    mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
) {
    let onicecandidate: Box<dyn FnMut(RtcPeerConnectionIceEvent)> = Box::new(
        move |event: RtcPeerConnectionIceEvent| {
            let candidate_json = match event.candidate() {
                Some(candidate) => js_sys::JSON::stringify(&candidate.to_json())
                    .expect("failed to serialize candidate")
                    .as_string()
                    .unwrap(),
                None => {
                    debug!(
                        "Received RtcPeerConnectionIceEvent with no candidate. This means there are no further ice candidates for this session"
                    );
                    "null".to_string()
                }
            };

            debug!("sending IceCandidate signal: {candidate_json:?}");
            signal_peer.send(PeerSignal::IceCandidate(candidate_json));
        },
    );
    let onicecandidate = Closure::wrap(onicecandidate);
    conn.set_onicecandidate(Some(onicecandidate.as_ref().unchecked_ref()));
    // note: we can let rust keep ownership of this closure, since we replace
    // the event handler later in this method when ice is finished

    // handle pending ICE candidates
    for candidate in received_candidates {
        debug!("adding ice candidate {candidate:?}");
        try_add_rtc_ice_candidate(&conn, &candidate).await;
    }

    // select for data channels ready or ice candidates
    debug!("waiting for data channels to open");
    loop {
        select! {
            _ = data_channels_ready_fut => {
                debug!("data channels ready");
                break;
            }
            msg = peer_signal_rx.next() => {
                if let Some(PeerSignal::IceCandidate(candidate)) = msg {
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
            warn!("received ice candidate event after handshake completed, ignoring");
        });
    let onicecandidate = Closure::wrap(onicecandidate);
    conn.set_onicecandidate(Some(onicecandidate.as_ref().unchecked_ref()));
    onicecandidate.forget();

    debug!(
        "handshake completed, ice gathering state: {:?}",
        conn.ice_gathering_state()
    );
}

async fn try_add_rtc_ice_candidate(connection: &RtcPeerConnection, candidate_string: &str) {
    let parsed_candidate = match js_sys::JSON::parse(candidate_string) {
        Ok(c) => c,
        Err(err) => {
            warn!("failed to parse ice candidate json, ignoring: {err:?}");
            return;
        }
    };

    let candidate_init = if parsed_candidate.is_null() {
        debug!("Received null ice candidate, this means there are no further ice candidates");
        None
    } else {
        let candidate = RtcIceCandidateInit::from(parsed_candidate);
        debug!("ice candidate received: {candidate:?}");
        Some(candidate)
    };

    JsFuture::from(
        connection.add_ice_candidate_with_opt_rtc_ice_candidate_init(candidate_init.as_ref()),
    )
    .await
    .expect("failed to add ice candidate");
}

fn create_rtc_peer_connection(ice_server_config: &RtcIceServerConfig) -> RtcPeerConnection {
    #[derive(Serialize)]
    struct IceServerConfig {
        urls: Vec<String>,
        username: String,
        credential: String,
    }

    let peer_config = RtcConfiguration::new();
    let ice_server_config = IceServerConfig {
        urls: ice_server_config.urls.clone(),
        username: ice_server_config.username.clone().unwrap_or_default(),
        credential: ice_server_config.credential.clone().unwrap_or_default(),
    };
    let ice_server_config_list = [ice_server_config];
    peer_config.set_ice_servers(&serde_wasm_bindgen::to_value(&ice_server_config_list).unwrap());
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
    peer_disconnected_tx: futures_channel::mpsc::Sender<()>,
    mut data_channel_ready_txs: Vec<futures_channel::mpsc::Sender<()>>,
    channel_config: &[ChannelConfig],
) -> Vec<RtcDataChannelWrapper> {
    channel_config
        .iter()
        .enumerate()
        .map(|(i, channel)| {
            create_data_channel(
                connection.clone(),
                incoming_tx.get_mut(i).unwrap().clone(),
                peer_id,
                peer_disconnected_tx.clone(),
                data_channel_ready_txs.pop().unwrap(),
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
    peer_disconnected_tx: futures_channel::mpsc::Sender<()>,
    mut channel_open: futures_channel::mpsc::Sender<()>,
    channel_config: &ChannelConfig,
    channel_id: usize,
) -> RtcDataChannelWrapper {
    let data_channel_config = data_channel_config(channel_config);
    data_channel_config.set_id(channel_id as u16);

    let channel = connection.create_data_channel_with_data_channel_dict(
        &format!("matchbox_socket_{channel_id}"),
        &data_channel_config,
    );

    channel.set_binary_type(RtcDataChannelType::Arraybuffer);

    leaking_channel_event_handler(
        |f| channel.set_onopen(f),
        move |_: JsValue| {
            info!("data channel open: {channel_id}");
            channel_open
                .try_send(())
                .expect("failed to notify about open connection");
        },
    );

    leaking_channel_event_handler(
        |f| channel.set_onmessage(f),
        move |event: MessageEvent| {
            trace!("data channel message received {event:?}");
            if let Ok(arraybuf) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                let uarray = js_sys::Uint8Array::new(&arraybuf);
                let body = uarray.to_vec();

                if let Err(e) = incoming_tx.unbounded_send((peer_id, body.into_boxed_slice())) {
                    // should only happen if the socket is dropped, or we are out of memory
                    warn!("failed to notify about data channel message: {e:?}");
                }
            }
        },
    );

    leaking_channel_event_handler(
        |f| channel.set_onerror(f),
        move |event: Event| {
            // Convert Event into a JsValue
            let js_val: JsValue = event.into();

            // Try to get the `error` property
            match Reflect::get(&js_val, &JsValue::from_str("error")) {
                Ok(err) => {
                    error!("DataChannel error: {:?}", err);
                }
                Err(_) => {
                    error!("DataChannel error: (no error field found)");
                }
            }
        },
    );

    leaking_channel_event_handler(
        |f| channel.set_onclose(f),
        move |event: Event| {
            warn!("data channel closed: {event:?}");
            if let Err(err) = peer_disconnected_tx.clone().try_send(()) {
                // should only happen if the socket is dropped, or we are out of memory
                warn!("failed to notify about data channel closing: {err:?}");
            }
        },
    );

    RtcDataChannelWrapper(channel)
}

/// Note that this function leaks some memory because the rust closure is dropped but still needs to
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
    let data_channel_config = RtcDataChannelInit::new();

    data_channel_config.set_ordered(channel_config.ordered);
    data_channel_config.set_negotiated(true);

    if let Some(n) = channel_config.max_retransmits {
        data_channel_config.set_max_retransmits(n);
    }

    data_channel_config
}
