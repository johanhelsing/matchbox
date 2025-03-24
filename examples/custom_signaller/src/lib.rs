pub(crate) mod direct_message;
mod iroh_gossip_signaller;
pub use iroh_gossip_signaller::IrohGossipSignallerBuilder;

#[cfg(target_arch = "wasm32")]
pub mod get_browser_url;

pub fn get_timestamp() -> u128 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap()
        .as_micros()
}
