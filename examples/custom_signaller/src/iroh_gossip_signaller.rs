use std::{collections::BTreeMap, time::Duration};

use anyhow::Context;
use futures::FutureExt;
use iroh::{protocol::Router, Endpoint, PublicKey};
use iroh_gossip::{
    net::{Event, Gossip, GossipEvent, GossipReceiver, GossipSender, Message, GOSSIP_ALPN},
    proto::TopicId,
};
use matchbox_socket::{
    async_trait::async_trait, PeerEvent, PeerId, PeerRequest, PeerSignal, SignalingError,
    Signaller, SignallerBuilder,
};
use n0_future::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};
use web_time::Instant;

use crate::{
    direct_message::{send_direct_message, DirectMessageProtocol, DIRECT_MESSAGE_ALPN},
    get_timestamp,
};

const GOSSIP_TOPIC_ID: TopicId = TopicId::from_bytes(*b"__matchbox_example_iroh_gossip__");

#[derive(Debug, Clone)]
pub struct IrohGossipSignallerBuilder {
    router: Router,
    gossip: Gossip,
    endpoint: Endpoint,
    matchbox_id: PeerId,
    iroh_id: PublicKey,
    direct_message_recv: async_broadcast::InactiveReceiver<(PublicKey, PeerEvent)>,
}

