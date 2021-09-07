use clap::Clap;
use std::net::SocketAddr;

#[derive(Clap, Debug)]
#[clap(
    name = "made_in_heaven",
    rename_all = "kebab-case",
    rename_all_env = "screaming-snake"
)]
pub struct Args {
    #[clap(default_value = "0.0.0.0:3536", env)]
    pub host: SocketAddr,
}
