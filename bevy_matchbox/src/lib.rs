#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

use bevy::{prelude::*, tasks::IoTaskPool};
pub use matchbox_socket;
use matchbox_socket::{BuildablePlurality, WebRtcSocket, WebRtcSocketBuilder};

/// A [`Resource`] wrapping [`WebRtcSocket`].
///
/// ```
/// # use bevy_matchbox::prelude::*;
/// # use bevy::prelude::*;
/// # fn system(mut commands: Commands) {
/// let room_url = "ws://matchbox.example.com";
/// let builder = WebRtcSocketBuilder::new(room_url).add_channel(ChannelConfig::reliable());
/// let socket = MatchboxSocket::<SingleChannel>::from(builder);
/// commands.insert_resource(socket);
/// # }
/// ```
#[derive(Resource, Component, Debug, Deref, DerefMut)]
pub struct MatchboxSocket<C: BuildablePlurality>(WebRtcSocket<C>);

impl<C: BuildablePlurality> From<WebRtcSocketBuilder<C>> for MatchboxSocket<C> {
    fn from(value: WebRtcSocketBuilder<C>) -> Self {
        let (socket, fut) = value.build();
        IoTaskPool::get().spawn(fut).detach();
        Self(socket)
    }
}

/// use `bevy_matchbox::prelude::*;` to import common resources and commands
pub mod prelude {
    pub use crate::MatchboxSocket;
    pub use matchbox_socket::{
        ChannelConfig, MultipleChannels, PeerId, PeerState, SingleChannel, WebRtcSocketBuilder,
    };
}
