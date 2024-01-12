use bevy::{prelude::*, utils::HashMap};
use bevy_ggrs::{
    AddRollbackCommandExtension, GgrsConfig, LocalInputs, LocalPlayers, PlayerInputs, Rollback,
    Session,
};
use bevy_matchbox::prelude::PeerId;
use bytemuck::{Pod, Zeroable};
use std::hash::Hash;

const BLUE: Color = Color::rgb(0.8, 0.6, 0.2);
const ORANGE: Color = Color::rgb(0., 0.35, 0.8);
const MAGENTA: Color = Color::rgb(0.9, 0.2, 0.2);
const GREEN: Color = Color::rgb(0.35, 0.7, 0.35);
const PLAYER_COLORS: [Color; 4] = [BLUE, ORANGE, MAGENTA, GREEN];

const INPUT_UP: u8 = 1 << 0;
const INPUT_DOWN: u8 = 1 << 1;
const INPUT_LEFT: u8 = 1 << 2;
const INPUT_RIGHT: u8 = 1 << 3;

const ACCELERATION: f32 = 18.0;
const MAX_SPEED: f32 = 3.0;
const FRICTION: f32 = 0.0018;
const PLANE_SIZE: f32 = 5.0;
const CUBE_SIZE: f32 = 0.2;

// You need to define a config struct to bundle all the generics of GGRS. bevy_ggrs provides a sensible default in `GgrsConfig`.
// (optional) You can define a type here for brevity.
pub type BoxConfig = GgrsConfig<BoxInput, PeerId>;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct BoxInput {
    pub inp: u8,
}

#[derive(Default, Component)]
pub struct Player {
    pub handle: usize,
}

// Components that should be saved/loaded need to support snapshotting. The built-in options are:
// - Clone (Recommended)
// - Copy
// - Reflect
// See `bevy_ggrs::Strategy` for custom alternatives
#[derive(Default, Reflect, Component, Clone)]
pub struct Velocity {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

// You can also register resources.
#[derive(Resource, Default, Reflect, Hash, Clone, Copy)]
#[reflect(Hash)]
pub struct FrameCount {
    pub frame: u32,
}

/// Collects player inputs during [`ReadInputs`](`bevy_ggrs::ReadInputs`) and creates a [`LocalInputs`] resource.
pub fn read_local_inputs(
    mut commands: Commands,
    keyboard_input: Res<Input<KeyCode>>,
    local_players: Res<LocalPlayers>,
) {
    let mut local_inputs = HashMap::new();

    for handle in &local_players.0 {
        let mut input: u8 = 0;

        if keyboard_input.pressed(KeyCode::W) {
            input |= INPUT_UP;
        }
        if keyboard_input.pressed(KeyCode::A) {
            input |= INPUT_LEFT;
        }
        if keyboard_input.pressed(KeyCode::S) {
            input |= INPUT_DOWN;
        }
        if keyboard_input.pressed(KeyCode::D) {
            input |= INPUT_RIGHT;
        }

        local_inputs.insert(*handle, BoxInput { inp: input });
    }

    commands.insert_resource(LocalInputs::<BoxConfig>(local_inputs));
}

pub fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    session: Res<Session<BoxConfig>>,
    mut camera_query: Query<&mut Transform, With<Camera>>,
) {
    let num_players = match &*session {
        Session::SyncTest(s) => s.num_players(),
        Session::P2P(s) => s.num_players(),
        Session::Spectator(s) => s.num_players(),
    };

    // A ground plane
    commands.spawn(PbrBundle {
        mesh: meshes.add(Mesh::from(shape::Plane {
            size: PLANE_SIZE,
            ..default()
        })),
        material: materials.add(Color::rgb(0.3, 0.5, 0.3).into()),
        ..default()
    });

    let r = PLANE_SIZE / 4.;

    for handle in 0..num_players {
        let rot = handle as f32 / num_players as f32 * 2. * std::f32::consts::PI;
        let x = r * rot.cos();
        let z = r * rot.sin();

        let mut transform = Transform::default();
        transform.translation.x = x;
        transform.translation.y = CUBE_SIZE / 2.;
        transform.translation.z = z;
        let color = PLAYER_COLORS[handle % PLAYER_COLORS.len()];

        // Entities which will be rolled back can be created just like any other...
        commands
            .spawn((
                // ...add visual information...
                PbrBundle {
                    mesh: meshes.add(Mesh::from(shape::Cube { size: CUBE_SIZE })),
                    material: materials.add(color.into()),
                    transform,
                    ..default()
                },
                // ...flags...
                Player { handle },
                // ...and components which will be rolled-back...
                Velocity::default(),
            ))
            // ...just ensure you call `add_rollback()`
            // This ensures a stable ID is available for the rollback system to refer to
            .add_rollback();
    }

    // light
    commands.spawn(PointLightBundle {
        transform: Transform::from_xyz(-4.0, 8.0, 4.0),
        ..default()
    });
    // camera
    for mut transform in camera_query.iter_mut() {
        *transform = Transform::from_xyz(-2.0, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y);
    }
}

