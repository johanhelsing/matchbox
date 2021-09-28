use bevy::{prelude::*, tasks::IoTaskPool};
use bevy_ggrs::{CommandsExt, GGRSApp, GGRSPlugin};
use ggrs::PlayerType;
use log::info;
use matchbox_socket::WebRtcNonBlockingSocket;

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

const SKY_COLOR: Color = Color::rgb(0.69, 0.69, 0.69);

fn main() {
    // read query string or command line arguments
    let args = Args::get();
    info!("{:?}", args);

    let mut app = App::new();
    app.insert_resource(Msaa { samples: 4 })
        .insert_resource(ClearColor(SKY_COLOR))
        .add_plugins(DefaultPlugins)
        // Some of our systems need the query parameters
        .insert_resource(args)
        // Make sure something polls the message tasks regularly
        .add_plugin(GGRSPlugin)
        // define frequency of game logic update
        .with_update_frequency(FPS)
        // define system that represents your inputs as a byte vector, so GGRS can send the inputs around
        .with_input_system(input)
        // register components that will be loaded/saved
        .register_rollback_type::<Transform>()
        .register_rollback_type::<Velocity>()
        // you can also register resources
        .insert_resource(FrameCount { frame: 0 })
        .register_rollback_type::<FrameCount>()
        // these systems will be executed as part of the advance frame update
        .with_rollback_schedule(
            Schedule::default().with_stage(
                "rollback_default",
                SystemStage::single_threaded()
                    .with_system(move_cube_system)
                    .with_system(increase_frame_system),
            ),
        )
        .add_state(AppState::Lobby)
        .add_system_set(
            SystemSet::on_enter(AppState::Lobby)
                .with_system(lobby_startup)
                .with_system(start_matchbox_socket),
        )
        .add_system_set(SystemSet::on_update(AppState::Lobby).with_system(lobby_system))
        .add_system_set(SystemSet::on_exit(AppState::Lobby).with_system(lobby_cleanup))
        .add_system_set(SystemSet::on_enter(AppState::InGame).with_system(setup_scene_system))
        .run();
}

fn start_matchbox_socket(mut commands: Commands, args: Res<Args>, task_pool: Res<IoTaskPool>) {
    let room_id = match &args.room {
        Some(id) => id.clone(),
        None => format!("next_{}", &args.players),
    };

    let room_url = format!("{}/{}", &args.matchbox, room_id);
    info!("connecting to matchbox server: {:?}", room_url);
    let (socket, message_loop) = WebRtcNonBlockingSocket::new(room_url);

    // The message loop needs to be awaited, or nothing will happen.
    // We do this here using bevy's task system.
    task_pool.spawn(message_loop).detach();

    commands.insert_resource(Some(socket));
}

// Marker components for UI
#[derive(Component)]
struct LobbyText;
#[derive(Component)]
struct LobbyUI;

fn lobby_startup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // All this is just for spawning centered text.
    commands.spawn_bundle(UiCameraBundle::default());
    commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                position_type: PositionType::Absolute,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::FlexEnd,
                ..Default::default()
            },
            color: Color::rgb(0.43, 0.41, 0.38).into(),
            ..Default::default()
        })
        .with_children(|parent| {
            parent
                .spawn_bundle(TextBundle {
                    style: Style {
                        align_self: AlignSelf::Center,
                        justify_content: JustifyContent::Center,
                        ..Default::default()
                    },
                    text: Text::with_section(
                        "Entering lobby...",
                        TextStyle {
                            font: asset_server.load("fonts/quicksand-light.ttf"),
                            font_size: 96.,
                            color: Color::BLACK,
                        },
                        Default::default(),
                    ),
                    ..Default::default()
                })
                .insert(LobbyText);
        })
        .insert(LobbyUI);
}

fn lobby_cleanup(query: Query<Entity, With<LobbyUI>>, mut commands: Commands) {
    for e in query.iter() {
        commands.entity(e).despawn_recursive();
    }
}

fn lobby_system(
    mut app_state: ResMut<State<AppState>>,
    args: Res<Args>,
    mut socket: ResMut<Option<WebRtcNonBlockingSocket>>,
    mut commands: Commands,
    mut query: Query<&mut Text, With<LobbyText>>,
) {
    let socket = socket.as_mut();

    socket.as_mut().unwrap().accept_new_connections();
    let connected_peers = socket.as_ref().unwrap().connected_peers().len();
    let remaining = args.players - (connected_peers + 1);
    query.single_mut().sections[0].value = format!("Waiting for {} more player(s)", remaining);

    if remaining > 0 {
        return;
    }

    info!("All peers have joined, going in-game");

    // consume the socket (currently required because ggrs takes ownership of its socket)
    let socket = socket.take().unwrap();

    // extract final player list
    let players = socket.players();

    // create a GGRS P2P session
    let mut p2p_session =
        ggrs::P2PSession::new_with_socket(args.players as u32, INPUT_SIZE, socket);

    // turn on sparse saving
    p2p_session.set_sparse_saving(true).unwrap();

    for (i, player) in players.into_iter().enumerate() {
        p2p_session
            .add_player(player, i)
            .expect("failed to add player");

        if player == PlayerType::Local {
            // set input delay for the local player
            p2p_session.set_frame_delay(2, i).unwrap();
        }
    }

    // set default expected update frequency (affects synchronization timings between players)
    p2p_session.set_fps(FPS).unwrap();

    // start the GGRS session
    commands.start_p2p_session(p2p_session);

    // transition to in-game state
    app_state
        .set(AppState::InGame)
        .expect("Tried to go in-game while already in-game");
}
