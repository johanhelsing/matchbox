#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

use bevy::{
    ecs::system::Command,
    prelude::{Commands, Deref, DerefMut, Resource, World},
    tasks::IoTaskPool,
};
pub use matchbox_socket;
use matchbox_socket::{BuildablePlurality, WebRtcSocket, WebRtcSocketBuilder};
use std::marker::PhantomData;

/// A [`Resource`] wrapping [`WebRtcSocket`].
///
/// To create and destroy this resource use the [`OpenSocket`] and [`CloseSocket`] [`Command`]s respectively.
#[derive(Resource, Debug, Deref, DerefMut)]
pub struct MatchboxSocket<C: BuildablePlurality>(WebRtcSocket<C>);

/// A [`Command`] used to open a [`MatchboxSocket`] and allocate it as a resource.
struct OpenSocket<C: BuildablePlurality>(WebRtcSocketBuilder<C>);

impl<C: BuildablePlurality + 'static> Command for OpenSocket<C> {
    fn write(self, world: &mut World) {
        let (socket, message_loop) = self.0.build();

        let task_pool = IoTaskPool::get();
        task_pool.spawn(message_loop).detach();

        world.insert_resource(MatchboxSocket(socket));
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
    fn write(self, world: &mut World) {
        world.remove_resource::<MatchboxSocket<C>>();
    }
}

/// A [`Commands`] extension used to close a [`WebRtcSocket`], deleting the [`MatchboxSocket`] resource.
pub trait CloseSocketExt<C> {
    /// Delete the [`MatchboxSocket`] resource.
    fn close_socket(&mut self);
}

impl<'w, 's, C: BuildablePlurality + 'static> CloseSocketExt<C> for Commands<'w, 's> {
    fn close_socket(&mut self) {
        self.add(CloseSocket::<C>(PhantomData::default()))
    }
}

/// use `bevy_matchbox::prelude::*;` to import common resources and commands
pub mod prelude {
    pub use crate::{CloseSocketExt, MatchboxSocket, OpenSocketExt};
    pub use matchbox_socket::{
        ChannelConfig, MultipleChannels, PeerId, PeerState, SingleChannel, WebRtcSocketBuilder,
    };
}
