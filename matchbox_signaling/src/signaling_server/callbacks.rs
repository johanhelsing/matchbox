use std::{fmt, rc::Rc};

use crate::SignalingCallbacks;

use super::handlers::WsUpgradeMeta;

/// Universal callback wrapper.
///
/// An `Rc` wrapper is used to make it cloneable.
pub struct Callback<In, Out = ()> {
    /// A callback which can be called multiple times
    pub(crate) cb: Rc<dyn Fn(In) -> Out>,
}

impl<In, Out, F: Fn(In) -> Out + 'static> From<F> for Callback<In, Out> {
    fn from(func: F) -> Self {
        Callback { cb: Rc::new(func) }
    }
}

impl<In, Out> Clone for Callback<In, Out> {
    fn clone(&self) -> Self {
        Self {
            cb: self.cb.clone(),
        }
    }
}

impl<In, Out> fmt::Debug for Callback<In, Out> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Callback<_>")
    }
}

impl<In, Out> Callback<In, Out> {
    /// This method calls the callback's function.
    pub fn emit(&self, value: In) -> Out {
        (*self.cb)(value)
    }
}

impl<In> Callback<In> {
    /// Creates a "no-op" callback which can be used when it is not suitable to use an
    /// `Option<Callback>`.
    pub fn noop() -> Self {
        Self::from(|_| ())
    }
}

impl<In> Default for Callback<In> {
    fn default() -> Self {
        Self::noop()
    }
}

/// Signaling callbacks for all topologies
#[derive(Default, Debug, Clone)]
pub struct SharedCallbacks {
    /// Triggered immediately on connect
    pub(crate) on_connect: Callback<WsUpgradeMeta>,
}
impl SignalingCallbacks for SharedCallbacks {}
#[allow(unsafe_code)]
unsafe impl Send for SharedCallbacks {}
#[allow(unsafe_code)]
unsafe impl Sync for SharedCallbacks {}
