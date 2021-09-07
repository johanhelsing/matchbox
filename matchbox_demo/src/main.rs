use bevy::{core::FixedTimestep, prelude::*};
use bevy_ggrs::{GGRSApp, GGRSPlugin};
use futures::{
    executor::LocalPool,
    lock::Mutex,
    task::{noop_waker_ref, LocalSpawnExt},
    FutureExt,
};
use ggrs::PlayerType;
use matchbox_peer::WebRtcNonBlockingSocket;
use std::{sync::Arc, task::Context};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    console::{log_1, log_2},
    Request, RequestInit, RequestMode,
};

mod args;
mod box_game;

use args::*;
use box_game::*;

const INPUT_SIZE: usize = std::mem::size_of::<u8>();
const FPS: u32 = 60;

fn main() {
    #[cfg(target_arch = "wasm32")]
    {
        // When building for WASM, print panics to the browser console
        console_error_panic_hook::set_once();
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
    log_1(&JsValue::from(format!("{:?}", args)));

    let (mut socket, message_loop) = WebRtcNonBlockingSocket::new(&args.room_url);

    let mut pool = LocalPool::new();
    pool.spawner()
        .spawn_local(message_loop)
        .expect("couldn't spawn message loop");

    {
        let mut peers_future = Box::pin(socket.wait_for_peers(args.num_players - 1));
        // Super-stupid busy wait before we can start bevy
        // TODO: should add support in bevy_ggrs for starting after bevy has started
        // if it isn't already supported?
        // Once we get rid of this hack, we can probably make main non-async again,
        // and get rid of all the future-related dependencies.
        let waker = noop_waker_ref();
        let mut ctx = Context::from_waker(waker);
        while let std::task::Poll::Pending = peers_future.poll_unpin(&mut ctx) {
            {
                let mut opts = RequestInit::new();
                opts.method("GET");
                opts.mode(RequestMode::Cors);

                // This is just a bogus request in order to pass the time
                let window = web_sys::window().unwrap();
                let url = window.location().href().unwrap();
                let request = Request::new_with_str_and_init(&url, &opts).unwrap();
                request.headers().set("Accept", "text/html").unwrap();
                let _ = JsFuture::from(window.fetch_with_request(&request)).await;
            }
            pool.run_until_stalled();
        }
    }

    let peers = socket.connected_peers();

    // create a GGRS P2P session
    let mut p2p_session =
        ggrs::start_p2p_session_with_socket(args.num_players as u32, INPUT_SIZE, socket)
            .expect("failed to start with socket");

    // turn on sparse saving
    p2p_session.set_sparse_saving(true)?;

    let handle_js = JsValue::from(args.player_handle as i32);
    log_2(&"Adding local player with handle".into(), &handle_js);
    p2p_session
        .add_player(PlayerType::Local, args.player_handle)
        .expect("failed to add local player");

    for addr in peers {
        let handle = (args.player_handle + 1) % 2;
        // TODO: Need some way of mapping between socket id/addrs and handles
        // (we don't know them before the app starts)
        let handle_js = JsValue::from(handle as i32);
        log_2(&"Adding remote player with handle".into(), &handle_js);
        p2p_session
            .add_player(PlayerType::Remote(addr.clone()), handle)
            .expect("failed to add remote player");
    }

    // set input delay for the local player
    p2p_session.set_frame_delay(2, args.player_handle)?;

    // set default expected update frequency (affects synchronization timings between players)
    p2p_session.set_fps(FPS)?;

    // start the GGRS session
    p2p_session.start_session()?;

    let mut app = App::new();

    app.insert_resource(Msaa { samples: 4 })
        .add_plugins(DefaultPlugins);

    #[cfg(target_arch = "wasm32")]
    app.add_plugin(bevy_webgl2::WebGL2Plugin);

    app.add_startup_system(setup_system.system())
        .insert_resource(SocketTaskPool(Arc::new(Mutex::new(pool))))
        // Make sure something polls the message tasks regularly
        .add_system(process_socket_tasks)
        .add_plugin(GGRSPlugin)
        // add your GGRS session
        .with_p2p_session(p2p_session)
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
        .add_rollback_system(increase_frame_system)
        .run();

    Ok(())
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
