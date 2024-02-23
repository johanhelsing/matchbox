use bevy::{log::LogPlugin, prelude::*};
use bevy_ggrs::{ggrs::SessionBuilder, GgrsApp, GgrsPlugin, GgrsSchedule, ReadInputs, Session};
use bevy_matchbox::prelude::*;

mod args;
mod box_game;

use args::*;
use box_game::*;

const FPS: usize = 60;

#[derive(Debug, Clone, Default, Eq, PartialEq, Hash, States)]
enum AppState {
    #[default]
    Lobby,
    InGame,
}

const SKY_COLOR: Color = Color::rgb(0.69, 0.69, 0.69);

fn main() {
    // read query string or command line arguments
    let args = Args::get();
    info!("{args:?}");

    App::new()
        .add_plugins(GgrsPlugin::<BoxConfig>::default())
        .set_rollback_schedule_fps(FPS)
        .add_systems(ReadInputs, read_local_inputs)
        // Rollback behavior can be customized using a variety of extension methods and plugins:
        // The FrameCount resource implements Copy, we can use that to have minimal overhead rollback
        .rollback_resource_with_copy::<FrameCount>()
        // Transform and Velocity components only implement Clone, so instead we'll use that to snapshot and rollback with
        .rollback_component_with_clone::<Transform>()
        .rollback_component_with_clone::<Velocity>()
        .insert_resource(ClearColor(SKY_COLOR))
        .add_plugins(DefaultPlugins.set(LogPlugin {
            filter: "info,wgpu_core=warn,wgpu_hal=warn,matchbox_socket=debug".into(),
            level: bevy::log::Level::DEBUG,
            ..default()
        }))
        // Some of our systems need the query parameters
        .insert_resource(args)
        .init_resource::<FrameCount>()
        .init_state::<AppState>()
        .add_systems(
            OnEnter(AppState::Lobby),
            (lobby_startup, start_matchbox_socket),
        )
        .add_systems(Update, lobby_system.run_if(in_state(AppState::Lobby)))
        .add_systems(OnExit(AppState::Lobby), lobby_cleanup)
        .add_systems(OnEnter(AppState::InGame), setup_scene)
        .add_systems(Update, log_ggrs_events.run_if(in_state(AppState::InGame)))
        // these systems will be executed as part of the advance frame update
        .add_systems(GgrsSchedule, (move_cube_system, increase_frame_system))
        .run();
}

fn start_matchbox_socket(mut commands: Commands, args: Res<Args>) {
    let room_id = match &args.room {
        Some(id) => id.clone(),
        None => format!("bevy_ggrs?next={}", &args.players),
    };

    let room_url = format!("{}/{}", &args.matchbox, room_id);
    info!("connecting to matchbox server: {room_url:?}");

    commands.insert_resource(MatchboxSocket::new_ggrs(room_url));
}

// Marker components for UI
#[derive(Component)]
struct LobbyText;
#[derive(Component)]
struct LobbyUI;

fn lobby_startup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(Camera3dBundle::default());

    // All this is just for spawning centered text.
    commands
        .spawn(NodeBundle {
            style: Style {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::FlexEnd,
                ..default()
            },
            background_color: Color::rgb(0.43, 0.41, 0.38).into(),
            ..default()
        })
        .with_children(|parent| {
            parent
                .spawn(TextBundle {
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
    mut app_state: ResMut<NextState<AppState>>,
    args: Res<Args>,
    mut socket: ResMut<MatchboxSocket<SingleChannel>>,
    mut commands: Commands,
    mut query: Query<&mut Text, With<LobbyText>>,
) {
    // regularly call update_peers to update the list of connected peers
    let Ok(peer_changes) = socket.try_update_peers() else {
        warn!("socket dropped");
        return;
    };

    for (peer, new_state) in peer_changes {
        // you can also handle the specific dis(connections) as they occur:
        match new_state {
            PeerState::Connected => info!("peer {peer} connected"),
            PeerState::Disconnected => info!("peer {peer} disconnected"),
        }
    }

    let connected_peers = socket.connected_peers().count();
    let remaining = args.players - (connected_peers + 1);
    query.single_mut().sections[0].value = format!("Waiting for {remaining} more player(s)",);
    if remaining > 0 {
        return;
    }

    info!("All peers have joined, going in-game");

    // extract final player list
    let players = socket.players();

    let max_prediction = 12;

    // create a GGRS P2P session
    let mut sess_build = SessionBuilder::<BoxConfig>::new()
        .with_num_players(args.players)
        .with_max_prediction_window(max_prediction)
        .unwrap()
        .with_input_delay(2)
        .with_fps(FPS)
        .expect("invalid fps");

    for (i, player) in players.into_iter().enumerate() {
        sess_build = sess_build
            .add_player(player, i)
            .expect("failed to add player");
    }

    let channel = socket.take_channel(0).unwrap();

    // start the GGRS session
    let sess = sess_build
        .start_p2p_session(channel)
        .expect("failed to start session");

    commands.insert_resource(Session::P2P(sess));

    // transition to in-game state
    app_state.set(AppState::InGame);
}

fn log_ggrs_events(mut session: ResMut<Session<BoxConfig>>) {
    match session.as_mut() {
        Session::P2P(s) => {
            for event in s.events() {
                info!("GGRS Event: {event:?}");
            }
        }
        _ => panic!("This example focuses on p2p."),
    }
}
