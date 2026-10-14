//! Boss and multi-part object opcodes (Kraken, Worm, Fire Elemental, Balrog, chain traps,
//! snake segments …). Each function returns `None`/`false` for opcodes it does not handle so the
//! interpreter in [`super::ops`] can report them.
//!
//! Ported from docs/re/object_opcodes.txt (addresses in the comments). Conventions:
//! - Positions are map pixels (16.16); cartridge ROM-space constants have 0x80 subtracted.
//!   Setting the cartridge's `x` *word* keeps the sub-pixel half ([`set_px`]).
//! - Editable mappings live in the variables at +0xAC (piece k: dx v190+8k, dy v192+8k, size
//!   v194+8k, attr v196+8k) or +0x9E (B75: pieces from v176); `o.boxes` mirrors the hit-box
//!   words +46..+60 that the scripts rewrite.
//! - Motion tables come from extract/boss_tables.py (`ctx.ops.tables`).
//! - Cartridge RAM shared between objects that is not a slot field (arm history, lightning path
//!   queue, plane-B scroll) is kept in `Effects::globals` under its RAM address.
use super::{
    super::{
        anim::{self, SpriteAnimator},
        camera::CameraPos,
        collision::{self, Collider, Contacts},
        coords::{Velocity, WorldPos, px, z},
        data::{RoomDef, RoomObjects, SpriteSheet},
        room::{BG_H, BG_W, BackgroundLayer, BackgroundTile, CurrentRoom, RoomEntity},
    },
    Obj, Pool, RoomCtx, SLOTS, anim_key,
    ops::{self, Effects, PlayersView, SlotWrite, copy_mapping, direction_code, fp, spawn, spawn_abs},
};
use bevy::prelude::*;
use super::super::audio::MUSIC_TIMER;
use super::ops::BOSS_FLAG;

/// Plane-B scroll registers written by the background-plane Balrog (0xFF0642 h, 0xFF05F2 v).
const PLANE_B_HSCROLL: u32 = 0xFF0642;
const PLANE_B_VSCROLL: u32 = 0xFF05F2;
/// Pseudo global: what the scripts did to plane B (keyed by the body tilemap's ROM address):
/// [`PLANE_B_BODY`] after B112 drew Balrog's body tilemap 0x2AAA6 over cells (0..11, 0..10),
/// [`PLANE_B_CLEARED`] after I232/B133 filled the whole plane with the blank tile 0x100 (0x2DE0).
/// Absent/0: the room's own background.
pub const PLANE_B_BOSS: u32 = 0x2AAA6;
const PLANE_B_BODY: i32 = 1;
const PLANE_B_CLEARED: i32 = 2;
/// Lightning path queue (8 step bytes, newest first) and its target (0xFF0BE4/0xFF0BE6).
const PATH_QUEUE: u32 = 0xFF0BDC;
const PATH_TARGET_X: u32 = 0xFF0BE4;
const PATH_TARGET_Y: u32 = 0xFF0BE6;
/// Path step "stay" (no movement, cartridge 0x1C37C).
const STEP_STAY: i32 = 4;

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

fn table<'a>(ctx: &'a RoomCtx, name: &str) -> &'a [i32] {
    ctx.ops.tables.get(name).map_or(&[], |v| v.as_slice())
}

fn entry(ctx: &RoomCtx, name: &str, i: usize) -> i32 {
    table(ctx, name).get(i).copied().unwrap_or(0)
}

/// (dx, dy) entry `i` of a pair table, dx negated when `neg` (cartridge 0x1A824/0x1A840/0x1A858).
fn pair(ctx: &RoomCtx, name: &str, i: i32, neg: bool) -> (i32, i32) {
    let i = i.max(0) as usize;
    let (dx, dy) = (entry(ctx, name, 2 * i), entry(ctx, name, 2 * i + 1));
    (if neg { -dx } else { dx }, dy)
}

/// `v = (v + 1) % len` on a script variable.
fn step_index(o: &mut Obj, off: usize, len: i32) -> i32 {
    let v = (o.var(off) + 1).rem_euclid(len);
    o.set_var(off, v);
    v
}

/// `subq.w #1,v; bcs reload`: true while the countdown is still running.
fn countdown(o: &mut Obj, off: usize, reload: i32) -> bool {
    if o.var(off) != 0 {
        o.set_var(off, o.var(off) - 1);
        true
    } else {
        o.set_var(off, reload);
        false
    }
}

/// Moves piece `k` of a +0xAC editable mapping.
fn set_piece(o: &mut Obj, k: usize, dx: i32, dy: i32) {
    o.set_var(190 + 8 * k, dx);
    o.set_var(192 + 8 * k, dy);
}

fn set_hurt_offset(o: &mut Obj, dx: i32, dy: i32) {
    if let Some(b) = o.boxes.as_mut() {
        b[2] = dx as i16;
        b[3] = dy as i16;
    }
}

fn set_attack_offset(o: &mut Obj, dx: i32, dy: i32) {
    if let Some(b) = o.boxes.as_mut() {
        b[6] = dx as i16;
        b[7] = dy as i16;
    }
}

/// Writes the integer pixel word of a coordinate, keeping its sub-pixel word.
fn set_px(v: &mut i32, px_val: i32) {
    *v = (px_val << 16) | (*v & 0xFFFF);
}

fn ram(fx: &Effects, addr: u32) -> i32 {
    fx.globals.get(&addr).copied().unwrap_or(0)
}

fn set_ram(fx: &mut Effects, addr: u32, v: i32) {
    fx.globals.insert(addr, v);
}

/// Clears the object budget (`clr.w obj_budget`, 0xFF0B54).
fn reset_budget(fx: &mut Effects) {
    fx.budget_refund = fx.budget_refund.saturating_add(1 << 20);
}

fn message(fx: &mut Effects, id: u32) {
    fx.messages.push(id);
}

/// Sets an anim by id like 0x269A (frame 0), falling back to the default graphics context
/// when the sheet has no variant for the object's switched bank.
fn set_anim(ctx: &RoomCtx, o: &mut Obj, anim: &mut SpriteAnimator, a: i32) {
    let a = a.max(0) as u32;
    let mut key = anim_key(a, o.tile_ofs, &o.gfx);
    if !ctx.sheet.anims.contains_key(&key) {
        key = anim_key(a, o.tile_ofs, "default");
    }
    o.anim_id = Some(a);
    anim.restart(&key);
}

/// (dx, dy) of the first piece of an object's current mapping (cartridge `mapping+18/+20`): the
/// RAM mapping copy, or the value the exporter read from the ROM mapping (`SheetFrame::piece0`;
/// sheets exported without it fall back to the frame's top-left).
pub fn first_piece(o: &Obj, sheet: &SpriteSheet, anim: &SpriteAnimator) -> (i32, i32) {
    if let Some(base) = o.ram_map {
        return (o.var(base + 18), o.var(base + 20));
    }
    sheet
        .anims
        .get(&anim.anim)
        .and_then(|d| d.frames.get(anim.index))
        .and_then(|f| sheet.frames.get(f.frame))
        .map_or((0, 0), |f| f.piece0.map_or((-f.origin[0], -f.origin[1]), |[x, y]| (x, y)))
}

