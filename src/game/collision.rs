//! Block collision shared by the player and other actors.
//!
//! This follows the cartridge's per-object physics step (see docs/re/physics.md §5): a
//! one-way surface/slope pass, then horizontal and vertical moves against solid blocks with
//! 8-pixel snapping. Positions are world pixels (x = centre, y = feet line) in 16.16.
use super::{
    coords::{Velocity, WorldPos, px},
    data::{GameData, RoomDef, attr},
};
use bevy::prelude::*;

/// Size of an actor's collision body.
#[derive(Component, Clone, Copy, Debug)]
pub struct Collider {
    pub half_width: i32,
    pub height: i32,
}

impl Default for Collider {
    fn default() -> Self {
        Self { half_width: 8, height: 64 }
    }
}

/// Contact results of the last physics step.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Contacts {
    /// Landed on a solid block this step.
    pub ground: bool,
    pub ceiling: bool,
    pub wall_left: bool,
    pub wall_right: bool,
    /// A one-way surface/slope was handled this step.
    pub surface_step: bool,
    /// Standing on a one-way surface/slope (persists between steps).
    pub on_surface: bool,
}

impl Contacts {
    /// Either kind of footing.
    pub fn landed(&self) -> bool {
        self.ground || self.on_surface
    }
}

fn solid(room: &RoomDef, x: i32, y: i32) -> bool {
    attr::is_solid(room.attr_at(x, y))
}

/// A feet position (x, y) in world pixels where a body `height` px tall stands on solid ground
/// without overlapping anything solid. Searches columns outward from `x`, top to bottom.
/// Used when a room is entered without a known arrival point (e.g. `--room N` for testing).
pub fn standing_spot(room: &RoomDef, x: i32, height: i32) -> Option<(i32, i32)> {
    let w = room.width_px();
    let x0 = (x.clamp(0, w - 1) & !15) + 8;
    let columns = (0..w / 16).flat_map(|d| [x0 + d * 16, x0 - d * 16]).filter(|&cx| cx > 0 && cx < w);
    for cx in columns {
        for y in (height..room.height_px()).step_by(16).map(|y| y & !15) {
            let clear = (y - height..y).step_by(8).all(|yy| !solid(room, cx, yy));
            if clear && solid(room, cx, y) {
                return Some((cx, y));
            }
        }
    }
    None
}

fn is_surface(a: u8) -> bool {
    a >> 4 == 1
}

fn mask_bit(masks: &[[u16; 16]], tile: u8, cx: i32, cy: i32) -> bool {
    let m = &masks[(tile & 0xF) as usize % masks.len()];
    m[cy as usize] & (1 << cx) != 0
}

/// One-way surfaces and slopes (cartridge 0x358E). Returns true if it handled the move.
fn surface_step(
    room: &RoomDef,
    masks: &[[u16; 16]],
    pos: &mut WorldPos,
    vel: &mut Velocity,
    c: &mut Contacts,
) -> bool {
    let was_on_surface = c.on_surface;
    let mut nx = pos.x + vel.x;
    let mut ny = pos.y + vel.y;
    let mut descended = false;
    // Bounded re-probing (the original loops across block boundaries).
    for _ in 0..8 {
        let (xi, yi) = (nx >> 16, ny >> 16);
        let a = room.attr_at(xi, yi);
        let b = room.attr_at(xi, yi - 1);
        if vel.y < 0 {
            c.on_surface = false;
            return false;
        }
        if !is_surface(a) && !is_surface(b) {
            c.on_surface = false;
            return c.surface_step;
        }
        c.surface_step = true;
        pos.x = nx;
        pos.y = ny;
        let (tile, row) = if is_surface(a) { (a, yi) } else { (b, yi - 1) };
        let cx = (pos.x >> 16) & 15;
        let mut cy = row & 15;
        if was_on_surface && !descended {
            // Stick to descending slopes.
            let reprobe;
            loop {
                if mask_bit(masks, tile, cx, cy) {
                    descended = true;
                    reprobe = true;
                    break;
                }
                pos.y += px(1);
                cy += 1;
                if cy == 16 {
                    reprobe = true;
                    break;
                }
            }
            if reprobe {
                nx = pos.x;
                ny = pos.y;
                continue;
            }
        }
        let mut reprobe = false;
        while mask_bit(masks, tile, cx, cy) {
            pos.y = px((pos.y >> 16) - 1);
            c.on_surface = true;
            vel.y = 0;
            cy -= 1;
            if cy < 0 {
                reprobe = true;
                break;
            }
        }
        if reprobe {
            nx = pos.x;
            ny = pos.y;
            continue;
        }
        return true;
    }
    true
}

