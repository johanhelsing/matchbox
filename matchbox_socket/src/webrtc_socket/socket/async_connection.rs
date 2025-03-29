use crate::webrtc_socket::{MatchboxDataChannel, PacketSendError};
use crate::Packet;
use futures::{Sink, Stream, StreamExt};
use futures_channel::mpsc::UnboundedReceiver;
use matchbox_protocol::PeerId;
use std::{pin::Pin, task::Poll};

/// WebRTC connection
///
/// `drop` to stop accepting new peers.
pub struct Connection {
    id: PeerId,
    events: UnboundedReceiver<PeerConnectionEvent>,
}

impl Connection {
    pub fn id(&self) -> PeerId {
        self.id
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        todo!()
    }
}

impl Stream for Connection {
    type Item = PeerConnectionEvent;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.events.poll_next_unpin(cx)
    }
}

pub struct Peer {
    pub id: PeerId,
    pub channels: Box<[PeerDataChannel]>,
    pub left_signaling_server: futures_channel::oneshot::Receiver<()>,
}

/// A [RTCDataChannel](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel).
pub struct PeerDataChannel {
    pub sink: DataChannelSink,
    pub stream: DataChannelStream,
}

pub enum PeerConnectionEvent {
    /// A new peer has connected.
    /// It can now be taken or sent messages via the [SimpleWebRtc.tx]
    PeerConnected(Peer),
    /// A new peer has disconnected.
    /// It can no longer be taken or sent messages via the [SimpleWebRtc.tx]
    PeerDisconnected(PeerId),
}

pub enum SimpleWebRtcClosedReason {
    Error(String),
    Closed,
}

/// [Sink] which sends packets to a specific [Peer] over a specific channel.
///
/// Sink is always ready for more data (buffer is unbounded).
///
/// Flush waits for the buffer to reach the [bufferedAmountLowThreshold](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/bufferedAmountLowThreshold).
///
/// The sink is closed when the [Peer] disconnects.
///
/// TODO: consider implementing ready threshold.
pub struct DataChannelSink {
    channel: Box<dyn MatchboxDataChannel>,
}

impl Sink<Packet> for DataChannelSink {
    // TODO: consider translating this error type to something platform independent.
    type Error = PacketSendError;

    fn poll_ready(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(mut self: Pin<&mut Self>, item: Packet) -> Result<(), Self::Error> {
        self.channel.send(item)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.channel.poll_buffer_low(cx)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.channel.poll_close(cx)
    }
}

/// A [Stream] of [Packet]s from a [PeerDataChannel].
///
/// Internally contains an unbounded buffer with explicit support for back-pressure over the network.
///
/// The stream is closed when the [Peer] disconnects.
///
/// TODO: The browser APIs for WebRTC seem to provide no access to flow control from the receive side, but the underlying [SCTP](https://en.wikipedia.org/wiki/Stream_Control_Transmission_Protocol) has some.
/// It is possible the native implementation could provide some explicit flow control,
/// and is also possible that that blocking the thread during the [message event](https://developer.mozilla.org/en-US/docs/Web/API/RTCDataChannel/message_event) provides some back pressure.
/// Further experimentation and/or research is needed in this area.
pub struct DataChannelStream {
    rx: UnboundedReceiver<Packet>,
}

impl Stream for DataChannelStream {
    type Item = Packet;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.rx.poll_next_unpin(cx)
    }
}
