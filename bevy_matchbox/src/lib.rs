#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod socket;

pub use socket::*;

/// use `bevy_matchbox::prelude::*;` to import common resources and commands
pub mod prelude {
    pub use crate::{CloseSocketExt, MatchboxSocket, OpenSocketExt};
    pub use matchbox_socket::{
        BuildablePlurality, ChannelConfig, MultipleChannels, PeerId, PeerState, SingleChannel,
        WebRtcSocketBuilder,
    };
}
