use std::sync::{Arc, Weak};
use std::time::Duration;
use async_trait::async_trait;
use futures_timer::Delay;
use once_cell::sync::OnceCell;
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
pub trait MaybeSend: Send {}
#[cfg(not(target_family = "wasm"))]
impl<T: Send> MaybeSend for T {}

#[cfg(target_family = "wasm")]
pub trait MaybeSend {}
#[cfg(target_family = "wasm")]
impl<T> MaybeSend for T {}

/// Trait representing a channel with a buffer, allowing querying of the buffered amount.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait BufferedChannel: MaybeSend + Sync {
    /// Returns the current buffered amount in the channel.
    async fn buffered_amount(&self) -> usize;
}

/// Manages multiple buffered channels, providing utilities to query and flush them.
#[derive(Debug, Clone, Default)]
pub struct PeerBuffered {
    channel_refs: Arc<Vec<OnceCell<Weak<dyn BufferedChannel>>>>
}

impl PeerBuffered {
    /// Creates a new PeerBuffered with the specified number of channels.
    pub(crate) fn new(number_of_channel: usize) -> Self {
        let mut channels = vec![];
        for _ in 0..number_of_channel {
            channels.push(Default::default());
        }

        Self {
            channel_refs: Arc::new(channels),
        }
    }

    /// Adds a channel at the given index.
    pub fn add_channel(&self, index: usize, channel: Arc<dyn BufferedChannel>) {
        let _ = self.channel_refs[index].set(Arc::downgrade(&channel));
    }

    /// Returns the buffered amount for the channel at the given index.
    pub async fn buffered_amount(&self, index: usize) -> usize {
        if let Some(channel) = self.channel_refs[index].get().and_then(|it| it.upgrade()) {
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
