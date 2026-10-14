//! Moving platforms (cartridge 0x3A06, docs/re/physics.md §5.6): objects flagged
//! [`flag::PLATFORM`] carry a falling player who lands on their 48-px-wide top.
use super::{
    AppState,
    collision::{Collider, Contacts},
    coords::{Velocity, WorldPos, px},
    objects::{Obj, flag, objects_running},
    player::{Player, PlayerState},
};
use crate::GameSystems;
use bevy::prelude::*;

/// Width of a platform's top, from its x (0x3ACA).
const WIDTH: i32 = 0x30;

/// Runs after the objects moved: each airborne player (P1 then P2) is put on the first platform
/// (lowest slot) whose top it overlaps: y = top, vy = 0, x moved by the platform's vx with the
/// platform's sub-pixel, on solid ground.
fn ride_platforms(
    mut players: Query<(&Player, &Collider, &mut WorldPos, &mut Velocity, &mut Contacts), Without<Obj>>,
    objs: Query<(&Obj, &WorldPos, &Velocity), Without<Player>>,
) {
    let mut platforms: Vec<_> = objs.iter().filter(|(o, ..)| o.flags & flag::PLATFORM != 0 && !o.delete).collect();
    if platforms.is_empty() {
        return;
    }
    platforms.sort_by_key(|(o, ..)| o.slot);
    let mut order: Vec<_> = players.iter_mut().collect();
    order.sort_by_key(|(p, ..)| p.slot);
    for (p, col, pos, vel, c) in order.iter_mut() {
        if p.state >= PlayerState::Dying || c.ground || c.on_surface || p.rope_latch || vel.y < 0 || p.ledge_timer > 0 {
            continue;
        }
        let (x, y) = (pos.px_x(), pos.px_y());
        let hw = col.half_width;
        for (_, opos, ovel) in &platforms {
            let (ox, oy) = (opos.px_x(), opos.px_y());
            if y < oy - 2 || y > oy + 8 || x + hw < ox || x - hw > ox + WIDTH {
                continue;
            }
            pos.y = px(oy);
            vel.y = 0;
            pos.x = (pos.x & !0xFFFF) | (opos.x & 0xFFFF);
            pos.x += ovel.x;
            c.ground = true;
            break;
        }
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        ride_platforms
            .in_set(GameSystems::Game)
            .after(super::objects::reap_objects)
            .run_if(in_state(AppState::InGame))
            .run_if(objects_running),
    );
}
