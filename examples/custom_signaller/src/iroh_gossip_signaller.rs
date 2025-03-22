use futures::StreamExt;
use iroh::{protocol::Router, Endpoint, PublicKey};
use iroh_gossip::{net::{Event, Gossip, GossipEvent, GossipReceiver, GossipSender, GossipTopic}, proto::TopicId, ALPN};
use log::{info,warn,error};
use matchbox_socket::{async_trait::async_trait, error::SignalingError, PeerEvent, PeerId, PeerRequest, Signaller, SignallerBuilder};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct IrohGossipSignallerBuilder {
    router: Router,
    gossip: Gossip,
    endpoint: Endpoint,
    matchbox_id: PeerId,
    iroh_id: PublicKey,
}

impl IrohGossipSignallerBuilder {
    pub async fn new() -> anyhow::Result<Self> {
        let endpoint = Endpoint::builder().discovery_n0().bind().await?;
        let iroh_id = endpoint.node_id();
        let matchbox_id = PeerId(uuid::Uuid::new_v4());
        let gossip = Gossip::builder().spawn(endpoint.clone()).await?;
        let router = Router::builder(endpoint.clone())
            .accept(ALPN, gossip.clone())
            .spawn()
            .await?;
        Ok(Self { router, gossip, endpoint, matchbox_id, iroh_id })
    }
}

const GOSSIP_TOPIC_ID: TopicId = TopicId::from_bytes(*b"__matchbox_example_iroh_gossip__");
#[async_trait]
impl SignallerBuilder for IrohGossipSignallerBuilder {
    async fn new_signaller(&self, _attempts: Option<u16>, room_url: String) -> Result<Box<dyn Signaller>, SignalingError> {
        for i in 0.._attempts.unwrap_or(3) {
            match self.try_new_signaller(_attempts, room_url.clone()).await {
                Ok(signaller) => {
                    return Ok(Box::new(signaller));
                }
                Err(e) => {
                    warn!("Failed to connect to gossip: {e}");
                    if i == _attempts.unwrap_or(1) - 1 {
                        return Err(to_user_error(e));
                    }
                }
            }
        }
        unreachable!()
    }
}

fn to_user_error<E: ToString>(e: E) -> SignalingError {
    SignalingError::UserImplementationError(e.to_string())
}

impl IrohGossipSignallerBuilder {
    async fn try_new_signaller(&self, _attempts: Option<u16>, room_url: String) -> anyhow::Result<IrohGossipSignaller> {
        info!("Creating new signaller");
        let bootstrap = if room_url.is_empty() { vec![] } else { vec![room_url.parse()?] };
        let mut gossip_topic = self.gossip.subscribe(GOSSIP_TOPIC_ID, bootstrap)?;
        if !room_url.is_empty() {
            info!("Joining gossip topic");
            gossip_topic.joined().await?;
        }
        let (sender, receiver) = gossip_topic.split();
        Ok(IrohGossipSignaller { sender, receiver, matchbox_id: self.matchbox_id.clone(), iroh_id: self.iroh_id.clone() })
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct GossipMessage {
    from_matchbox_id: PeerId,
    from_iroh_id: PublicKey,
    message: PeerRequest,
}


struct IrohGossipSignaller {
    matchbox_id: PeerId,
    iroh_id: PublicKey,
    sender: GossipSender,
    receiver: GossipReceiver,

}

#[async_trait]
impl Signaller for IrohGossipSignaller {
    async fn send(&mut self, request: PeerRequest) -> Result<(), SignalingError> {
        let request = match request {
            PeerRequest::KeepAlive => return Ok(()),
            PeerRequest::Signal { receiver, data } => {
                self.send_matchbox_signal(receiver, data).await?;
                return Ok(());
            }
        };
        let message = GossipMessage {
            from_matchbox_id: self.matchbox_id.clone(),
            from_iroh_id: self.iroh_id.clone(),
            message: request,
        };
        let message = serde_json::to_vec(&message).map_err(to_user_error)?;
        self.sender.broadcast(message.into()).await.map_err(to_user_error)?;
        Ok(())

    }
    async fn next_message(&mut self) -> Result<PeerEvent, SignalingError> {
        while let Some(message) = self.receiver.next().await {
            match message.map_err(to_user_error)? {
                Event::Gossip(GossipEvent::Received(message)) => {
                    let msg: GossipMessage = serde_json::from_slice(&message.content).map_err(to_user_error)?;
                    if let Some(event) = self.convert_message(msg).await {
                        return Ok(event);
                    }
                },
                _m => {
                    info!("Ignoring gossip event: {:?}", _m);
                }
            }
        }
        Err(SignalingError::StreamExhausted)
    }
}

impl IrohGossipSignaller {
    async fn convert_message(&self, message: GossipMessage) -> Option<PeerEvent> {
        match message.message {
            PeerRequest::Signal { receiver, data } => {
                if receiver == self.matchbox_id {
                    Some(PeerEvent::Signal { sender: message.from_matchbox_id, data })
                } else {
                    None
                }
            }
            PeerRequest::KeepAlive => {
                None
            }
        }
    }
}
