use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use futures_timer::Delay;
use serde::{Deserialize, Serialize};

/// Events go from signaling server to peer
pub type PeerEvent = matchbox_protocol::PeerEvent<PeerSignal>;

/// Requests go from peer to signaling server
pub type PeerRequest = matchbox_protocol::PeerRequest<PeerSignal>;

/// Signals go from peer to peer via the signaling server
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum PeerSignal {
    /// Ice Candidate
    IceCandidate(String),
    /// Offer
    Offer(String),
    /// Answer
    Answer(String),
}

#[cfg(not(target_family = "wasm"))]
pub trait MaybeSend: Send + Sync {}
#[cfg(not(target_family = "wasm"))]
impl<T: Send + Sync> MaybeSend for T {}

#[cfg(target_family = "wasm")]
pub trait MaybeSend: Send {}

#[cfg(target_family = "wasm")]
impl<T> MaybeSend for T where T: Send {}

/// Trait representing a channel with a buffer, allowing querying of the buffered amount.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait BufferedChannel: MaybeSend + Sync {
    /// Returns the current buffered amount in the channel.
    async fn buffered_amount(&self) -> usize;
}

/// Manages multiple buffered channels, providing utilities to query and flush them.
#[derive(Clone, Default)]
pub struct PeerBuffered {
    channel_refs: Arc<Vec<Box<dyn BufferedChannel>>>
}

impl Debug for PeerBuffered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerBuffered")
            .field("channel_refs", &self.channel_refs.len())
            .finish()
    }
}

impl PeerBuffered {
    /// Creates a new PeerBuffered with the specified number of channels.
    pub(crate) fn new(channels: Vec<Box<dyn BufferedChannel>>) -> Self {

        Self {
            channel_refs: Arc::new(channels)
        }
    }

    /// Returns the buffered amount for the channel at the given index.
    pub async fn buffered_amount(&self, index: usize) -> usize {
        if let Some(channel) = self.channel_refs.get(index) {
            return channel.buffered_amount().await
        }

        log::error!("Channel not found");

        0
    }

    /// Flushes all channels, waiting until their buffers are empty.
    pub async fn flush_all(&self) {
        for i in 0..self.channel_refs.len() {
            self.flush(i).await;
        }
    }

    /// Returns the sum of buffered amounts across all channels.
    pub async fn sum_buffered_amount(&self) -> usize {
        let mut sum = 0;
        for i in 0..self.channel_refs.len() {
            sum += self.buffered_amount(i).await;
        }

        sum
    }
    
    /// Waits until the buffer for the channel at the given index is empty.
    pub async fn flush(&self, index: usize) {
        loop {
            let buffered_amount = self.buffered_amount(index).await;
            if buffered_amount == 0 {
                break;
            }

            Delay::new(Duration::from_millis(10)).await;
        }
    }
}
