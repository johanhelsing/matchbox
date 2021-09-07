use clap::Clap;
use serde::Deserialize;
use std::{ffi::OsString, net::SocketAddr};

#[derive(Clap, Debug, Clone, Deserialize)]
#[serde(default)]
#[clap(
    name = "box_game_web",
    rename_all = "kebab-case",
    rename_all_env = "screaming-snake"
)]
pub struct Args {
    #[clap(short, long, default_value = "1235")]
    pub local_port: u16,

    #[clap(short, long)]
    pub players: Vec<String>,

    #[clap(short, long)]
    pub spectators: Vec<SocketAddr>,

    #[clap(default_value = "0", env)]
    pub player_handle: usize,

    // #[clap(default_value = "ws://127.0.0.1:3536/room_a")]
    #[clap(default_value = "wss://match.johanhelsing.studio/room_a")]
    pub room_url: String,
}

impl Default for Args {
    fn default() -> Self {
        let args = Vec::<OsString>::new();
        Args::parse_from(args)
    }
}

impl Args {
    pub fn get() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let qs = web_sys::window()
                .unwrap()
                .location()
                .search()
                .unwrap()
                .trim_start_matches("?")
                .to_owned();

            let js = qs.clone().into();
            web_sys::console::log_1(&js);

            Args::from_query(&qs)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Args::parse()
        }
    }

    // #[allow(dead_code)]
    #[cfg(target_arch = "wasm32")]
    fn from_query(query: &str) -> Self {
        // TODO: result?
        serde_qs::from_str(query).unwrap()
    }
}
