#![warn(missing_docs)]
#![doc = "A flecs-rust integration for the matchbox WebRTC networking library, inspired by bevy_matchbox."]
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

/// use `flecs_matchbox::prelude::*;` to import common types and traits.
pub mod prelude {
    pub use crate::{CloseSocketExt, FlecsMatchboxSocket, OpenSocketExt};
    use cfg_if::cfg_if;
    pub use matchbox_socket::{ChannelConfig, PeerId, PeerState, WebRtcSocketBuilder};

    cfg_if! {
        if #[cfg(all(not(target_arch = "wasm32"), feature = "signaling"))] {
            pub use crate::signaling::{FlecsMatchboxServer, StartServerExt, StopServerExt};
            pub use matchbox_signaling::SignalingServerBuilder;
        }
    }
}
