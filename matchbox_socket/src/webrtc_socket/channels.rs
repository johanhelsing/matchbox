use super::{messages::PeerId, Packet};

#[derive(Debug)]
pub struct ReceiverChannels {
    pub messages_from_peers_rx: Vec<futures_channel::mpsc::UnboundedReceiver<(PeerId, Packet)>>,
    pub new_connected_peers_rx: futures_channel::mpsc::UnboundedReceiver<PeerId>,
    pub disconnected_peers_rx: futures_channel::mpsc::UnboundedReceiver<PeerId>,
    pub peer_messages_out_tx: Vec<futures_channel::mpsc::UnboundedSender<(PeerId, Packet)>>,
}

// TODO: Channel operations should go here, and there's plenty of them.
