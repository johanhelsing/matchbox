use futures::{pin_mut, stream::FuturesUnordered, Future, FutureExt, SinkExt, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_util::select;
use js_sys::Reflect;
use serde::Serialize;
use std::{collections::HashMap, pin::Pin};
use uuid::Uuid;
use wasm_bindgen::{prelude::*, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    console::{log_1, log_2},
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit, RtcDataChannelType,
    RtcIceGatheringState, RtcPeerConnection, RtcSdpType, RtcSessionDescriptionInit,
};
use ws_stream_wasm::{WsMessage, WsMeta};

mod messages;
mod signal_peer;

use messages::*;
use signal_peer::*;

type Packet = Box<[u8]>;

#[derive(Debug)]
pub struct WebRtcSocket {
    messages_from_peers: futures_channel::mpsc::UnboundedReceiver<(String, Packet)>,
    new_connected_peers: futures_channel::mpsc::UnboundedReceiver<String>,
    // peer_messages_out: futures_channel::mpsc::UnboundedSender<(String, Packet)>,
    peer_messages_out: futures_channel::mpsc::Sender<(String, Packet)>,
    peers: Vec<String>,
}

impl WebRtcSocket {
    pub fn new(room_url: &str) -> (Self, Pin<Box<dyn Future<Output = ()>>>) {
        let (messages_from_peers_tx, messages_from_peers) = futures_channel::mpsc::unbounded();
        let (new_connected_peers_tx, new_connected_peers) = futures_channel::mpsc::unbounded();
        // let (peer_messages_out_tx, peer_messages_out_rx) =
        //     futures_channel::mpsc::unbounded::<(String, Packet)>();
        let (peer_messages_out_tx, peer_messages_out_rx) =
            futures_channel::mpsc::channel::<(String, Packet)>(32);

        (
            Self {
                messages_from_peers,
                peer_messages_out: peer_messages_out_tx,
                new_connected_peers,
                peers: vec![],
            },
            Box::pin(message_loop(
                room_url.to_string(),
                peer_messages_out_rx,
                new_connected_peers_tx,
                messages_from_peers_tx,
            )),
        )
    }

    // pub async fn wait_for_peers(&mut self, peers: usize) -> Vec<SocketAddr> {
    pub async fn wait_for_peers(&mut self, peers: usize) -> Vec<String> {
        log_1(&"waiting for peers to join".into());
        let mut addrs = vec![];
        while let Some(id) = self.new_connected_peers.next().await {
            addrs.push(id.clone());
            if addrs.len() == peers {
                log_1(&"all peers joined".into());
                self.peers.extend(addrs.clone());
                return addrs;
            }
        }
        panic!("Signal server died")
    }

    pub fn connected_peers(&self) -> Vec<String> {
        self.peers.clone() // TODO: could probably be an iterator or reference instead?
    }

    pub fn receive_messages(&mut self) -> Vec<(String, Packet)> {
        std::iter::repeat_with(|| self.messages_from_peers.try_next())
            // .map_while(|poll| match p { // map_while is nightly-only :(
            .take_while(|p| !p.is_err())
            .map(|p| match p.unwrap() {
                Some((id, packet)) => (id, packet),
                None => todo!("Handle connection closed??"),
            })
            .collect()
    }

    pub fn send(&mut self, packet: Packet, id: String) {
        // log_1(&"sending message on internal channel".into());

        // self.peer_messages_out
        //     .unbounded_send((id, packet))
        //     .expect("send_to failed");

        self.peer_messages_out
            .try_send((id, packet))
            .expect("send_to failed");
    }
}

async fn message_loop(
    room_url: String,
    mut peer_messages_out_rx: futures_channel::mpsc::Receiver<(String, Packet)>,
    // mut peer_messages_out_rx: futures_channel::mpsc::UnboundedReceiver<(String, Packet)>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<String>,
    messages_from_peers_tx: futures_channel::mpsc::UnboundedSender<(String, Packet)>,
) {
    let (_ws, mut wsio) = WsMeta::connect(&room_url, None)
        .await
        .expect("failed to connect to signalling server");

    let (requests_sender, mut requests_receiver) =
        futures_channel::mpsc::unbounded::<PeerRequest>();

    requests_sender
        .unbounded_send(PeerRequest::Uuid(Uuid::new_v4().to_string()))
        .expect("failed to send uuid");

    let mut offer_handshakes = FuturesUnordered::new();
    let mut accept_handshakes = FuturesUnordered::new();
    let mut handshake_signals = HashMap::new();
    let mut data_channels: HashMap<String, RtcDataChannel> = HashMap::new();

    loop {
        let next_signal_event = wsio.next().fuse();
        let next_request = requests_receiver.next().fuse();
        let next_peer_message_out = peer_messages_out_rx.next().fuse();
        // pin_mut!(next_signal_event, next_request, next_peer_message_out);
        pin_mut!(next_signal_event, next_request, next_peer_message_out);

        select! {
            res = offer_handshakes.select_next_some() => {
                log_1(&"select handshakes".into());
                check(&res);
                let peer = res.unwrap();
                data_channels.insert(peer.0.clone(), peer.1.clone());
                log_1(&"Notifying about new peer".into());
                new_connected_peers_tx.unbounded_send(peer.0).expect("send failed");
            },
            res = accept_handshakes.select_next_some() => {
                // TODO: this could be de-duplicated
                log_1(&"select handshakes".into());
                check(&res);
                let peer = res.unwrap();
                data_channels.insert(peer.0.clone(), peer.1.clone());
                log_1(&"Notifying about new peer".into());
                new_connected_peers_tx.unbounded_send(peer.0).expect("send failed");
            },

            message = next_signal_event => {
                log_1(&"select next event".into());
                let message = match message.unwrap() {
                    WsMessage::Text(message) => message,
                    WsMessage::Binary(_) => panic!("binary data from signal server"),
                };

                let message_js: JsValue = message.clone().into();
                web_sys::console::log_1(&message_js);

                let event: PeerEvent = serde_json::from_str(&message)
                    .expect(&format!("couldn't parse peer event {}", message));

                match event {
                    PeerEvent::NewPeer(peer_uuid) => {
                        let (signal_sender, signal_receiver) = futures_channel::mpsc::unbounded();
                        handshake_signals.insert(peer_uuid.clone(), signal_sender);
                        let signal_peer = SignalPeer::new(peer_uuid, requests_sender.clone());
                        offer_handshakes.push(handshake_offer(signal_peer, signal_receiver, messages_from_peers_tx.clone()));
                    }
                    PeerEvent::Signal { sender, data } => {
                        let from_peer_sender = handshake_signals.entry(sender.clone()).or_insert_with(|| {
                            let (from_peer_sender, from_peer_receiver) = futures_channel::mpsc::unbounded();
                            let signal_peer = SignalPeer::new(sender, requests_sender.clone());
                            // We didn't start signalling with this peer, assume we're the accepting part
                            accept_handshakes.push(handshake_accept(signal_peer, from_peer_receiver, messages_from_peers_tx.clone()));
                            from_peer_sender
                        });
                        from_peer_sender.unbounded_send(data)
                            .expect("failed to forward signal to handshaker");
                    }
                }
            }

            request = next_request => {
                log_1(&"select next request".into());
                let request = serde_json::to_string(&request).expect("serializing request");

                let request_js: JsValue = request.clone().into();
                log_2(&"->".into(), &request_js);

                wsio.send(WsMessage::Text(request)).await.expect("request send error");
                log_1(&"sent request".into());
            }

            message = next_peer_message_out => {
                log_1(&"Sending message to peer".into());
                let message = message.unwrap();
                let data_channel = data_channels.get(&message.0).expect("couldn't find data channel for peer");
                data_channel.send_with_u8_array(&message.1).expect("failed to send");
            }
        }
    }
}

#[derive(Debug)]
struct JsError(JsValue);

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl std::error::Error for JsError {}

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

async fn handshake_offer(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    messages_from_peers_tx: UnboundedSender<(String, Packet)>,
) -> Result<(String, RtcDataChannel), Box<dyn std::error::Error>> {
    log_1(&"making offer".into());
    let conn = create_rtc_peer_connection();
    let (channel_ready_tx, mut channel_ready_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(
        conn.clone(),
        messages_from_peers_tx,
        signal_peer.id.clone(),
        channel_ready_tx,
    );
    // create_ice_handler(conn.clone(), signal_peer.clone());

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

    log_1(&"created offer for new peer".into());

    signal_peer.send(PeerSignal::Offer(conn.local_description().unwrap().sdp()));

    let signal = signal_receiver.next().await.ok_or("No more signals :(")?;

    let answer = match signal {
        PeerSignal::Answer(answer) => answer,
        PeerSignal::Offer(_) => panic!("offers shouldn't get here... I think"),
        PeerSignal::IceCandidate(_) => panic!("HELP ICE"),
    };

    let sdp = answer;

    let mut remote_description: RtcSessionDescriptionInit =
        RtcSessionDescriptionInit::new(RtcSdpType::Answer);

    remote_description.sdp(&sdp);

    JsFuture::from(conn.set_remote_description(&remote_description))
        .await
        .efix()?;

    // ice_dance(conn, signal_receiver, signal_peer.clone()).await;
    channel_ready_rx.next().await;

    Ok((signal_peer.id, data_channel))
}

async fn handshake_accept(
    signal_peer: SignalPeer,
    mut signal_receiver: UnboundedReceiver<PeerSignal>,
    messages_from_peers_tx: UnboundedSender<(String, Packet)>,
) -> Result<(String, RtcDataChannel), Box<dyn std::error::Error>> {
    log_1(&"handshake_accept".into());

    let conn = create_rtc_peer_connection();
    let (channel_ready_tx, mut channel_ready_rx) = futures_channel::mpsc::channel(1);
    let data_channel = create_data_channel(
        conn.clone(),
        messages_from_peers_tx,
        signal_peer.id.clone(),
        channel_ready_tx,
    );
    // create_ice_handler(conn.clone(), signal_peer.clone());

    let offer: Option<String>;
    loop {
        match signal_receiver.next().await.ok_or("error")? {
            PeerSignal::Offer(o) => {
                offer = Some(o);
                break;
            }
            _ => {
                log_1(&"ignoring other signal!!!".into());
            }
        }
    }
    let offer = offer.unwrap();
    log_1(&"received offer".into());

    // Set remote description
    {
        let mut remote_description: RtcSessionDescriptionInit =
            RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        let sdp = offer;
        remote_description.sdp(&sdp);
        JsFuture::from(conn.set_remote_description(&remote_description))
            .await
            .expect("failed to set remote description");
        log_1(&"set remote_description from offer".into());
    }

    let answer = JsFuture::from(conn.create_answer())
        .await
        .expect("error creating answer");

    log_1(&"created answer".into());

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

    // ice_dance(conn, signal_receiver, signal_peer.clone()).await;
    channel_ready_rx.next().await;

    Ok((signal_peer.id, data_channel))
}

fn create_rtc_peer_connection() -> RtcPeerConnection {
    #[derive(Serialize)]
    pub struct IceServerConfig {
        pub urls: [String; 1],
    }

    let mut peer_config: RtcConfiguration = RtcConfiguration::new();
    let ice_server_config = IceServerConfig {
        // urls: ["stun:stun.l.google.com:19302".to_string()],
        urls: [
            "stun:stun.johanhelsing.studio:3478".to_string(),
            //"turn:stun.johanhelsing.studio:3478".to_string(),
        ],
    };
    let ice_server_config_list = [ice_server_config];
    peer_config.ice_servers(&JsValue::from_serde(&ice_server_config_list).unwrap());
    let connection = RtcPeerConnection::new_with_configuration(&peer_config).unwrap();
    connection
}

// async fn ice_dance(
//     connection: RtcPeerConnection,
//     mut signal_receiver: UnboundedReceiver<PeerSignal>,
//     signal_peer: SignalPeer,
// ) {
//     while let Some(s) = signal_receiver.next().await {
//         match s {
//             PeerSignal::IceCandidate(ice) => {
//                 // log_2(&"received ice".into(), &ice);
//                 let candidate_init = RtcIceCandidateInit::new(&ice);
//                 let candidate = RtcIceCandidate::new(&candidate_init).unwrap();
//                 JsFuture::from(
//                     connection.add_ice_candidate_with_opt_rtc_ice_candidate(Some(&candidate)),
//                 )
//                 .await
//                 .expect("ice error");
//             }
//             _ => todo! {},
//         }
//     }
// }

// fn create_ice_handler(conn: RtcPeerConnection, signal_peer: SignalPeer) {
//     let conn_clone = conn.clone();
//     let ice_candidate_func: Box<dyn FnMut(RtcPeerConnectionIceEvent)> =
//         Box::new(move |event: RtcPeerConnectionIceEvent| {
//             log_1(&"Found new ice candidate".into());
//             // null candidate represents end-of-candidates.
//             // TODO: do we need to deregister handler?
//             if event.candidate().is_none() {
//                 log_1(&"Found all ice candidates - sending answer".into());
//                 let answer_sdp_string = conn_clone.local_description().unwrap().sdp();
//                 // signal_peer.send(PeerSignal::IceCandidate(answer_sdp_string));
//             }
//         });
//     let ice_candidate_callback = Closure::wrap(ice_candidate_func);
//     conn.set_onicecandidate(Some(ice_candidate_callback.as_ref().unchecked_ref()));
//     ice_candidate_callback.forget();
// }

async fn wait_for_ice_complete(conn: RtcPeerConnection) {
    if conn.ice_gathering_state() == RtcIceGatheringState::Complete {
        log_1(&"Ice already completed".into());
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
    log_1(&"Ice completed".into());
}

fn create_data_channel(
    connection: RtcPeerConnection,
    incoming_tx: futures_channel::mpsc::UnboundedSender<(String, Packet)>,
    peer_id: String,
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

    let peer_id = peer_id.clone();
    let incoming_tx = incoming_tx.clone();
    let channel_clone = channel.clone();
    let channel_onopen_func: Box<dyn FnMut(JsValue)> = Box::new(move |_| {
        log_1(&"Rtc data channel opened :D :D".into());
        // let mut from_client_sender_clone_2 = from_client_sender_clone.clone();
        let peer_id = peer_id.clone();
        let incoming_tx = incoming_tx.clone();
        let channel_onmsg_func: Box<dyn FnMut(MessageEvent)> =
            Box::new(move |event: MessageEvent| {
                // log_2(&"Received a rtc data message".into(), &event);
                if let Ok(arraybuf) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                    let uarray: js_sys::Uint8Array = js_sys::Uint8Array::new(&arraybuf);
                    // log_2(
                    //     &"Receive data of length {}".into(),
                    //     &(uarray.length().into()),
                    // );

                    // TODO: There probably are ways to avoid copying/zeroing here...
                    let mut body = vec![0 as u8; uarray.length() as usize];
                    uarray.copy_to(&mut body[..]);
                    incoming_tx
                        .unbounded_send((peer_id.clone(), body.into_boxed_slice()))
                        .unwrap();
                }
            });
        let channel_onmsg_closure = Closure::wrap(channel_onmsg_func);
        channel_clone.set_onmessage(Some(channel_onmsg_closure.as_ref().unchecked_ref()));
        channel_onmsg_closure.forget();

        channel_ready
            .try_send(1)
            .expect("failed to notify about open connection");
        // channel_clone.send_with_str("Hello from data channel :D:D");
    });
    let channel_onopen_closure = Closure::wrap(channel_onopen_func);
    channel.set_onopen(Some(channel_onopen_closure.as_ref().unchecked_ref()));
    channel_onopen_closure.forget();

    channel
}

// Expect/unwrap is broken in select for some reason :/
fn check(res: &Result<(String, RtcDataChannel), Box<dyn std::error::Error>>) {
    // but doing it inside a typed function works fine
    res.as_ref().expect("handshake failed");
}
