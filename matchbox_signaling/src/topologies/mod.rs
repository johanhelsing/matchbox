use crate::signaling_server::signaling::SignalingStateMachine;

mod full_mesh;

pub trait Topology {
    fn state_machine() -> SignalingStateMachine;
}

#[derive(Debug, Default)]
pub struct FullMesh;

impl Topology for FullMesh {
    fn state_machine() -> SignalingStateMachine {
        SignalingStateMachine::from(full_mesh::state_machine)
    }
}

#[derive(Debug, Default)]
pub struct ClientServer;
