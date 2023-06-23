#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

use cfg_if::cfg_if;

mod socket;
pub use socket::*;

cfg_if! {
    if #[cfg(all(not(target_arch = "wasm32"), feature = "signaling"))] {
        mod signaling;
        pub use signaling::*;
    }
}

/// use `bevy_matchbox::prelude::*;` to import common resources and commands
pub mod prelude {
    pub use crate::{CloseSocketExt, MatchboxSocket, OpenSocketExt};
    use cfg_if::cfg_if;
    pub use matchbox_socket::{
        BuildablePlurality, ChannelConfig, MultipleChannels, PeerId, PeerState, SingleChannel,
        WebRtcSocketBuilder,
    };

    cfg_if! {
        if #[cfg(all(not(target_arch = "wasm32"), feature = "signaling"))] {
            pub use crate::signaling::{MatchboxServer, StartServerExt, StopServerExt};
            pub use matchbox_signaling::SignalingServerBuilder;
        }
    }
}
