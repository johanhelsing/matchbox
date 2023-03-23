use crate::signaling_server::handlers::WsUpgrade;

use super::{ClientServer, SignalingTopology};
use async_trait::async_trait;

#[async_trait]
impl SignalingTopology for ClientServer {
    async fn state_machine(upgrade: WsUpgrade) {
        todo!()
    }
}