/// x,y = slot 0 + (dx, dy) (B118–B122, B132: Balrog's parts riding on the body).
fn follow_slot0(pos: &mut WorldPos, fx: &Effects, dx: i32, dy: i32) {
    if let Some(s0) = fx.slot(0) {
        *pos = WorldPos { x: s0.wx + px(dx), y: s0.wy + px(dy) };
    }
}

/// Free flight with acceleration, no collision (0x1703E, = B6).
fn free_flight(o: &Obj, pos: &mut WorldPos, vel: &mut Velocity) {
    vel.y += o.ay;
    vel.x += o.ax;
    pos.x += vel.x;
    pos.y += vel.y;
}

/// Accelerated movement with tile collision (0x1701E, = B5).
fn move_with_collision(ctx: &RoomCtx, o: &Obj, pos: &mut WorldPos, vel: &mut Velocity, col: &Collider, c: &mut Contacts) {
    vel.y += o.ay;
    vel.x += o.ax;
    collision::step(ctx.def, ctx.data, col, pos, vel, c);
}

/// 0x18F1E: while the boss core (slot 0) flashes from a hit, raise its "was hit" flag v166
/// ([FF1151] flash byte → [FF1166]).
fn notify_boss_hit(o: &mut Obj, fx: &mut Effects) {
    if o.slot == 0 {
        if o.flash != 0 {
            o.set_var(166, 1);
        }
    } else if fx.slot(0).is_some_and(|s0| s0.flash != 0) {
        fx.slot_writes.push(SlotWrite::Var { slot: 0, offset: 166, value: 1 });
    }
}

// ---------------------------------------------------------------------------------------------
// Kraken arms (room 11) and the spiked-ball chains (room 8)
// ---------------------------------------------------------------------------------------------

/// Base of an arm's 32-word position history: scratch RAM at 0xFF14C0 - v164 (inside slot 4's
/// block; v164 = 0 / 0x40 for the two arms), x samples at +158.., y samples at +190...
fn arm_history_base(o: &Obj) -> u32 {
    0xFF14C0u32.wrapping_sub(o.var(164) as u16 as u32)
}

/// 0x1A7FC: shifts the history by one sample, then stores the newest (x, y).
fn arm_history_push(o: &Obj, fx: &mut Effects, x: i32, y: i32) {
    let base = arm_history_base(o);
    for off in (158..=218u32).rev().step_by(2) {
        let v = ram(fx, base + off);
        set_ram(fx, base + off + 2, v);
    }
    set_ram(fx, base + 158, x);
    set_ram(fx, base + 190, y);
}

fn arm_history(o: &Obj, fx: &Effects, i: u32) -> (i32, i32) {
    let base = arm_history_base(o);
    (ram(fx, base + 158 + 2 * i), ram(fx, base + 190 + 2 * i))
}

/// 0x17FC2: chain pieces 6..0 at 7/8..1/8 of increasingly older tip samples (a whip-like lag).
fn arm_pieces_from_history(o: &mut Obj, fx: &Effects) {
    for (piece, sample, k) in [(6, 3, 7), (5, 7, 6), (4, 9, 5), (3, 11, 4), (2, 12, 3), (1, 13, 2), (0, 14, 1)] {
        let (hx, hy) = arm_history(o, fx, sample);
        set_piece(o, piece, ((hx * k) >> 3) - 8, ((hy * k) >> 3) - 16);
    }
}

/// B66 (0x17F36, table 0x180BC, every 2nd frame) / B69 (0x1830A, table 0x1838C, every frame):
/// the arm tip follows a motion table; hurt + attack boxes sit on the tip.
fn kraken_arm(ctx: &RoomCtx, o: &mut Obj, fx: &mut Effects, name: &str, len: i32, every_other: bool) {
    if every_other && countdown(o, 160, 1) {
        return;
    }
    let (dx, dy) = pair(ctx, name, o.var(158), o.var(162) == 1);
    set_hurt_offset(o, dx, dy);
    set_attack_offset(o, dx, dy);
    o.set_var(246, dx - 8);
    o.set_var(248, dy - 16);
    step_index(o, 158, len);
    arm_history_push(o, fx, dx, dy);
    arm_pieces_from_history(o, fx);
}

/// B68 (0x181E2) / B70 (0x1852C): the arm grows out of the body: tip x += dir per frame, the
/// other pieces evenly spaced along it (k/8 of the newest sample).
fn kraken_arm_extend(o: &mut Obj, fx: &mut Effects, dir: i32) {
    arm_history_push(o, fx, o.var(246) + 8, o.var(248) + 16);
    o.set_var(246, o.var(246) + dir);
    let (hx, hy) = arm_history(o, fx, 0);
    for k in 1..=7 {
        set_piece(o, k as usize - 1, ((hx * k) >> 3) - 8, ((hy * k) >> 3) - 16);
    }
}

/// B72 (0x185A0): a destroyed arm crumbles from the tip: every 15 frames one piece turns into
/// the explosion tile 0x20F8, 9 frames later it is blanked (attr 0x100, size 0); sound A8 every 30.
fn arm_crumble(o: &mut Obj, fx: &mut Effects) {
    let t = o.state_timer as i32;
    for j in 0..8 {
        let piece = 7 - j as usize;
        let start = 15 * j;
        if t == start {
            if start % 30 == 0 {
                fx.sounds.push(0xA8);
            }
            o.set_var(196 + 8 * piece, 0x20F8);
        } else if t == start + 9 {
            o.set_var(196 + 8 * piece, 0x100);
            o.set_var(194 + 8 * piece, 0);
        }
    }
}

/// B74 (0x18900): spiked ball on a chain, tip = table 0x18A40 every v166+1 frames; the ball is
/// pieces 0/1, links 2..6 at 9/11 .. 1/11 of the tip offset.
fn chain_ball(ctx: &RoomCtx, o: &mut Obj) {
    let reload = o.var(166);
    if countdown(o, 160, reload) {
        return;
    }
    let (tx, ty) = pair(ctx, "0x18A40", o.var(158), o.var(162) == 1);
    o.set_var(168, tx);
    o.set_var(170, ty);
    set_hurt_offset(o, tx - 8, ty + 0x1C);
    set_piece(o, 0, tx - 0x18, ty);
    set_piece(o, 1, tx - 0x18, ty + 0x10);
    step_index(o, 158, 60);
    for (piece, k) in [(2, 9), (3, 7), (4, 5), (5, 3), (6, 1)] {
        set_piece(o, piece, tx * k / 11 - 0x10, ty * k / 11 + 8);
    }
}

