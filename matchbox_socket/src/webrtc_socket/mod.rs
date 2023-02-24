use crate::Error;
use cfg_if::cfg_if;
use futures::Future;
use messages::*;
use std::pin::Pin;
mod channels;
mod messages;
mod signal_peer;
mod socket;

// TODO: Should be a WebRtcConfig field
/// The duration, in milliseconds, to send "Keep Alive" requests
const KEEP_ALIVE_INTERVAL: u64 = 10_000;

/// The raw format of data being sent and received.
type Packet = Box<[u8]>;

pub(crate) mod error;
pub(crate) use socket::MessageLoopChannels;
pub use socket::{ChannelConfig, RtcIceServerConfig, WebRtcSocket, WebRtcSocketConfig};
cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        // TODO: figure out if it's possible to implement Send in wasm as well
        type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>>>>;
        mod wasm {
            mod message_loop;
            mod signalling_loop;
            pub use message_loop::*;
            pub use signalling_loop::*;
        }
        use wasm::*;
    } else {
        type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;
        mod native {
            mod message_loop;
            mod signalling_loop;
            pub use message_loop::*;
            pub use signalling_loop::*;
        }
        use native::*;
    }
}
