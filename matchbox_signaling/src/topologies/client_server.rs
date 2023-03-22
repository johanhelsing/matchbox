use super::{ClientServer, SignalingTopology};
use crate::signaling_server::signaling::WsUpgrade;
use async_trait::async_trait;

#[async_trait]
impl SignalingTopology for ClientServer {
    async fn state_machine(upgrade: WsUpgrade) {
        todo!()
    }
}
