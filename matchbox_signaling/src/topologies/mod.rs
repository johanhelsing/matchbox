use crate::signaling_server::signaling::WsUpgrade;
use async_trait::async_trait;

mod client_server;
mod full_mesh;

#[async_trait]
pub trait SignalingTopology {
    /// A run-to-completion state machine, spawned once for every websocket.
    async fn state_machine(upgrade: WsUpgrade);
}

#[derive(Debug, Default)]
pub struct FullMesh;

#[derive(Debug, Default)]
pub struct ClientServer;
