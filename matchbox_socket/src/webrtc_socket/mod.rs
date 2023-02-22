use crate::Error;
use futures::{future::Fuse, stream::FusedStream, Future, FutureExt, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_util::select;
use log::{debug, error};
use messages::*;
use std::pin::Pin;
use uuid::Uuid;
pub(crate) mod error;
mod messages;
mod signal_peer;

/// The duration, in milliseconds, to send "Keep Alive" requests
const KEEP_ALIVE_INTERVAL: u64 = 10_000;

/// The raw format of data being sent and received.
type Packet = Box<[u8]>;

// TODO: maybe use cfg-if to make this slightly tidier
#[cfg(not(target_arch = "wasm32"))]
mod native {
    mod message_loop;
    mod signalling_loop;
    pub use message_loop::*;
    pub use signalling_loop::*;
}
#[cfg(not(target_arch = "wasm32"))]
use native::*;

#[cfg(target_arch = "wasm32")]
mod wasm {
    mod message_loop;
    mod signalling_loop;
    pub use message_loop::*;
    pub use signalling_loop::*;
}
#[cfg(target_arch = "wasm32")]
use wasm::*;

/// General configuration options for a WebRtc connection.
///
/// See [`WebRtcSocket::new_with_config`]
#[derive(Debug)]
pub struct WebRtcSocketConfig {
    /// The url for the room to connect to
    ///
    /// This is a websocket url, starting with `ws://` or `wss://` followed by
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
    /// Configuration for one or multiple reliable or unreliable data channels
    pub channels: Vec<ChannelConfig>,
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

/// Configuration options for a data channel
/// See also: https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel
#[derive(Debug)]
pub struct ChannelConfig {
    /// Whether messages sent on the channel are guaranteed to arrive in order
    /// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/ordered>
    pub ordered: bool,
    /// Maximum number of retransmit attempts of a message before giving up
    /// See also: <https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/maxRetransmits>
    pub max_retransmits: Option<u16>,
}

impl ChannelConfig {
    /// Messages sent via an unreliable channel may arrive in any order or not at all, but arrive as
    /// quickly as possible
    pub fn unreliable() -> Self {
        ChannelConfig {
            ordered: false,
            max_retransmits: Some(0),
        }
    }

    /// Messages sent via a reliable channel are guaranteed to arrive in order and will be resent
    /// until they arrive
    pub fn reliable() -> Self {
        ChannelConfig {
            ordered: true,
            max_retransmits: None,
        }
    }
}

impl Default for WebRtcSocketConfig {
    fn default() -> Self {
        WebRtcSocketConfig {
            room_url: "ws://localhost:3536/example_room".to_string(),
            ice_server: RtcIceServerConfig::default(),
            channels: vec![ChannelConfig::unreliable()],
        }
    }
}

impl Default for RtcIceServerConfig {
    fn default() -> Self {
        Self {
            urls: vec![
                "stun:stun.l.google.com:19302".to_string(),
                "stun:stun1.l.google.com:19302".to_string(),
                "stun:stun2.l.google.com:19302".to_string(),
                "stun:stun3.l.google.com:19302".to_string(),
                "stun:stun4.l.google.com:19302".to_string(),
            ],
            username: Default::default(),
            credential: Default::default(),
        }
    }
}

/// Contains the interface end of a full-mesh web rtc connection
///
/// Used to send and receive messages from other peers
#[derive(Debug)]
pub struct WebRtcSocket {
    messages_from_peers: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    new_connected_peers: futures_channel::mpsc::UnboundedReceiver<PeerId>,
    disconnected_peers: futures_channel::mpsc::UnboundedReceiver<PeerId>,
    peer_messages_out: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
    peers: Vec<PeerId>,
    id: PeerId,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;
// TODO: figure out if it's possible to implement Send in wasm as well
#[cfg(target_arch = "wasm32")]
pub(crate) type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>>>>;

impl WebRtcSocket {
    /// Create a new connection to the given room with a single unreliable data channel
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
        if config.channels.is_empty() {
            panic!("You need to configure at least one channel in WebRtcSocketConfig");
        }

        let (messages_from_peers_tx, messages_from_peers) = new_senders_and_receivers(&config);
        let (new_connected_peers_tx, new_connected_peers) = futures_channel::mpsc::unbounded();
        let (disconnected_peers_tx, disconnected_peers) = futures_channel::mpsc::unbounded();
        let (peer_messages_out_tx, peer_messages_out_rx) = new_senders_and_receivers(&config);

        // Would perhaps be smarter to let signalling server decide this...
        let id = Uuid::new_v4().to_string();

        (
            Self {
                id: id.clone(),
                messages_from_peers,
                peer_messages_out: peer_messages_out_tx,
                new_connected_peers,
                disconnected_peers,
                peers: vec![],
            },
            Box::pin(run_socket(
                config,
                id,
                peer_messages_out_rx,
                new_connected_peers_tx,
                disconnected_peers_tx,
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

    /// Check for peer disconnections and return a Vec of ids of disconnected peers.
    ///
    /// See also: [`WebRtcSocket::connected_peers`]
    pub fn disconnected_peers(&mut self) -> Vec<PeerId> {
        let mut ids = vec![];
        // Collect all disconnected peers
        while let Ok(Some(id)) = self.disconnected_peers.try_next() {
            if let Some(index) = self.peers.iter().position(|x| x == &id) {
                self.peers.remove(index);
            }
            ids.push(id);
        }
        // If the channel dropped or becomes terminated, flush all peers
        if self.disconnected_peers.is_terminated() {
            ids.extend(self.peers.clone());
            self.peers.clear();
        }
        ids
    }

    /// Returns a Vec of the ids of the connected peers.
    ///
    /// See also: [`WebRtcSocket::disconnected_peers`]
    pub fn connected_peers(&self) -> Vec<PeerId> {
        self.peers.clone() // TODO: could probably be an iterator or reference instead?
    }

    /// Call this where you want to handle new received messages from the default channel (with
    /// index 0) which will be the only channel if you didn't configure any explicitly
    ///
    /// messages are removed from the socket when called
    ///
    /// See also: [`WebRtcSocket::receive_on_channel`]
    pub fn receive(&mut self) -> Vec<(PeerId, Packet)> {
        self.receive_on_channel(0)
    }

    /// Call this where you want to handle new received messages from a specific channel as
    /// configured in [`WebRtcSocketConfig::channels`]. The index of a channel is its index in
    /// the vec [`WebRtcSocketConfig::channels`] as you configured it before (or 0 for the
    /// default channel if you use the default configuration).
    ///
    /// messages are removed from the socket when called   
    pub fn receive_on_channel(&mut self, index: usize) -> Vec<(PeerId, Packet)> {
        std::iter::repeat_with(|| {
            self.messages_from_peers
                .get_mut(index)
                .unwrap_or_else(|| panic!("No data channel with index {}", index))
                .try_next()
        })
        // .map_while(|poll| match p { // map_while is nightly-only :(
        .take_while(|p| !p.is_err())
        .map(|p| match p.unwrap() {
            Some((peer_id, packet)) => (peer_id, packet),
            None => todo!("Handle connection closed??"),
        })
        .collect()
    }

    /// Send a packet to the given peer on the default channel (with index 0) which will be the only
    /// channel if you didn't configure any explicitly
    ///
    /// See also [`WebRtcSocket::send_on_channel`]
    pub fn send<T: Into<PeerId>>(&mut self, packet: Packet, id: T) {
        self.send_on_channel(packet, id, 0);
    }

    /// Send a packet to the given peer on a specific channel as configured in
    /// [`WebRtcSocketConfig::channels`].
    ///
    /// The index of a channel is its index in the vec [`WebRtcSocketConfig::channels`] as you
    /// configured it before (or 0 for the default channel if you use the default
    /// configuration).
    pub fn send_on_channel<T: Into<PeerId>>(&mut self, packet: Packet, id: T, index: usize) {
        self.peer_messages_out
            .get(index)
            .unwrap_or_else(|| panic!("No data channel with index {}", index))
            .unbounded_send((id.into(), packet))
            .expect("send_to failed");
    }

    /// Send a packet to all connected peers on a specific channel as configured in
    /// [`WebRtcSocketConfig::channels`].
    ///
    /// The index of a channel is its index in the vec [`WebRtcSocketConfig::channels`] on socket
    /// creation.
    pub fn broadcast_on_channel(&mut self, packet: Packet, index: usize) {
        let sender = self
            .peer_messages_out
            .get(index)
            .unwrap_or_else(|| panic!("No data channel with index {}", index));

        for peer_id in self.connected_peers() {
            sender
                .unbounded_send((peer_id, packet.clone()))
                .expect("send_to failed");
        }
    }

    /// Returns the id of this peer
    pub fn id(&self) -> &PeerId {
        &self.id
    }
}

/// All the channels needed for the messaging loop.
pub struct MessageLoopChannels {
    requests_sender: futures_channel::mpsc::UnboundedSender<PeerRequest>,
    events_receiver: futures_channel::mpsc::UnboundedReceiver<PeerEvent>,
    peer_messages_out_rx: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    disconnected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
}

async fn run_socket(
    config: WebRtcSocketConfig,
    id: PeerId,
    peer_messages_out_rx: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    new_connected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    disconnected_peers_tx: futures_channel::mpsc::UnboundedSender<PeerId>,
    messages_from_peers_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
) -> Result<(), Error> {
    debug!("Starting WebRtcSocket message loop");

    let (requests_sender, requests_receiver) = futures_channel::mpsc::unbounded::<PeerRequest>();
    let (events_sender, events_receiver) = futures_channel::mpsc::unbounded::<PeerEvent>();

    let signalling_loop_fut =
        signalling_loop(config.room_url.clone(), requests_receiver, events_sender);

    let channels = MessageLoopChannels {
        requests_sender,
        events_receiver,
        peer_messages_out_rx,
        new_connected_peers_tx,
        disconnected_peers_tx,
        messages_from_peers_tx,
    };
    let message_loop_fut = message_loop(id, config, channels);

    let mut message_loop_done = Box::pin(message_loop_fut.fuse());
    let mut signalling_loop_done = Box::pin(signalling_loop_fut.fuse());
    loop {
        select! {
            _ = message_loop_done => {
                debug!("Message loop completed");
                break;
            }

            sigloop = signalling_loop_done => {
                match sigloop {
                    Ok(()) => debug!("Signalling loop completed"),
                    Err(e) => {
                        // TODO: Reconnect X attempts if configured to reconnect.
                        error!("{e:?}");
                        return Err(Error::from(e));
                    },
                }
            }

            complete => break
        }
    }
    Ok(())
}

pub(crate) fn new_senders_and_receivers<T>(
    config: &WebRtcSocketConfig,
) -> (Vec<UnboundedSender<T>>, Vec<UnboundedReceiver<T>>) {
    (0..config.channels.len())
        .map(|_| futures_channel::mpsc::unbounded())
        .unzip()
}

fn create_data_channels_ready_fut(
    config: &WebRtcSocketConfig,
) -> (
    Vec<futures_channel::mpsc::Sender<u8>>,
    Pin<Box<Fuse<impl Future<Output = ()>>>>,
) {
    let (senders, receivers) = (0..config.channels.len())
        .map(|_| futures_channel::mpsc::channel(1))
        .unzip();

    (senders, Box::pin(wait_for_ready(receivers).fuse()))
}

async fn wait_for_ready(channel_ready_rx: Vec<futures_channel::mpsc::Receiver<u8>>) {
    for mut receiver in channel_ready_rx {
        if receiver.next().await.is_none() {
            panic!("Sender closed before channel was ready");
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{Error, WebRtcSocket};

    #[futures_test::test]
    async fn unreachable_server() {
        // .invalid is a reserved tld for testing and documentation
        let (_socket, fut) = WebRtcSocket::new("wss://example.invalid");

        let result = fut.await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Signalling(_)));
    }
}
