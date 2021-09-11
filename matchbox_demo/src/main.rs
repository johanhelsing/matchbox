use bevy::{core::FixedTimestep, prelude::*};
use bevy_ggrs::{CommandsExt, GGRSApp, GGRSPlugin};
use futures::{executor::LocalPool, lock::Mutex, task::LocalSpawnExt};
use log::info;
use matchbox_socket::WebRtcNonBlockingSocket;
use std::sync::Arc;

mod args;
mod box_game;

use args::*;
use box_game::*;

const INPUT_SIZE: usize = std::mem::size_of::<u8>();
const FPS: u32 = 60;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum AppState {
    Lobby,
    InGame,
}

fn main() {
    #[cfg(target_arch = "wasm32")]
    {
        // When building for WASM, print panics to the browser console
        console_error_panic_hook::set_once();
        console_log::init_with_level(log::Level::Debug).expect("Failed to init logs");
        log::info!("log::info logs are working");
        wasm_bindgen_futures::spawn_local(async move {
            main_async().await.expect("main failed");
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    futures::executor::block_on(async move {
        main_async().await.expect("main failed");
    })
}

async fn main_async() -> Result<(), Box<dyn std::error::Error>> {
    // read query string or command line arguments
    let args = Args::get();
    info!("{:?}", args);

    let room_id = match &args.room_id {
        Some(id) => id.clone(),
        None => format!("next_{}", &args.num_players),
    };
    let room_url = format!("{}/{}", &args.matchbox, room_id);
    info!("connecting to {:?}", room_url);
    let (socket, message_loop) = WebRtcNonBlockingSocket::new(&room_url).await;

    let pool = LocalPool::new();
    pool.spawner()
        .spawn_local(message_loop)
        .expect("couldn't spawn message loop");

    let mut app = App::new();

    app.insert_resource(Msaa { samples: 4 })
        .add_plugins(DefaultPlugins);

    #[cfg(target_arch = "wasm32")]
    app.add_plugin(bevy_webgl2::WebGL2Plugin);

    app.insert_resource(args)
        .insert_resource(SocketTaskPool(Arc::new(Mutex::new(pool))))
        // Make sure something polls the message tasks regularly
        .add_system(process_socket_tasks)
        .add_plugin(GGRSPlugin)
        .insert_resource(Some(socket))
        // define frequency of game logic update
        .with_rollback_run_criteria(FixedTimestep::steps_per_second(FPS as f64))
        // define system that represents your inputs as a byte vector, so GGRS can send the inputs around
        .with_input_system(input.system())
        // register components that will be loaded/saved
        .register_rollback_type::<Transform>()
        .register_rollback_type::<Velocity>()
        // you can also register resources
        .insert_resource(FrameCount { frame: 0 })
        .register_rollback_type::<FrameCount>()
        // these systems will be executed as part of the advance frame update
        .add_rollback_system(move_cube_system)
        .add_rollback_system(increase_frame_system);

    app.add_state(AppState::Lobby)
        .add_system_set(SystemSet::on_update(AppState::Lobby).with_system(lobby_system))
        .add_system_set(SystemSet::on_enter(AppState::InGame).with_system(setup_scene_system));

    app.run();

    Ok(())
}

fn lobby_system(
    mut app_state: ResMut<State<AppState>>,
    args: Res<Args>,
    mut socket: ResMut<Option<WebRtcNonBlockingSocket>>,
    mut commands: Commands,
) {
    let socket = socket.as_mut();

    socket.as_mut().unwrap().accept_new_connections();

    let current_players = socket.as_ref().unwrap().connected_peers().len();
    if current_players + 1 < args.num_players {
        info!("not enough players: {}", current_players);
        return;
    }

    info!("All peers have joined, going in-game");

    // consume the socket (currently required because ggrs takes ownership of its socket)
    let socket = socket.take().unwrap();

    // extract final player list
    let players = socket.players();
    let player_handle = players
        .iter()
        .enumerate()
        .filter(|(_, player_type)| match player_type {
            ggrs::PlayerType::Local => true,
            _ => false,
        })
        .map(|(i, _)| i)
        .next()
        .expect("Couldn't get local player handle");

    // create a GGRS P2P session
    let mut p2p_session =
        ggrs::new_p2p_session_with_socket(args.num_players as u32, INPUT_SIZE, socket)
            .expect("failed to start with socket");

    // turn on sparse saving
    p2p_session.set_sparse_saving(true).unwrap();

    info!("Adding local player with handle: {}", player_handle);

    for (i, player) in players.into_iter().enumerate() {
        p2p_session
            .add_player(player, i)
            .expect("failed to add local player");
    }

    // set input delay for the local player
    p2p_session.set_frame_delay(2, player_handle).unwrap();

    // set default expected update frequency (affects synchronization timings between players)
    p2p_session.set_fps(FPS).unwrap();

    // start the GGRS session
    commands.start_p2p_session(p2p_session);

    // transition to in-game state
    app_state
        .set(AppState::InGame)
        .expect("Tried to go in-game while already in-game");
}

// TODO: it would probably make sense to put the below into a bevy_matchbox crate

// In single-threaded wasm it's probably a bit overkill to use an Arc<Mutex>,
// but if we want to add native support later, this is the way to go.
// ...or if web-browsers get proper threads someday...
struct SocketTaskPool(Arc<Mutex<LocalPool>>);
unsafe impl Send for SocketTaskPool {}
unsafe impl Sync for SocketTaskPool {}

fn process_socket_tasks(pool: ResMut<SocketTaskPool>) {
    let mut pool = pool.0.try_lock().expect("Couldn't lock socket task pool");
    pool.run_until_stalled();
}