impl IrohGossipSignallerBuilder {
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        info!("Shutting down IrohGossipSignallerBuilder");
        self.router.shutdown().await?;
        Ok(())
    }

    pub fn get_matchbox_id(&self) -> PeerId {
        self.matchbox_id
    }

    pub async fn new() -> anyhow::Result<Self> {
        info!("Creating new IrohGossipSignallerBuilder");
        let endpoint = Endpoint::builder()
            .discovery_n0()
            .alpns(vec![DIRECT_MESSAGE_ALPN.to_vec(), GOSSIP_ALPN.to_vec()])
            .bind()
            .await?;
        let iroh_id = endpoint.node_id();
        let matchbox_id = PeerId(uuid::Uuid::new_v4());
        warn!(
            r#"
|----------------------------------------------------------------
|
|        NODE IDENTIFIERS:
|
|        Iroh ID: {iroh_id}
|        Matchbox ID: {matchbox_id}
|
|        Join room with:
|
|            cargo run  -- "{iroh_id}"
|
|        Or visit:
|
|            http://127.0.0.1:1334/#{iroh_id}
|
|
|----------------------------------------------------------------
        "#
        );
        let gossip = Gossip::builder().spawn(endpoint.clone()).await?;
        let (mut direct_message_send, mut direct_message_recv) = async_broadcast::broadcast(2048);
        direct_message_send.set_overflow(true);
        direct_message_recv.set_overflow(true);
        let direct_message_recv = direct_message_recv.deactivate();

        let router = Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(
                DIRECT_MESSAGE_ALPN,
                DirectMessageProtocol(direct_message_send),
            )
            .spawn()
            .await?;
        Ok(Self {
            router,
            gossip,
            endpoint,
            matchbox_id,
            iroh_id,
            direct_message_recv,
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl SignallerBuilder for IrohGossipSignallerBuilder {
    async fn new_signaller(
        &self,
        _attempts: Option<u16>,
        room_url: String,
    ) -> Result<Box<dyn Signaller>, SignalingError> {
        let room_pubkey: Option<PublicKey> = if room_url.is_empty() {
            None
        } else {
            Some(room_url.parse().map_err(to_user_error)?)
        };
        for i in 0.._attempts.unwrap_or(3) {
            match self.try_new_signaller(room_pubkey).await {
                Ok(signaller) => {
                    return Ok(Box::new(signaller));
                }
                Err(e) => {
                    warn!("Failed to connect to gossip: {e:#?}");
                    if i == _attempts.unwrap_or(1) - 1 {
                        return Err(to_user_error(e));
                    }
                }
            }
        }
        unreachable!()
    }
}

fn to_user_error<E: std::fmt::Debug>(e: E) -> SignalingError {
    SignalingError::UserImplementationError(format!("{e:#?}"))
}

impl IrohGossipSignallerBuilder {
    async fn try_new_signaller(
        &self,
        room_pubkey: Option<PublicKey>,
    ) -> anyhow::Result<IrohGossipSignaller> {
        info!("Creating new signaller");
        let bootstrap = room_pubkey.iter().cloned().collect();
        info!(
            "Subscribing to gossip topic {:?} with bootstrap: {:?}",
            GOSSIP_TOPIC_ID, bootstrap
        );
        let mut gossip_topic = self.gossip.subscribe(GOSSIP_TOPIC_ID, bootstrap)?;
        info!("Joining gossip topic...");
        gossip_topic.joined().await?;
        info!("Connected to gossip topic.");
        let (gossip_send, gossip_recv) = gossip_topic.split();

        let (req_send, req_recv) = tokio::sync::mpsc::channel(2048);
        let (event_send, event_recv) = tokio::sync::mpsc::channel(2048);

        let _task = self
            .clone()
            .spawn_task(
                gossip_recv,
                gossip_send,
                req_recv,
                event_send,
                self.direct_message_recv.activate_cloned(),
            )
            .then(|r| async move {
                match r {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        error!("Error in gossip task: {e:#?}. \n Gossip task exit.");
                        Err(e)
                    }
                }
            });
        let _task = n0_future::task::AbortOnDropHandle::new(n0_future::task::spawn(_task));
        Ok(IrohGossipSignaller {
            recv: event_recv,
            send: req_send,
            _task,
        })
    }

    async fn spawn_task(
        self,
        // gossip msg from other nodes
        mut gossip_recv: GossipReceiver,
        // gossip msg into other nodes
        gossip_send: GossipSender,
        // sent requests from client
        mut req_recv: tokio::sync::mpsc::Receiver<PeerRequest>,
        // send events to client
        event_send: tokio::sync::mpsc::Sender<PeerEvent>,
        // get direct messages from other clients
        mut direct_message_recv: async_broadcast::Receiver<(PublicKey, PeerEvent)>,
    ) -> anyhow::Result<()> {
        const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
        const STALE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
        info!("Spawning signal task");

        // send AssignedId into client
        event_send
            .send(PeerEvent::IdAssigned(self.matchbox_id))
            .await?;

        // send gossip message to ensure other nodes have our iroh id
        self.send_gossip_message(&gossip_send).await?;

        // map ids to each other, including timestamps
        let mut matchbox_to_iroh = BTreeMap::<PeerId, (PublicKey, Instant)>::new();
        let mut iroh_to_matchbox = BTreeMap::<PublicKey, (PeerId, Instant)>::new();

        let mut refresh_interval = n0_future::time::interval(REFRESH_INTERVAL);

        loop {
            tokio::select! {
                gossip_msg = gossip_recv.next().fuse() => {
                    debug!("Received gossip message.");
                    // receive gossip messages and send NewPeer events
                    let Some(Ok(gossip_msg)) = gossip_msg else {
                        anyhow::bail!("Gossip receiver stream problem: {:#?}", gossip_msg);
                    };
                    match gossip_msg {
                        Event::Lagged => {
                            // Iroh will close the receiver after this event, so we can exit here.
                            anyhow::bail!("Gossip receiver lagged");
                        }
                        Event::Gossip(GossipEvent::Received(Message { content: gossip_msg, ..})) => {
                            let GossipMessage{iroh_id, matchbox_id, ..} = serde_json::from_slice(&gossip_msg)?;
                            let now = Instant::now();
                            let is_new = !matchbox_to_iroh.contains_key(&matchbox_id);
                            matchbox_to_iroh.insert(matchbox_id, (iroh_id, now));
                            iroh_to_matchbox.insert(iroh_id, (matchbox_id, now));
                            if is_new {
                                info!("New peer connection: matchbox ID {matchbox_id} = Iroh ID {iroh_id}");

                                if matchbox_id < self.matchbox_id {
                                    info!("Sending NewPeer event for smaller id");
                                    event_send.send(PeerEvent::NewPeer(matchbox_id)).await?;
                                } else {
                                    info!("Not sending NewPeer event for larger id");
                                }
                                // on new peer, send gossip message to ensure new peer has our iroh id
                                self.send_gossip_message(&gossip_send).await?;
                            }
                        }
                        _ => {
                            // ignore other gossip events, they're not relevant here
                        }
                    }
                },
                req_msg = req_recv.recv().fuse() => {
                    // on client request, either send keep alive to gossip, or send direct message with the signal to peer
                    let Some(req_msg) = req_msg else {
                        anyhow::bail!("Request receiver stream problem: {:#?}", req_msg);
                    };
                    match req_msg {
                        PeerRequest::KeepAlive => {
                            // send keep alive to gossip, containing all of our IDs
                            self.send_gossip_message(&gossip_send).await?;
                        }
                        PeerRequest::Signal { receiver, data } => {
                            // send direct message to MatchboxSignalProtocol
                            match self.send_direct_message(receiver, data, &matchbox_to_iroh).await {
                                Ok(_) => {}
                                Err(e) => {
                                    error!("Error sending direct message: {e:#?}");
                                }
                            }
                        }
                    }
                },
                direct_msg = direct_message_recv.next().fuse() => {
                    let Some((from_iroh_id, event)) = direct_msg else {
                        anyhow::bail!("Direct message receiver stream problem: {:#?}", direct_msg);
                    };
                    // check message is from who it said it is
                    if let PeerEvent::Signal {sender, ..} = &event {
                        if matchbox_to_iroh.get(sender).map(|(node_id, _)| node_id) == Some(&from_iroh_id) {
                            debug!("
                            Received direct message:
                                From Matchbox ID: {sender}
                                From Iroh ID: {from_iroh_id}
                                Event: {event:#?}
                            ");
                            event_send.send(event.clone()).await?;
                        } else {
                            warn!("Received message from {from_iroh_id} with wrong sender: {sender}");
                        }
                    } else {
                        warn!("Received message from {from_iroh_id} with wrong event type: {event:#?}");
                    }
                }
                _ = refresh_interval.tick().fuse() => {
                    self.send_gossip_message(&gossip_send).await?;
                    // check for stale connections and send PeerLeft events
                    let now = Instant::now();
                    let dead_peers = matchbox_to_iroh
                        .iter()
                        .filter(|(_, (_, timestamp))| now.duration_since(*timestamp) >= STALE_CONNECTION_TIMEOUT)
                        .map(|(peer_id, (node_id, _))| (*peer_id, *node_id))
                        .collect::<Vec<_>>();
                    for (peer_id, node_id) in dead_peers {
                        info!("Removing dead peer connection: {peer_id} -> {node_id}");
                        matchbox_to_iroh.remove(&peer_id);
                        iroh_to_matchbox.remove(&node_id);
                        event_send.send(PeerEvent::PeerLeft(peer_id)).await?;
                    }
                }
            }
        }
    }

    async fn send_gossip_message(&self, gossip_send: &GossipSender) -> anyhow::Result<()> {
        debug!("Sending gossip message");
        let timestamp = get_timestamp();
        let message = GossipMessage {
            matchbox_id: self.matchbox_id,
            iroh_id: self.iroh_id,
            timestamp,
        };
        let message = serde_json::to_vec(&message)?;
        gossip_send.broadcast(message.into()).await?;
        Ok(())
    }

    async fn send_direct_message(
        &self,
        receiver: PeerId,
        data: PeerSignal,
        matchbox_to_iroh: &BTreeMap<PeerId, (PublicKey, Instant)>,
    ) -> anyhow::Result<()> {
        let event = PeerEvent::Signal {
            sender: self.matchbox_id,
            data,
        };
        let target_node_id = matchbox_to_iroh
            .get(&receiver)
            .map(|(node_id, _)| *node_id)
            .with_context(|| format!("No known connection to peer {receiver}"))?;
        debug!(
            "
        Sending direct message to:
            Matchbox ID: {receiver} 
            Iroh     ID: {target_node_id}
            Event: {event:#?}
        "
        );

        send_direct_message(&self.endpoint, target_node_id, event).await?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct GossipMessage {
    matchbox_id: PeerId,
    iroh_id: PublicKey,
    timestamp: u128,
}

struct IrohGossipSignaller {
    recv: tokio::sync::mpsc::Receiver<PeerEvent>,
    send: tokio::sync::mpsc::Sender<PeerRequest>,
    _task: n0_future::task::AbortOnDropHandle<Result<(), anyhow::Error>>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Signaller for IrohGossipSignaller {
    async fn send(&mut self, request: PeerRequest) -> Result<(), SignalingError> {
        debug!("\n\nSignaller: Sending request: {request:#?}\n\n");
        self.send.send(request).await.map_err(to_user_error)?;
        Ok(())
    }
    async fn next_message(&mut self) -> Result<PeerEvent, SignalingError> {
        let Some(message) = self.recv.recv().await else {
            info!("\n\nSignaller: Stream exhausted\n\n");
            return Err(SignalingError::StreamExhausted);
        };
        debug!("\n\n Signaller: Received message: {message:#?}\n\n");
        Ok(message)
    }
}
