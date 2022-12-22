use std::pin::Pin;

use futures::{Future, FutureExt, StreamExt};
use futures_util::select;
use log::debug;

mod messages;
mod signal_peer;

const KEEP_ALIVE_INTERVAL: u64 = 10_000;

// TODO: maybe use cfg-if to make this slightly tidier
#[cfg(not(target_arch = "wasm32"))]
mod native {
    mod message_loop;
    mod signalling_loop;
    pub use message_loop::*;
    pub use signalling_loop::*;
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    mod message_loop;
    mod signalling_loop;
    pub use message_loop::*;
    pub use signalling_loop::*;
}

#[cfg(not(target_arch = "wasm32"))]
use native::*;
#[cfg(target_arch = "wasm32")]
use wasm::*;

use messages::*;
use uuid::Uuid;

type Packet = Box<[u8]>;

/// General configuration options for a WebRtc connection
///
/// See [`WebRtcSocket::new_with_config`]
#[derive(Debug)]
pub struct WebRtcSocketConfig {
    /// The url for the room to connect to
    ///
    /// This is a websocket url, starting with `ws://` or `ws://` followed by
    /// the hostname and path to a matchbox server, followed by a room id and
    /// optional query parameters.
    ///
    /// e.g.: `wss://matchbox.example.com/your_game`
    ///
    /// or: `wss://matchbox.example.com/your_game?next=2`
    ///
    /// The last form will pair player in the order they connect.
    pub room_url: String,
    /// Configuration for the (single) ICE server
    pub ice_server: RtcIceServerConfig,
    /// Configuration for the (single) data channel
    pub data_channel: RtcDataChannelConfig,
}

/// Configuration options for an RTC data channel.
/// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel>
#[derive(Debug)]
pub struct RtcDataChannelConfig {
    pub ordered: bool,
    pub max_retransmits: u16,
}

/// Configuration options for an ICE server connection.
/// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCIceServer#example>
#[derive(Debug)]
pub struct RtcIceServerConfig {
    /// An ICE server instance can have several URLs
    pub urls: Vec<String>,
    /// A username for authentication with the ICE server
    ///
    /// See: <https://developer.mozilla.org/en-US/docs/Web/API/RTCIceServer/username>
    pub username: Option<String>,
    /// A password or token when authenticating with a turn server
    ///
    /// See: <https://developer.mozilla.org/en-US/docs/Web/API/RTCIceServer/credential>
    pub credential: Option<String>,
}

impl Default for RtcDataChannelConfig {
    fn default() -> Self {
        Self { 
            ordered: false, 
            max_retransmits: 0 
        }
    }
}

impl Default for RtcIceServerConfig {
    fn default() -> Self {
        Self {
            urls: vec![
                "stun:stun.l.google.com:19302".to_string(),
                //"stun:stun.johanhelsing.studio:3478".to_string(),
                //"turn:stun.johanhelsing.studio:3478".to_string(),
            ],
            username: Default::default(),
            credential: Default::default(),
        }
    }
}

impl Default for WebRtcSocketConfig {
    fn default() -> Self {
        WebRtcSocketConfig {
            room_url: "ws://localhost:3536/example_room".to_string(),
            ice_server: RtcIceServerConfig::default(),
            data_channel: RtcDataChannelConfig::default(),
        }
    }
}

/// Contains the interface end of a full-mesh web rtc connection
///
/// Used to send and receive messages from other peers
#[derive(Debug)]
pub struct WebRtcSocket {
    messages_from_peers: futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>,
    new_connected_peers: futures_channel::mpsc::UnboundedReceiver<PeerId>,
    peer_messages_out: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
    peers: Vec<PeerId>,
    id: PeerId,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type MessageLoopFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
// TODO: figure out if it's possible to implement Send in wasm as well
#[cfg(target_arch = "wasm32")]
pub(crate) type MessageLoopFuture = Pin<Box<dyn Future<Output = ()>>>;

impl WebRtcSocket {
    /// Create a new connection to the given room
    ///
    /// See [`WebRtcSocketConfig::room_url`] for details on the room url.
    ///
    /// The returned future should be awaited in order for messages to be sent and received.
    #[must_use]
    pub fn new<T: Into<String>>(room_url: T) -> (Self, MessageLoopFuture) {
        WebRtcSocket::new_with_config(WebRtcSocketConfig {
            room_url: room_url.into(),
            ..Default::default()
        })
    }

    /// Create a new connection with the given [`WebRtcSocketConfig`]
    ///
    /// The returned future should be awaited in order for messages to be sent and received.
    #[must_use]
    pub fn new_with_config(config: WebRtcSocketConfig) -> (Self, MessageLoopFuture) {
        let (messages_from_peers_tx, messages_from_peers) = futures_channel::mpsc::unbounded();
        let (new_connected_peers_tx, new_connected_peers) = futures_channel::mpsc::unbounded();
        let (peer_messages_out_tx, peer_messages_out_rx) =
            futures_channel::mpsc::unbounded::<(PeerId, Packet)>();

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
                config,
                id,
                peer_messages_out_rx,
                new_connected_peers_tx,
                messages_from_peers_tx,
            )),
        )
    }

    /// Returns a future that resolves when the given number of peers have connected
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

    /// Check if new peers have connected and if so add them as peers
    pub fn accept_new_connections(&mut self) -> Vec<PeerId> {
        let mut ids = Vec::new();
        while let Ok(Some(id)) = self.new_connected_peers.try_next() {
            self.peers.push(id.clone());
            ids.push(id);
        }
        ids
    }

    /// Returns a Vec of the ids of the connected peers
    pub fn connected_peers(&self) -> Vec<PeerId> {
        self.peers.clone() // TODO: could probably be an iterator or reference instead?
    }

    /// Call this where you want to handle new received messages
    ///
    /// messages are removed from the socked when called
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

    /// Send a packet to the given peer
    pub fn send<T: Into<PeerId>>(&mut self, packet: Packet, id: T) {
        self.peer_messages_out
            .unbounded_send((id.into(), packet))
            .expect("send_to failed");
    }

    /// Returns the id of this peer
    pub fn id(&self) -> &PeerId {
        &self.id
    }
}

async fn run_socket(
    config: WebRtcSocketConfig,
    id: PeerId,
    peer_messages_out_rx: futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>,
) {
    debug!("Starting WebRtcSocket message loop");

    let (requests_sender, requests_receiver) = futures_channel::mpsc::unbounded::<PeerRequest>();
    let (events_sender, events_receiver) = futures_channel::mpsc::unbounded::<PeerEvent>();

    let signalling_loop_fut =
        signalling_loop(config.room_url.clone(), requests_receiver, events_sender);

    let message_loop_fut = message_loop(
        id,
        config,
        requests_sender,
        events_receiver,
        peer_messages_out_rx,
        new_connected_peers_tx,
        messages_from_peers_tx,
    );

    let mut message_loop_done = Box::pin(message_loop_fut.fuse());
    let mut signalling_loop_done = Box::pin(signalling_loop_fut.fuse());
    loop {
        select! {
            _ = message_loop_done => {
                debug!("Message loop completed");
                break;
            }

            _ = signalling_loop_done => {
                debug!("Signalling loop completed");
                // todo!{"reconnect?"}
            }

            complete => break
        }
    }
}
