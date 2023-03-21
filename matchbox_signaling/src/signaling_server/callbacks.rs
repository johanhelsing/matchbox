use futures::future::BoxFuture;

/// Callbacks used by the signalling server
pub struct Callbacks {
    /// Triggered on a new connection to the signalling server
    pub(crate) on_peer_connected: BoxFuture<'static, ()>,
    /// Triggered on a disconnection to the signalling server
    pub(crate) on_peer_disconnected: BoxFuture<'static, ()>,
}

impl Default for Callbacks {
    fn default() -> Self {
        Self {
            on_peer_connected: Box::pin(async {}),
            on_peer_disconnected: Box::pin(async {}),
        }
    }
}
