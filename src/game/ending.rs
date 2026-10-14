//! The ending (docs/re/ui_screens.md "Ending"): the last room plays itself from a recorded pad
//! stream; when the stream runs out the princess screen, staff roll and "PRESENTED BY TAITO" follow.
use super::{
    AppState,
    dialog::Dialog,
    frontend::ScreenEntity,
    input::{DemoInput, Joypad},
    room::CurrentRoom,
    transition::{FADE_TICKS, FadeTo, ScreenFade, Transition},
    ui::ScreenRoot,
};
use crate::{GameSystems, controller::Button};
use bevy::{prelude::*, sprite::Anchor};

/// The room whose playback leads into the ending (0x2F).
pub const ENDING_ROOM: usize = 47;
/// Dialogue shown when the ending room starts (0x45) and on the princess screen (0x47).
const ROOM_TEXT: u32 = 0x45;
const THANKS_TEXT: u32 = 0x47;

/// Starts the recorded input when the ending room is entered (cartridge 0x30FEE).
fn start_ending_room(
    room: Option<Res<CurrentRoom>>,
    transition: Option<Res<Transition>>,
    mut demo: ResMut<DemoInput>,
    mut dialog: ResMut<Dialog>,
    data: super::GameDataRef,
    mut started: Local<bool>,
    mut next: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    let in_room = room.is_some_and(|r| r.index == ENDING_ROOM) && transition.is_none();
    if !in_room {
        *started = false;
        return;
    }
    if !*started {
        *started = true;
        if let Some(d) = data.get() {
            demo.start(d.demos.get("ending").cloned().unwrap_or_default());
        }
        dialog.open_timed(ROOM_TEXT, 0, 0x3D, 0x51);
        return;
    }
    // Stream exhausted → fade and show the ending screens (0x40E4 → main state 4).
    if demo.finished() && !dialog.is_open() {
        demo.stop();
        commands.insert_resource(FadeTo { target: 1.0, ticks: FADE_TICKS });
        next.set(AppState::Ending);
    }
}

#[derive(Component)]
struct Princess;
#[derive(Component)]
struct EndingBackground;
#[derive(Component)]
struct StaffRoll;
#[derive(Component)]
struct FinalScreen;

#[derive(Resource, Default)]
struct EndingTimeline {
    t: u32,
    stage: u8,
    mark: u32,
}

fn enter_ending(
    mut commands: Commands,
    server: Res<AssetServer>,
    roots: Query<Entity, With<ScreenRoot>>,
    mut fade: ResMut<ScreenFade>,
    old: Query<Entity, With<super::room::RoomEntity>>,
    hud: Query<Entity, With<super::ui::Hud>>,
    players: Query<Entity, With<super::player::Player>>,
) {
    for e in old.iter().chain(hud.iter()).chain(players.iter()) {
        commands.entity(e).despawn();
    }
    let Ok(root) = roots.single() else { return };
    fade.0 = 1.0;
    commands.insert_resource(FadeTo { target: 0.0, ticks: FADE_TICKS });
    let sprite = |path: &str, x: i32, y: i32, z: f32| {
        (
            Sprite::from_image(server.load(path.to_owned())),
            Anchor::TOP_LEFT,
            Transform::from_xyz(x as f32, -(y as f32), z),
            ChildOf(root),
            ScreenEntity,
        )
    };
    commands.spawn((sprite("screens/ending/plane_b.png", 0, 0, 1.0), EndingBackground));
    commands.spawn((sprite("screens/ending/princess.png", 104, 56, 2.0), Princess));
    commands.insert_resource(EndingTimeline::default());
}

