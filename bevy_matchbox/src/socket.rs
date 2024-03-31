use bevy::{
    ecs::system::Command,
    prelude::{Commands, Component, Resource, World},
    tasks::IoTaskPool,
};
pub use matchbox_socket;
use matchbox_socket::{
    BuildablePlurality, MessageLoopFuture, SingleChannel, WebRtcSocket, WebRtcSocketBuilder,
};
use std::{
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

/// A [`WebRtcSocket`] as a [`Component`] or [`Resource`].
///
/// As a [`Component`], directly
/// ```
/// use bevy_matchbox::prelude::*;
/// use bevy::prelude::*;
///
/// fn open_socket_system(mut commands: Commands) {
///     let room_url = "wss://matchbox.example.com";
///     let builder = WebRtcSocketBuilder::new(room_url).add_channel(ChannelConfig::reliable());
///     commands.spawn(MatchboxSocket::from(builder));
/// }
///
/// fn close_socket_system(
///     mut commands: Commands,
///     socket: Query<Entity, With<MatchboxSocket<SingleChannel>>>
/// ) {
///     let socket = socket.single();
///     commands.entity(socket).despawn();
/// }
/// ```
///
/// As a [`Resource`], with [`Commands`]
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
///     commands.close_socket::<SingleChannel>();
/// }
/// ```
///
/// As a [`Resource`], directly
/// ```
/// use bevy_matchbox::prelude::*;
/// use bevy::prelude::*;
///
/// fn open_socket_system(mut commands: Commands) {
///     let room_url = "wss://matchbox.example.com";
///
///     let socket: MatchboxSocket<SingleChannel> = WebRtcSocketBuilder::new(room_url)
///         .add_channel(ChannelConfig::reliable())
///         .into();
///
///     commands.insert_resource(socket);
/// }
///
/// fn close_socket_system(mut commands: Commands) {
///     commands.remove_resource::<MatchboxSocket<SingleChannel>>();
/// }
/// ```
#[derive(Resource, Component, Debug)]
#[allow(dead_code)] // keep the task alive so it doesn't drop before the socket
pub struct MatchboxSocket<C: BuildablePlurality>(WebRtcSocket<C>, Box<dyn Debug + Send + Sync>);

impl<C: BuildablePlurality> Deref for MatchboxSocket<C> {
    type Target = WebRtcSocket<C>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: BuildablePlurality> DerefMut for MatchboxSocket<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<C: BuildablePlurality> From<WebRtcSocketBuilder<C>> for MatchboxSocket<C> {
    fn from(builder: WebRtcSocketBuilder<C>) -> Self {
        Self::from(builder.build())
    }
}

impl<C: BuildablePlurality> From<(WebRtcSocket<C>, MessageLoopFuture)> for MatchboxSocket<C> {
    fn from((socket, message_loop_fut): (WebRtcSocket<C>, MessageLoopFuture)) -> Self {
        let task_pool = IoTaskPool::get();
        let task = task_pool.spawn(message_loop_fut);
        MatchboxSocket(socket, Box::new(task))
    }
}

/// A [`Command`] used to open a [`MatchboxSocket`] and allocate it as a resource.
struct OpenSocket<C: BuildablePlurality>(WebRtcSocketBuilder<C>);

impl<C: BuildablePlurality + 'static> Command for OpenSocket<C> {
    fn apply(self, world: &mut World) {
        world.insert_resource(MatchboxSocket::from(self.0));
    }
}

/// A [`Commands`] extension used to open a [`MatchboxSocket`] and allocate it as a resource.
pub trait OpenSocketExt<C: BuildablePlurality> {
    /// Opens a [`MatchboxSocket`] and allocates it as a resource.
    fn open_socket(&mut self, socket_builder: WebRtcSocketBuilder<C>);
}

impl<'w, 's, C: BuildablePlurality + 'static> OpenSocketExt<C> for Commands<'w, 's> {
    fn open_socket(&mut self, socket_builder: WebRtcSocketBuilder<C>) {
        self.add(OpenSocket(socket_builder))
    }
}

/// A [`Command`] used to close a [`WebRtcSocket`], deleting the [`MatchboxSocket`] resource.
struct CloseSocket<C: BuildablePlurality>(PhantomData<C>);

impl<C: BuildablePlurality + 'static> Command for CloseSocket<C> {
    fn apply(self, world: &mut World) {
        world.remove_resource::<MatchboxSocket<C>>();
    }
}

/// A [`Commands`] extension used to close a [`WebRtcSocket`], deleting the [`MatchboxSocket`]
/// resource.
pub trait CloseSocketExt {
    /// Delete the [`MatchboxSocket`] resource.
    fn close_socket<C: BuildablePlurality + 'static>(&mut self);
}

impl<'w, 's> CloseSocketExt for Commands<'w, 's> {
    fn close_socket<C: BuildablePlurality + 'static>(&mut self) {
        self.add(CloseSocket::<C>(PhantomData))
    }
}

impl MatchboxSocket<SingleChannel> {
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
    pub fn new_unreliable(room_url: impl Into<String>) -> MatchboxSocket<SingleChannel> {
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
    pub fn new_reliable(room_url: impl Into<String>) -> MatchboxSocket<SingleChannel> {
        Self::from(WebRtcSocket::new_reliable(room_url))
    }

    /// Create a new socket with a single ggrs-compatible channel
    ///
    /// ```rust
    /// use bevy_matchbox::prelude::*;
    /// use bevy::prelude::*;
    ///
    /// fn open_channel_system(mut commands: Commands) {
    ///     let room_url = "wss://matchbox.example.com";
    ///     let socket = MatchboxSocket::new_ggrs(room_url);
    ///     commands.spawn(socket);
    /// }
    /// ```
    #[cfg(feature = "ggrs")]
    pub fn new_ggrs(room_url: impl Into<String>) -> MatchboxSocket<SingleChannel> {
        Self::from(WebRtcSocket::new_ggrs(room_url))
    }
}
