//! The core `FlecsMatchboxSocket` and related extension traits.

use flecs_ecs::prelude::*;
use futures_lite::future::block_on;
use log::info;
pub use matchbox_socket;
use matchbox_socket::{MessageLoopFuture, WebRtcSocket, WebRtcSocketBuilder};
use std::ops::{Deref, DerefMut};

/// A [`WebRtcSocket`] wrapped for use as a Flecs singleton component.
///
/// When this component is created, it will spawn a background task to run the
/// socket's message loop. When the component is removed from the world (or the
/// world is dropped), the underlying socket will be dropped, and the background
/// task will terminate.
///
/// Create one using [`WebRtcSocketBuilder`] and `.into()` or `::from()`, or use the
/// [`OpenSocketExt`] trait on `&mut World`.
#[derive(Component)]
pub struct FlecsMatchboxSocket(WebRtcSocket);

impl Deref for FlecsMatchboxSocket {
    type Target = WebRtcSocket;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for FlecsMatchboxSocket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<WebRtcSocketBuilder> for FlecsMatchboxSocket {
    fn from(builder: WebRtcSocketBuilder) -> Self {
        Self::from(builder.build())
    }
}

impl From<(WebRtcSocket, MessageLoopFuture)> for FlecsMatchboxSocket {
    fn from((socket, message_loop_fut): (WebRtcSocket, MessageLoopFuture)) -> Self {
        std::thread::spawn(move || {
            info!("Matchbox message loop started");
            if let Err(e) = block_on(message_loop_fut) {
                log::error!("Message loop ended with error: {e:?}");
            }
            info!("Matchbox message loop finished");
        });
        FlecsMatchboxSocket(socket)
    }
}

/// An extension trait for `flecs_ecs::prelude::World` to simplify opening sockets.
pub trait OpenSocketExt {
    /// Creates a [`FlecsMatchboxSocket`] from a builder and adds it as a singleton.
    fn open_socket(&mut self, socket_builder: WebRtcSocketBuilder);
}

impl OpenSocketExt for World {
    fn open_socket(&mut self, socket_builder: WebRtcSocketBuilder) {
        let socket = FlecsMatchboxSocket::from(socket_builder);
        self.set(socket);
    }
}

/// An extension trait for `flecs_ecs::prelude::World` to simplify closing sockets.
pub trait CloseSocketExt {
    /// Removes the [`FlecsMatchboxSocket`] singleton, closing the connection.
    fn close_socket(&mut self);
}

impl CloseSocketExt for World {
    fn close_socket(&mut self) {
        self.remove::<FlecsMatchboxSocket>();
    }
}

impl FlecsMatchboxSocket {
    /// Create a new socket with a single unreliable channel.
    pub fn new_unreliable(room_url: impl Into<String>) -> Self {
        Self::from(WebRtcSocket::new_unreliable(room_url))
    }

    /// Create a new socket with a single reliable channel.
    pub fn new_reliable(room_url: impl Into<String>) -> Self {
        Self::from(WebRtcSocket::new_reliable(room_url))
    }
}
