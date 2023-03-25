use crate::signaling_server::{
    error::ClientRequestError, handlers::WsStateMeta, server::SignalingState,
};
use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use futures::{future::BoxFuture, stream::SplitSink, Future, StreamExt};
use matchbox_protocol::JsonPeerRequest;
use std::{str::FromStr, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

mod client_server;
mod full_mesh;

#[derive(Debug, Default)]
pub struct FullMesh;
pub use full_mesh::FullMeshState;

#[derive(Debug, Default)]
pub struct ClientServer;
pub use client_server::ClientServerState;

#[derive(Clone)]
pub struct SignalingStateMachine<S>(
    pub Arc<Box<dyn Fn(WsStateMeta<S>) -> BoxFuture<'static, ()> + Send + Sync>>,
);

impl<S> SignalingStateMachine<S>
where
    S: SignalingState + 'static,
{
    pub fn from_topology<Topology>(_: Topology) -> Self
    where
        Topology: SignalingTopology<S>,
    {
        Self::new(|ws| <Topology as SignalingTopology<S>>::state_machine(ws))
    }

    pub fn new<F, Fut>(callback: F) -> Self
    where
        F: Fn(WsStateMeta<S>) -> Fut + 'static + Send + Sync,
        Fut: Future<Output = ()> + 'static + Send,
    {
        Self(Arc::new(Box::new(move |ws| Box::pin(callback(ws)))))
    }
}

#[async_trait]
pub trait SignalingTopology<S>
where
    S: SignalingState + 'static,
{
    /// A run-to-completion state machine, spawned once for every websocket.
    async fn state_machine(upgrade: WsStateMeta<S>);
}

pub(crate) fn parse_request(
    request: Result<Message, ClientRequestError>,
) -> Result<JsonPeerRequest, ClientRequestError> {
    match request? {
        Message::Text(text) => Ok(JsonPeerRequest::from_str(&text)?),
        Message::Close(_) => Err(ClientRequestError::Close),
        m => Err(ClientRequestError::UnsupportedType(m)),
    }
}

/// Common helper method to spawn a sender
pub(crate) fn spawn_sender_task(
    sender: SplitSink<WebSocket, Message>,
) -> mpsc::UnboundedSender<Result<Message, axum::Error>> {
    let (client_sender, receiver) = mpsc::unbounded_channel();
    tokio::task::spawn(UnboundedReceiverStream::new(receiver).forward(sender));
    client_sender
}
