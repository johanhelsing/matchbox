use std::{fmt, rc::Rc};

use matchbox_protocol::{JsonPeerRequest, PeerId, PeerRequest};

/// Universal callback wrapper.
///
/// An `Rc` wrapper is used to make it cloneable.
pub struct Callback<IN, OUT = ()> {
    /// A callback which can be called multiple times
    pub(crate) cb: Rc<dyn Fn(IN) -> OUT>,
}

impl<IN, OUT, F: Fn(IN) -> OUT + 'static> From<F> for Callback<IN, OUT> {
    fn from(func: F) -> Self {
        Callback { cb: Rc::new(func) }
    }
}

impl<IN, OUT> Clone for Callback<IN, OUT> {
    fn clone(&self) -> Self {
        Self {
            cb: self.cb.clone(),
        }
    }
}

impl<IN, OUT> fmt::Debug for Callback<IN, OUT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Callback<_>")
    }
}

impl<IN, OUT> Callback<IN, OUT> {
    /// This method calls the callback's function.
    pub fn emit(&self, value: IN) -> OUT {
        (*self.cb)(value)
    }
}

impl<IN> Callback<IN> {
    /// Creates a "no-op" callback which can be used when it is not suitable to use an
    /// `Option<Callback>`.
    pub fn noop() -> Self {
        Self::from(|_| ())
    }
}

impl<IN> Default for Callback<IN> {
    fn default() -> Self {
        Self::noop()
    }
}

/// Callbacks used by the signalling server
#[derive(Default, Debug, Clone)]
pub struct Callbacks {
    /// Triggered on peer requests to the signalling server
    pub(crate) on_signal: Callback<JsonPeerRequest>,
    /// Triggered on a new connection to the signalling server
    pub(crate) on_peer_connected: Callback<PeerId>,
    /// Triggered on a disconnection to the signalling server
    pub(crate) on_peer_disconnected: Callback<PeerId>,
}

#[allow(unsafe_code)]
unsafe impl Send for Callbacks {}
#[allow(unsafe_code)]
unsafe impl Sync for Callbacks {}
