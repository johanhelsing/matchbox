use bevy::{log::LogSettings, prelude::*, tasks::IoTaskPool};
use bevy_ggrs::{GGRSPlugin, SessionType};
use ggrs::{P2PSession, SessionBuilder};
use log::info;
use matchbox_socket::WebRtcSocket;

mod args;
mod box_game;

use args::*;
use box_game::*;

const FPS: usize = 60;
const ROLLBACK_DEFAULT: &str = "rollback_default";

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

    GGRSPlugin::<GGRSConfig>::new()
        // define frequency of rollback game logic update
        .with_update_frequency(FPS)
        // define system that returns inputs given a player handle, so GGRS can send the inputs around
        .with_input_system(input)
        // register types of components AND resources you want to be rolled back
        .register_rollback_type::<Transform>()
        .register_rollback_type::<Velocity>()
        .register_rollback_type::<FrameCount>()
        // these systems will be executed as part of the advance frame update
        .with_rollback_schedule(
            Schedule::default().with_stage(
                ROLLBACK_DEFAULT,
                SystemStage::parallel()
                    .with_system(move_cube_system)
                    .with_system(increase_frame_system),
            ),
        )
        // make it happen in the bevy app
        .build(&mut app);

    // (optional) Enable debug log level for matchbox
    app.insert_resource(LogSettings {
        filter: "info,wgpu_core=warn,wgpu_hal=warn,matchbox_socket=debug".into(),
        level: bevy::log::Level::DEBUG,
    });

    app.insert_resource(ClearColor(SKY_COLOR))
        .add_plugins(DefaultPlugins)
        // Some of our systems need the query parameters
        .insert_resource(args)
        .init_resource::<FrameCount>()
        .add_state(AppState::Lobby)
        .add_system_set(
            SystemSet::on_enter(AppState::Lobby)
                .with_system(lobby_startup)
                .with_system(start_matchbox_socket),
        )
        .add_system_set(SystemSet::on_update(AppState::Lobby).with_system(lobby_system))
        .add_system_set(SystemSet::on_exit(AppState::Lobby).with_system(lobby_cleanup))
        .add_system_set(SystemSet::on_enter(AppState::InGame).with_system(setup_scene_system))
        .add_system_set(SystemSet::on_update(AppState::InGame).with_system(log_ggrs_events))
        .run();
}

fn start_matchbox_socket(mut commands: Commands, args: Res<Args>) {
    let room_id = match &args.room {
        Some(id) => id.clone(),
        None => format!("example_room?next={}", &args.players),
    };

    let room_url = format!("{}/{}", &args.matchbox, room_id);
    info!("connecting to matchbox server: {:?}", room_url);
    let (socket, message_loop) = WebRtcSocket::new(room_url);

    // The message loop needs to be awaited, or nothing will happen.
    // We do this here using bevy's task system.
    let task_pool = IoTaskPool::get();
    task_pool.spawn(message_loop).detach();

    commands.insert_resource(Some(socket));
}

// Marker components for UI
#[derive(Component)]
struct LobbyText;
#[derive(Component)]
struct LobbyUI;

fn lobby_startup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn_bundle(Camera3dBundle::default());

    // All this is just for spawning centered text.
    commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                position_type: PositionType::Absolute,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::FlexEnd,
                ..default()
            },
            color: Color::rgb(0.43, 0.41, 0.38).into(),
            ..default()
        })
        .with_children(|parent| {
            parent
                .spawn_bundle(TextBundle {
                    style: Style {
                        align_self: AlignSelf::Center,
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    text: Text::from_section(
                        "Entering lobby...",
                        TextStyle {
                            font: asset_server.load("fonts/quicksand-light.ttf"),
                            font_size: 96.,
                            color: Color::BLACK,
                        },
                    ),
                    ..default()
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
    mut socket: ResMut<Option<WebRtcSocket>>,
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

    let max_prediction = 12;

    // create a GGRS P2P session
    let mut sess_build = SessionBuilder::<GGRSConfig>::new()
        .with_num_players(args.players)
        .with_max_prediction_window(max_prediction)
        .with_input_delay(2)
        .with_fps(FPS)
        .expect("invalid fps");

    for (i, player) in players.into_iter().enumerate() {
        sess_build = sess_build
            .add_player(player, i)
            .expect("failed to add player");
    }

    // start the GGRS session
    let sess = sess_build
        .start_p2p_session(socket)
        .expect("failed to start session");

    commands.insert_resource(sess);
    commands.insert_resource(SessionType::P2PSession);

    // transition to in-game state
    app_state
        .set(AppState::InGame)
        .expect("Tried to go in-game while already in-game");
}

fn log_ggrs_events(mut session: ResMut<P2PSession<GGRSConfig>>) {
    for event in session.events() {
        info!("GGRS Event: {:?}", event);
    }
}
