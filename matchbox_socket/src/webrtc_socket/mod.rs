use std::pin::Pin;

use futures::{Future, FutureExt, StreamExt};
use futures_util::select;
use log::debug;

mod messages;
mod signal_peer;
#[cfg(target_arch = "wasm32")]
mod wasm_message_loop;
#[cfg(target_arch = "wasm32")]
mod wasm_signalling_loop;

use messages::*;
use uuid::Uuid;
#[cfg(target_arch = "wasm32")]
use wasm_message_loop::*;
#[cfg(target_arch = "wasm32")]
use wasm_signalling_loop::*;

type Packet = Box<[u8]>;

#[derive(Debug)]
pub struct WebRtcSocket {
    messages_from_peers: futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>,
    new_connected_peers: futures_channel::mpsc::UnboundedReceiver<PeerId>,
    peer_messages_out: futures_channel::mpsc::Sender<(PeerId, Packet)>,
    peers: Vec<PeerId>,
    id: PeerId,
}

impl WebRtcSocket {
    #[must_use]
    pub fn new<T: Into<String>>(room_url: T) -> (Self, Pin<Box<dyn Future<Output = ()>>>) {
        let (messages_from_peers_tx, messages_from_peers) = futures_channel::mpsc::unbounded();
        let (new_connected_peers_tx, new_connected_peers) = futures_channel::mpsc::unbounded();
        let (peer_messages_out_tx, peer_messages_out_rx) =
            futures_channel::mpsc::channel::<(PeerId, Packet)>(32);

        // Would perhaps be smarter to let signalling server decide this...
        let id = Uuid::new_v4().to_string();

        (
            Self {
                id: id.clone(),
                messages_from_peers,
                peer_messages_out: peer_messages_out_tx,
                new_connected_peers,
                peers: vec![],
            },
            Box::pin(run_socket(
                room_url.into(),
                id,
                peer_messages_out_rx,
                new_connected_peers_tx,
                messages_from_peers_tx,
            )),
        )
    }

    pub async fn wait_for_peers(&mut self, peers: usize) -> Vec<PeerId> {
        debug!("waiting for peers to join");
        let mut addrs = vec![];
        while let Some(id) = self.new_connected_peers.next().await {
            addrs.push(id.clone());
            if addrs.len() == peers {
                debug!("all peers joined");
                self.peers.extend(addrs.clone());
                return addrs;
            }
        }
        panic!("Signal server died")
    }

    pub fn accept_new_connections(&mut self) -> Vec<PeerId> {
        let mut ids = Vec::new();
        while let Ok(Some(id)) = self.new_connected_peers.try_next() {
            self.peers.push(id.clone());
            ids.push(id);
        }
        ids
    }

    pub fn connected_peers(&self) -> Vec<PeerId> {
        self.peers.clone() // TODO: could probably be an iterator or reference instead?
    }

    pub fn receive(&mut self) -> Vec<(PeerId, Packet)> {
        std::iter::repeat_with(|| self.messages_from_peers.try_next())
            // .map_while(|poll| match p { // map_while is nightly-only :(
            .take_while(|p| !p.is_err())
            .map(|p| match p.unwrap() {
                Some((id, packet)) => (id, packet),
                None => todo!("Handle connection closed??"),
            })
            .collect()
    }

    pub fn send<T: Into<PeerId>>(&mut self, packet: Packet, id: T) {
        self.peer_messages_out
            .try_send((id.into(), packet))
            .expect("send_to failed");
    }

    pub fn id(&self) -> &PeerId {
        &self.id
    }
}

async fn run_socket(
    room_url: String,
    id: PeerId,
    peer_messages_out_rx: futures_channel::mpsc::Receiver<(PeerId, Packet)>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
) {
    debug!("Starting WebRtcSocket message loop");

    let (requests_sender, requests_receiver) = futures_channel::mpsc::unbounded::<PeerRequest>();
    let (events_sender, events_receiver) = futures_channel::mpsc::unbounded::<PeerEvent>();

    let message_loop_fut = message_loop(
        id,
        requests_sender,
        events_receiver,
        peer_messages_out_rx,
        new_connected_peers_tx,
        messages_from_peers_tx,
    );

    let signalling_loop_fut = signalling_loop(room_url, requests_receiver, events_sender);

    let mut message_loop_done = Box::pin(message_loop_fut.fuse());
    let mut signalling_loop_done = Box::pin(signalling_loop_fut.fuse());
    loop {
        select! {
            _ = message_loop_done => {
                debug!("Message loop completed");
                break;
            }

            _ = signalling_loop_done => {
                debug!("Message loop completed");
                // todo!{"reconnect?"}
            }

            complete => break
        }
    }
}
