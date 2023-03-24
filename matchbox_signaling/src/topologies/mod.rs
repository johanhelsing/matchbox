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
pub struct SignalingStateMachine<State>(
    pub Arc<Box<dyn Fn(WsStateMeta<State>) -> BoxFuture<'static, ()> + Send + Sync>>,
);

impl<State> SignalingStateMachine<State> {
    pub fn from_topology<T>(_: T) -> Self
    where
        T: SignalingTopology,
    {
        Self::new(|ws| <T as SignalingTopology>::state_machine(ws))
    }

    pub fn new<F, Fut>(callback: F) -> Self
    where
        F: Fn(WsStateMeta<State>) -> Fut + 'static + Send + Sync,
        Fut: Future<Output = ()> + 'static + Send,
    {
        Self(Arc::new(Box::new(move |ws| Box::pin(callback(ws)))))
    }
}

#[async_trait]
pub trait SignalingTopology {
    /// A run-to-completion state machine, spawned once for every websocket.
    async fn state_machine<State>(upgrade: WsStateMeta<State>);
}