// Example system, manipulating a resource, will be added to the rollback schedule.
// Increases the frame count by 1 every update step. If loading and saving resources works correctly,
// you should see this resource rolling back, counting back up and finally increasing by 1 every update step
#[allow(dead_code)]
pub fn increase_frame_system(mut frame_count: ResMut<FrameCount>) {
    frame_count.frame += 1;
}

// Example system that moves the cubes, will be added to the rollback schedule.
// Filtering for the rollback component is a good way to make sure your game logic systems
// only mutate components that are being saved/loaded.
#[allow(dead_code)]
pub fn move_cube_system(
    mut query: Query<(&mut Transform, &mut Velocity, &Player), With<Rollback>>,
    //                                                              ^------^ Added by `add_rollback` earlier
    inputs: Res<PlayerInputs<BoxConfig>>,
    // Thanks to RollbackTimePlugin, this is rollback safe
    time: Res<Time>,
) {
    let dt = time.delta().as_secs_f32();

    for (mut t, mut v, p) in query.iter_mut() {
        let input = inputs[p.handle].0.inp;
        // set velocity through key presses
        if input & INPUT_UP != 0 && input & INPUT_DOWN == 0 {
            v.z -= ACCELERATION * dt;
        }
        if input & INPUT_UP == 0 && input & INPUT_DOWN != 0 {
            v.z += ACCELERATION * dt;
        }
        if input & INPUT_LEFT != 0 && input & INPUT_RIGHT == 0 {
            v.x -= ACCELERATION * dt;
        }
        if input & INPUT_LEFT == 0 && input & INPUT_RIGHT != 0 {
            v.x += ACCELERATION * dt;
        }

        // slow down
        if input & INPUT_UP == 0 && input & INPUT_DOWN == 0 {
            v.z *= FRICTION.powf(dt);
        }
        if input & INPUT_LEFT == 0 && input & INPUT_RIGHT == 0 {
            v.x *= FRICTION.powf(dt);
        }
        v.y *= FRICTION.powf(dt);

        // constrain velocity
        let mag = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        if mag > MAX_SPEED {
            let factor = MAX_SPEED / mag;
            v.x *= factor;
            v.y *= factor;
            v.z *= factor;
        }

        // apply velocity
        t.translation.x += v.x * dt;
        t.translation.y += v.y * dt;
        t.translation.z += v.z * dt;

        // constrain cube to plane
        t.translation.x = t.translation.x.max(-1. * (PLANE_SIZE - CUBE_SIZE) * 0.5);
        t.translation.x = t.translation.x.min((PLANE_SIZE - CUBE_SIZE) * 0.5);
        t.translation.z = t.translation.z.max(-1. * (PLANE_SIZE - CUBE_SIZE) * 0.5);
        t.translation.z = t.translation.z.min((PLANE_SIZE - CUBE_SIZE) * 0.5);
    }
}
