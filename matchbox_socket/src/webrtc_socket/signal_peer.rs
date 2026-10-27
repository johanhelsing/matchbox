use crate::webrtc_socket::{PeerId, PeerRequest, PeerSignal};
use futures_channel::mpsc::UnboundedSender;
use log::warn;

#[derive(Debug, Clone)]
pub struct SignalPeer {
    pub id: PeerId,
    pub sender: UnboundedSender<PeerRequest>,
}

impl SignalPeer {
    pub fn send(&self, signal: PeerSignal) {
        let req = PeerRequest::Signal {
            receiver: self.id,
            data: signal,
        };
        if self.sender.unbounded_send(req).is_err() {
            // The signaling loop is gone; the handshake this signal belongs
            // to cannot complete anyway. Warn instead of panicking: a panic
            // here would tear down the whole socket task, including peer
            // connections that are unaffected by the signaling loss.
            warn!("dropping signaling signal: signaling connection is gone");
        }
    }

    pub fn new(id: PeerId, sender: UnboundedSender<PeerRequest>) -> Self {
        Self { id, sender }
    }
}
