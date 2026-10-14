//! Room exits and transitions (docs/re/world.md §1, physics.md §5.1).
//!
//! Exits are block attributes: 0x2N door (Up), 0x3N stairs (Up, same room), 0x4N edge (walk in),
//! 0x5N warp (Up). Leaving a room fades out, swaps the room, places the player and fades in.
use super::{
    camera::{self, CameraPos},
    collision::Contacts,
    coords::{Velocity, WorldPos, px},
    data::{ExitDest, RoomDef},
    input::Joypads,
    player::{Action, Facing, Player, PlayerState},
    room::{self, CurrentRoom, CycleState, RoomEntity},
    ui::ScreenRoot,
};
use super::audio::{PlaySound, SetMusicTimer};
use super::objects::ops::{BOSS_FLAG, Effects};
use crate::{GameSystems, controller::Button};
use bevy::{prelude::*, sprite::Anchor};

/// Duration of a full fade in ticks (7 colour levels x 3 components x 2 ticks).
pub const FADE_TICKS: u32 = 42;

#[derive(Debug, Clone)]
pub enum Phase {
    FadeOut(u32),
    Loading,
    FadeIn(u32),
}

/// An ongoing room change. Gameplay is frozen while it exists.
#[derive(Resource, Debug, Clone)]
pub struct Transition {
    pub dest: ExitDest,
    pub phase: Phase,
}

/// Full-screen black overlay used for fades.
#[derive(Component)]
pub struct FadeOverlay;

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct ScreenFade(pub f32);

/// Animates [`ScreenFade`] towards `target` over `ticks`.
#[derive(Resource, Debug, Clone, Copy)]
pub struct FadeTo {
    pub target: f32,
    pub ticks: u32,
}

fn run_fade_to(mut commands: Commands, f: Option<Res<FadeTo>>, mut fade: ResMut<ScreenFade>) {
    let Some(f) = f else { return };
    let step = 1.0 / f.ticks.max(1) as f32;
    if (fade.0 - f.target).abs() <= step {
        fade.0 = f.target;
        commands.remove_resource::<FadeTo>();
    } else {
        fade.0 += step * (f.target - fade.0).signum();
    }
}

pub fn no_transition(t: Option<Res<Transition>>) -> bool {
    t.is_none()
}

