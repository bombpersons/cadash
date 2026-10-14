//! Camera (cartridge 0x10428): eases towards the players at up to 6 px per tick, never rises
//! while a player is airborne, and stays inside the room.
//!
//! With several players the target is the average position of the living ones (dead players leave
//! the average, 0x10458). Vertically it follows both ways only while every living player climbs,
//! and rises only when every living player stands on solid ground or any stands on a surface.
use super::{
    collision::Contacts,
    coords::WorldPos,
    data::RoomDef,
    lifecycle::DeathState,
    player::{Player, PlayerState},
    room::CurrentRoom,
};
use crate::screen::GameCamera;
use bevy::prelude::*;

/// Size of the scrolling playfield (the HUD covers the rest of the 224-line screen).
pub const VIEW_W: i32 = 320;
pub const PLAYFIELD_H: i32 = 192;
pub const SCREEN_H: i32 = 224;
/// Where the camera keeps the player's centre/feet inside the playfield.
pub const TARGET_X: i32 = 156;
pub const TARGET_Y: i32 = 128;
/// Cartridge word holding the vertical target (raw sprite coordinates, 0x100 = `TARGET_Y`); some boss
/// scripts set 0x120 (player 32 px lower on screen) and 0x100 again (0x1051A). Reset by every room load.
pub const TARGET_Y_WORD: u32 = 0xFF0A5A;
pub const MAX_STEP: i32 = 6;

/// Top-left of the playfield in world pixels.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct CameraPos {
    pub x: i32,
    pub y: i32,
}

pub fn bounds(def: &RoomDef) -> (i32, i32) {
    ((def.width_px() - VIEW_W).max(0), (def.height_px() - PLAYFIELD_H).max(0))
}

/// Centres the camera on a position immediately (room entry).
pub fn snap_to(cam: &mut CameraPos, def: &RoomDef, pos: &WorldPos) {
    let (mx, my) = bounds(def);
    cam.x = (pos.px_x() - TARGET_X).clamp(0, mx);
    cam.y = (pos.px_y() - TARGET_Y).clamp(0, my);
}

pub fn follow_player(
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    players: Query<(&WorldPos, &Player, &Contacts, Option<&DeathState>)>,
    mut cam: ResMut<CameraPos>,
    mut fx: ResMut<super::objects::ops::Effects>,
    mut last_room: Local<Option<usize>>,
) {
    let Some(room) = room else { return };
    if *last_room != Some(room.index) {
        *last_room = Some(room.index);
        fx.globals.remove(&TARGET_Y_WORD);
    }
    let target_y = fx.globals.get(&TARGET_Y_WORD).map_or(TARGET_Y, |v| v - 0x80);
    let Some(def) = rooms.get(&room.def) else { return };
    // Players still in the game (a dead player whose partner lives on has left it).
    let tracked: Vec<_> = players
        .iter()
        .filter(|(.., d)| !d.is_some_and(|d| d.removed))
        .map(|(pos, p, c, _)| (pos, p, c))
        .collect();
    if tracked.is_empty() {
        return;
    }
    let n = tracked.len() as i32;
    let ax = tracked.iter().map(|(p, ..)| p.px_x()).sum::<i32>() / n;
    let ay = tracked.iter().map(|(p, ..)| p.px_y()).sum::<i32>() / n;
    let climbing = tracked.iter().all(|(_, p, _)| matches!(p.state, PlayerState::ClimbIdle | PlayerState::ClimbMove));
    let grounded = tracked.iter().all(|(.., c)| c.ground) || tracked.iter().any(|(.., c)| c.on_surface);
    let (mx, my) = bounds(def);
    let dx = (ax - TARGET_X) - cam.x;
    cam.x = (cam.x + dx.clamp(-MAX_STEP, MAX_STEP)).clamp(0, mx);
    let dy = (ay - target_y) - cam.y;
    if climbing || dy > 0 || grounded {
        cam.y = (cam.y + dy.clamp(-MAX_STEP, MAX_STEP)).clamp(0, my);
    }
}

pub fn apply_camera(cam: Res<CameraPos>, mut q: Query<&mut Transform, With<GameCamera>>) {
    for mut tf in &mut q {
        tf.translation.x = (cam.x + VIEW_W / 2) as f32;
        tf.translation.y = -((cam.y + SCREEN_H / 2) as f32);
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<CameraPos>()
        .add_systems(FixedUpdate, follow_player.in_set(crate::GameSystems::Camera))
        .add_systems(PostUpdate, apply_camera.before(TransformSystems::Propagate));
}
