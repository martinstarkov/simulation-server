use bevy::prelude::*;
use crossbeam_channel as xchan;
use interface::{ServerMsg, ServerMsgBody};

const TARGET_FPS: f64 = 1.0;

#[derive(Resource)]
struct FpsLimiter(Timer);

#[derive(Resource)]
pub struct Inbox(pub xchan::Receiver<ServerMsg>);

#[derive(Resource)]
pub struct StepBarrier(pub xchan::Sender<()>);

#[derive(Component)]
pub struct SimObject {
    pub _id: u32,
}

#[derive(Resource, Default)]
pub struct SimState {
    pub tick: u64, // newest tick from the server
}

#[derive(Component)]
pub struct TickText;

pub fn run_bevy(rx_app: xchan::Receiver<ServerMsg>, tx_done: xchan::Sender<()>) {
    App::new()
        .add_plugins(DefaultPlugins.build().disable::<bevy::log::LogPlugin>())
        .insert_resource(Inbox(rx_app))
        .insert_resource(StepBarrier(tx_done))
        .insert_resource(SimState::default())
        .add_systems(Startup, (setup_scene, setup_ui))
        //.add_systems(Update, send_initial_step_ready_when_window_ready)
        .add_systems(Update, (drain_inbox, update_tick_ui, run_sim_logic).chain())
        .run();
}

fn run_sim_logic(
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut limiter: ResMut<FpsLimiter>,
    barrier: Res<StepBarrier>,
) {
    limiter.0.tick(time.delta());

    // Only run this logic when timer fires
    if limiter.0.is_finished() || keyboard.pressed(KeyCode::Space) {
        let _ = barrier.0.try_send(());
    }
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(FpsLimiter(Timer::from_seconds(
        1.0 / TARGET_FPS as f32,
        TimerMode::Repeating,
    )));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 5.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let mesh = meshes.add(Mesh::from(Cuboid::from_length(1.0)));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 0.8, 0.9),
        ..default()
    });

    commands.spawn((
        SimObject { _id: 1 },
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));
}

fn setup_ui(mut commands: Commands, _asset_server: Res<AssetServer>) {
    // Root UI node
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(10.0),
                left: Val::Px(10.0),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Tick: 0"),
                TextFont {
                    font: default(),
                    font_size: 24.0,
                    ..default()
                },
                TextColor(Color::srgb(1.0, 0.84, 0.0)), // “gold”-ish color
                TickText,
            ));
        });
}

fn drain_inbox(
    inbox: Res<Inbox>,
    mut state: ResMut<SimState>,
    mut q: Query<(&SimObject, &mut Transform)>,
) {
    for msg in inbox.0.try_iter() {
        match msg.body {
            Some(ServerMsgBody::Tick(t)) => {
                state.tick = t.seq;
                println!("Received tick: {}", t.seq);
            }
            Some(ServerMsgBody::State(_obs)) => {
                println!("Received state");
                for (_obj, mut _tf) in q.iter_mut() {
                    // TODO: update transform from sim state.
                }
            }
            _ => {}
        }
    }
}

fn update_tick_ui(state: Res<SimState>, mut query: Query<&mut Text, With<TickText>>) {
    if let Ok(mut text) = query.single_mut() {
        text.0 = format!("Tick: {}", state.tick);
    }
}