/// Checks the block each player stands in for exits (cartridge 0x4238, P1 then P2, each with
/// their own pad). Any player can take the party through an exit. Exits are shut while a boss
/// fight is on (0x41AA).
pub fn check_exits(
    mut commands: Commands,
    pads: Res<Joypads>,
    fx: Res<Effects>,
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    mut sound: MessageWriter<PlaySound>,
    mut q: Query<(&mut Player, &mut WorldPos, &mut Velocity, &Contacts)>,
) {
    let Some(def) = room.as_ref().and_then(|r| rooms.get(&r.def)) else { return };
    if fx.global(BOSS_FLAG) != 0 {
        return;
    }
    // Stairs are refused while a player is in the dying animation (0x42D8).
    let anyone_dying = q.iter().any(|(p, ..)| p.state == PlayerState::Dying);
    let mut stairs_to: Option<(WorldPos, u32, u32)> = None;
    let mut order: Vec<_> = q.iter_mut().collect();
    order.sort_by_key(|(p, ..)| p.slot);
    for (p, pos, vel, c) in order.iter_mut() {
        if !c.ground || p.action != Action::None || p.state >= PlayerState::Dying || p.freeze > 0 {
            continue;
        }
        let a = def.attr_at(pos.px_x(), pos.px_y() - 1);
        let n = (a & 0x0F).to_string();
        let up = pads.get(p.slot).pressed(Button::Up);
        // In room 12 doors only work while the player is shrunk; room 45 ignores edge exits.
        let door_ok = def.index != 12 || p.small;
        let dest = match a >> 4 {
            0x2 if up && door_ok => def.exits.door.get(&n),
            0x4 if def.index == 45 => None,
            0x4 => def.exits.edge.get(&n),
            0x5 if up => def.exits.warp.get(&n),
            0x3 if up && !anyone_dying => {
                if let Some(st) = def.exits.stairs.get(&n) {
                    let mut to = **pos;
                    to.y += px(st.dy);
                    stairs_to = Some((to, st.freeze, st.iframes));
                }
                None
            }
            _ => None,
        };
        if let Some(dest) = dest {
            vel.x = 0;
            vel.y = 0;
            // Warps sound B7 (0x447A).
            if let Some(id) = dest.sound {
                sound.write(PlaySound(id));
            }
            commands.insert_resource(Transition { dest: dest.clone(), phase: Phase::FadeOut(0) });
            return;
        }
        if stairs_to.is_some() {
            break;
        }
    }
    // Stairs move every living player to the climber's destination (0x432C).
    if let Some((to, freeze, iframes)) = stairs_to {
        for (p, pos, vel, _) in order.iter_mut() {
            if p.state >= PlayerState::Dying {
                continue;
            }
            **pos = to;
            **vel = Velocity::default();
            p.state = PlayerState::Stand;
            p.action = Action::None;
            p.freeze = freeze;
            p.invulnerable = p.invulnerable.max(iframes);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_transition(
    mut commands: Commands,
    server: Res<AssetServer>,
    transition: Option<ResMut<Transition>>,
    mut current: ResMut<CurrentRoom>,
    rooms: Res<Assets<RoomDef>>,
    mut fade: ResMut<ScreenFade>,
    mut cam: ResMut<CameraPos>,
    mut cycles: ResMut<CycleState>,
    mut obj_world: ResMut<super::objects::ObjectWorld>,
    old: Query<Entity, With<RoomEntity>>,
    roots: Query<Entity, With<ScreenRoot>>,
    mut sound: MessageWriter<PlaySound>,
    mut music_timer: MessageWriter<SetMusicTimer>,
    mut player: Query<(&mut Player, &mut WorldPos, &mut Velocity)>,
    mut fx: ResMut<Effects>,
) {
    let Some(mut t) = transition else { return };
    match t.phase {
        Phase::FadeOut(n) => {
            fade.0 = (n + 1) as f32 / FADE_TICKS as f32;
            t.phase = if n + 1 >= FADE_TICKS { Phase::Loading } else { Phase::FadeOut(n + 1) };
            if matches!(t.phase, Phase::Loading) {
                current.index = t.dest.room;
                current.def = server.load(room::room_path(t.dest.room));
            }
        }
        Phase::Loading => {
            if !server.is_loaded_with_dependencies(&current.def) {
                return;
            }
            let Some(def) = rooms.get(&current.def) else { return };
            let player_count = player.iter().count();
            room::despawn_room(&mut commands, &old);
            super::objects::clear_objects(&mut obj_world);
            // A new room never starts mid-fight; its boss sets the flag again when it appears.
            fx.set_global(BOSS_FLAG, 0);
            let (mx, my) = camera::bounds(def);
            cam.x = t.dest.camera[0].clamp(0, mx);
            cam.y = t.dest.camera[1].clamp(0, my);
            if let Some(root) = room::screen_root(&roots) {
                room::spawn_room_layers(&mut commands, def, root, &cam, &mut cycles);
            }
            for (mut p, mut pos, mut vel) in &mut player {
                // Dying/dead players stay out of the new room until revived (0x4874).
                if p.state >= PlayerState::Dying && player_count > 1 {
                    continue;
                }
                // In the ending room player 2 stands 32 px to the right (0x48C0).
                let dx = if t.dest.room == 47 && p.slot > 0 { 0x20 } else { 0 };
                *pos = WorldPos::from_px(t.dest.x + dx, t.dest.y);
                *vel = Velocity::default();
                p.facing = if t.dest.facing == "left" { Facing::Left } else { Facing::Right };
                p.state = PlayerState::Stand;
                p.action = Action::None;
            }
            // An exit record may change the music (0x48EA): the timer is cleared and the song starts.
            if t.dest.music != 0 {
                music_timer.write(SetMusicTimer(0));
                sound.write(PlaySound(t.dest.music));
            }
            t.phase = Phase::FadeIn(0);
        }
        Phase::FadeIn(n) => {
            fade.0 = 1.0 - (n + 1) as f32 / FADE_TICKS as f32;
            if n + 1 >= FADE_TICKS {
                fade.0 = 0.0;
                commands.remove_resource::<Transition>();
            } else {
                t.phase = Phase::FadeIn(n + 1);
            }
        }
    }
}

/// Scripted warps: objects set event 0xFF0A74 = 1..4 when a boss falls (cartridge 0x13BD6).
pub fn check_event_warp(
    mut commands: Commands,
    mut fx: ResMut<super::objects::ops::Effects>,
    data: super::GameDataRef,
    transition: Option<Res<Transition>>,
) {
    if transition.is_some() {
        return;
    }
    let ev = fx.globals.get(&0xFF0A74).copied().unwrap_or(0);
    if ev == 0 {
        return;
    }
    fx.globals.insert(0xFF0A74, 0);
    if let Some(dest) = data.get().and_then(|d| d.event_warps.get(&ev.to_string())) {
        commands.insert_resource(Transition { dest: dest.clone(), phase: Phase::FadeOut(0) });
    }
}

fn spawn_fade(mut commands: Commands, roots: Query<Entity, With<ScreenRoot>>) {
    let Ok(root) = roots.single() else { return };
    commands.spawn((
        FadeOverlay,
        Sprite::from_color(Color::BLACK.with_alpha(0.0), Vec2::new(320.0, 224.0)),
        Anchor::TOP_LEFT,
        Transform::from_xyz(0.0, 0.0, super::coords::z::HUD + 5.0),
        ChildOf(root),
    ));
}

/// Fades use quantised steps like the cartridge's palette fade (7 brightness levels).
fn apply_fade(fade: Res<ScreenFade>, mut q: Query<&mut Sprite, With<FadeOverlay>>) {
    let level = (fade.0.clamp(0.0, 1.0) * 7.0).round() / 7.0;
    for mut s in &mut q {
        s.color = Color::BLACK.with_alpha(level);
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<ScreenFade>()
        .add_systems(PostStartup, spawn_fade.after(super::ui::setup_screen_root))
        .add_systems(
            FixedUpdate,
            (
                check_exits.after(super::player::tick).run_if(no_transition),
                check_event_warp,
                run_transition,
                run_fade_to,
            )
                .chain()
                .in_set(GameSystems::Game),
        )
        .add_systems(Update, apply_fade);
}