fn horizontal(room: &RoomDef, col: &Collider, pos: &mut WorldPos, vel: &Velocity, c: &mut Contacts) {
    let mut nx = pos.x + vel.x;
    let nxi = nx >> 16;
    let y = pos.px_y();
    let rows = (0..col.height / 16)
        .map(|k| y - col.height + 16 * k)
        .chain(std::iter::once(y - 1));
    let rows: Vec<i32> = rows.collect();
    if vel.x < 0 && rows.iter().any(|&r| solid(room, nxi - col.half_width, r)) {
        nx = px((nxi + 8) & !7);
        c.wall_left = true;
    }
    if vel.x > 0 && rows.iter().any(|&r| solid(room, nxi + col.half_width, r)) {
        nx = px(nxi & !7);
        c.wall_right = true;
    }
    pos.x = nx;
}

fn vertical(room: &RoomDef, col: &Collider, pos: &mut WorldPos, vel: &mut Velocity, c: &mut Contacts) {
    let mut ny = pos.y + vel.y;
    let x = pos.px_x();
    let (left, right) = (x - col.half_width, x + col.half_width - 1);
    let vy = vel.y;
    if vy != 0 && (solid(room, left, ny >> 16) || solid(room, right, ny >> 16)) {
        ny = px((ny >> 16) & !7);
        c.ground = true;
        c.on_surface = false;
        vel.y = 0;
        let mut push = 0;
        while solid(room, x, (ny >> 16) - 1) && push < 5 {
            push += 1;
            ny -= px(8);
        }
    }
    let nyi = ny >> 16;
    if vy < 0
        && (solid(room, left, nyi - col.height) || solid(room, right, nyi - col.height))
    {
        // The cartridge does not clear vy here: the head sticks until gravity wins.
        ny = px((nyi + 8) & !7);
        c.ceiling = true;
    }
    pos.y = ny;
}

/// Horizontal-only move with wall collision (cartridge 0x3488).
pub fn horizontal_only(room: &RoomDef, col: &Collider, pos: &mut WorldPos, vel: &Velocity, c: &mut Contacts) {
    c.wall_left = false;
    c.wall_right = false;
    horizontal(room, col, pos, vel, c);
}

/// Full physics step for one actor (cartridge 0x343C).
pub fn step(
    room: &RoomDef,
    data: &GameData,
    col: &Collider,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    c: &mut Contacts,
) {
    c.ground = false;
    c.ceiling = false;
    c.wall_left = false;
    c.wall_right = false;
    c.surface_step = false;
    if surface_step(room, &data.surface_masks, pos, vel, c) {
        return;
    }
    horizontal(room, col, pos, vel, c);
    vertical(room, col, pos, vel, c);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::data::{ClassLevels, GameData};

    /// A flat room: solid floor from y = 368 down.
    fn flat_room() -> RoomDef {
        let (w, h) = (40u32, 30u32);
        let attrs = (0..w * h).map(|i| if i / w >= 23 { 0x80 } else { 0 }).collect();
        RoomDef {
            index: 0,
            blocks_w: w,
            blocks_h: h,
            attrs,
            backdrop: [0, 0, 0],
            fg: String::new(),
            fg_hi: String::new(),
            bg: String::new(),
            bg_scroll: [0.5, 0.5],
            palette_cycles: vec![],
            music: 0,
            exits: default(),
            spawns: vec![],
            objects: String::new(),
            objects_handle: default(),
            fg_image: default(),
            fg_hi_image: default(),
            bg_image: default(),
            cycle_images: vec![],
        }
    }

    fn data() -> GameData {
        GameData {
            surface_masks: vec![[0; 16]; 7],
            stairs: vec![],
            levels: ClassLevels { fighter: vec![], mage: vec![] },
            event_warps: Default::default(),
            demos: Default::default(),
        }
    }

    /// Jump arc measured in the original (Fighter, room 0, frames 3305/3312/3320 after takeoff).
    #[test]
    fn jump_arc_matches_cartridge() {
        let (room, data) = (flat_room(), data());
        let col = Collider::default();
        let mut pos = WorldPos::from_px(300, 368);
        let mut vel = Velocity { x: 0, y: -0x4_4000 };
        let mut c = Contacts::default();
        let mut ys = vec![];
        for _ in 0..44 {
            vel.y = (vel.y + 0x3000).min(0x4_0000);
            step(&room, &data, &col, &mut pos, &mut vel, &mut c);
            ys.push(pos.y);
        }
        // Original RAM: 352.875 (+3 ticks), 333.625 (+10), 322.875 (+18) counted from the first airborne tick.
        assert_eq!(ys[3], px(352) + 0xE000);
        assert_eq!(ys[10], px(333) + 0xA000);
        assert_eq!(ys[18], px(322) + 0xE000);
        let mut ticks = 44;
        while !c.ground && ticks < 60 {
            vel.y = (vel.y + 0x3000).min(0x4_0000);
            step(&room, &data, &col, &mut pos, &mut vel, &mut c);
            ticks += 1;
        }
        assert!(c.ground && pos.y == px(368), "landed back on the floor");
        assert!((44..=46).contains(&ticks), "airtime about 44 ticks, got {ticks}");
    }
}
