use crate::signaling_server::{
    NoCallbacks, NoState, SignalingCallbacks, SignalingState, handlers::WsStateMeta,
};
use async_trait::async_trait;
use futures::{Future, future::BoxFuture};
use std::sync::Arc;

/// An implementation of a client server topolgy
pub mod client_server;
/// An implementation of a full mesh topology
pub mod full_mesh;

#[derive(Clone)]
pub(crate) struct SignalingStateMachine<Cb, S>(
    #[allow(clippy::type_complexity)]
    pub  Arc<Box<dyn Fn(WsStateMeta<Cb, S>) -> BoxFuture<'static, ()> + Send + Sync>>,
);

impl<Cb, S> SignalingStateMachine<Cb, S>
where
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    pub(crate) fn from_topology<Topology>(_: Topology) -> Self
    where
        Topology: SignalingTopology<Cb, S>,
    {
        Self::new(|ws| <Topology as SignalingTopology<Cb, S>>::state_machine(ws))
    }

    pub(crate) fn new<F, Fut>(callback: F) -> Self
    where
        F: Fn(WsStateMeta<Cb, S>) -> Fut + 'static + Send + Sync,
        Fut: Future<Output = ()> + 'static + Send,
    {
        Self(Arc::new(Box::new(move |ws| Box::pin(callback(ws)))))
    }
}

/// Topology produced by the signaling server
#[async_trait]
pub trait SignalingTopology<Cb = NoCallbacks, S = NoState>
where
    Cb: SignalingCallbacks,
    S: SignalingState,
{
    /// A run-to-completion state machine, spawned once for every socket.
    async fn state_machine(upgrade: WsStateMeta<Cb, S>);
}

/// Common, re-usable logic and types shared between topologies and which may be useful if building
/// your own topology.
pub mod common_logic {
    use crate::signaling_server::error::{ClientRequestError, SignalingError};
    use axum::extract::ws::{Message, WebSocket};
    use futures::{StreamExt, stream::SplitSink};
    use matchbox_protocol::JsonPeerRequest;
    use std::{
        str::FromStr,
        sync::{Arc, Mutex},
    };
    use tokio::sync::mpsc::{self, UnboundedSender};
    use tokio_stream::wrappers::UnboundedReceiverStream;

    /// Alias for Arc<Mutex<T>>
    pub type StateObj<T> = Arc<Mutex<T>>;

    /// Alias for UnboundedSender<Result<Message, axum::Error>>
    pub type SignalingChannel = UnboundedSender<Result<Message, axum::Error>>;

    /// Send a message to a channel without blocking.
    pub fn try_send(sender: &SignalingChannel, message: Message) -> Result<(), SignalingError> {
        sender.send(Ok(message)).map_err(SignalingError::from)
    }

    /// Helper to parse a request, currently we only support JSON text messages for signaling.
    pub fn parse_request(
        request: Result<Message, axum::Error>,
    ) -> Result<JsonPeerRequest, ClientRequestError> {
        match request? {
            Message::Text(text) => Ok(JsonPeerRequest::from_str(&text)?),
            Message::Close(_) => Err(ClientRequestError::Close),
            m => Err(ClientRequestError::UnsupportedType(m)),
        }
    }

    /// Common helper method to spawn a sender
    pub fn spawn_sender_task(
        sender: SplitSink<WebSocket, Message>,
    ) -> mpsc::UnboundedSender<Result<Message, axum::Error>> {
        let (client_sender, receiver) = mpsc::unbounded_channel();
        tokio::task::spawn(UnboundedReceiverStream::new(receiver).forward(sender));
        client_sender
    }
}
