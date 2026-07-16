use bevy::{
    prelude::{Command, Commands, Resource, World},
    tasks::IoTaskPool,
};
pub use matchbox_socket;
use matchbox_socket::{MessageLoopFuture, WebRtcSocket, WebRtcSocketBuilder};
use std::ops::{Deref, DerefMut};

/// A [`WebRtcSocket`] as a Bevy [`Resource`].
///
/// Open with [`Commands`]:
/// ```
/// use bevy_matchbox::prelude::*;
/// use bevy::prelude::*;
///
/// fn open_socket_system(mut commands: Commands) {
///     let room_url = "wss://matchbox.example.com";
///     commands.open_socket(WebRtcSocketBuilder::new(room_url).add_channel(ChannelConfig::reliable()));
/// }
///
/// fn close_socket_system(mut commands: Commands) {
///     commands.close_socket();
/// }
/// ```
///
/// Or insert directly:
/// ```
/// use bevy_matchbox::prelude::*;
/// use bevy::prelude::*;
///
/// fn open_socket_system(mut commands: Commands) {
///     let room_url = "wss://matchbox.example.com";
///
///     let socket: MatchboxSocket = WebRtcSocketBuilder::new(room_url)
///         .add_channel(ChannelConfig::reliable())
///         .into();
///
///     commands.insert_resource(socket);
/// }
///
/// fn close_socket_system(mut commands: Commands) {
///     commands.remove_resource::<MatchboxSocket>();
/// }
/// ```
#[derive(Resource, Debug)]
pub struct MatchboxSocket(WebRtcSocket);

impl Deref for MatchboxSocket {
    type Target = WebRtcSocket;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MatchboxSocket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<WebRtcSocketBuilder> for MatchboxSocket {
    fn from(builder: WebRtcSocketBuilder) -> Self {
        Self::from(builder.build())
    }
}

impl From<(WebRtcSocket, MessageLoopFuture)> for MatchboxSocket {
    fn from((socket, message_loop_fut): (WebRtcSocket, MessageLoopFuture)) -> Self {
        let task_pool = IoTaskPool::get();
        task_pool.spawn(message_loop_fut).detach();
        MatchboxSocket(socket)
    }
}

/// A [`Command`] used to open a [`MatchboxSocket`] and allocate it as a resource.
struct OpenSocket(WebRtcSocketBuilder);

impl Command for OpenSocket {
    type Out = ();

    fn apply(self, world: &mut World) {
        world.insert_resource(MatchboxSocket::from(self.0));
    }
}

/// A [`Commands`] extension used to open a [`MatchboxSocket`] and allocate it as a resource.
pub trait OpenSocketExt {
    /// Opens a [`MatchboxSocket`] and allocates it as a resource.
    fn open_socket(&mut self, socket_builder: WebRtcSocketBuilder);
}

impl OpenSocketExt for Commands<'_, '_> {
    fn open_socket(&mut self, socket_builder: WebRtcSocketBuilder) {
        self.queue(OpenSocket(socket_builder))
    }
}

/// A [`Command`] used to close a [`WebRtcSocket`], deleting the [`MatchboxSocket`] resource.
struct CloseSocket;

impl Command for CloseSocket {
    type Out = ();

    fn apply(self, world: &mut World) {
        world.remove_resource::<MatchboxSocket>();
    }
}

/// A [`Commands`] extension used to close a [`WebRtcSocket`], deleting the [`MatchboxSocket`]
/// resource.
pub trait CloseSocketExt {
    /// Delete the [`MatchboxSocket`] resource.
    fn close_socket(&mut self);
}

impl CloseSocketExt for Commands<'_, '_> {
    fn close_socket(&mut self) {
        self.queue(CloseSocket)
    }
}

impl MatchboxSocket {
    /// Create a new socket with a single unreliable channel
    ///
    /// ```rust
    /// use bevy_matchbox::prelude::*;
    /// use bevy::prelude::*;
    ///
    /// fn open_channel_system(mut commands: Commands) {
    ///     let room_url = "wss://matchbox.example.com";
    ///     let socket = MatchboxSocket::new_unreliable(room_url);
    ///     commands.spawn(socket);
    /// }
    /// ```
    pub fn new_unreliable(room_url: impl Into<String>) -> MatchboxSocket {
        Self::from(WebRtcSocket::new_unreliable(room_url))
    }

    /// Create a new socket with a single reliable channel
    ///
    /// ```rust
    /// use bevy_matchbox::prelude::*;
    /// use bevy::prelude::*;
    ///
    /// fn open_channel_system(mut commands: Commands) {
    ///     let room_url = "wss://matchbox.example.com";
    ///     let socket = MatchboxSocket::new_reliable(room_url);
    ///     commands.spawn(socket);
    /// }
    /// ```
    pub fn new_reliable(room_url: impl Into<String>) -> MatchboxSocket {
        Self::from(WebRtcSocket::new_reliable(room_url))
    }
}
