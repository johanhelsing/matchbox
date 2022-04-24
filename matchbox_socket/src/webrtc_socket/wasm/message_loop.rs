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
    RtcIceGatheringState, RtcPeerConnection, RtcSdpType, RtcSessionDescriptionInit,
};

use crate::webrtc_socket::KEEP_ALIVE_INTERVAL;
use crate::webrtc_socket::{
    messages::{PeerEvent, PeerId, PeerRequest, PeerSignal},
    signal_peer::SignalPeer,
    Packet, WebRtcSocketConfig,
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
                                let signal_peer = SignalPeer::new(sender, requests_sender.clone());
                                // We didn't start signalling with this peer, assume we're the accepting part
                                accept_handshakes.push(handshake_accept(signal_peer, from_peer_receiver, messages_from_peers_tx.clone(), &config));
                                from_peer_sender
                            });
                            from_peer_sender.unbounded_send(data)
                                .expect("failed to forward signal to handshaker");
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
    let conn = create_rtc_peer_connection(config.ice_server.urls.clone());
    let (channel_ready_tx, mut channel_ready_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(
        conn.clone(),
        messages_from_peers_tx,
        signal_peer.id.clone(),
        channel_ready_tx,
    );

    let offer = JsFuture::from(conn.create_offer()).await.efix()?;

    let offer_sdp = Reflect::get(&offer, &JsValue::from_str("sdp"))
        .efix()?
        .as_string()
        .ok_or("")?;

    let mut rtc_session_desc_init_dict: RtcSessionDescriptionInit =
        RtcSessionDescriptionInit::new(RtcSdpType::Offer);

    let offer_description = rtc_session_desc_init_dict.sdp(&offer_sdp);

    JsFuture::from(conn.set_local_description(offer_description))
        .await
        .efix()?;

    wait_for_ice_complete(conn.clone()).await;

    debug!("created offer for new peer");

    signal_peer.send(PeerSignal::Offer(conn.local_description().unwrap().sdp()));

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

    let mut remote_description: RtcSessionDescriptionInit =
        RtcSessionDescriptionInit::new(RtcSdpType::Answer);

    remote_description.sdp(&sdp);

    debug!("setting remote description");
    JsFuture::from(conn.set_remote_description(&remote_description))
        .await
        .efix()?;

    debug!("waiting for data channel to open");
    channel_ready_rx.next().await;

    Ok((signal_peer.id, data_channel))
}

async fn handshake_accept(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    messages_from_peers_tx: UnboundedSender<(PeerId, Packet)>,
    config: &WebRtcSocketConfig,
) -> Result<(PeerId, RtcDataChannel), Box<dyn std::error::Error>> {
    debug!("handshake_accept");

    let conn = create_rtc_peer_connection(config.ice_server.urls.clone());
    let (channel_ready_tx, mut channel_ready_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(
        conn.clone(),
        messages_from_peers_tx,
        signal_peer.id.clone(),
        channel_ready_tx,
    );

    let offer: Option<String>;
    loop {
        match signal_receiver.next().await.ok_or("error")? {
            PeerSignal::Offer(o) => {
                offer = Some(o);
                break;
            }
            _ => {
                warn!("ignoring other signal!!!");
            }
        }
    }
    let offer = offer.unwrap();
    debug!("received offer");

    // Set remote description
    {
        let mut remote_description: RtcSessionDescriptionInit =
            RtcSessionDescriptionInit::new(RtcSdpType::Offer);
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

    let mut session_desc_init: RtcSessionDescriptionInit =
        RtcSessionDescriptionInit::new(RtcSdpType::Answer);

    let answer_sdp = Reflect::get(&answer, &JsValue::from_str("sdp"))
        .efix()?
        .as_string()
        .ok_or("")?;

    let answer_description = session_desc_init.sdp(&answer_sdp);

    JsFuture::from(conn.set_local_description(answer_description))
        .await
        .efix()?;

    wait_for_ice_complete(conn.clone()).await;

    let answer = PeerSignal::Answer(conn.local_description().unwrap().sdp());
    signal_peer.send(answer);

    debug!("waiting for data channel to open");
    channel_ready_rx.next().await;

    Ok((signal_peer.id, data_channel))
}

fn create_rtc_peer_connection(ice_server_urls: Vec<String>) -> RtcPeerConnection {
    #[derive(Serialize)]
    pub struct IceServerConfig {
        pub urls: Vec<String>,
    }

    let mut peer_config: RtcConfiguration = RtcConfiguration::new();
    let ice_server_config = IceServerConfig {
        urls: ice_server_urls,
    };
    let ice_server_config_list = [ice_server_config];
    peer_config.ice_servers(&JsValue::from_serde(&ice_server_config_list).unwrap());
    RtcPeerConnection::new_with_configuration(&peer_config).unwrap()
}

async fn wait_for_ice_complete(conn: RtcPeerConnection) {
    if conn.ice_gathering_state() == RtcIceGatheringState::Complete {
        debug!("Ice already completed");
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
    debug!("Ice completed");
}

fn create_data_channel(
    connection: RtcPeerConnection,
    incoming_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
    peer_id: PeerId,
    mut channel_ready: futures_channel::mpsc::Sender<u8>,
) -> RtcDataChannel {
    let mut data_channel_config: RtcDataChannelInit = RtcDataChannelInit::new();
    data_channel_config.ordered(false);
    data_channel_config.max_retransmits(0);
    data_channel_config.negotiated(true);
    data_channel_config.id(0);

    let channel: RtcDataChannel =
        connection.create_data_channel_with_data_channel_dict("webudp", &data_channel_config);
    channel.set_binary_type(RtcDataChannelType::Arraybuffer);

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

    let channel_onopen_func: Box<dyn FnMut(JsValue)> = Box::new(move |_| {
        debug!("Rtc data channel opened :D :D");
        channel_ready
            .try_send(1)
            .expect("failed to notify about open connection");
    });
    let channel_onopen_closure = Closure::wrap(channel_onopen_func);
    channel.set_onopen(Some(channel_onopen_closure.as_ref().unchecked_ref()));
    channel_onopen_closure.forget();

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
