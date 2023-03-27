pub(crate) mod auth;
pub(crate) mod builder;
pub(crate) mod callbacks;
pub(crate) mod error;
pub(crate) mod handlers;
pub(crate) mod server;

use self::{auth::AuthKey, handlers::WsUpgradeMeta};
use axum::response::Response;

pub use server::SignalingServer;

/// State managed by the signaling server
pub trait SignalingState: Clone + Send + Sync + 'static {}

/// Callbacks used by the signaling server
pub trait SignalingCallbacks: Default + Clone + Send + Sync + 'static {}

/// Supported Authentication types used by the signaling server
pub trait Authentication: 'static {
    fn verify(meta: WsUpgradeMeta, auth_key: AuthKey) -> Result<bool, Response>;
}

/// No-Op signaling callbacks
#[derive(Default, Debug, Copy, Clone)]
pub struct NoOpCallouts {}
impl SignalingCallbacks for NoOpCallouts {}

/// Store no state
#[derive(Clone)]
pub struct NoState {}
impl SignalingState for NoState {}
