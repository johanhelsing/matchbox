use crate::signaling_server::handlers::WsStateMeta;
use async_trait::async_trait;
use futures::{future::BoxFuture, Future};
use std::sync::Arc;

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
    S: Clone + Send + Sync + 'static,
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
    S: Clone + Send + Sync + 'static,
{
    /// A run-to-completion state machine, spawned once for every websocket.
    async fn state_machine(upgrade: WsStateMeta<S>);
}
