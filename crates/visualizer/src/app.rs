use bevy::prelude::*;
use crossbeam_channel as xchan;
use interface::interface::{ServerMsg, server_msg};
use std::time::{Duration, Instant};

fn limit_fps_system(mut last: Local<Option<Instant>>) {
    let target = Duration::from_millis(100); // 16 ms == ~60 FPS
    let now = Instant::now();

    if let Some(prev) = *last {
        let elapsed = now.duration_since(prev);
        if elapsed < target {
            std::thread::sleep(target - elapsed);
        }
    }

    *last = Some(now);
}

#[derive(Resource)]
pub struct Inbox(pub xchan::Receiver<ServerMsg>);

#[derive(Resource)]
pub struct StepBarrier(pub xchan::Sender<()>);

#[derive(Component)]
pub struct SimObject {
    pub id: u32,
}

#[derive(Resource, Default)]
pub struct SimState {
    pub latest_tick: Option<u64>,    // newest tick from the server
    pub displayed_tick: Option<u64>, // tick currently shown on screen
}

#[derive(Component)]
pub struct TickText;

pub fn run_bevy(rx_app: xchan::Receiver<ServerMsg>, tx_done: xchan::Sender<()>) {
    App::new()
        .add_plugins(DefaultPlugins.build().disable::<bevy::log::LogPlugin>())
        .insert_resource(Inbox(rx_app))
        .insert_resource(StepBarrier(tx_done))
        .add_systems(Startup, (setup_scene, setup_ui))
        .insert_resource(SimState::default())
        .add_systems(Update, (drain_inbox, update_tick_ui, drain_and_apply_state))
        .add_systems(
            Last,
            (
                advance_displayed_tick,
                limit_fps_system,
                notify_worker_frame_done,
            )
                .chain(),
        )
        .run();
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
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
        SimObject { id: 1 },
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));
}

fn setup_ui(mut commands: Commands, asset_server: Res<AssetServer>) {
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

fn notify_worker_frame_done(barrier: Res<StepBarrier>) {
    // non-blocking send; ignore if full
    let _ = barrier.0.try_send(());
}

fn drain_and_apply_state(inbox: Res<Inbox>, mut q: Query<(&SimObject, &mut Transform)>) {
    for msg in inbox.0.try_iter() {
        if let Some(server_msg::Msg::State(_obs)) = msg.msg {
            for (_obj, mut _tf) in q.iter_mut() {
                // TODO: update transform from sim state.
            }
        }
    }
}

fn drain_inbox(inbox: Res<Inbox>, mut state: ResMut<SimState>) {
    for msg in inbox.0.try_iter() {
        match msg.msg {
            Some(server_msg::Msg::Tick(t)) => {
                state.latest_tick = Some(t.seq);
            }
            Some(server_msg::Msg::State(_obs)) => {
                // update object transforms etc. if needed
            }
            _ => {}
        }
    }
}

fn advance_displayed_tick(mut state: ResMut<SimState>) {
    if state.latest_tick != state.displayed_tick {
        state.displayed_tick = state.latest_tick;
    }
}
fn update_tick_ui(state: Res<SimState>, mut query: Query<&mut Text, With<TickText>>) {
    if let Some(tick) = state.displayed_tick {
        if let Ok(mut text) = query.single_mut() {
            *text = Text::new(format!("Tick: {}", tick));
        }
    }
}