#[allow(clippy::too_many_arguments)]
fn run_ending(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut tl: ResMut<EndingTimeline>,
    mut dialog: ResMut<Dialog>,
    pad: Res<Joypad>,
    fade: Res<ScreenFade>,
    roots: Query<Entity, With<ScreenRoot>>,
    mut bg: Query<&mut Sprite, (With<EndingBackground>, Without<Princess>)>,
    mut princess: Query<(&mut Sprite, &mut Transform), (With<Princess>, Without<StaffRoll>)>,
    mut roll: Query<&mut Transform, (With<StaffRoll>, Without<Princess>)>,
    scenery: Query<Entity, With<ScreenEntity>>,
    mut sound: MessageWriter<super::audio::PlaySound>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Ok(root) = roots.single() else { return };
    tl.t += 1;
    let t = tl.t;
    match tl.stage {
        // Fade in, 61 ticks, then the thank-you message (auto-timed).
        0 => {
            if t == FADE_TICKS + 61 {
                dialog.open(THANKS_TEXT, 1);
                tl.stage = 1;
            }
        }
        1 => {
            if !dialog.is_open() {
                tl.stage = 2;
                tl.mark = t;
            }
        }
        // 61 ticks, then the background fades out and the princess turns sepia.
        2 => {
            let d = t - tl.mark;
            // The ending song starts as the background fades (0x11562).
            if d == 61 {
                sound.write(super::audio::PlaySound(0x8E));
            }
            if d >= 61 {
                let k = ((d - 61) as f32 / FADE_TICKS as f32).min(1.0);
                for mut s in &mut bg {
                    s.color = Color::srgb(1.0 - k, 1.0 - k, 1.0 - k);
                }
                if k >= 1.0 {
                    if let Ok((mut s, _)) = princess.single_mut() {
                        s.image = server.load("screens/ending/princess_tinted.png");
                    }
                    // Black mask above and below the staff-roll window (y 48..167).
                    for (y, h) in [(0, 48), (168, 56)] {
                        commands.spawn((
                            Sprite::from_color(Color::BLACK, Vec2::new(320.0, h as f32)),
                            Anchor::TOP_LEFT,
                            Transform::from_xyz(0.0, -(y as f32), 5.0),
                            ChildOf(root),
                            ScreenEntity,
                        ));
                    }
                    tl.stage = 3;
                    tl.mark = t;
                }
            }
        }
        // The princess slides 2 px every 2 ticks to the left edge (104 px).
        3 => {
            let d = t - tl.mark;
            if let Ok((_, mut tf)) = princess.single_mut() {
                tf.translation.x = (104 - (d as i32 / 2 * 2).min(104)) as f32;
            }
            if d >= 104 {
                commands.spawn((
                    Sprite::from_image(server.load("screens/ending/staff_roll.png")),
                    Anchor::TOP_LEFT,
                    Transform::from_xyz(112.0, -168.0, 3.0),
                    ChildOf(root),
                    ScreenEntity,
                    StaffRoll,
                ));
                tl.stage = 4;
                tl.mark = t;
            }
        }
        // Staff roll: 0.09375 px per tick upwards, ~8533 ticks.
        4 => {
            let d = t - tl.mark;
            if let Ok(mut tf) = roll.single_mut() {
                tf.translation.y = -(168.0 - d as f32 * 0.09375).floor();
            }
            if d >= 8533 {
                commands.insert_resource(FadeTo { target: 1.0, ticks: FADE_TICKS });
                tl.stage = 5;
                tl.mark = t;
            }
        }
        // PRESENTED BY TAITO, held for 2400 ticks or START.
        5 => {
            if fade.0 >= 1.0 {
                for e in &scenery {
                    commands.entity(e).despawn();
                }
                commands.spawn((
                    Sprite::from_image(server.load("screens/ending/final_presented_by_taito.png")),
                    Anchor::TOP_LEFT,
                    Transform::from_xyz(104.0, -96.0, 1.0),
                    ChildOf(root),
                    ScreenEntity,
                    FinalScreen,
                ));
                commands.insert_resource(FadeTo { target: 0.0, ticks: FADE_TICKS });
                // PRESENTED BY TAITO comes with the game-over tune (0x1175C).
                sound.write(super::audio::PlaySound(0x96));
                tl.stage = 6;
                tl.mark = t;
            }
        }
        6 => {
            let d = t - tl.mark;
            if d >= 2400 || pad.pressed(Button::Start) {
                commands.insert_resource(FadeTo { target: 1.0, ticks: FADE_TICKS });
                tl.stage = 7;
            }
        }
        _ => {
            if fade.0 >= 1.0 {
                next.set(AppState::Sega);
            }
        }
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(AppState::Ending), enter_ending).add_systems(
        FixedUpdate,
        (
            start_ending_room.run_if(in_state(AppState::InGame)),
            run_ending.run_if(in_state(AppState::Ending)),
        )
            .in_set(GameSystems::Game),
    );
}
