pub(crate) mod direct_message;
mod iroh_gossip_signaller;
pub use iroh_gossip_signaller::IrohGossipSignallerBuilder;

pub fn get_timestamp() -> u128 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap()
        .as_micros()
}