/// B75 (0x18AB8): sea-of-flame fire serpent (mapping at +0x9E: pieces from v176). Every 2nd frame
/// the head follows word table 0x18C6A; pieces 1..8 at 8/9 .. 1/9; attack box on piece 2.
fn fire_serpent(ctx: &RoomCtx, o: &mut Obj) {
    if countdown(o, 164, 1) {
        return;
    }
    let (d0, d1) = pair(ctx, "0x18C6A", o.var(160), o.var(162) == 1);
    o.set_var(168, d0);
    o.set_var(170, d1);
    set_hurt_offset(o, d0 - 8, d1 + 0x20);
    o.set_var(176, d0 - 0x18);
    o.set_var(178, d1);
    step_index(o, 160, 120);
    // (piece var offset, numerator, dx adjust, dy adjust)
    let layout = [(184, 8, -0x18, 0), (192, 7, -0x18, 0), (200, 6, -0x14, 4), (208, 5, -0x14, 4), (216, 4, -0x14, 4), (224, 3, -0x10, 8), (232, 2, -0x10, 8), (240, 1, -0x10, 8)];
    for (off, k, ax, ay) in layout {
        let (px_, py_) = (d0 * k / 9 + ax, d1 * k / 9 + ay);
        o.set_var(off, px_);
        o.set_var(off + 2, py_);
        if off == 192 {
            set_attack_offset(o, px_ + 0x10, py_ + 0x10);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Worm (room 17, rematch in room 27)
// ---------------------------------------------------------------------------------------------

/// Record offset of the room-27 rematch (its records follow the room's own, +0x24).
fn worm_record_base(fx: &Effects) -> usize {
    if fx.room == 27 { 0x24 } else { 0 }
}

/// B76 (0x18E4A): body segment crawling with collision; reverses (and swaps its left/right anim
/// v160/v162 + v190) at walls. On its first frame it snaps 24 px behind the previous segment.
#[allow(clippy::too_many_arguments)]
fn worm_segment(
    ctx: &RoomCtx,
    o: &mut Obj,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    col: &Collider,
    c: &mut Contacts,
    fx: &mut Effects,
) {
    // 0x18F34
    let prev = (o.slot != 0 && o.state_timer == 0 && o.var(176) != 0).then(|| fx.slot(o.slot - 1).copied()).flatten();
    if let Some(prev) = prev.filter(|p| p.var(158) == o.var(158)) {
        o.set_var(176, 0);
        let dx = if o.var(158) == 0 { 24 } else { -24 };
        *pos = WorldPos { x: prev.wx + px(dx), y: prev.wy };
    }
    vel.y += o.ay;
    vel.x += o.ax;
    let saved = *vel; // v168/v172: velocity before the wall stopped it
    collision::step(ctx.def, ctx.data, col, pos, vel, c);
    o.set_var(178, pos.px_x());
    notify_boss_hit(o, fx);
    if c.wall_right || c.wall_left {
        *vel = saved;
        vel.x = -vel.x;
        let right = c.wall_right;
        o.set_var(158, if right { 0 } else { 1 });
        let a = if right { o.var(160) } else { o.var(162) } + o.var(190);
        set_anim(ctx, o, anim, a);
    }
}

/// B77 (0x18F9C): the Worm appears: spawns its 7 body segments 24 px apart, boss music/flag,
/// persist flag, message 0x58. In room 27 this waits for state_timer 22 and switches graphics.
#[allow(clippy::too_many_arguments)]
fn worm_appear(ctx: &RoomCtx, o: &mut Obj, pos: &WorldPos, anim: &mut SpriteAnimator, fx: &mut Effects) {
    if fx.room == 27 {
        if o.state_timer != 22 {
            return;
        }
        // Bank 0x7AF9A (+0xA00) / palette 0x4B8A: the "B77" graphics context of the exporter.
        o.gfx = "B77".into();
        fx.set_global(0xFF0A5A, 0x120);
        set_anim(ctx, o, anim, 0x58);
    }
    let base = worm_record_base(fx);
    for k in 1..=7 {
        spawn(ctx, o, pos, fx, base + k, 24 * k as i32, 0, Pool::Any);
    }
    fx.sounds.push(0x8B);
    fx.set_global(BOSS_FLAG, 1);
    fx.persist.push(o.param118);
    message(fx, 0x58);
}

/// 0x19218: after a direction change (v186) pick the left/right anim v160/v162 + v190.
fn worm_update_anim(ctx: &RoomCtx, o: &mut Obj, anim: &mut SpriteAnimator) {
    if o.var(186) == 0 {
        return;
    }
    o.set_var(186, 0);
    let a = if o.var(158) == 0 { o.var(160) } else { o.var(162) } + o.var(190);
    set_anim(ctx, o, anim, a);
}

/// B78 (0x1915A): coiled around the head: slot 0 + table 0x191B6 (dx mirrored by the head's
/// facing) unless released (v184).
fn worm_orbit(ctx: &RoomCtx, o: &mut Obj, pos: &mut WorldPos, anim: &mut SpriteAnimator, fx: &mut Effects) {
    notify_boss_hit(o, fx);
    if o.var(184) == 0 {
        if let Some(s0) = fx.slot(0).copied() {
            let (dx, dy) = pair(ctx, "0x191B6", o.var(180), s0.var(158) == 0);
            *pos = WorldPos { x: s0.wx + px(dx), y: s0.wy + px(dy) };
        }
        step_index(o, 180, 30);
    }
    worm_update_anim(ctx, o, anim);
}

/// B79 (0x191F2): stuck to the head unless released.
fn worm_stick(ctx: &RoomCtx, o: &mut Obj, pos: &mut WorldPos, anim: &mut SpriteAnimator, fx: &mut Effects) {
    notify_boss_hit(o, fx);
    if o.var(184) == 0 {
        follow_slot0(pos, fx, 0, 0);
    }
    worm_update_anim(ctx, o, anim);
}

/// B80 (0x19266) / B100 (0x19AE4): head moving with collision; every 8 frames from `first` it
/// releases the next segment (slot+7 first): v184 = 1, placed at the head, flags &= 0xCDFF.
#[allow(clippy::too_many_arguments)]
fn worm_release(
    ctx: &RoomCtx,
    o: &mut Obj,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    col: &Collider,
    c: &mut Contacts,
    fx: &mut Effects,
    first: u32,
) {
    notify_boss_hit(o, fx);
    move_with_collision(ctx, o, pos, vel, col, c);
    let t = o.state_timer;
    if t >= first && (t - first).is_multiple_of(8) && (t - first) / 8 < 7 {
        let slot = o.slot + 7 - ((t - first) / 8) as usize;
        if slot < SLOTS {
            fx.slot_writes.push(SlotWrite::Var { slot, offset: 184, value: 1 });
            fx.slot_writes.push(SlotWrite::Pos { slot, x: pos.x, y: pos.y });
            fx.slot_writes.push(SlotWrite::Flags { slot, and: 0xCDFF, or: 0 });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Fire Elemental (room 24)
// ---------------------------------------------------------------------------------------------

/// B84 (0x193F2) / B85 (0x19576): four fireballs on a rotating cross (word table 0x1949A,
/// 55 steps; v158 counts up to 0x37 / down to 0).
fn fire_cross(ctx: &RoomCtx, o: &mut Obj, forward: bool) {
    let (r0, r1) = pair(ctx, "0x1949A", o.var(158), false);
    o.set_var(162, r0);
    o.set_var(164, r1);
    if forward {
        step_index(o, 158, 0x37);
    } else {
        o.set_var(158, (o.var(158) - 1).max(0));
    }
    set_piece(o, 0, r0 - 8, r1 - 0x30);
    set_piece(o, 1, -r0 - 8, -r1 - 0x30);
    set_piece(o, 2, r1 - 8, -r0 - 0x30);
    set_piece(o, 3, -r1 - 8, r0 - 0x30);
}

/// B86 (0x195B8): flies straight, bouncing off the box x ∈ [0x80, 0x2F0], y ∈ [0x80, 0x140] (ROM).
fn bounce_in_box(pos: &mut WorldPos, vel: &mut Velocity) {
    pos.x += vel.x;
    pos.y += vel.y;
    // ROM box [0x80, 0x2F0] x [0x80, 0x140] in map pixels.
    let (x0, x1, y0, y1) = (0, 0x2F0 - 0x80, 0, 0x140 - 0x80);
    if pos.px_x() < x0 {
        set_px(&mut pos.x, x0);
        vel.x = -vel.x;
    } else if pos.px_x() >= x1 {
        set_px(&mut pos.x, x1);
        vel.x = -vel.x;
    }
    if pos.px_y() < y0 {
        set_px(&mut pos.y, y0);
        vel.y = -vel.y;
    } else if pos.px_y() >= y1 {
        set_px(&mut pos.y, y1);
        vel.y = -vel.y;
    }
}

/// B87 (0x19622): expanding ring of four flames: table 0x1971A (doubled, 40 steps) scaled by
/// v160/60 while v160 grows to 60; pieces at 4/4 .. 1/4 of that radius.
fn fire_ring(ctx: &RoomCtx, o: &mut Obj) {
    let (t0, t1) = pair(ctx, "0x1971A", o.var(158), false);
    let scale = o.var(160);
    let (r0, r1) = (2 * t0 * scale / 60, 2 * t1 * scale / 60);
    o.set_var(162, r0);
    o.set_var(164, r1);
    step_index(o, 158, 40);
    o.set_var(160, (o.var(160) + 1).min(60));
    for (piece, k) in [(0, 4), (1, 3), (2, 2), (3, 1)] {
        set_piece(o, piece, r0 * k / 4 - 8, r1 * k / 4 - 0x30);
    }
}

// ---------------------------------------------------------------------------------------------
// Snake segments (rooms 26/27)
// ---------------------------------------------------------------------------------------------

/// B59 (0x17B56) / B60 (0x17BC8): follow the previous slot 7 samples late. History in ROM space
/// (x: v158..v168,v188; y: v190..v200,v220); B59 samples the previous slot's position, B60 its
/// oldest history sample (a longer body).
fn snake_follow(o: &mut Obj, pos: &mut WorldPos, fx: &Effects, from_history: bool) -> Option<ops::OtherObj> {
    let prev = fx.slot(o.slot.checked_sub(1)?).copied()?;
    let (nx, ny) = if from_history { (prev.var(188), prev.var(220)) } else { (prev.x + 0x80, prev.y + 0x80) };
    set_px(&mut pos.x, o.var(188) - 0x80);
    for (dst, src) in [(188, 168), (168, 166), (166, 164), (164, 162), (162, 160), (160, 158)] {
        o.set_var(dst, o.var(src));
    }
    o.set_var(158, nx);
    set_px(&mut pos.y, o.var(220) - 0x80);
    for (dst, src) in [(220, 200), (200, 198), (198, 196), (196, 194), (194, 192), (192, 190)] {
        o.set_var(dst, o.var(src));
    }
    o.set_var(190, ny);
    Some(prev)
}

// ---------------------------------------------------------------------------------------------
// Balrog (rooms 44/46)
// ---------------------------------------------------------------------------------------------

/// Scrolls plane B so the body tilemap follows the object (0x19F60/0x1A292).
fn plane_b_scroll(pos: &WorldPos, fx: &mut Effects) {
    set_ram(fx, PLANE_B_HSCROLL, pos.px_x() + 0x80 - 0xA8);
    set_ram(fx, PLANE_B_VSCROLL, 0xD0 - (pos.px_y() + 0x80));
}

/// B123 (0x1A23C): the background-plane Balrog drifts between x 224 and 400 (ROM) at 0.5 px/f,
/// turning when hit (v162 counts the hits).
fn plane_b_patrol(o: &mut Obj, pos: &mut WorldPos, fx: &mut Effects) {
    if o.var(158) != 0 {
        pos.x -= fp(0.5);
        if pos.x < px(224 - 0x80) {
            o.set_var(158, o.var(158) ^ 1);
        }
    } else {
        pos.x += fp(0.5);
        if pos.x >= px(400 - 0x80) {
            o.set_var(158, o.var(158) ^ 1);
        }
    }
    if o.hit_count != 0 {
        o.hit_count = 0;
        o.set_var(158, o.var(158) ^ 1);
        o.set_var(162, o.var(162) + 1);
    }
    plane_b_scroll(pos, fx);
}

/// B106 (0x19CEC): Balrog swooping with acceleration inside x ∈ [153, 416], y ∈ [208, 296] (ROM);
/// v164/v166 remember the direction for I221; ay flips at state_timer 30; a hit reverses.
fn balrog_swoop(o: &mut Obj, pos: &mut WorldPos, vel: &mut Velocity) {
    free_flight(o, pos, vel);
    if o.state_timer == 30 {
        o.ay = -o.ay;
    }
    if o.hit_count != 0 {
        o.hit_count = 0;
        vel.x = -vel.x;
        o.ax = -o.ax;
        vel.y = -vel.y;
        o.ay = -o.ay;
        o.set_var(164, o.var(164) ^ 1);
        o.set_var(166, o.var(166) ^ 1);
    }
    if pos.x >= px(416 - 0x80) && vel.x >= 0 {
        o.set_var(164, 1);
        vel.x = -vel.x;
        o.ax = -o.ax;
    }
    if pos.x < px(153 - 0x80) && vel.x < 0 {
        o.set_var(164, 0);
        vel.x = -vel.x;
        o.ax = -o.ax;
    }
    if pos.y >= px(296 - 0x80) && vel.y >= 0 {
        o.set_var(166, 1);
        vel.y = -vel.y;
        o.ay = -o.ay;
    }
    if pos.y < px(208 - 0x80) && vel.y < 0 {
        o.set_var(166, 0);
        vel.y = -vel.y;
        o.ay = -o.ay;
    }
}

fn lightning_anim_key(ctx: &RoomCtx, o: &Obj, step: i32) -> String {
    let desc = entry(ctx, "0x1C472", step.clamp(0, 8) as usize);
    let key = format!("rom{desc:X}:0:{}", o.gfx);
    if ctx.sheet.anims.contains_key(&key) { key } else { format!("rom{desc:X}:0:default") }
}

/// 0x1C372: moves a lightning segment one path step (16 px, table 0x1C44E) and shows the
/// matching bolt piece (anims 0x1C472, keeping the frame); step 4 = stay. Steps the anim once
/// more like the cartridge (0x26E8).
fn lightning_step(ctx: &RoomCtx, o: &mut Obj, pos: &mut WorldPos, vel: &mut Velocity, anim: &mut SpriteAnimator, step: i32) {
    if step != STEP_STAY {
        let s = step.clamp(0, 8) as usize;
        vel.x = (entry(ctx, "0x1C44E", 2 * s) << 16) | (vel.x & 0xFFFF);
        vel.y = (entry(ctx, "0x1C44E", 2 * s + 1) << 16) | (vel.y & 0xFFFF);
        let key = lightning_anim_key(ctx, o, step);
        anim.swap_keep_frame(&key);
        pos.x += vel.x;
        pos.y += vel.y;
    }
    anim::step(ctx.sheet, anim);
}

/// 0x1C3F8: a lightning segment starts at Balrog's hand (slot 0, 48 px up), at rest.
fn lightning_place(ctx: &RoomCtx, o: &mut Obj, pos: &mut WorldPos, vel: &mut Velocity, anim: &mut SpriteAnimator, fx: &Effects) {
    *vel = Velocity::default();
    follow_slot0(pos, fx, 0, -48);
    let key = lightning_anim_key(ctx, o, 3);
    anim.restart(&key);
}

/// 0xCBE8: next path step toward the target, limited by the previous step (table 0xCC4A rows) so
/// the bolt zig-zags instead of turning around.
fn lightning_next_step(ctx: &RoomCtx, x: i32, y: i32, tx: i32, ty: i32, prev: i32) -> i32 {
    let col = if tx >= x + 4 {
        0
    } else if tx < x - 4 {
        2
    } else {
        1
    };
    let row = if ty < y - 4 {
        0
    } else if ty >= y + 4 {
        2
    } else {
        1
    };
    let prev = prev.clamp(0, 8) as usize;
    entry(ctx, "0xCC4A", prev * 9 + 3 * col + row)
}

// ---------------------------------------------------------------------------------------------
// Opcode dispatch
// ---------------------------------------------------------------------------------------------

/// Per-tick behaviour opcodes. Returns true if `op` was handled.
#[allow(clippy::too_many_arguments)]
pub fn behaviour(
    ctx: &RoomCtx,
    o: &mut Obj,
    op: u16,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    col: &mut Collider,
    c: &mut Contacts,
    p: &PlayersView,
    fx: &mut Effects,
) -> bool {
    let t = o.state_timer;
    match op {
        // B52 (0x178BE): circular motion, position += 0x177CE[v160] - 0x177CE[v160 - 1].
        52 => {
            let v = step_index(o, 160, 120);
            let prev = if v == 0 { 119 } else { v - 1 };
            let neg = o.var(162) == 1;
            let (ox, oy) = pair(ctx, "0x177CE", prev, neg);
            let (nx, ny) = pair(ctx, "0x177CE", v, neg);
            pos.x += px(nx - ox);
            pos.y += px(ny - oy);
        }
        // B56 (0x17998): a volley of three projectiles chosen by the direction to the nearest
        // player (table 0x17A34), from 32 px above; sound AC.
        56 => {
            if let Some(n) = p.nearest(pos.px_x()) {
                let d = direction_code(pos.px_x(), pos.px_y(), n.x, n.y) as usize;
                for i in 0..3 {
                    let rec = entry(ctx, "0x17A34", 4 * d + i);
                    if rec >= 0 {
                        spawn(ctx, o, pos, fx, rec as usize, 0, -32, Pool::High);
                    }
                }
            }
            fx.sounds.push(0xAC);
        }
        59 => {
            snake_follow(o, pos, fx, false);
        }
        60 => {
            snake_follow(o, pos, fx, true);
        }
        // B61 (0x17C3A): B60 + face the previous segment (anim per direction, table 0x17D04).
        61 => {
            if let Some(prev) = snake_follow(o, pos, fx, true) {
                let d = direction_code(pos.px_x(), pos.px_y(), prev.x, prev.y) as usize;
                let a = entry(ctx, "0x17D04", d);
                if a != -1 {
                    set_anim(ctx, o, anim, a);
                }
            }
        }
        66 => kraken_arm(ctx, o, fx, "0x180BC", 104, true),
        // B67 (0x1818C): the Kraken grows its two arms (records 1/2).
        67 => {
            spawn(ctx, o, pos, fx, 1, 16, -8, Pool::Any);
            spawn(ctx, o, pos, fx, 2, -16, -8, Pool::Any);
        }
        68 => kraken_arm_extend(o, fx, 1),
        69 => kraken_arm(ctx, o, fx, "0x1838C", 208, false),
        70 => kraken_arm_extend(o, fx, -1),
        // B71 (0x1855E): spit a projectile (record 3) from the arm tip.
        71 => {
            let (dx, dy) = (o.var(246) + 8, o.var(248) + 16);
            spawn(ctx, o, pos, fx, 3, dx, dy, Pool::High);
        }
        72 => arm_crumble(o, fx),
        // B73 (0x18720): Kraken death: explosions, then 9 gold bags (record 4) rain down.
        73 => {
            if [0, 24, 48, 72].contains(&t) {
                fx.sounds.push(0xA8);
            }
            if t == 94 {
                for k in 0..9 {
                    spawn(ctx, o, pos, fx, 4, -64 + 16 * k, -64, Pool::Any);
                }
            }
        }
        74 => chain_ball(ctx, o),
        75 => fire_serpent(ctx, o),
        76 => worm_segment(ctx, o, pos, vel, anim, col, c, fx),
        77 => worm_appear(ctx, o, pos, anim, fx),
        78 => worm_orbit(ctx, o, pos, anim, fx),
        79 => worm_stick(ctx, o, pos, anim, fx),
        80 => worm_release(ctx, o, pos, vel, col, c, fx, 60),
        // B81 (0x1936E): spit fire (record 8 right / 9 left, +0x24 in room 27) on state entry.
        81 => {
            notify_boss_hit(o, fx);
            if t == 0 {
                let n = if o.var(158) != 0 { 8 } else { 9 } + worm_record_base(fx);
                spawn(ctx, o, pos, fx, n, 0, 0, Pool::High);
            }
        }
        // B82 (0x193DA)
        82 => {
            if o.var(158) != 0 {
                vel.x = -vel.x;
            }
        }
        // B83 (0x193EA)
        83 => {
            notify_boss_hit(o, fx);
            free_flight(o, pos, vel);
        }
        84 => fire_cross(ctx, o, true),
        85 => fire_cross(ctx, o, false),
        86 => bounce_in_box(pos, vel),
        87 => fire_ring(ctx, o),
        // B88 (0x1976A): Fire Elemental breaks up: clear slots 2..12, flames (records 9..12 left,
        // 14..17 right).
        88 => {
            if t == 0 {
                fx.sounds.push(0xA8);
                fx.clear_slots = Some((2, 12));
                reset_budget(fx);
                for n in 9..=12 {
                    spawn(ctx, o, pos, fx, n, -16, 0, Pool::Any);
                }
                for n in 14..=17 {
                    spawn(ctx, o, pos, fx, n, 16, 0, Pool::Any);
                }
            }
        }
        // B89 (0x19822): room music restarts ([FF0A56] = 0x11); the reward (record 18) appears at ROM (376, 145).
        89 => {
            if t == 0 {
                fx.set_global(MUSIC_TIMER, 0x11);
                spawn_abs(ctx, o, fx, 18, 376, 145, Pool::Any);
            }
        }
        90 => {
            if t == 1 {
                message(fx, 0x31);
            }
        }
        91 => {
            if t == 30 {
                message(fx, 0x59);
            }
        }
        // B98 (0x1996E): boss defeated (room 27): persist flag, clear slots 1..10, debris 28..35.
        98 => {
            fx.persist.push(o.param118);
            fx.clear_slots = Some((1, 10));
            for n in 28..=35 {
                spawn(ctx, o, pos, fx, n, 0, -32, Pool::Any);
            }
        }
        100 => worm_release(ctx, o, pos, vel, col, c, fx, 0),
        // B101 (0x199FA): bank/palette switch on entry (exporter "B101" graphics); at state_timer 2
        // the fight ends: boss flag off, room music restarts ([FF0A56] = 0x11), [FF0A5A] = 0x100, delete self.
        101 => {
            if t == 2 {
                fx.set_global(BOSS_FLAG, 0);
                fx.set_global(MUSIC_TIMER, 0x11);
                fx.set_global(0xFF0A5A, 0x100);
                o.delete = true;
            }
        }
        // B102 (0x19BEC): Balrog's graphics bank/palette on entry: rendered by the exporter ("B102").
        102 => {}
        106 => balrog_swoop(o, pos, vel),
        // B107 (0x19DF2): follow the previous slot 3 samples late (history v174..v188, map px).
        107 => {
            for (dst, src) in [(186, 182), (182, 178), (178, 174), (188, 184), (184, 180), (180, 176)] {
                o.set_var(dst, o.var(src));
            }
            if let Some(prev) = o.slot.checked_sub(1).and_then(|s| fx.slot(s)) {
                o.set_var(174, prev.x);
                o.set_var(176, prev.y);
            }
            set_px(&mut pos.x, o.var(186));
            set_px(&mut pos.y, o.var(188));
        }
        // B108 (0x19E3E): Balrog's wings (records 1, 2, 1, 2).
        108 => {
            for n in [1, 2, 1, 2] {
                spawn(ctx, o, pos, fx, n, 0, 0, Pool::Any);
            }
        }
        // B109 (0x19E7C): lightning bolt: head (record 3) + 7 trailing segments (16..22).
        109 => {
            for n in [3, 16, 17, 18, 19, 20, 21, 22] {
                spawn(ctx, o, pos, fx, n, 0, -28, Pool::Any);
            }
        }
        // B111 (0x19EF2): Balrog lands: wings (slots 1..4) leave (v162 = 2), sound A8, dust (record 5).
        111 => {
            if t == 0 {
                for slot in 1..=4 {
                    fx.slot_writes.push(SlotWrite::Var { slot, offset: 162, value: 2 });
                }
                fx.sounds.push(0xA8);
                spawn(ctx, o, pos, fx, 5, 0, -48, Pool::High);
            }
        }
        // B112 (0x19F4C): Balrog becomes a background-plane boss: scroll plane B onto it, then draw
        // its body tilemap (0x2AAA6) and activate the riding parts (slots 1..5 v162 = 2).
        112 => {
            if t == 0 {
                plane_b_scroll(pos, fx);
            } else if t == 1 {
                set_ram(fx, PLANE_B_BOSS, PLANE_B_BODY);
                for slot in 1..=5 {
                    fx.slot_writes.push(SlotWrite::Var { slot, offset: 162, value: 2 });
                }
                for off in (158..=188).step_by(2) {
                    o.set_var(off, 0);
                }
            }
        }
        // B113 (0x19FE8): attack pattern on the ground (arm, claws, head, fire).
        113 => {
            let (n, dx, dy, sound) = match t {
                60 => (7, 0, -48, Some(0xA8)),
                90 => (24, 0, -48, Some(0xA8)),
                120 => (23, 0, -16, Some(0x9E)),
                140 => (25, -16, -24, Some(0x9E)),
                160 => (26, 0, 0, None),
                _ => return true,
            };
            spawn(ctx, o, pos, fx, n, dx, dy, Pool::Any);
            if let Some(s) = sound {
                fx.sounds.push(s);
            }
        }
        // B114 (0x1A102): lightning bolt: the head (defence 0) picks the next path step toward the
        // target and pushes it on the queue; segment k replays the step taken k frames ago.
        114 => {
            if o.defence != 0 {
                let step = ram(fx, PATH_QUEUE + o.defence as u32);
                lightning_step(ctx, o, pos, vel, anim, step);
            } else {
                if t == 0 {
                    fx.sounds.push(0xAC);
                }
                let prev = ram(fx, PATH_QUEUE);
                let (tx, ty) = (ram(fx, PATH_TARGET_X), ram(fx, PATH_TARGET_Y));
                let step = lightning_next_step(ctx, pos.px_x(), pos.px_y(), tx, ty, prev);
                for i in (0..7u32).rev() {
                    let v = ram(fx, PATH_QUEUE + i);
                    set_ram(fx, PATH_QUEUE + i + 1, v);
                }
                set_ram(fx, PATH_QUEUE, step);
                lightning_step(ctx, o, pos, vel, anim, step);
            }
        }
        118 => follow_slot0(pos, fx, -32, -60),
        // B119 (0x1A1AA): Balrog's head; growls (AB) on state entry while [FF1160] (slot0.v160).
        119 => {
            if t == 0 && fx.slot(0).is_some_and(|s0| s0.var(160) != 0) {
                fx.sounds.push(0xAB);
            }
            follow_slot0(pos, fx, -16, -56);
        }
        120 => follow_slot0(pos, fx, 56, 0),
        121 => follow_slot0(pos, fx, -28, -36),
        122 => follow_slot0(pos, fx, 0, 0),
        123 => plane_b_patrol(o, pos, fx),
        // B124 (0x1A2B2): free flight + plane-B follow.
        124 => {
            free_flight(o, pos, vel);
            plane_b_scroll(pos, fx);
        }
        // B125 (0x1A2F2): hover back down below y 0xD0 (ROM), then patrol.
        125 => {
            if pos.px_y() + 0x80 < 0xD0 {
                vel.y = fp(1.0);
                o.ay = fp(-0.0625);
            }
            vel.y += o.ay;
            pos.y += vel.y;
            plane_b_patrol(o, pos, fx);
        }
        // B126 (0x1A320): tentacle hanging from slot 1's first mapping piece; every 2nd frame its
        // tip follows table 0x1A4EA (72 steps), pieces at 6/7 .. 1/7; hurt box on piece 1, attack
        // box on piece 4.
        126 => {
            if let Some(s1) = fx.slot(1).copied() {
                set_px(&mut pos.x, s1.x + s1.piece0.0 + 16 - 8);
                set_px(&mut pos.y, s1.y + s1.piece0.1 + 16 + 8);
            }
            if countdown(o, 160, 1) {
                return true;
            }
            let (d0, d1) = pair(ctx, "0x1A4EA", o.var(158), o.var(162) == 1);
            o.set_var(168, d0);
            o.set_var(170, d1);
            set_piece(o, 0, d0 - 16, d1 - 16);
            step_index(o, 158, 72);
            let layout = [(1, 6, 16), (2, 5, 16), (3, 4, 12), (4, 3, 12), (5, 2, 12), (6, 1, 8)];
            for (piece, k, sub) in layout {
                let (x, y) = (d0 * k / 7 - sub, d1 * k / 7 - sub);
                set_piece(o, piece, x, y);
                if piece == 1 {
                    set_hurt_offset(o, x + 16, y + 32);
                } else if piece == 4 {
                    set_attack_offset(o, x + 12, y + 24);
                }
            }
            set_piece(o, 7, -8, -8);
        }
        // B127 (0x1A57A): on entry the arm grows a tentacle (record 27); rides the body like B118.
        127 => {
            if t == 0 {
                spawn(ctx, o, pos, fx, 27, 0, 0, Pool::Any);
            }
            follow_slot0(pos, fx, -32, -60);
        }
        // B130 (0x1A5F0): on entry fire (record 35) from the first mapping piece; sound A6.
        130 => {
            if t == 0 {
                let (dx, dy) = first_piece(o, ctx.sheet, anim);
                spawn(ctx, o, pos, fx, 35, dx + 16 - 8, dy + 16 + 8, Pool::Any);
                fx.sounds.push(0xA6);
            }
            follow_slot0(pos, fx, -28, -64);
        }
        // B131 (0x1A64C): Balrog loses its arm: delete slot 3, debris (34), dust (6); sound AE.
        131 => {
            if t == 0 {
                fx.slot_writes.push(SlotWrite::Delete { slot: 3 });
                spawn(ctx, o, pos, fx, 34, 0, 0, Pool::Any);
                spawn(ctx, o, pos, fx, 6, 0, -40, Pool::Any);
                fx.sounds.push(0xAE);
            }
        }
        132 => follow_slot0(pos, fx, -28, -64),
        // B133 (0x1A6D4): final defeat: clear plane B and slots 1..13, debris 36..47; sound 9F.
        133 => {
            if t == 0 {
                set_ram(fx, PLANE_B_BOSS, PLANE_B_CLEARED);
                fx.clear_slots = Some((1, 13));
                reset_budget(fx);
                for n in 36..=47 {
                    spawn(ctx, o, pos, fx, n, 0, -32, Pool::Any);
                }
                fx.sounds.push(0x9F);
            }
        }
        _ => return false,
    }
    true
}

/// State-entry opcodes. Returns true if `op` was handled.
#[allow(clippy::too_many_arguments)]
pub fn init(
    ctx: &RoomCtx,
    o: &mut Obj,
    op: u16,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    p: &PlayersView,
    fx: &mut Effects,
) -> bool {
    match op {
        // I20 (0x155A2): the mermaid swims off: vx = 1, boss flag [FF0B92] = 1, persist flag (0x15732).
        20 => {
            vel.x = fp(1.0);
            o.face_left = false;
            fx.set_global(BOSS_FLAG, 1);
            fx.persist.push(o.param118);
        }
        // I155 (0x16042): Kraken gone: room music restarts ([FF0A56] = 0x11), boss flag off, persist flag.
        155 => {
            fx.set_global(MUSIC_TIMER, 0x11);
            fx.set_global(BOSS_FLAG, 0);
            fx.persist.push(o.param118);
        }
        // I192 (0x1686A): the room music restarts ([FF0A56] = 0x11).
        192 => fx.set_global(MUSIC_TIMER, 0x11),
        // I196 (0x1690C): boss fight starts: music 8B, boss flag, persist flag.
        196 => {
            fx.sounds.push(0x8B);
            fx.set_global(BOSS_FLAG, 1);
            fx.persist.push(o.param118);
        }
        // I195 (0x168B0): sound 9B, 8 fireballs (records 1..8) from 48 px up, then I215.
        195 => {
            fx.sounds.push(0x9B);
            for n in 1..=8 {
                spawn(ctx, o, pos, fx, n, 0, -48, Pool::Any);
            }
            fx.set_global(0xFF0AEC, 1);
            o.set_var(158, 0x37);
            o.flags = 0x4180;
            copy_mapping(ctx, o, anim, 0xAC);
        }
        // I219 (0x16AF8): the Jelly Giant appears: boss music/flag, message 0x89; bank 0x7AF9A /
        // palette 0x4C6A ("I219" graphics in the exporter); [FF0A5A] = 0x120.
        219 => {
            o.set_var(190, 0x39);
            o.set_var(162, 4);
            fx.sounds.push(0x8B);
            fx.set_global(BOSS_FLAG, 1);
            message(fx, 0x89);
            fx.set_global(0xFF0A5A, 0x120);
        }
        // I221 (0x16BE8): next swoop of Balrog's flight (direction from v164/v166).
        221 => {
            o.set_var(162, o.var(162) + 1);
            if o.var(162) != 5 {
                vel.x = fp(2.8125);
                vel.y = 0;
                o.ax = fp(-0.046875);
                o.ay = fp(0.0625);
                if o.var(164) != 0 {
                    vel.x = -vel.x;
                    o.ax = -o.ax;
                }
                if o.var(166) != 0 {
                    vel.y = -vel.y;
                    o.ay = -o.ay;
                }
            }
        }
        // I225 (0x16C9A): lightning segment start; the head (defence 0) also resets the path
        // queue, aims at the nearest player (x, y-40) and takes its first step up (0x1C3CA).
        225 => {
            if o.defence != 0 {
                lightning_place(ctx, o, pos, vel, anim, fx);
            } else {
                for i in 0..8 {
                    set_ram(fx, PATH_QUEUE + i, STEP_STAY);
                }
                lightning_place(ctx, o, pos, vel, anim, fx);
                if let Some(n) = p.nearest(pos.px_x()) {
                    set_ram(fx, PATH_TARGET_X, n.x);
                    set_ram(fx, PATH_TARGET_Y, n.y - 0x28);
                }
                set_ram(fx, PATH_QUEUE, 3);
                lightning_step(ctx, o, pos, vel, anim, 3);
            }
        }
        // I232 (0x16D58): Balrog (room 44) is beaten: clear plane B, delete every object, spawn the
        // ending Balrog (record 28) at ROM (144, 304).
        232 => {
            set_ram(fx, PLANE_B_BOSS, PLANE_B_CLEARED);
            fx.delete_all = true;
            reset_budget(fx);
            spawn_abs(ctx, o, fx, 28, 144, 304, Pool::High);
        }
        // I233 (0x16DF6): bank/palette switch ("I233" graphics), sound 8F, walk right at 1 px/f.
        233 => {
            fx.sounds.push(0x8F);
            vel.x = fp(1.0);
            vel.y = 0;
            o.ax = 0;
            o.ay = 0;
        }
        // I234 (0x16ED6): the room-46 Balrog's parts (records 29..33).
        234 => {
            for n in 29..=33 {
                spawn(ctx, o, pos, fx, n, 0, 0, Pool::Any);
            }
        }
        _ => return false,
    }
    true
}

/// Condition opcodes. Returns Some(result) if `op` was handled.
#[allow(clippy::too_many_arguments, unused_variables)]
pub fn condition(
    ctx: &RoomCtx,
    o: &mut Obj,
    op: u16,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    c: &Contacts,
    p: &PlayersView,
    fx: &mut Effects,
) -> Option<bool> {
    None
}

/// One of the four wrap copies of Balrog's plane-B body (the plane repeats every 512x256 px).
#[derive(Component)]
pub struct PlaneBBossSprite {
    ix: i32,
    iy: i32,
}

/// Backdrop-coloured patch under the body inside each background tile: B112 overwrites those
/// 11x10 cells of the cave-wall nametable, so the wall must not show through the body's holes.
#[derive(Component)]
pub struct PlaneBBodyCells;

/// Balrog's background-plane form (rooms 44/46). The cartridge scrolls the whole plane B to the
/// boss (B112/B123/B124/B125 write 0xFF0642/0xFF05F2 every frame), so the room's plane-B nametable
/// and the body tilemap at plane cell (0, 0) move together; here the scroll goes to
/// `BackgroundLayer::script_scroll` and the body is a sprite placed at plane origin + scroll
/// (screen (h, -v), wrapped every 512x256 like the plane). In these rooms plane A is an opaque,
/// low-priority cave wall, so the plane-B cave texture is never visible; only the body is, because
/// all its cells have the VDP priority bit: it draws above plane A *and above low-priority sprites*
/// (the players, low objects), below Balrog's high-priority parts (z just under FOREGROUND_HIGH).
/// The cells B112 replaced are a backdrop-coloured patch in the background tiles; I232/B133 blank
/// the whole plane (background tiles hidden).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn render_plane_b(
    mut commands: Commands,
    mut fx: ResMut<Effects>,
    cam: Res<CameraPos>,
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    objects: Res<Assets<RoomObjects>>,
    sheets: Res<Assets<SpriteSheet>>,
    mut last_room: Local<Option<usize>>,
    mut layers: Query<&mut BackgroundLayer>,
    mut tiles: Query<(Entity, &mut Visibility), With<BackgroundTile>>,
    mut cells: Query<&mut Visibility, (With<PlaneBBodyCells>, Without<BackgroundTile>)>,
    mut q: Query<(&PlaneBBossSprite, &mut Sprite, &mut Transform, &mut Visibility), (Without<BackgroundTile>, Without<PlaneBBodyCells>)>,
) {
    let index = room.as_ref().map(|r| r.index);
    if *last_room != index || layers.iter_mut().any(|l| l.is_added()) {
        // Plane B and its scroll are rebuilt by every room load.
        *last_room = index;
        for key in [PLANE_B_BOSS, PLANE_B_HSCROLL, PLANE_B_VSCROLL] {
            fx.globals.remove(&key);
        }
    }
    let scripted = fx.globals.contains_key(&PLANE_B_HSCROLL) || fx.globals.contains_key(&PLANE_B_VSCROLL);
    let (hs, vs) = (fx.global(PLANE_B_HSCROLL), fx.global(PLANE_B_VSCROLL));
    let scroll = if scripted { IVec2::new(hs, vs) } else { IVec2::ZERO };
    for mut l in &mut layers {
        if l.script_scroll != scroll {
            l.script_scroll = scroll;
        }
    }
    let content = fx.global(PLANE_B_BOSS);
    let wall = if content == PLANE_B_CLEARED { Visibility::Hidden } else { Visibility::Inherited };
    for (_, mut vis) in &mut tiles {
        vis.set_if_neq(wall);
    }

    let def = room.as_ref().and_then(|r| rooms.get(&r.def));
    let sheet = def.and_then(|d| objects.get(&d.objects_handle)).and_then(|o| sheets.get(&o.sheet_handle));
    let frame = sheet.and_then(|s| {
        let key = format!("planeB{PLANE_B_BOSS:X}:0:default");
        let f = s.anims.get(&key)?.frames.first()?;
        Some((s.image.clone(), s.frames.get(f.frame)?.rect))
    });
    let Some((image, [x, y, w, h])) = frame.filter(|_| content == PLANE_B_BODY) else {
        for (_, _, _, mut vis) in &mut q {
            vis.set_if_neq(Visibility::Hidden);
        }
        for mut vis in &mut cells {
            vis.set_if_neq(Visibility::Hidden);
        }
        return;
    };
    // The replaced cells inside every background tile.
    if cells.is_empty() {
        let [r, g, b] = def.map_or([0, 0, 0], |d| d.backdrop);
        for (tile, _) in &tiles {
            commands.spawn((
                Sprite::from_color(Color::srgb_u8(r, g, b), Vec2::new(w as f32, h as f32)),
                bevy::sprite::Anchor::TOP_LEFT,
                Transform::from_xyz(0.0, 0.0, 0.05),
                PlaneBBodyCells,
                ChildOf(tile),
            ));
        }
    }
    for mut vis in &mut cells {
        vis.set_if_neq(Visibility::Inherited);
    }
    // Plane origin on screen, wrapped; the copies one plane width/height up-left cover the rest.
    let base = IVec2::new(hs.rem_euclid(BG_W), (-vs).rem_euclid(BG_H));
    let rect = Some(Rect::new(x as f32, y as f32, (x + w) as f32, (y + h) as f32));
    let place = |ix: i32, iy: i32| {
        let at = base - IVec2::new(ix * BG_W, iy * BG_H);
        super::super::coords::to_bevy(cam.x + at.x, cam.y + at.y, z::FOREGROUND_HIGH - 0.5)
    };
    if q.is_empty() {
        for (ix, iy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            commands.spawn((
                Sprite { image: image.clone(), rect, ..default() },
                bevy::sprite::Anchor::TOP_LEFT,
                Transform::from_translation(place(ix, iy)),
                Visibility::Inherited,
                PlaneBBossSprite { ix, iy },
                RoomEntity,
            ));
        }
    }
    for (c, mut s, mut tf, mut vis) in &mut q {
        if s.image != image {
            s.image = image.clone();
        }
        s.rect = rect;
        tf.translation = place(c.ix, c.iy);
        vis.set_if_neq(Visibility::Inherited);
    }
}
