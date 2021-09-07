use futures_channel::mpsc::UnboundedSender;

use super::{PeerRequest, PeerSignal};

#[derive(Debug, Clone)]
pub struct SignalPeer {
    pub id: String,
    pub sender: UnboundedSender<PeerRequest>,
}

impl SignalPeer {
    pub fn send(&self, signal: PeerSignal) {
        let req = PeerRequest::Signal {
            receiver: self.id.clone(),
            data: signal,
        };
        self.sender.unbounded_send(req).expect("Send error");
    }

    pub fn new(id: String, sender: UnboundedSender<PeerRequest>) -> Self {
        Self { id, sender }
    }
}

// let to_peer_signal_sender = requests_sender.clone().with(move |signal| {
//     future::ok::<PeerRequest, SendError>(
//         PeerRequest::Signal {
//             receiver: peer_uuid.clone(),
//             data: signal
//         }
//     )
// });
