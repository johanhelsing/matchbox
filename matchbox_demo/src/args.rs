use clap::Parser;
use serde::Deserialize;
use std::ffi::OsString;

#[derive(Parser, Debug, Clone, Deserialize)]
#[serde(default)]
#[clap(
    name = "box_game_web",
    rename_all = "kebab-case",
    rename_all_env = "screaming-snake"
)]
pub struct Args {
    // #[clap(default_value = "wss://match.johanhelsing.studio")]
    #[clap(default_value = "ws://127.0.0.1:3536")]
    pub matchbox: String,

    pub room: Option<String>,

    #[clap(default_value = "2")]
    pub players: usize,
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
