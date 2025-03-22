pub(crate) mod direct_message;
mod iroh_gossip_signaller;
pub use iroh_gossip_signaller::IrohGossipSignallerBuilder;

pub fn get_timestamp() -> u64 {
    web_time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

