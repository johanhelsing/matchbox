use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(feature = "bevy-plugin")] {
        mod bevy_plugin;
        pub use bevy_plugin::*;
    }
}
#[cfg(feature = "ggrs-socket")]
mod ggrs_socket;
